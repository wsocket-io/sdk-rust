# wSocket Rust SDK

Official Rust SDK for [wSocket](https://wsocket.io) — Realtime Pub/Sub over WebSockets.

[![Crates.io](https://img.shields.io/crates/v/wsocket-sdk)](https://crates.io/crates/wsocket-sdk)
[![docs.rs](https://docs.rs/wsocket-sdk/badge.svg)](https://docs.rs/wsocket-sdk)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
wsocket-sdk = "0.1.0"
```

## Quick Start

```rust
use wsocket_sdk::Client;

#[tokio::main]
async fn main() {
    let client = Client::connect("wss://your-server.com", "your-api-key")
        .await
        .unwrap();

    let chat = client.channel("chat:general").await;

    chat.subscribe(|data, meta| {
        println!("[{}] {}", meta.channel, data);
    }).await;

    chat.publish(serde_json::json!({"text": "Hello from Rust!"})).await;

    client.wait().await;
}
```

## Features

- **Pub/Sub** — Subscribe and publish to channels in real-time
- **Presence** — Track who is online in a channel
- **History** — Retrieve past messages
- **Connection Recovery** — Automatic reconnection with message replay
- **Async** — Built on `tokio` and `tokio-tungstenite`

## Presence

```rust
let chat = client.channel("chat:general").await;

chat.presence().on_enter(|m| {
    println!("joined: {}", m.client_id);
}).await;

chat.presence().on_leave(|m| {
    println!("left: {}", m.client_id);
}).await;

chat.presence().enter(serde_json::json!({"name": "Alice"})).await;
let members = chat.presence().get().await;
```

## History

```rust
chat.on_history(|result| {
    for msg in &result.messages {
        println!("[{}] {}", msg.timestamp, msg.data);
    }
}).await;

chat.history(HistoryOptions { limit: Some(50), ..Default::default() }).await;
```

## Requirements

- Rust edition 2021+
- `tokio`, `tokio-tungstenite`, `serde`, `serde_json`

## Development

```bash
cargo build
cargo test
```

## License

MIT
