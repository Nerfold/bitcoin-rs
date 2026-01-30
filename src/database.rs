use sled::{Db, IVec, Tree};
use serde::{Serialize, Deserialize};
use crate::types::hash::H256;
use std::path::Path;

// 定义 Bucket (类似 SQL 的表)
const BLOCK_TREE: &str = "blocks";
const STATE_TREE: &str = "state_nodes";
const META_TREE: &str = "meta";

#[derive(Clone)]
pub struct Storage {
    db: Db,
    pub blocks: Tree,
    pub state_nodes: Tree,
    pub meta: Tree,
}

impl Storage {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let db = sled::Config::default()
            .path(path)
            .flush_every_ms(Some(1000)) // 每1000ms自动刷一次盘
            .open()
            .expect("Failed to open database");
        let blocks = db.open_tree(BLOCK_TREE).expect("Failed to open block tree");
        let state_nodes = db.open_tree(STATE_TREE).expect("Failed to open state tree");
        let meta = db.open_tree(META_TREE).expect("Failed to open meta tree");

        Self { db, blocks, state_nodes, meta }
    }

    
    pub fn insert_item<T: Serialize>(&self, tree: &Tree, key: &[u8], value: &T) {
        let bytes = bincode::serialize(value).expect("Serialization failed");
        tree.insert(key, bytes).expect("DB insert failed");
    }

    pub fn get_item<T: for<'a> Deserialize<'a>>(&self, tree: &Tree, key: &[u8]) -> Option<T> {
        match tree.get(key).expect("DB read failed") {
            Some(data) => Some(bincode::deserialize(&data).expect("Deserialization failed")),
            None => None,
        }
    }


    pub fn save_state_node<T: Serialize>(&self, hash: &H256, node: &T) {
        self.insert_item(&self.state_nodes, hash.as_ref(), node);
    }

    // 批量写入状态节点 (原子操作)
    pub fn batch_save_state_nodes<T: Serialize>(&self, nodes: &std::collections::HashMap<H256, T>) {
        let mut batch = sled::Batch::default();
        for (hash, node) in nodes {
            let bytes = bincode::serialize(node).unwrap();
            batch.insert(hash.as_ref(), bytes);
        }
        self.state_nodes.apply_batch(batch).expect("Batch apply failed");
    }

    pub fn get_state_node<T: for<'a> Deserialize<'a>>(&self, hash: &H256) -> Option<T> {
        self.get_item(&self.state_nodes, hash.as_ref())
    }

    // Tip Hash 用于重启恢复
    pub fn save_tip(&self, hash: &H256) {
        self.insert_item(&self.meta, b"tip", hash);
    }

    pub fn get_tip(&self) -> Option<H256> {
        self.get_item(&self.meta, b"tip")
    }

    // 确保数据落盘
    pub fn flush(&self) {
        self.db.flush().expect("Flush failed");
    }
}