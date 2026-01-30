use crate::types::block::Block;
use std::collections::HashMap;
use crate::types::hash::{H256, Hashable};
use crate::types::address::Address;
use crate::types::state_trie::{StateTrie, Node}; // 确保引入 Node
use crate::database::Storage;
use std::sync::Arc;
use log::{info, error, warn, debug};
use std::hash::Hash;
use ring::digest;
use std::convert::TryInto;
use crate::miner::BLOCK_REWARD;
use crate::types::merkle::MerkleTree;

// Account 定义保持不变
#[derive(Clone, Debug, Default, Copy, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub nonce: u64,
    pub balance: u64,
}

impl Hashable for Account {
    fn hash(&self) -> H256 {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        bytes.extend_from_slice(&self.balance.to_be_bytes());
        let hash = digest::digest(&digest::SHA256, &bytes);
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(hash.as_ref());
        H256::from(hash_bytes)
    }
}

pub struct Blockchain {
    pub tip: H256,
    pub storage: Arc<Storage>,
}

impl Blockchain {
    pub fn new(path: &str) -> Self {
        let storage = Arc::new(Storage::new(path));

        if let Some(tip) = storage.get_item(&storage.meta, b"tip") {
            info!("Restoring blockchain from DB: {}", path);
            return Self { tip, storage };
        }

        info!("Initializing Genesis State at {}", path);

        let trie = StateTrie::new(storage.clone());

        //god_address
        let hex_str = "67d39da22d106b686c4f301b6f357600d28fc104";
        let bytes: Vec<u8> = hex::decode(hex_str).expect("Invalid hex string");
        let array: [u8; 20] = bytes.try_into().expect("Wrong length");
        let god_address = Address::from(array);

        let god_account = Account {
            nonce: 0,
            balance: 100_000_000, 
        };


        let (genesis_state_root, nodes) = trie.insert(god_address, god_account);

        // 持久化状态节点
        storage.batch_save_state_nodes(&nodes);

        let genesis_block = Block::genesis(genesis_state_root);
        let genesis_hash = genesis_block.hash();

        info!("Genesis Block Created. Hash: {:?}, State Root: {:?}", genesis_hash, genesis_state_root);

        // 存入数据库
        storage.insert_item(&storage.blocks, genesis_hash.as_ref(), &genesis_block);
        storage.insert_item(&storage.meta, b"tip", &genesis_hash);
        storage.insert_item(&storage.meta, genesis_hash.as_ref(), &0u64); // Height = 0

        // 刷盘
        storage.flush();

        Self {
            tip: genesis_hash,
            storage,
        }
    }

    // --- 获取信息相关函数保持不变 ---
    pub fn get_account(&self, addr: &Address) -> Account {
        let state = self.get_state_at_tip();
        state.get(addr).unwrap_or_default()
    }

    pub fn flush(&self) {
        self.storage.flush();
    }

    pub fn get_state_at_tip(&self) -> StateTrie {
        let tip_block = self.get_block(&self.tip).unwrap();
        StateTrie::new_from_root(tip_block.state_root, self.storage.clone())
    }

    pub fn tip(&self) -> H256 {
        self.tip
    }

    pub fn get_difficulty(&self) -> H256 {
        self.get_block(&self.tip).unwrap().get_difficulty()
    }

    pub fn get_block(&self, hash: &H256) -> Option<Block> {
        self.storage.get_item(&self.storage.blocks, hash.as_ref())
    }

    pub fn contains_block(&self, hash: &H256) -> bool {
        self.storage.blocks.contains_key(hash.as_ref()).unwrap_or(false)
    }

    pub fn get_height(&self, hash: &H256) -> u64 {
        self.storage.get_item(&self.storage.meta, hash.as_ref()).unwrap_or(0)
    }

    pub fn all_blocks_in_longest_chain(&self) -> Vec<H256> {
        let mut chain = Vec::new();
        let mut curr = self.tip;
        loop {
            chain.push(curr);
            let height = self.get_height(&curr);
            if height == 0 { break; }
            let block = self.get_block(&curr).unwrap();
            curr = block.get_parent();
        }
        chain.reverse();
        chain
    }


