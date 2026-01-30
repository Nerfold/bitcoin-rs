pub mod worker;

use log::info;
use crossbeam::channel::{unbounded, Receiver, Sender, TryRecvError};
use std::time::{self, SystemTime, UNIX_EPOCH};
use std::thread;
use crate::types::block::Block;
use crate::blockchain::Blockchain;
use crate::types::hash::{Hashable, H256};
use rand::Rng;
use std::sync::{Arc, Mutex};
use crate::types::merkle::MerkleTree;
use crate::types::mempool::Mempool;
use std::collections::HashMap;
use crate::types::address::Address;
use crate::types::transaction::{Transaction, SignedTransaction};
use crate::types::state_trie::{StateTrie, Node};
use crate::blockchain::Account; // 引入 Account

pub const BLOCK_REWARD: u64 = 50;

enum ControlSignal {
    Start(u64), // the number controls the lambda of interval between block generation
    Stop,
    Update, // update the block in mining, it may due to new blockchain tip or new transaction
    Exit,
}

enum OperatingState {
    Paused,
    Run(u64),
    ShutDown,
    MinedWait(u64),
}

pub struct Context {
    /// Channel for receiving control signal
    control_chan: Receiver<ControlSignal>,
    operating_state: OperatingState,
    finished_block_chan: Sender<(Block, HashMap<H256, Node>)>,
    blockchain: Arc<Mutex<Blockchain>>,
    mempool: Arc<Mutex<Mempool>>,
    miner_address: Address, 
}

#[derive(Clone)]
pub struct Handle {
    /// Channel for sending signal to the miner thread
    control_chan: Sender<ControlSignal>,
}

pub fn new(
    blockchain: &Arc<Mutex<Blockchain>>,
    mempool: &Arc<Mutex<Mempool>>,
    miner_address: Address
) -> (Context, 
      Handle, 
      Receiver<(Block, HashMap<H256, Node>)>
     ) {
    let (signal_chan_sender, signal_chan_receiver) = unbounded();
    let (finished_block_sender, finished_block_receiver) = unbounded();

    let ctx = Context {
        control_chan: signal_chan_receiver,
        operating_state: OperatingState::Paused,
        finished_block_chan: finished_block_sender,
        blockchain: blockchain.clone(),
        mempool: mempool.clone(),
        miner_address,
    };

    let handle = Handle {
        control_chan: signal_chan_sender,
    };

    (ctx, handle, finished_block_receiver)
}


impl Handle {
    pub fn exit(&self) {
        self.control_chan.send(ControlSignal::Exit).unwrap();
    }

    pub fn start(&self, lambda: u64) {
        self.control_chan
            .send(ControlSignal::Start(lambda))
            .unwrap();
    }

    pub fn stop(&self) {
        self.control_chan.send(ControlSignal::Stop).unwrap();
    }

    pub fn update(&self) {
        self.control_chan.send(ControlSignal::Update).unwrap();
    }
}

impl Context {
    pub fn start(mut self) {
        thread::Builder::new()
            .name("miner".to_string())
            .spawn(move || {
                self.miner_loop();
            })
            .unwrap();
        info!("Miner initialized into paused mode");
    }

