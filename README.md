# bitcoin-rs
blockchain course pj 

A simplified but functional blockchain implementation written in Rust. This project demonstrates core blockchain concepts including Proof-of-Work (PoW) consensus, P2P networking, and persistent storage.

While inspired by Bitcoin's architecture, this implementation features an Ethereum-style Account Model (State Trie) rather than the UTXO model, supporting balance checks and nonce management natively.

> **Note**: This project is based on the COS-ECE470 course project, with enhancements such as data persistence (Sled DB), a CLI wallet, and state management.

## Key Features

- **Core Blockchain**: Proof-of-Work consensus algorithm with difficulty adjustment.
- **Account Model**: Implements a global state trie (Merkle Binary Tree) to manage account balances and nonces, similar to Ethereum.
- **P2P Network**: Decentralized node communication with gossip protocol for blocks and transactions.
- **Persistence**: Uses [Sled](https://github.com/spacejam/sled) for high-performance, embedded database storage.
- **Architecture**: Decoupled design separating the Backend (Miner/P2P/DB) from the Frontend (Wallet/CLI).
- **Miner**: Multi-threaded miner with adjustable block generation intervals.

## Architecture

The system consists of two main components communicating via a local API:

1. **Server (Backend)**: Handles the P2P network, blockchain synchronization, database (Sled), and mining worker.
2. **Client (Frontend/Wallet)**: A CLI tool for user authentication, key generation, and transaction signing.

## Getting Started

### Prerequisites

- **Rust & Cargo**: Ensure you have the latest stable version.

  Bash

  ```
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

- **GCC**: Required for building dependencies on Linux.

  - *MacOS users*: `brew install rust`

### Installation

Clone the repository and prepare the database directories.

Bash

```
git clone <your-repo-url>
cd <your-repo-name>

# Create directories for node storage
mkdir -p db/db1 db/db2
```

### Running the Network

To simulate a network, we will run two nodes locally.

#### 1. Start the First Node (Genesis Node)

**Step A: Start the Server** Open a terminal (Terminal 1) and run the backend server.

Bash

```
cargo run -- server \
  --p2p 127.0.0.1:6000 \
  --api 127.0.0.1:7000 \
  --data db/db1
```

**Step B: Start the Client (Wallet)** Open a new terminal (Terminal 2) and connect to the server.

Bash

```
cargo run -- client --api 127.0.0.1:7000
```

- **Authentication**: Press `Enter` to generate a new key pair, or paste a private key if you have one.
- **Commands**:
  - Type `info` to see your public key (address).
  - Type `miner start 0` to start mining immediately.

#### 2. Start the Second Node (Peer)

**Step A: Start the Server** Open Terminal 3. Note that we specify `--connect` to join the first node.

Bash

```
cargo run -- server \
  --p2p 127.0.0.1:6001 \
  --api 127.0.0.1:7001 \
  --data db/db2 \
  --connect 127.0.0.1:6000
```

**Step B: Start the Client** Open Terminal 4.

Bash

```
cargo run -- client --api 127.0.0.1:7001
```

### CLI Commands & Interaction

Once the client is running, you can interact with the blockchain:

- **Mining Control**:
  - `miner start 0`: Start mining continuously.
  - `miner start <t>`: Start mining with a specific interval `t`.
  - `miner stop`: Stop the miner.
- **Info**:
  - `chain`: Display the longest chain info.
  - `balance`: Check current account balance and nonce.
  - `info`: Show current node credentials.

#### Genesis / God Address

For testing purposes, you can use the pre-funded Genesis account:

- **Seed**: `f1e8ef289734f9ed1310a71227d8ac9207651ba59f38138db273ed7cd94b8c81`
- **Address**: `67d39da22d106b686c4f301b6f357600d28fc104`

------

## Technical Details

### Data Structures

- **Block**: Contains Header (Parent Hash, Nonce, Difficulty, Timestamp, Merkle Root, **State Root**) and Body (Transactions).
- **Transaction**: Similar to Ethereum (Nonce, Gas Price, Gas Limit, To, Value, Data).
- **State Trie**: A flattened Merkle Binary Tree stored in the database. It maps addresses to `Account` structs (`nonce`, `balance`). This allows the blockchain to verify the global state after every block execution.

### Consensus & Verification

- **Execution**: Blocks are executed to verify transactions. State transitions (balance changes) are calculated, and the resulting State Root is compared against the block header.
- **Atomic Updates**: The `Blockchain` struct ensures that block commitment and state tree updates are atomic.

### P2P Protocol

Implemented in `network/worker`, handling message types such as:

- `Ping/Pong`: Peer discovery.
- `NewBlockHashes/GetBlocks`: Block propagation.
- `Transactions`: Mempool synchronization.
- **Sync Logic**: On startup, nodes sync `BlockHeight`. If behind, the node requests the full blockchain from the peer with the longest chain.

### Storage

The project uses **Sled**, an embedded KV database.

- `blocks`: Stores serialized blocks.
- `state_nodes`: Stores nodes of the State Merkle Tree.
- `meta`: Stores metadata like the current chain tip.