    pub fn execute_block(storage: Arc<Storage>, block: &Block) -> Result<(H256, HashMap<H256, Node>), String> {
        let block_hash = block.hash();
        let parent_hash = block.get_parent();

        // 验证 parent
        let parent_block = match storage.get_item::<Block>(&storage.blocks, parent_hash.as_ref()) {
            Some(b) => b,
            None => return Err(format!("Parent block not found: {:?}", parent_hash)),
        };

        // 验证 PoW 难度
        let block_difficulty = block.get_difficulty();
        if block_hash > block_difficulty {
            return Err(format!("PoW difficulty not satisfied. Hash: {:?}, Target: {:?}", block_hash, block_difficulty));
        }

        // 验证难度一致性
        if block.get_difficulty() != parent_block.get_difficulty() {
             return Err("Difficulty mismatch with parent".to_string());
        }

        // 验证交易签名与 Coinbase 数额
        let mut total_fee: u64 = 0;
        for (idx, tx) in block.data.iter().enumerate() {
            if !tx.verify() {
                 return Err(format!("Invalid signature in tx index {}", idx));
            }
            total_fee += tx.transaction.gas_price * tx.transaction.gas_limit; 
        }

        let expected_reward = BLOCK_REWARD + total_fee;
        if block.coinbase.value != expected_reward {
            return Err(format!("Coinbase value mismatch. Expected: {}, Got: {}", expected_reward, block.coinbase.value));
        }

        // 验证 Merkle Root
        let calculated_root = MerkleTree::new(&block.data).root();
        if calculated_root != block.get_merkle_root() {
            return Err(format!("Invalid Merkle Root. Calc: {:?}, Header: {:?}", calculated_root, block.get_merkle_root()));
        }

        // 验证 state_root
        let state = StateTrie::new_from_root(parent_block.state_root, storage.clone());
        
        let mut account_updates: HashMap<Address, Account> = HashMap::new();

        for tx in &block.data {
            let sender_addr = tx.sender_address();
            let receiver_addr = tx.transaction.to;
            let total_cost = tx.transaction.value + tx.transaction.gas_price * tx.transaction.gas_limit;

            let mut sender_acc = account_updates.get(&sender_addr).cloned()
                .unwrap_or_else(|| state.get(&sender_addr).unwrap_or_default());

            // 验证 Nonce
            if tx.transaction.nonce != sender_acc.nonce {
                return Err(format!("Invalid nonce for tx {:?}, expected {}, got {}", tx.hash(), sender_acc.nonce, tx.transaction.nonce));
            }
            // 验证余额
            if sender_acc.balance < total_cost {
                return Err(format!("Insufficient balance for tx {:?}", tx.hash()));
            }

            // 执行转账
            sender_acc.balance -= total_cost;
            sender_acc.nonce += 1;
            account_updates.insert(sender_addr, sender_acc);

            let mut receiver_acc = account_updates.get(&receiver_addr).cloned()
                .unwrap_or_else(|| state.get(&receiver_addr).unwrap_or_default());
            receiver_acc.balance += tx.transaction.value;
            account_updates.insert(receiver_addr, receiver_acc);
        }

        //  处理 Coinbase
        let miner_addr = block.coinbase.to;
        let mut miner_acc = account_updates.get(&miner_addr).cloned()
            .unwrap_or_else(|| state.get(&miner_addr).unwrap_or_default());
        miner_acc.balance += block.coinbase.value;
        account_updates.insert(miner_addr, miner_acc);

        //  计算新 Root (Batch Insert - CPU 密集型)
        let (final_root, new_nodes) = state.insert_batch(account_updates);

        //  验证 Root 是否匹配
        if final_root != block.state_root {
            return Err(format!("State root mismatch! Calc: {:?}, Block: {:?}", final_root, block.state_root));
        }

        Ok((block_hash, new_nodes))
    }



    pub fn commit_block(&mut self, block: &Block, new_nodes: HashMap<H256, Node>) {
        let block_hash = block.hash();
        
        // 幂等性检查
        if self.contains_block(&block_hash) { return; }

        let parent_hash = block.get_parent();
        
        // 确保父块还在
        if !self.contains_block(&parent_hash) {
            warn!("Orphan block during commit: {:?}", block_hash);
            return;
        }

        //  写入 Block 和 State Nodes 
        self.storage.insert_item(&self.storage.blocks, block_hash.as_ref(), block);
        self.storage.batch_save_state_nodes(&new_nodes);

        //  更新高度
        let parent_height = self.get_height(&parent_hash);
        let current_height = parent_height + 1;
        self.storage.insert_item(&self.storage.meta, block_hash.as_ref(), &current_height);
        
        //  更新 Tip (如果更长)
        let tip_height = self.get_height(&self.tip);
        if current_height > tip_height {
            info!("New Tip: {} Height: {}", block_hash, current_height);
            self.tip = block_hash;
            self.storage.insert_item(&self.storage.meta, b"tip", &block_hash);
        } else {
             info!("Fork block commited: {} Height: {}", block_hash, current_height);
        }
        
    }
}