use serde::{Deserialize, Serialize};
use crate::blockchain::Blockchain;
use crate::miner::Handle as MinerHandle;
use crate::network::server::Handle as NetworkServerHandle;
use crate::network::message::Message;
use crate::types::hash::Hashable;
use crate::types::transaction::SignedTransaction;
use crate::types::address::Address;
use crate::types::mempool::Mempool; // 引入 Mempool

use log::{info, error, warn};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use tiny_http::{Header, Response, Server as HTTPServer, Request, Method};
use url::Url;
use std::convert::TryInto;

pub struct Server {
    handle: HTTPServer,
    miner: MinerHandle,
    network: NetworkServerHandle,
    blockchain: Arc<Mutex<Blockchain>>,
    mempool: Arc<Mutex<Mempool>>, // Server 需要访问 Mempool 插入交易
}

#[derive(Serialize)]
struct ApiResponse<T> {
    success: bool,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

#[derive(Serialize)]
struct AccountInfo {
    address: String,
    nonce: u64,
    balance: u64,
}

impl Server {
    pub fn start(
        addr: std::net::SocketAddr,
        miner: &MinerHandle,
        network: &NetworkServerHandle,
        blockchain: &Arc<Mutex<Blockchain>>,
        mempool: &Arc<Mutex<Mempool>>, // 传入 Mempool
    ) {
        let handle = HTTPServer::http(&addr).unwrap();
        let server = Self {
            handle,
            miner: miner.clone(),
            network: network.clone(),
            blockchain: Arc::clone(blockchain),
            mempool: Arc::clone(mempool),
        };

        info!("API Server started at http://{}", addr);

        thread::spawn(move || {
            for mut req in server.handle.incoming_requests() {
                let miner = server.miner.clone();
                let network = server.network.clone();
                let blockchain = server.blockchain.clone();
                let mempool = server.mempool.clone();

                let response = handle_request(&mut req, &miner, &network, &blockchain, &mempool, addr);
                if let Err(e) = req.respond(response) {
                    error!("Failed to send response: {}", e);
                }
            }
        });
    }
}

fn handle_request(
    req: &mut Request,
    miner: &MinerHandle,
    network: &NetworkServerHandle,
    blockchain: &Arc<Mutex<Blockchain>>,
    mempool: &Arc<Mutex<Mempool>>,
    addr: std::net::SocketAddr
) -> Response<std::io::Cursor<Vec<u8>>> {
    
    let base_url = Url::parse(&format!("http://{}/", &addr)).unwrap();
    let url = match base_url.join(req.url()) {
        Ok(u) => u,
        Err(e) => return json_response::<()>(false, &format!("Invalid URL: {}", e), None),
    };

    match (req.method(), url.path()) {
        // --- Miner ---
        (Method::Get, "/miner/start") => {
            let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
            let lambda = params.get("lambda")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            
            miner.start(lambda);
            json_response::<()>(true, "Miner started", None)
        }
        (Method::Get, "/miner/stop") => {
            miner.stop();
            json_response::<()>(true, "Miner stopped", None)
        }
        // Update (新增 - 强制刷新)
        (Method::Get, "/miner/update") => {
            miner.update();
            json_response::<()>(true, "Miner update signal sent", None)
        }

        // --- Network ---
        (Method::Get, "/network/ping") => {
            network.broadcast(Message::Ping(String::from("API Ping")));
            json_response::<()>(true, "Ping broadcasted", None)
        }

        // --- Blockchain ---
        (Method::Get, "/blockchain/longest-chain") => {
            let chain = blockchain.lock().unwrap();
            let v = chain.all_blocks_in_longest_chain();
            let v_string: Vec<String> = v.into_iter().map(|h| h.to_string()).collect();
            json_response(true, "Longest chain fetched", Some(v_string))
        }
        (Method::Get, "/blockchain/block") => {
            let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
            let hash_str = match params.get("hash") {
                Some(h) => h,
                None => return json_response::<()>(false, "Missing hash parameter", None),
            };

            let hash_vec = match hex::decode(hash_str) {
                Ok(v) => v,
                Err(_) => return json_response::<()>(false, "Invalid hex format", None),
            };
            
            let hash_array: [u8; 32] = match hash_vec.try_into() {
                Ok(a) => a,
                Err(_) => return json_response::<()>(false, "Hash must be 32 bytes", None),
            };
            let h256 = crate::types::hash::H256::from(hash_array);

            let chain = blockchain.lock().unwrap();
            match chain.get_block(&h256) {
                Some(block) => json_response(true, "Block found", Some(block)),
                None => json_response::<()>(false, "Block not found", None),
            }
        }

        // 获取账户状态 (Client 需要 nonce 和 balance 来构建交易)
        (Method::Get, "/blockchain/account") => {
            let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
            let addr_str = match params.get("address") {
                Some(a) => a,
                None => return json_response::<()>(false, "Missing address parameter", None),
            };

            let recipient_bytes = match hex::decode(addr_str) {
                Ok(b) => b,
                Err(_) => return json_response::<()>(false, "Invalid hex address", None),
            };
            let byte_array: [u8; 20] = match recipient_bytes.try_into() {
                Ok(arr) => arr,
                Err(_) => return json_response::<()>(false, "Address must be 20 bytes", None),
            };
            let address = Address::from(byte_array);

            let chain = blockchain.lock().unwrap();
            let account = chain.get_account(&address);
            
            let info = AccountInfo {
                address: addr_str.to_string(),
                nonce: account.nonce,
                balance: account.balance,
            };
            json_response(true, "Account info", Some(info))
        }

        // 提交已签名的交易
        (Method::Post, "/transaction/submit") => {
            let mut content = String::new();
            req.as_reader().read_to_string(&mut content).unwrap();
            
            // 反序列化为 SignedTransaction
            let tx: SignedTransaction = match serde_json::from_str(&content) {
                Ok(t) => t,
                Err(e) => return json_response::<()>(false, &format!("Invalid Transaction JSON: {}", e), None),
            };

            // 验证签名 (Server 端的安全防线)
            if !tx.verify() {
                warn!("Received transaction with invalid signature");
                return json_response::<()>(false, "Invalid signature", None);
            }

            let hash = tx.hash();
            
            // 插入 Mempool
            {
                let mut mp = mempool.lock().unwrap();
                mp.insert(tx);
            }

            // 广播给 P2P 网络
            network.broadcast(Message::NewTransactionHashes(vec![hash]));

            json_response(true, "Transaction submitted", Some(hash.to_string()))
        }

        _ => {
            let payload = ApiResponse::<()> {
                success: false,
                message: "Endpoint not found".to_string(),
                data: None,
            };
            let data = serde_json::to_string_pretty(&payload).unwrap();
            Response::from_string(data)
                .with_header("Content-Type: application/json".parse::<Header>().unwrap())
                .with_status_code(404)
        }
    }
}

fn json_response<T: Serialize>(success: bool, message: &str, data: Option<T>) -> Response<std::io::Cursor<Vec<u8>>> {
    let payload = ApiResponse {
        success,
        message: message.to_string(),
        data,
    };
    let json = serde_json::to_string_pretty(&payload).unwrap();
    Response::from_string(json)
        .with_header("Content-Type: application/json".parse::<Header>().unwrap())
}
