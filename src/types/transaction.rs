use serde::{Serialize,Deserialize};
use ring::signature::{Ed25519KeyPair, Signature, UnparsedPublicKey, KeyPair};
use rand::Rng;
use bincode;
use crate::types::address::Address;
use crate::types::hash::{H256, Hashable};
use ring::digest;

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Transaction {
    pub nonce: u64,
    pub gas_price: u64,
    pub gas_limit: u64,
    pub to: Address,
    pub value: u64,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct SignedTransaction {
    pub transaction: Transaction,
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}


impl Transaction {
    pub fn new(nonce: u64, gas_price: u64, gas_limit: u64, to: Address, value: u64, data: Vec<u8>) -> Self {
        Transaction {
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            data,
        }
    }

    
}

impl SignedTransaction {

    pub fn sender_address(&self) -> Address {
        Address::from_public_key_bytes(&self.public_key)
    }
    
    /// 验证交易合法性
    pub fn verify(&self) -> bool {
        verify(&self.transaction, &self.public_key, &self.signature)
    }

}

impl Hashable for Transaction {
    fn hash(&self) -> H256 {
        let encoded: Vec<u8> = bincode::serialize(&self).expect("Serialization failed");
        let digest: H256 = digest::digest(&digest::SHA256, &encoded).into();
        digest
    }
}


impl Hashable for SignedTransaction {
    fn hash(&self) -> H256 {
        let encoded: Vec<u8> = bincode::serialize(&self).expect("Serialization failed");
        let digest: H256 = digest::digest(&digest::SHA256, &encoded).into();
        digest
    }
}


/// Create digital signature of a transaction
pub fn sign(t: &Transaction, key: &Ed25519KeyPair) -> Signature {
    let bytes_to_sign = bincode::serialize(t).expect("error in sign");
    key.sign(&bytes_to_sign)
}

/// Verify digital signature of a transaction, using public key instead of secret key
pub fn verify(t: &Transaction, public_key: &[u8], signature: &[u8]) -> bool {
    let bytes_to_verify = bincode::serialize(t).expect("error in verify");
    let key = UnparsedPublicKey::new(&ring::signature::ED25519, public_key);
    key.verify(&bytes_to_verify, signature).is_ok()
}




