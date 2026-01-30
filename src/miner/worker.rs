use crossbeam::channel::{unbounded, Receiver, Sender, TryRecvError};
use log::{debug, info};
use crate::types::block::Block;
use crate::network::server::Handle as ServerHandle;
use std::thread;
use crate::blockchain::Blockchain;
use std::sync::{Arc, Mutex};
use crate::types::hash::{H256, Hashable};

use crate::network::message::Message::NewBlockHashes;
use crate::network::peer;
use crate::types::mempool::Mempool;
use crate::miner::Handle;

use std::collections::HashMap;
use crate::types::state_trie::Node;



#[derive(Clone)]
pub struct Worker {
    server: ServerHandle,
    finished_block_chan: Receiver<(Block, HashMap<H256, Node>)>,
    blockchain: Arc<Mutex<Blockchain>>,
    mempool: Arc<Mutex<Mempool>>,
    miner: Handle,
}

impl Worker {
    pub fn new(
        server: &ServerHandle,
        finished_block_chan: Receiver<(Block, HashMap<H256, Node>)>,
        blockchain: &Arc<Mutex<Blockchain>>,
        mempool: &Arc<Mutex<Mempool>>,
        miner: &Handle,
    ) -> Self {
        Self {
            server: server.clone(),
            finished_block_chan,
            blockchain: blockchain.clone(),
            mempool: mempool.clone(),
            miner: miner.clone(),
        }
    }

    pub fn start(self) {
        thread::Builder::new()
            .name("miner-worker".to_string())
            .spawn(move || {
                self.worker_loop();
            })
            .unwrap();
        info!("Miner initialized into paused mode");
    }

    fn worker_loop(&self) {
        loop {
            let (block, new_nodes) = self.finished_block_chan.recv().expect("Receive finished block error");
            
            // TODO for student: insert this finished block to blockchain, and broadcast this block hash
            {
                self.server.broadcast(NewBlockHashes(vec![block.hash()]));
                {
                    let mut chain = self.blockchain.lock().unwrap();
                    chain.commit_block(&block, new_nodes);
                }
                {
                    let mut mempool = self.mempool.lock().unwrap();
                    let tx_hashes: Vec<H256> = block.data.iter().map(|t| t.hash()).collect();
                    mempool.remove_transactions(&tx_hashes);
                }
                self.miner.update();
            }
        }
    }
}