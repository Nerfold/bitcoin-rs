use super::message::Message;
use super::peer;
use super::server::Handle as ServerHandle;
use crate::types::hash::{H256, Hashable};
use crate::types::block::Block;
use crate::blockchain::Blockchain;
use crate::types::mempool::Mempool;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use log::{debug, warn, error, info};
use std::thread;
use crate::miner::{Handle, BLOCK_REWARD};
use crate::types::merkle::MerkleTree;

#[cfg(any(test,test_utilities))]
use super::peer::TestReceiver as PeerTestReceiver;
#[cfg(any(test,test_utilities))]
use super::server::TestReceiver as ServerTestReceiver;

#[derive(Clone)]
pub struct Worker {
    msg_chan: smol::channel::Receiver<(Vec<u8>, peer::Handle)>,
    num_worker: usize,
    server: ServerHandle,
    blockchain: Arc<Mutex<Blockchain>>,
    orphan_buffer: Arc<Mutex<HashMap<H256, Vec<Block>>>>,
    mempool: Arc<Mutex<Mempool>>,
    miner: Handle,
}

impl Worker {
    pub fn new(
        num_worker: usize,
        msg_src: smol::channel::Receiver<(Vec<u8>, peer::Handle)>,
        server: &ServerHandle,
        blockchain: &Arc<Mutex<Blockchain>>,
        mempool: &Arc<Mutex<Mempool>>,
        miner: &Handle,
    ) -> Self {
        Self {
            msg_chan: msg_src,
            num_worker,
            server: server.clone(),
            blockchain: blockchain.clone(),
            orphan_buffer: Arc::new(Mutex::new(HashMap::new())),
            mempool: mempool.clone(),
            miner: miner.clone(),
        }
    }

    pub fn start(self) {
        let num_worker = self.num_worker;
        for i in 0..num_worker {
            let cloned = self.clone();
            thread::spawn(move || {
                cloned.worker_loop();
                warn!("Worker thread {} exited", i);
            });
        }
    }

