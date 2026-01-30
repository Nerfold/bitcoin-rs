use std::sync::Arc;
use ring::signature::KeyPair;
use crate::types::transaction::{Transaction, SignedTransaction, sign};
use crate::types::address::Address;
use crate::types::hash::Hashable;

// 别名
pub type RKeyPair = ring::signature::Ed25519KeyPair;

pub struct Wallet {
    key_pair: Arc<RKeyPair>,
}

impl Wallet {
    /// 创建新钱包，仅持有密钥对
    pub fn new(key_pair: RKeyPair) -> Self {
        Self {
            key_pair: Arc::new(key_pair),
        }
    }

    pub fn get_public_key_bytes(&self) -> Vec<u8> {
        self.key_pair.public_key().as_ref().to_vec()
    }

    pub fn get_my_address(&self) -> Address {
        Address::from_public_key_bytes(self.key_pair.public_key().as_ref())
    }

    /// 本地构建并签名交易
    /// 注意：nonce 和 balance 必须由外部（Client）通过查询 API 提供
    pub fn create_signed_transaction(
        &self,
        receiver: Address,
        amount: u64,
        fee_price: u64,
        fee_limit: u64,
        nonce: u64, // 必须从网络获取当前的 nonce
    ) -> SignedTransaction {
        
        let t = Transaction::new(
            nonce + 1, // 交易 nonce 通常是当前状态 nonce + 1
            fee_price,
            fee_limit,
            receiver,
            amount, 
            vec![]
        );

        let signature = sign(&t, &self.key_pair);
        
        SignedTransaction {
            transaction: t,
            signature: signature.as_ref().to_vec(),
            public_key: self.key_pair.public_key().as_ref().to_vec(),
        }
    }
}