    fn miner_loop(&mut self) {
        // main mining loop
        loop {
            // check and react to control signals
            match self.operating_state {
                OperatingState::Paused => {
                    let signal = self.control_chan.recv().unwrap();
                    match signal {
                        ControlSignal::Exit => {
                            info!("Miner shutting down");
                            self.operating_state = OperatingState::ShutDown;
                        }
                        ControlSignal::Start(i) => {
                            info!("Miner starting in continuous mode with lambda {}", i);
                            self.operating_state = OperatingState::Run(i);
                        }
                        ControlSignal::Stop => {
                            //already paused
                        }
                        ControlSignal::Update => {
                            // in paused state, don't need to update
                        }
                    };
                    continue;
                }
                OperatingState::ShutDown => {
                    return;
                }

                OperatingState::MinedWait(lambda) => {
                    let signal = self.control_chan.recv().unwrap();
                    match signal {
                        ControlSignal::Exit => { self.operating_state = OperatingState::ShutDown; }
                        ControlSignal::Start(i) => { self.operating_state = OperatingState::Run(i); }
                        ControlSignal::Stop => {
                            //already paused
                        }
                        ControlSignal::Update => { 
                            self.operating_state = OperatingState::Run(lambda); 
                        } 
                    };
                    continue;
                }

                _ => match self.control_chan.try_recv() {
                    Ok(signal) => {
                        match signal {
                            ControlSignal::Exit => {
                                info!("Miner shutting down");
                                self.operating_state = OperatingState::ShutDown;
                            }
                            ControlSignal::Start(i) => {
                                info!("Miner starting in continuous mode with lambda {}", i);
                                self.operating_state = OperatingState::Run(i);
                            }
                            ControlSignal::Stop => {
                                info!("Miner stopping...");
                                self.operating_state = OperatingState::Paused;
                                continue; // 立即跳过本次循环，进入 Paused 逻辑
                            }
                            ControlSignal::Update => {
                                
                            }
                        };
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => panic!("Miner control channel detached"),
                },
            }
            if let OperatingState::ShutDown = self.operating_state {
                return;
            }

            let (parent_hash, difficulty, parent_state_root, storage) = {
                let chain = self.blockchain.lock().unwrap();
                let tip = chain.tip();
                let block = chain.get_block(&tip).unwrap(); 
                (tip, chain.get_difficulty(), block.state_root, chain.storage.clone())
            };

            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
            

            let state_trie = StateTrie::new_from_root(parent_state_root, storage.clone());
            
            let mut transactions = {
                let mempool = self.mempool.lock().unwrap();
                let mut all_txs = mempool.select_transactions();

                all_txs.sort_by(|a, b| {
                    let sender_a = a.sender_address();
                    let sender_b = b.sender_address();
                    if sender_a == sender_b {
                        a.transaction.nonce.cmp(&b.transaction.nonce)
                    } else {
                        sender_a.cmp(&sender_b) 
                    }
                });

                let mut valid_txs = Vec::new();
                
                let mut temp_state: HashMap<Address, (u64, u64)> = HashMap::new(); 
                
                for tx in all_txs {
                    let sender = tx.sender_address();
                    let total_cost = tx.transaction.value + tx.transaction.gas_price * tx.transaction.gas_limit;


                    let (curr_nonce, curr_balance) = temp_state.entry(sender).or_insert_with(|| {
                        state_trie.get(&sender)
                            .map(|acc| (acc.nonce, acc.balance))
                            .unwrap_or((0, 0))
                    });

                    if tx.transaction.nonce == *curr_nonce && *curr_balance >= total_cost {
                        valid_txs.push(tx);
                        *curr_nonce += 1;
                        *curr_balance -= total_cost;
                    } else if tx.transaction.nonce > *curr_nonce {
                        continue;
                    }
                }
                valid_txs
            };

            let mut total_fee: u64 = 0;
            let mut account_updates: HashMap<Address, Account> = HashMap::new();

            for tx in &transactions {
                let fee = tx.transaction.gas_price * tx.transaction.gas_limit;
                total_fee += fee;

                let sender_addr = tx.sender_address();
                let receiver_addr = tx.transaction.to;

                let mut sender_acc = account_updates.get(&sender_addr).cloned()
                    .unwrap_or_else(|| state_trie.get(&sender_addr).unwrap_or_default());

                sender_acc.balance -= (tx.transaction.value + fee);
                sender_acc.nonce += 1;
                account_updates.insert(sender_addr, sender_acc);

                let mut receiver_acc = account_updates.get(&receiver_addr).cloned()
                    .unwrap_or_else(|| state_trie.get(&receiver_addr).unwrap_or_default());
                
                receiver_acc.balance += tx.transaction.value;
                account_updates.insert(receiver_addr, receiver_acc);
            }

            let total_reward = BLOCK_REWARD + total_fee;
            let coinbase = Transaction::new(
                0,                  
                total_reward,       
                0,                  
                self.miner_address, 
                0,                  
                vec![]              
            );

            let mut miner_account = account_updates.get(&self.miner_address).cloned()
                .unwrap_or_else(|| state_trie.get(&self.miner_address).unwrap_or_default());
            miner_account.balance += total_reward;
            account_updates.insert(self.miner_address, miner_account);

            let (final_state_root, new_nodes) = state_trie.insert_batch(account_updates);

         
            let mut block_template = Block::new(
                parent_hash,
                0, 
                difficulty,
                timestamp,
                final_state_root,
                coinbase,
                transactions, 
            );

            let mut mined = false;
            loop {
                if block_template.hash() <= difficulty {
                    self.finished_block_chan.send((block_template.clone(), new_nodes.clone())).expect("Send finished block error");
                    info!("Mined a block: {}", block_template.hash());
                    mined = true;
                    break; 
                }
                
                let new_nonce = block_template.get_nonce().wrapping_add(1);
                block_template.set_nonce(&new_nonce);
                
                if new_nonce % 10000000 == 0 {
                    block_template.set_timestamp(&SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis());
                }

                if new_nonce % 10000 == 0 {
                    if !self.control_chan.is_empty() {
                        info!("Signal received, interrupting mining...");
                        break; 
                    }
                }
            }

            if mined {
                if let OperatingState::Run(lambda) = self.operating_state {
                    self.operating_state = OperatingState::MinedWait(lambda);
                }
            }

            if let OperatingState::Run(i) = self.operating_state {
                if i != 0 {
                    let interval = time::Duration::from_micros(i as u64);
                    thread::sleep(interval);
                }
            }
        }
    }
}