    fn worker_loop(&self) {
        loop {
            let result = smol::block_on(self.msg_chan.recv());
            if let Err(e) = result {
                error!("network worker terminated {}", e);
                break;
            }
            let (msg, mut peer) = result.unwrap();
            let msg: Message = bincode::deserialize(&msg).unwrap();
            match msg {
                Message::Ping(nonce) => {
                    debug!("Ping: {}", nonce);
                    peer.write(Message::Pong(nonce.to_string()));
                }
                Message::Pong(nonce) => {
                    debug!("Pong: {}", nonce);
                }
                Message::NewBlockHashes(hashes)=> {
                    debug!("Received NewBlockHashes: {:?}", hashes);
                    let mut hashes_to_request = Vec::new();
                    let blockchain = self.blockchain.lock().unwrap();
                    for hash in hashes {
                        if !blockchain.contains_block(&hash) {
                            hashes_to_request.push(hash);
                        }
                    }
                    drop(blockchain);

                    if !hashes_to_request.is_empty() {
                        peer.write(Message::GetBlocks(hashes_to_request));
                    }
                }
                Message::GetBlocks(hashes) => {
                    debug!("Received GetBlocks request: {:?}", hashes);
                    let mut blocks_to_send = Vec::new();
                    let blockchain = self.blockchain.lock().unwrap();
                    for hash in hashes {
                        if let Some(block) = blockchain.get_block(&hash) {
                            blocks_to_send.push(block);
                        }
                    }
                    drop(blockchain);

                    if !blocks_to_send.is_empty() {
                        peer.write(Message::Blocks(blocks_to_send));
                    }
                }
                Message::Blocks(blocks) => {
                    debug!("Received Blocks: {} blocks", blocks.len());
                    let mut new_blocks_hashes = Vec::new();

                    for block in &blocks {
                        let block_hash = block.hash();
                        
                        // Parent Check
                        let parent_hash = block.get_parent();
                        let blockchain_lock = self.blockchain.lock().unwrap();
                        let parent_exists = blockchain_lock.contains_block(&parent_hash);
                        let storage = blockchain_lock.storage.clone();
                        drop(blockchain_lock); 
                    
                        if !parent_exists {
                            // 父块不存在，加入孤块缓冲区
                            let mut orphans = self.orphan_buffer.lock().unwrap();
                            orphans.entry(parent_hash).or_insert(Vec::new()).push(block.clone());
                            debug!("Orphan block {} added to buffer, waiting for {}", block_hash, parent_hash);
                            peer.write(Message::GetBlocks(vec![parent_hash]));
                            continue;
                        }

                        // Process Block Queue (处理当前块及其可能的孤块后代)
                        let mut process_queue = vec![block.clone()];
                        
                        while let Some(blk) = process_queue.pop() {
                            let blk_hash = blk.hash();

                            let execution_result = Blockchain::execute_block(storage.clone(), &blk);
                            
                            match execution_result {
                                Ok((_, new_nodes)) => {
                                    let mut blockchain = self.blockchain.lock().unwrap();
                                    blockchain.commit_block(&blk, new_nodes);
                                    drop(blockchain); // 提交完立即释放
                                    
                                    info!("Block committed: {}", blk_hash);
                                    
                                    // 清理 Mempool
                                    let mut mempool = self.mempool.lock().unwrap();
                                    let tx_hashes: Vec<H256> = blk.data.iter().map(|t| t.hash()).collect();
                                    mempool.remove_transactions(&tx_hashes);
                                    drop(mempool);

                                    // 通知 Miner 更新
                                    self.miner.update();
                                    
                                    new_blocks_hashes.push(blk_hash);

                                    // 检查孤块 (唤醒子块)
                                    let mut orphans_map = self.orphan_buffer.lock().unwrap();
                                    if let Some(orphans) = orphans_map.remove(&blk_hash) {
                                        for orphan in orphans {
                                            process_queue.push(orphan);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Block execution failed for {}: {}", blk_hash, e);
                                    // 如果执行失败，它的子块也都不用处理了，直接丢弃
                                    continue;
                                }
                            }
                        }                                                                   
                    }

                    if !new_blocks_hashes.is_empty() {
                        self.server.broadcast(Message::NewBlockHashes(new_blocks_hashes));
                    }
                }
                Message::NewTransactionHashes(hashes) => {
                    let mut hashes_to_request = Vec::new();
                    let mempool = self.mempool.lock().unwrap();
                    for hash in hashes {
                        if !mempool.contains(&hash) { 
                            hashes_to_request.push(hash);
                        }
                    }
                    drop(mempool);
                    if !hashes_to_request.is_empty() {
                        peer.write(Message::GetTransactions(hashes_to_request));
                    }
                }
                Message::GetTransactions(hashes) => {
                    let mempool = self.mempool.lock().unwrap();
                    let mut txs_to_send = Vec::new();
                    for hash in hashes {
                        if let Some(tx) = mempool.get_transaction(&hash) {
                            txs_to_send.push(tx);
                        }
                    }
                    drop(mempool);
                    if !txs_to_send.is_empty() {
                        peer.write(Message::Transactions(txs_to_send));
                    }
                }
                Message::Transactions(txs) => {
                    let mut new_tx_hashes = Vec::new();
                    let mut mempool = self.mempool.lock().unwrap();
                    for tx in txs {
                        if !tx.verify() {
                            warn!("Invalid transaction signature received");
                            continue;
                        }
                        let hash = tx.hash();
                        mempool.insert(tx);
                        new_tx_hashes.push(hash);
                    }
                    drop(mempool);

                    if !new_tx_hashes.is_empty() {
                        self.server.broadcast(Message::NewTransactionHashes(new_tx_hashes));
                    }
                }
                Message::GetBlockchain => {
                    let blockchain = self.blockchain.lock().unwrap();
                    let blocks_hash = blockchain.all_blocks_in_longest_chain();
                    let mut blocks = Vec::new();
                    for h in blocks_hash {
                        if let Some(b) = blockchain.get_block(&h) {
                            blocks.push(b);
                        }
                    }
                    peer.write(Message::SendBlockchain(blocks));
                }
                Message::SendBlockchain(blocks) => {
                    let blockchain_lock = self.blockchain.lock().unwrap();
                    let storage = blockchain_lock.storage.clone(); 
                    drop(blockchain_lock); // 释放锁，允许 execute 并发运行

                    for block in blocks {
                        match Blockchain::execute_block(storage.clone(), &block) {
                            Ok((_, new_nodes)) => {
                                // 执行成功，获取锁进行提交
                                let mut bc = self.blockchain.lock().unwrap();
                                bc.commit_block(&block, new_nodes); // 传入缺失的 new_nodes
                            }
                            Err(e) => {
                                error!("Error processing synced block {:?}: {}", block.hash(), e);
                                // 如果同步的链中间有坏块，停止处理后续块
                                break; 
                            }
                        }
                    }
                    //self.miner.update();
                }
                Message::GetMempool => {
                    debug!("Received GetMempool Request");
                    let mempool = self.mempool.lock().unwrap();
                    // 获取 mempool 中所有交易
                    let transactions = mempool.select_transactions();
                    drop(mempool);
                    
                    if !transactions.is_empty() {
                        debug!("Sending {} transactions from mempool", transactions.len());
                        peer.write(Message::SendMempool(transactions));
                    }
                }
                Message::SendMempool(transactions) => {
                    debug!("Received Mempool sync: {} transactions", transactions.len());
                    let mut mempool = self.mempool.lock().unwrap();
                    let mut count = 0;
                    for tx in transactions {
                        let hash = tx.hash();
                        if !mempool.contains(&hash) {
                            // 必须验证签名！防止脏数据攻击
                            if tx.verify() {
                                mempool.insert(tx);
                                count += 1;
                            } else {
                                warn!("Invalid signature in SendMempool for tx {:?}", hash);
                            }
                        }
                    }
                    drop(mempool);
                    debug!("Synced {} new transactions into Mempool", count);
                }
                Message::BlockHeight(peer_height) => {
                    let blockchain = self.blockchain.lock().unwrap();
                    let my_height = blockchain.get_height(&blockchain.tip());
                    drop(blockchain);
                    debug!("Height Check: Peer {}, Me {}", peer_height, my_height);
                    if peer_height > my_height {
                        info!("Peer chain is longer ({} > {}). Requesting synchronization...", peer_height, my_height);
                        peer.write(Message::GetBlockchain);
                        peer.write(Message::GetMempool); 
                    } else {
                        debug!("Peer chain is shorter or equal. No sync needed.");
                    }
                }
                Message::GetBlockHeight => {
                    let blockchain = self.blockchain.lock().unwrap();
                    let height = blockchain.get_height(&blockchain.tip()); 
                    drop(blockchain);
                    peer.write(Message::BlockHeight(height));
                }
            }
        }
    }
}
