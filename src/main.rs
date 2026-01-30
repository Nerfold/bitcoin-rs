// main.rs
#[cfg(test)]
#[macro_use]
extern crate hex_literal;

pub mod api;
pub mod blockchain;
pub mod types;
pub mod miner;
pub mod network;
pub mod wallet;
pub mod database;

use clap::{clap_app, ArgMatches};
use log::{error, info, warn};
use std::net;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use crate::blockchain::Blockchain;
use crate::types::mempool::Mempool;
use crate::network::message::Message;
use std::sync::atomic::{AtomicBool, Ordering};
use std::convert::TryInto;

// Client 和 Server 都需要用到
use crate::wallet::wallet::Wallet; 
use ring::signature::Ed25519KeyPair;
use ring::rand::SystemRandom;
use ring::rand::SecureRandom; // 确保引用 SecureRandom trait

// 定义默认 API 地址
const DEFAULT_API_ADDR: &str = "127.0.0.1:7000";

fn main() {
    let matches = clap_app!(Bitcoin =>
        (version: "0.2")
        (about: "Bitcoin client & server")
        (@subcommand server =>
            (about: "Runs the Bitcoin Node")
            (@arg verbose: -v ... "Increases the verbosity of logging")
            (@arg peer_addr: --p2p [ADDR] default_value("127.0.0.1:6000") "P2P listening address")
            (@arg api_addr: --api [ADDR] default_value(DEFAULT_API_ADDR) "API listening address")
            (@arg known_peer: -c --connect ... [PEER] "Peers to connect to")
            (@arg p2p_workers: --("p2p-workers") [INT] default_value("4") "Number of P2P workers")
            (@arg data_dir: --data [PATH] default_value("./db/db1") "Path to database directory")
        )
        (@subcommand client =>
            (about: "Interactive wallet to control the node")
            (@arg api_addr: --api [ADDR] default_value(DEFAULT_API_ADDR) "Target API address")
        )
    ).get_matches();

    match matches.subcommand() {
        ("server", Some(sub_m)) => run_server(sub_m),
        ("client", Some(sub_m)) => run_client(sub_m),
        _ => {
            println!("Please specify 'server' or 'client'. See --help.");
        }
    }
}

// --- Server Logic ---
fn run_server(matches: &ArgMatches) {
    let verbosity = matches.occurrences_of("verbose") as usize;
    stderrlog::new().verbosity(verbosity).init().unwrap();

    let p2p_addr = matches.value_of("peer_addr").unwrap().parse::<net::SocketAddr>().expect("Invalid P2P Address");
    let api_addr = matches.value_of("api_addr").unwrap().parse::<net::SocketAddr>().expect("Invalid API Address");
    let p2p_workers = matches.value_of("p2p_workers").unwrap().parse::<usize>().expect("Invalid Worker Count");
    let data_dir = matches.value_of("data_dir").unwrap();

    // 核心组件初始化
    let blockchain = Arc::new(Mutex::new(Blockchain::new(data_dir)));
    let mempool = Arc::new(Mutex::new(Mempool::new()));

    // Network Server
    let (msg_tx, msg_rx) = smol::channel::bounded(10000);
    let (server_ctx, server) = network::server::new(p2p_addr, msg_tx).unwrap();
    server_ctx.start().unwrap();

    println!("==========================================================");
    println!("⛏️  MINER CONFIGURATION");
    println!("Please enter the HEX ADDRESS to receive mining rewards.");
    println!("(If you don't have one, run 'client' in another terminal to generate one)");
    println!("==========================================================");

    let addr_input = rpassword::prompt_password("Miner Address > ").unwrap();
    let addr_input = addr_input.trim();
    
    // 验证地址格式
    let miner_address = if addr_input.is_empty() {
        warn!("No address provided. Using a dummy address (00...00). Rewards will be lost!");
        crate::types::address::Address::from([0u8; 20])
    } else {
        let bytes = hex::decode(addr_input).expect("Invalid Hex format");
        let array: [u8; 20] = bytes.try_into().expect("Address must be 20 bytes");
        crate::types::address::Address::from(array)
    };

    info!("Miner configured to receive rewards at: {:?}", miner_address);

    // Miner & Workers (不再传入 Wallet，只传入 Address)
    let (miner_ctx, miner, finished_block_chan) = miner::new(&blockchain, &mempool, miner_address);
    let miner_worker_ctx = miner::worker::Worker::new(&server, finished_block_chan, &blockchain, &mempool, &miner);
    let worker_ctx = network::worker::Worker::new(p2p_workers, msg_rx, &server, &blockchain, &mempool, &miner);

    worker_ctx.start();

    // Known Peers logic (same as before)
    if let Some(known_peers) = matches.values_of("known_peer") {
        let known_peers: Vec<String> = known_peers.map(|x| x.to_owned()).collect();
        let server = server.clone();
        thread::spawn(move || {
            for peer in known_peers {
                let addr = match peer.parse::<net::SocketAddr>() {
                    Ok(x) => x,
                    Err(e) => { error!("Invalid peer: {}", e); continue; }
                };
                info!("Connect peer: {}", addr);
                match server.connect(addr) {
                    Ok(_) => {
                        server.broadcast(Message::GetBlockchain);
                        server.broadcast(Message::GetMempool);
                    }
                    Err(e) => warn!("Connect failed: {}", e),
                }
                thread::sleep(Duration::from_millis(500));
            }
        });
    }

    info!("Waiting for sync...");
    thread::sleep(Duration::from_secs(3)); 
    
    info!("Starting Miner...");
    miner_ctx.start();
    miner_worker_ctx.start();

    // API Server Start (不再传入 Wallet)
    api::Server::start(api_addr, &miner, &server, &blockchain, &mempool);

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        info!("Shutting down...");
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }
    
    {
        let chain = blockchain.lock().unwrap();
        chain.flush(); 
    }
    info!("Goodbye!");
}

