use serde::{Serialize, Deserialize};
use crate::types::hash::{H256, Hashable};
use std::time::{SystemTime, UNIX_EPOCH};
use ring::digest;
use rand::Rng;
use crate::types::merkle::MerkleTree;
use crate::types::transaction::{Transaction, SignedTransaction};
use crate::types::address::Address;



#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Block {
    parent: H256,
    nonce: u32,
    difficulty: H256,
    timestamp: u128,
    merkle_root: H256,
    pub state_root: H256,
    pub coinbase: Transaction,
    pub data: Vec<SignedTransaction>,
}

impl Hashable for Block {
    fn hash(&self) -> H256 {
        let header_data = (
            &self.parent,
            &self.nonce,
            &self.difficulty,
            &self.timestamp,
            &self.merkle_root,
            &self.state_root,
            &self.coinbase,
        );

        let encoded: Vec<u8> = bincode::serialize(&header_data).expect("Serialization failed");
        
        let digest: H256 = digest::digest(&digest::SHA256, &encoded).into();
        digest
    }
}


impl Block {

    pub fn new(
        parent: H256,
        nonce: u32,
        difficulty: H256,
        timestamp: u128,
        state_root: H256,
        coinbase: Transaction,
        data: Vec<SignedTransaction>,
    ) -> Self {

        let merkle_root = MerkleTree::new(&data).root();

        Block {
            parent,
            nonce,
            difficulty,
            timestamp,
            merkle_root: merkle_root,
            state_root: state_root,
            coinbase: coinbase,
            data,
        }
    }

    pub fn get_parent(&self) -> H256 {
        self.parent
    }

    pub fn get_difficulty(&self) -> H256 {
        self.difficulty
    }

    pub fn get_merkle_root(&self) -> H256 {
        self.merkle_root
    }

    pub fn get_timestamp(&self) -> u128 {
        self.timestamp
    }

    pub fn get_nonce(&self) -> u32 {
        self.nonce
    }

    pub fn set_nonce(&mut self, nonce: &u32) {
        self.nonce = nonce.clone();
    }

    pub fn set_timestamp(&mut self, timestamp: &u128) {
        self.timestamp = timestamp.clone();
    }

    pub fn genesis(state_root: H256) -> Self {
        let zero_hash = H256::from([0u8; 32]);
        let mut difficulty_bytes = [255u8; 32];
        for i in  0..3 {
            difficulty_bytes[i] = 0;
        }
        let genesis_difficulty = H256::from(difficulty_bytes);

        let data = Vec::new();
        let merkle_root = MerkleTree::new(&data).root();
        let coinbase = Transaction::default();

        Block {
            parent: zero_hash,
            nonce: 0,
            difficulty: genesis_difficulty,
            timestamp: 0,
            merkle_root: merkle_root,
            state_root: state_root,
            coinbase: coinbase,
            data,
        }
    }
}

