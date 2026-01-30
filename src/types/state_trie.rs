use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use crate::types::hash::{H256, Hashable};
use crate::types::address::Address;
use crate::blockchain::Account;
use crate::database::Storage;
use std::sync::Arc;
use std::hash::Hash;
use ring::digest;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum NodeData {
    Empty,
    Leaf(Address, Account),
    Branch(H256, H256), 
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub hash: H256,    
    pub data: NodeData,
}

impl Node {
    pub fn new(data: NodeData) -> Self {
        let hash = data.hash(); 
        Self { hash, data }
    }
}

impl Hashable for NodeData {
    fn hash(&self) -> H256 {
        let mut bytes = Vec::new();
        match self {
            NodeData::Empty => bytes.push(0x00),
            NodeData::Leaf(addr, acc) => {
                bytes.push(0x01);
                bytes.extend_from_slice(addr.as_ref());
                bytes.extend_from_slice(acc.hash().as_ref());
            }
            NodeData::Branch(l, r) => {
                bytes.push(0x02);
                bytes.extend_from_slice(l.as_ref());
                bytes.extend_from_slice(r.as_ref());
            }
        }
        let hash = digest::digest(&digest::SHA256, &bytes);
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(hash.as_ref());
        H256::from(hash_bytes)
    }
}

// 定义别名方便使用
type UpdatePair = (Address, Account);

#[derive(Clone)]
pub struct StateTrie {
    pub root_hash: H256,
    storage: Arc<Storage>, // 持有 DB 引用用于懒加载读取
}

impl StateTrie {
    /// 创建一个新的空 Trie
    pub fn new(storage: Arc<Storage>) -> Self {
        let empty_node = Node::new(NodeData::Empty);
        storage.save_state_node(&empty_node.hash, &empty_node);
        Self {
            root_hash: empty_node.hash,
            storage,
        }
    }

    /// 从已有的 Root Hash 加载 Trie (用于切换分叉/回滚)
    pub fn new_from_root(root: H256, storage: Arc<Storage>) -> Self {
        Self {
            root_hash: root,
            storage,
        }
    }

    /// 获取账户余额
    pub fn get(&self, address: &Address) -> Option<Account> {
        self.get_recursive(self.root_hash, address, 0)
    }

    fn get_recursive(&self, node_hash: H256, key: &Address, depth: usize) -> Option<Account> {
        // 从 DB 读取节点 (Lazy Load)
        let node: Node = self.storage.get_state_node(&node_hash)?;

        match node.data {
            NodeData::Empty => None,
            NodeData::Leaf(leaf_addr, account) => {
                if leaf_addr == *key { Some(account) } else { None }
            }
            NodeData::Branch(left, right) => {
                let bit = get_bit_at(key, depth);
                if bit == 0 {
                    self.get_recursive(left, key, depth + 1)
                } else {
                    self.get_recursive(right, key, depth + 1)
                }
            }
        }
    }

    pub fn insert(&self, address: Address, account: Account) -> (H256, HashMap<H256, Node>) {
        let mut new_nodes = HashMap::new();
        let new_root = self.insert_recursive(self.root_hash, address, account, 0, &mut new_nodes);
        (new_root, new_nodes)
    }

    pub fn insert_batch(&self, updates: HashMap<Address, Account>) -> (H256, HashMap<H256, Node>) {
        let mut new_nodes = HashMap::new();
        let update_list: Vec<UpdatePair> = updates.into_iter().collect();

        if update_list.is_empty() {
            return (self.root_hash, new_nodes);
        }

        let new_root = self.insert_batch_recursive(
            self.root_hash, 
            &update_list, 
            0, 
            &mut new_nodes
        );

        (new_root, new_nodes)
    }

    fn insert_batch_recursive(
        &self,
        node_hash: H256,
        updates: &[UpdatePair], 
        depth: usize,
        new_nodes: &mut HashMap<H256, Node>,
    ) -> H256 {
        if updates.is_empty() {
            return node_hash;
        }

        let node = if let Some(n) = new_nodes.get(&node_hash) {
            n.clone()
        } else {
            self.storage.get_state_node(&node_hash).unwrap_or_else(|| Node::new(NodeData::Empty))
        };

        match node.data {
            NodeData::Empty => {
                self.build_subtree_from_scratch(updates, depth, new_nodes)
            }

            NodeData::Leaf(curr_addr, curr_acc) => {
                let overridden = updates.iter().any(|(addr, _)| *addr == curr_addr);

                if overridden {
                    self.build_subtree_from_scratch(updates, depth, new_nodes)
                } else {
                    let mut combined_updates = updates.to_vec();
                    combined_updates.push((curr_addr, curr_acc));
                    self.build_subtree_from_scratch(&combined_updates, depth, new_nodes)
                }
            }

            NodeData::Branch(left, right) => {
                // 切分数据
                let (left_updates, right_updates): (Vec<UpdatePair>, Vec<UpdatePair>) = 
                    updates.iter().cloned().partition(|(addr, _)| {
                        get_bit_at(addr, depth) == 0
                    });

                // 递归构建
                let new_left = self.insert_batch_recursive(left, &left_updates, depth + 1, new_nodes);
                let new_right = self.insert_batch_recursive(right, &right_updates, depth + 1, new_nodes);

                let new_node = Node::new(NodeData::Branch(new_left, new_right));
                new_nodes.insert(new_node.hash, new_node.clone());
                new_node.hash
            }
        }
    }

