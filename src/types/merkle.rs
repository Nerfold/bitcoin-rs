use super::hash::{Hashable, H256};
use serde::{Serialize, Deserialize};

/// A Merkle tree.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MerkleTree {
    values: Vec<Vec<H256>>
}

fn hash_pair(left: &H256, right: &H256) -> H256 {
    let mut combined = [0u8; 64];
    combined[0..32].copy_from_slice(left.as_ref());
    combined[32..64].copy_from_slice(right.as_ref());
    ring::digest::digest(&ring::digest::SHA256, &combined).into()
}


impl MerkleTree {

    pub fn new<T>(data: &[T]) -> Self where T: Hashable, {
        if data.is_empty() {
            return Self::default();
        }

        let leaves: Vec<H256> = data.iter().map(
            |item| item.hash()
        ).collect();

        let mut values: Vec<Vec<H256>> = vec![leaves];

        while values.last().unwrap().len() > 1 {
            let current = values.last().unwrap();
            let mut next: Vec<H256> = Vec::new();
            for chunk in current.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() == 2 {
                    chunk[1]
                } else {
                    chunk[0]
                };
                let parent = hash_pair(&left, &right);
                next.push(parent);
            }
            values.push(next);
        }

        Self{values}
     
    }

    pub fn root(&self) -> H256 {
        if self.values.is_empty() {
            return H256::default();
        }
        self.values.last().unwrap()[0]
    }

    /// Returns the Merkle Proof of data at index i
    pub fn proof(&self, index: usize) -> Vec<H256> {
        let mut proof = Vec::new();
        let mut cur_idx = index;
        if self.values.is_empty() || index >= self.values[0].len() {
            return proof;   //return an empty proof
        }

        for level in 0..self.values.len() - 1 {
            let cur_level = &self.values[level];
            let sib_idx = cur_idx ^ 1;
            if sib_idx < cur_level.len() {
                proof.push(cur_level[sib_idx]);
            } else {
                proof.push(cur_level[cur_idx]);
            }
            cur_idx /= 2;
        }
        proof
    }
}

/// Verify that the datum hash with a vector of proofs will produce the Merkle root. Also need the
/// index of datum and `leaf_size`, the total number of leaves.
pub fn verify(root: &H256, datum: &H256, proof: &[H256], index: usize, leaf_size: usize) -> bool {
    if index >= leaf_size {
        return false;
    }

    let mut cur_hash = *datum;
    let mut cur_idx = index;
    for proof_hash in proof {
        if cur_idx % 2 == 0 {
            cur_hash = hash_pair(&cur_hash, proof_hash);
        } else {
            cur_hash = hash_pair(proof_hash, &cur_hash);
        }
        cur_idx /= 2;
    }
    cur_hash == *root
}