// --- Client Logic (The Real Wallet) ---
use std::io::{self, Write};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct AccountResponse {
    success: bool,
    data: Option<AccountInfo>,
}

#[derive(Deserialize)]
struct AccountInfo {
    nonce: u64,
    balance: u64,
}

fn run_client(matches: &ArgMatches) {
    let api_addr = matches.value_of("api_addr").unwrap();
    let base_url = format!("http://{}", api_addr);

    println!("==========================================================");
    println!("🔐  WALLET LOGIN");
    println!("Enter your Private Key Seed (Hex) to login.");
    println!("(Press ENTER to generate a NEW key)");
    println!("==========================================================");

    let secret_input = rpassword::prompt_password("Seed > ").unwrap();
    let secret_input = secret_input.trim();
    let rng = SystemRandom::new();

    let keypair = if secret_input.is_empty() {
        let mut seed_bytes = [0u8; 32];
        rng.fill(&mut seed_bytes).unwrap();
        println!("\n[!] GENERATED NEW WALLET. SAVE THIS SEED:\n{}\n", hex::encode(seed_bytes));
        Ed25519KeyPair::from_seed_unchecked(&seed_bytes).unwrap()
    } else {
        let seed_bytes = hex::decode(secret_input).expect("Invalid hex");
        Ed25519KeyPair::from_seed_unchecked(&seed_bytes).expect("Invalid seed")
    };

    // 初始化本地 Wallet (仅包含密钥)
    let wallet = Wallet::new(keypair);
    let my_address = wallet.get_my_address();
    println!("Wallet initialized. Address: {}", hex::encode(&my_address));

    let stdin = io::stdin();
    loop {
        print!("Bitcoin> ");
        io::stdout().flush().ok();

        let mut input = String::new();
        if stdin.read_line(&mut input).is_err() { break; }
        let input = input.trim();
        if input.is_empty() { continue; }

        let parts: Vec<&str> = input.split_whitespace().collect();
        match parts[0] {
            "help" => {
                println!("Commands:");
                println!("  info                    - Show local wallet address");
                println!("  balance                 - Query network for balance");
                println!("  transfer <addr> <amt>   - Create & Sign & Submit Tx");
                println!("  miner start <lambda>    - Control miner via API");
                println!("  miner stop              - Pause mining");
                println!("  miner update            - Force refresh block template");
                println!("  exit                    - Quit");
            }
            "exit" => break,
            "info" => {
                println!("My Address: {}", hex::encode(&my_address));
                println!("(Copy this address to the server to receive mining rewards)");
            },
            "balance" => {
                // 1. 调用 API 查询我的账户状态
                let url = format!("{}/blockchain/account?address={}", base_url, my_address);
                match reqwest::blocking::get(&url) {
                    Ok(resp) => {
                        let acc_res: AccountResponse = resp.json().unwrap_or(AccountResponse { success: false, data: None });
                        if let Some(info) = acc_res.data {
                            println!("Balance: {}, Nonce: {}", info.balance, info.nonce);
                        } else {
                            println!("Account not found or empty.");
                        }
                    },
                    Err(e) => println!("API Error: {}", e),
                }
            },
            "miner" => {
                if parts.len() < 2 {
                    println!("Usage: miner <start|stop|update> [lambda]");
                    continue;
                }
                
                let action = parts[1];
                let endpoint = match action {
                    "start" => {
                        let lambda = if parts.len() > 2 { parts[2] } else { "0" };
                        format!("{}/miner/start?lambda={}", base_url, lambda)
                    },
                    "stop" => format!("{}/miner/stop", base_url),
                    "update" => format!("{}/miner/update", base_url),
                    _ => {
                        println!("Unknown miner command. Use start, stop, or update.");
                        continue;
                    }
                };

                match reqwest::blocking::get(endpoint) {
                    Ok(resp) => println!("{}", resp.text().unwrap_or_default()),
                    Err(e) => println!("Error: {}", e),
                }
            },
            "chain" => {
                match reqwest::blocking::get(format!("{}/blockchain/longest-chain", base_url)) {
                    Ok(resp) => {
                        let json: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
                        
                        if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                            println!("Longest Chain (Height: {}):", data.len());
                            // 遍历打印所有哈希
                            for (i, hash) in data.iter().enumerate() {
                                println!("[{}] {}", i, hash.as_str().unwrap_or("?"));
                            }
                        } else {
                            println!("Failed to parse chain data.");
                        }
                    },
                    Err(e) => println!("Error: {}", e),
                }
            }
            "block" => {
                if parts.len() < 2 {
                    println!("Usage: block <hash>");
                    continue;
                }
                let hash = parts[1];
                let url = format!("{}/blockchain/block?hash={}", base_url, hash);
                
                match reqwest::blocking::get(&url) {
                    Ok(resp) => {
                        // 直接漂亮地打印 JSON
                        let json: serde_json::Value = resp.json().unwrap_or(serde_json::Value::Null);
                        if let Some(block_data) = json.get("data") {
                            println!("{}", serde_json::to_string_pretty(block_data).unwrap());
                        } else {
                            println!("Block not found or error.");
                        }
                    },
                    Err(e) => println!("Error: {}", e),
                }
            }
            "transfer" => {
                if parts.len() < 3 {
                    println!("Usage: transfer <to_addr_hex> <amount>");
                    continue;
                }
                let to_hex = parts[1];
                let amount: u64 = parts[2].parse().unwrap_or(0);

                // 解析目标地址
                let to_bytes = match hex::decode(to_hex) {
                    Ok(b) => b,
                    Err(_) => { println!("Invalid hex address"); continue; }
                };
                let to_array: [u8; 20] = match to_bytes.try_into() {
                    Ok(a) => a,
                    Err(_) => { println!("Address must be 20 bytes"); continue; }
                };
                let to_addr = crate::types::address::Address::from(to_array);

                // 1. 获取当前 Nonce 和 Balance
                println!("Fetching account state...");
                let url = format!("{}/blockchain/account?address={}", base_url, my_address);
                let acc_info = match reqwest::blocking::get(&url) {
                    Ok(resp) => {
                         let r: AccountResponse = resp.json().unwrap();
                         r.data.unwrap_or(AccountInfo { nonce: 0, balance: 0 })
                    },
                    Err(_) => { println!("Failed to fetch account info"); continue; }
                };

                if acc_info.balance < amount {
                    println!("Insufficient funds (Balance: {})", acc_info.balance);
                    continue;
                }

                // 2. 本地构造并签名交易
                println!("Signing transaction...");
                let signed_tx = wallet.create_signed_transaction(
                    to_addr,
                    amount,
                    1, // price
                    10, // limit
                    acc_info.nonce // 使用网络上的 nonce
                );

                // 3. 提交签名后的交易
                println!("Submitting transaction...");
                let client = reqwest::blocking::Client::new();
                let res = client.post(format!("{}/transaction/submit", base_url))
                    .json(&signed_tx) // 直接发送 SignedTransaction 对象
                    .send();

                match res {
                    Ok(resp) => println!("Response: {}", resp.text().unwrap_or_default()),
                    Err(e) => println!("Submission Failed: {}", e),
                }
            }
            _ => println!("Unknown command."),
        }
    }
}