    fn build_subtree_from_scratch(
        &self,
        items: &[UpdatePair],
        depth: usize,
        new_nodes: &mut HashMap<H256, Node>
    ) -> H256 {
        if items.is_empty() {
            let empty = Node::new(NodeData::Empty);
            return empty.hash;
        }

        if items.len() == 1 {
            let (addr, acc) = &items[0];
            let leaf = Node::new(NodeData::Leaf(addr.clone(), acc.clone()));
            new_nodes.insert(leaf.hash, leaf.clone());
            return leaf.hash;
        }

        let (left_items, right_items): (Vec<UpdatePair>, Vec<UpdatePair>) = 
            items.iter().cloned().partition(|(addr, _)| {
                get_bit_at(addr, depth) == 0
            });

        let left_hash = self.build_subtree_from_scratch(&left_items, depth + 1, new_nodes);
        let right_hash = self.build_subtree_from_scratch(&right_items, depth + 1, new_nodes);

        let branch = Node::new(NodeData::Branch(left_hash, right_hash));
        new_nodes.insert(branch.hash, branch.clone());
        branch.hash
    }

    fn insert_recursive(
        &self, 
        node_hash: H256, 
        address: Address, 
        account: Account, 
        depth: usize,
        new_nodes: &mut HashMap<H256, Node>
    ) -> H256 {
        let node = if let Some(n) = new_nodes.get(&node_hash) {
            n.clone()
        } else {
            self.storage.get_state_node(&node_hash).unwrap_or_else(|| Node::new(NodeData::Empty))
        };

        match node.data {
            NodeData::Empty => {
                let new_node = Node::new(NodeData::Leaf(address, account));
                new_nodes.insert(new_node.hash, new_node.clone());
                new_node.hash
            },
            
            NodeData::Leaf(curr_addr, curr_acc) => {
                if curr_addr == address {
                    let new_node = Node::new(NodeData::Leaf(address, account));
                    new_nodes.insert(new_node.hash, new_node.clone());
                    new_node.hash
                } else {
                    let empty = Node::new(NodeData::Empty);
                    new_nodes.insert(empty.hash, empty.clone());
                    let branch_node = Node::new(NodeData::Branch(empty.hash, empty.hash));
                    let h1 = self.insert_recursive_on_data(branch_node, curr_addr, curr_acc, depth, new_nodes);
                    self.insert_recursive(h1, address, account, depth, new_nodes)
                }
            },
            
            NodeData::Branch(left, right) => {
                let bit = get_bit_at(&address, depth);
                let new_data = if bit == 0 {
                    let new_left = self.insert_recursive(left, address, account, depth + 1, new_nodes);
                    NodeData::Branch(new_left, right)
                } else {
                    let new_right = self.insert_recursive(right, address, account, depth + 1, new_nodes);
                    NodeData::Branch(left, new_right)
                };
                
                let new_node = Node::new(new_data);
                new_nodes.insert(new_node.hash, new_node.clone());
                new_node.hash
            }
        }
    }
    

    fn insert_recursive_on_data(
        &self, 
        node: Node, 
        address: Address, 
        account: Account, 
        depth: usize, 
        new_nodes: &mut HashMap<H256, Node>
    ) -> H256 {
        let new_data = match node.data {
            NodeData::Empty => NodeData::Leaf(address, account),
            NodeData::Leaf(curr_addr, curr_acc) => {
                if curr_addr == address {
                    NodeData::Leaf(address, account)
                } else {
                    let curr_bit = get_bit_at(&curr_addr, depth);
                    let new_bit = get_bit_at(&address, depth);

                    if curr_bit != new_bit {
                        let curr_node_new = Node::new(NodeData::Leaf(curr_addr, curr_acc));
                        let new_node_new = Node::new(NodeData::Leaf(address, account));
                        
                        new_nodes.insert(curr_node_new.hash, curr_node_new.clone());
                        new_nodes.insert(new_node_new.hash, new_node_new.clone());
                        
                        if new_bit == 0 {
                            NodeData::Branch(new_node_new.hash, curr_node_new.hash)
                        } else {
                            NodeData::Branch(curr_node_new.hash, new_node_new.hash)
                        }
                    } else {
                        let empty = Node::new(NodeData::Empty);
                        new_nodes.insert(empty.hash, empty.clone());
                        
                        let child_hash = self.insert_recursive_on_data(
                            node.clone(), 
                            address, 
                            account, 
                            depth + 1, 
                            new_nodes
                        );
                        
                        if new_bit == 0 {
                            NodeData::Branch(child_hash, empty.hash)
                        } else {
                            NodeData::Branch(empty.hash, child_hash)
                        }
                    }
                }
            },
            NodeData::Branch(left, right) => {
                let bit = get_bit_at(&address, depth);
                if bit == 0 {
                    let new_left = self.insert_recursive(left, address, account, depth + 1, new_nodes);
                    NodeData::Branch(new_left, right)
                } else {
                    let new_right = self.insert_recursive(right, address, account, depth + 1, new_nodes);
                    NodeData::Branch(left, new_right)
                }
            }
        };

        let new_node = Node::new(new_data);
        new_nodes.insert(new_node.hash, new_node.clone());
        new_node.hash
    }
} 

fn get_bit_at(data: &Address, index: usize) -> u8 {
    if index >= 160 { return 0; }
    let byte_index = index / 8;
    let bit_index = 7 - (index % 8);
    (data.as_ref()[byte_index] >> bit_index) & 1
}