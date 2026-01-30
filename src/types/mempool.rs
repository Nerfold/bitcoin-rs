use super::{
    hash::{Hashable, H256},
    transaction::SignedTransaction,
};



use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone)]
pub struct Mempool {
    transactions: HashMap<H256, SignedTransaction>,
}

impl Mempool {
    
    pub fn new() -> Self {
        Self{
            transactions: HashMap::new(),
        }
    }

    pub fn insert(&mut self, tx: SignedTransaction) {
        let hash = tx.hash();
        if !self.transactions.contains_key(&hash) {
            self.transactions.insert(hash, tx);
        }
    }

    pub fn select_transactions(&self) -> Vec<SignedTransaction> {
        self.transactions.values().cloned().collect()
    }

    pub fn remove_transactions(&mut self, hashes: &[H256]) {
        for hash in hashes {
            self.transactions.remove(hash);
        }
    }

    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    pub fn get_transaction(&self, hash: &H256) -> Option<SignedTransaction> {
        self.transactions.get(hash).cloned()
    }

    pub fn contains(&self, hash: &H256) -> bool {
        self.transactions.contains_key(hash)
    }

}
