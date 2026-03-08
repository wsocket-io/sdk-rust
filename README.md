# wSocket Rust SDK

Official Rust SDK for [wSocket](https://wsocket.io) — Realtime Pub/Sub over WebSockets.

[![Crates.io](https://img.shields.io/crates/v/wsocket-io)](https://crates.io/crates/wsocket-io)
[![docs.rs](https://docs.rs/wsocket-io/badge.svg)](https://docs.rs/wsocket-io)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
wsocket-io = "0.1.0"
```

## Quick Start

```rust
use wsocket_io::Client;

#[tokio::main]
async fn main() {
    let client = Client::connect("wss://node00.wsocket.online", "your-api-key")
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

## Push Notifications

```rust
let push = PushClient::new("https://your-server.com", "your-api-key", "your-app-id");

// Register & send
push.register_fcm("device-token", "user-123").await?;
push.send_to_member("user-123", json!({"title": "Hello"})).await?;
push.broadcast(json!({"title": "News"})).await?;

// Channel targeting
push.add_channel("subscription-id", "alerts").await?;
push.remove_channel("subscription-id", "alerts").await?;

// VAPID key & subscriptions
let vapid_key = push.get_vapid_key().await?;
let subs = push.list_subscriptions("user-123").await?;
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
