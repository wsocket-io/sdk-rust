//! wSocket Rust SDK — Realtime Pub/Sub client with Presence, History, and Connection Recovery.
//!
//! # Example
//! ```no_run
//! use wsocket_io::Client;
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = Client::connect("ws://localhost:9001", "your-api-key").await.unwrap();
//!
//!     let chat = client.channel("chat:general").await;
//!     chat.subscribe(|data, meta| {
//!         println!("[{}] {}", meta.channel, data);
//!     }).await;
//!
//!     // Presence
//!     chat.presence().on_enter(|m| println!("joined: {}", m.client_id)).await;
//!     chat.presence().enter(serde_json::json!({"name": "Alice"})).await;
//!
//!     // History
//!     chat.on_history(|r| println!("{} messages", r.messages.len())).await;
//!     chat.history(HistoryOptions { limit: Some(50), ..Default::default() }).await;
//!
//!     client.wait().await;
//! }
//! ```

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

// ─── Types ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMeta {
    pub id: String,
    pub channel: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PresenceMember {
    #[serde(default, rename = "clientId")]
    pub client_id: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default, rename = "joinedAt")]
    pub joined_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default, rename = "publisherId")]
    pub publisher_id: String,
    #[serde(default)]
    pub timestamp: u64,
    #[serde(default)]
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub struct HistoryResult {
    pub channel: String,
    pub messages: Vec<HistoryMessage>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryOptions {
    pub limit: Option<u32>,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub direction: Option<String>,
}

type MessageCb = Arc<dyn Fn(Value, MessageMeta) + Send + Sync>;
type PresenceCb = Arc<dyn Fn(PresenceMember) + Send + Sync>;
type MembersCb = Arc<dyn Fn(Vec<PresenceMember>) + Send + Sync>;
type HistoryCb = Arc<dyn Fn(HistoryResult) + Send + Sync>;

// ─── Presence ───────────────────────────────────────────────

struct PresenceInner {
    enter_cbs: Vec<PresenceCb>,
    leave_cbs: Vec<PresenceCb>,
    update_cbs: Vec<PresenceCb>,
    members_cbs: Vec<MembersCb>,
}

#[derive(Clone)]
pub struct PresenceHandle {
    channel_name: String,
    inner: Arc<Mutex<PresenceInner>>,
    sender: Arc<Mutex<Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
}

impl PresenceHandle {
    fn new(
        channel_name: String,
        sender: Arc<Mutex<Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
    ) -> Self {
        Self {
            channel_name,
            inner: Arc::new(Mutex::new(PresenceInner {
                enter_cbs: Vec::new(),
                leave_cbs: Vec::new(),
                update_cbs: Vec::new(),
                members_cbs: Vec::new(),
            })),
            sender,
        }
    }

    /// Enter the presence set with optional data.
    pub async fn enter(&self, data: Value) {
        self.send_action("presence.enter", Some(data)).await;
    }

    /// Leave the presence set.
    pub async fn leave(&self) {
        self.send_action("presence.leave", None).await;
    }

    /// Update presence data.
    pub async fn update(&self, data: Value) {
        self.send_action("presence.update", Some(data)).await;
    }

    /// Get current members.
    pub async fn get(&self) {
        self.send_action("presence.get", None).await;
    }

    pub async fn on_enter<F: Fn(PresenceMember) + Send + Sync + 'static>(&self, cb: F) {
        self.inner.lock().await.enter_cbs.push(Arc::new(cb));
    }

    pub async fn on_leave<F: Fn(PresenceMember) + Send + Sync + 'static>(&self, cb: F) {
        self.inner.lock().await.leave_cbs.push(Arc::new(cb));
    }

    pub async fn on_update<F: Fn(PresenceMember) + Send + Sync + 'static>(&self, cb: F) {
        self.inner.lock().await.update_cbs.push(Arc::new(cb));
    }

    pub async fn on_members<F: Fn(Vec<PresenceMember>) + Send + Sync + 'static>(&self, cb: F) {
        self.inner.lock().await.members_cbs.push(Arc::new(cb));
    }

    async fn send_action(&self, action: &str, data: Option<Value>) {
        let mut msg = json!({ "action": action, "channel": self.channel_name });
        if let Some(d) = data {
            msg["data"] = d;
        }
        if let Some(ref mut sink) = *self.sender.lock().await {
            let _ = sink.send(Message::Text(msg.to_string())).await;
        }
    }

    async fn emit_enter(&self, member: PresenceMember) {
        let cbs = self.inner.lock().await.enter_cbs.clone();
        for cb in &cbs { cb(member.clone()); }
    }

    async fn emit_leave(&self, member: PresenceMember) {
        let cbs = self.inner.lock().await.leave_cbs.clone();
        for cb in &cbs { cb(member.clone()); }
    }

    async fn emit_update(&self, member: PresenceMember) {
        let cbs = self.inner.lock().await.update_cbs.clone();
        for cb in &cbs { cb(member.clone()); }
    }

    async fn emit_members(&self, members: Vec<PresenceMember>) {
        let cbs = self.inner.lock().await.members_cbs.clone();
        for cb in &cbs { cb(members.clone()); }
    }
}

// ─── Channel ────────────────────────────────────────────────

struct ChannelInner {
    subscribed: bool,
    callbacks: Vec<MessageCb>,
    history_cbs: Vec<HistoryCb>,
}

#[derive(Clone)]
pub struct ChannelHandle {
    pub name: String,
    inner: Arc<Mutex<ChannelInner>>,
    presence: PresenceHandle,
    sender: Arc<Mutex<Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
}

impl ChannelHandle {
    fn new(
        name: String,
        sender: Arc<Mutex<Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
    ) -> Self {
        let presence = PresenceHandle::new(name.clone(), sender.clone());
        Self {
            name,
            inner: Arc::new(Mutex::new(ChannelInner {
                subscribed: false,
                callbacks: Vec::new(),
                history_cbs: Vec::new(),
            })),
            presence,
            sender,
        }
    }

    /// Get the presence API for this channel.
    pub fn presence(&self) -> &PresenceHandle {
        &self.presence
    }

    /// Subscribe to messages on this channel.
    pub async fn subscribe<F: Fn(Value, MessageMeta) + Send + Sync + 'static>(&self, cb: F) {
        self.subscribe_with_rewind(cb, 0).await;
    }

    /// Subscribe with optional rewind.
    pub async fn subscribe_with_rewind<F: Fn(Value, MessageMeta) + Send + Sync + 'static>(
        &self, cb: F, rewind: u32,
    ) {
        let mut inner = self.inner.lock().await;
        inner.callbacks.push(Arc::new(cb));
        if !inner.subscribed {
            let mut msg = json!({ "action": "subscribe", "channel": self.name });
            if rewind > 0 { msg["rewind"] = json!(rewind); }
            if let Some(ref mut sink) = *self.sender.lock().await {
                let _ = sink.send(Message::Text(msg.to_string())).await;
            }
            inner.subscribed = true;
        }
    }

    /// Publish data to this channel.
    pub async fn publish(&self, data: Value) {
        self.publish_with_opts(data, true).await;
    }

    /// Publish with persist option.
    pub async fn publish_with_opts(&self, data: Value, persist: bool) {
        let mut msg = json!({
            "action": "publish",
            "channel": self.name,
            "data": data,
            "id": Uuid::new_v4().to_string(),
        });
        if !persist { msg["persist"] = json!(false); }
        if let Some(ref mut sink) = *self.sender.lock().await {
            let _ = sink.send(Message::Text(msg.to_string())).await;
        }
    }

    /// Query message history.
    pub async fn history(&self, opts: HistoryOptions) {
        let mut msg = json!({ "action": "history", "channel": self.name });
        if let Some(l) = opts.limit { msg["limit"] = json!(l); }
        if let Some(b) = opts.before { msg["before"] = json!(b); }
        if let Some(a) = opts.after { msg["after"] = json!(a); }
        if let Some(ref d) = opts.direction { msg["direction"] = json!(d); }
        if let Some(ref mut sink) = *self.sender.lock().await {
            let _ = sink.send(Message::Text(msg.to_string())).await;
        }
    }

    /// Listen for history query results.
    pub async fn on_history<F: Fn(HistoryResult) + Send + Sync + 'static>(&self, cb: F) {
        self.inner.lock().await.history_cbs.push(Arc::new(cb));
    }

    /// Unsubscribe from this channel.
    pub async fn unsubscribe(&self) {
        let msg = json!({ "action": "unsubscribe", "channel": self.name });
        if let Some(ref mut sink) = *self.sender.lock().await {
            let _ = sink.send(Message::Text(msg.to_string())).await;
        }
        let mut inner = self.inner.lock().await;
        inner.subscribed = false;
        inner.callbacks.clear();
        inner.history_cbs.clear();
    }

    async fn emit(&self, data: Value, meta: MessageMeta) {
        let cbs = self.inner.lock().await.callbacks.clone();
        for cb in &cbs { cb(data.clone(), meta.clone()); }
    }

    async fn emit_history(&self, result: HistoryResult) {
        let cbs = self.inner.lock().await.history_cbs.clone();
        for cb in &cbs { cb(result.clone()); }
    }

    async fn mark_for_resubscribe(&self) {
        self.inner.lock().await.subscribed = false;
    }

    async fn has_listeners(&self) -> bool {
        !self.inner.lock().await.callbacks.is_empty()
    }
}

// ─── PubSub Namespace ───────────────────────────────────────

/// Convenience proxy that exposes pub/sub channel access.
pub struct PubSubNamespace {
    channels: Arc<RwLock<HashMap<String, ChannelHandle>>>,
    sender: Arc<Mutex<Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
}

impl PubSubNamespace {
    /// Get or create a channel reference.
    pub async fn channel(&self, name: &str) -> ChannelHandle {
        let mut channels = self.channels.write().await;
        channels
            .entry(name.to_string())
            .or_insert_with(|| ChannelHandle::new(name.to_string(), self.sender.clone()))
            .clone()
    }
}

// ─── Client ─────────────────────────────────────────────────

pub struct Client {
    channels: Arc<RwLock<HashMap<String, ChannelHandle>>>,
    sender: Arc<Mutex<Option<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>>>,
    last_message_ts: Arc<RwLock<u64>>,
    resume_token: Arc<RwLock<Option<String>>>,
    read_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    ping_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Pub/Sub namespace — convenience proxy for `channel()`.
    pub pubsub: PubSubNamespace,
    /// Push notification client (configured via `configure_push()`).
    pub push: Arc<RwLock<Option<PushClient>>>,
}

impl Client {
    /// Connect to the wSocket server.
    pub async fn connect(url: &str, api_key: &str) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let ws_url = format!("{}/?key={}", url, api_key);
        let (ws, _) = connect_async(&ws_url).await?;
        let (sink, stream) = ws.split();

        let sender = Arc::new(Mutex::new(Some(sink)));
        let channels: Arc<RwLock<HashMap<String, ChannelHandle>>> = Arc::new(RwLock::new(HashMap::new()));
        let last_message_ts = Arc::new(RwLock::new(0u64));
        let resume_token: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));

        let client = Arc::new(Self {
            channels: channels.clone(),
            sender: sender.clone(),
            last_message_ts: last_message_ts.clone(),
            resume_token: resume_token.clone(),
            read_handle: Arc::new(Mutex::new(None)),
            ping_handle: Arc::new(Mutex::new(None)),
            pubsub: PubSubNamespace { channels: channels.clone(), sender: sender.clone() },
            push: Arc::new(RwLock::new(None)),
        });

        // Ping loop
        let ping_sender = sender.clone();
        let ping_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30));
            ticker.tick().await; // skip first immediate tick
            loop {
                ticker.tick().await;
                if let Some(ref mut sink) = *ping_sender.lock().await {
                    let msg = json!({"action": "ping"});
                    if sink.send(Message::Text(msg.to_string())).await.is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
        });
        *client.ping_handle.lock().await = Some(ping_handle);

        // Read loop
        let read_channels = channels.clone();
        let read_ts = last_message_ts.clone();
        let read_rt = resume_token.clone();
        let read_handle = tokio::spawn(async move {
            Self::read_loop(stream, read_channels, read_ts, read_rt).await;
        });
        *client.read_handle.lock().await = Some(read_handle);

        Ok(client)
    }

    /// Get or create a channel reference.
    #[deprecated(note = "Use client.pubsub.channel() instead")]
    pub async fn channel(&self, name: &str) -> ChannelHandle {
        let mut channels = self.channels.write().await;
        channels
            .entry(name.to_string())
            .or_insert_with(|| ChannelHandle::new(name.to_string(), self.sender.clone()))
            .clone()
    }

    /// Configure push notification access and return a reference to the PushClient.
    pub async fn configure_push(&self, base_url: &str, token: &str, app_id: &str) -> PushClient {
        let pc = PushClient::new(base_url, token, app_id);
        *self.push.write().await = Some(PushClient::new(base_url, token, app_id));
        pc
    }

    /// Wait for the read loop to finish (blocks forever unless disconnected).
    pub async fn wait(&self) {
        if let Some(handle) = self.read_handle.lock().await.take() {
            let _ = handle.await;
        }
    }

    /// Disconnect from the server.
    pub async fn disconnect(&self) {
        if let Some(h) = self.ping_handle.lock().await.take() { h.abort(); }
        if let Some(h) = self.read_handle.lock().await.take() { h.abort(); }
        if let Some(ref mut sink) = *self.sender.lock().await {
            let _ = sink.close().await;
        }
        *self.sender.lock().await = None;
    }

    async fn read_loop(
        mut stream: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        channels: Arc<RwLock<HashMap<String, ChannelHandle>>>,
        last_ts: Arc<RwLock<u64>>,
        resume_token: Arc<RwLock<Option<String>>>,
    ) {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Text(text) = msg {
                Self::handle_message(&text, &channels, &last_ts, &resume_token).await;
            }
        }
    }

    async fn handle_message(
        raw: &str,
        channels: &Arc<RwLock<HashMap<String, ChannelHandle>>>,
        last_ts: &Arc<RwLock<u64>>,
        resume_token: &Arc<RwLock<Option<String>>>,
    ) {
        let root: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => return,
        };
        let action = match root.get("action").and_then(|a| a.as_str()) {
            Some(a) => a,
            None => return,
        };
        let channel = root.get("channel").and_then(|c| c.as_str()).unwrap_or("");

        match action {
            "message" => {
                let chs = channels.read().await;
                if let Some(ch) = chs.get(channel) {
                    let data = root.get("data").cloned().unwrap_or(Value::Null);
                    let id = root.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let ts = root.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
                    {
                        let mut lts = last_ts.write().await;
                        if ts > *lts { *lts = ts; }
                    }
                    ch.emit(data, MessageMeta { id, channel: channel.to_string(), timestamp: ts }).await;
                }
            }
            "presence.enter" => {
                let chs = channels.read().await;
                if let Some(ch) = chs.get(channel) {
                    let member = Self::parse_presence_member(root.get("data"));
                    ch.presence().emit_enter(member).await;
                }
            }
            "presence.leave" => {
                let chs = channels.read().await;
                if let Some(ch) = chs.get(channel) {
                    let member = Self::parse_presence_member(root.get("data"));
                    ch.presence().emit_leave(member).await;
                }
            }
            "presence.update" => {
                let chs = channels.read().await;
                if let Some(ch) = chs.get(channel) {
                    let member = Self::parse_presence_member(root.get("data"));
                    ch.presence().emit_update(member).await;
                }
            }
            "presence.members" => {
                let chs = channels.read().await;
                if let Some(ch) = chs.get(channel) {
                    let members = match root.get("data").and_then(|d| d.as_array()) {
                        Some(arr) => arr.iter().map(|v| Self::parse_presence_member(Some(v))).collect(),
                        None => Vec::new(),
                    };
                    ch.presence().emit_members(members).await;
                }
            }
            "history" => {
                let chs = channels.read().await;
                if let Some(ch) = chs.get(channel) {
                    let result = Self::parse_history_result(root.get("data"), channel);
                    ch.emit_history(result).await;
                }
            }
            "ack" => {
                if root.get("id").and_then(|v| v.as_str()) == Some("resume") {
                    if let Some(token) = root.get("data").and_then(|d| d.get("resumeToken")).and_then(|t| t.as_str()) {
                        *resume_token.write().await = Some(token.to_string());
                    }
                }
            }
            "error" => {
                let err = root.get("error").and_then(|e| e.as_str()).unwrap_or("unknown");
                eprintln!("[wSocket] Error: {}", err);
            }
            _ => {}
        }
    }

    fn parse_presence_member(data: Option<&Value>) -> PresenceMember {
        match data {
            Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
            None => PresenceMember::default(),
        }
    }

    fn parse_history_result(data: Option<&Value>, channel: &str) -> HistoryResult {
        let mut result = HistoryResult {
            channel: channel.to_string(),
            messages: Vec::new(),
            has_more: false,
        };
        if let Some(obj) = data {
            if let Some(ch) = obj.get("channel").and_then(|c| c.as_str()) {
                result.channel = ch.to_string();
            }
            result.has_more = obj.get("hasMore").and_then(|h| h.as_bool()).unwrap_or(false);
            if let Some(msgs) = obj.get("messages").and_then(|m| m.as_array()) {
                for m in msgs {
                    result.messages.push(HistoryMessage {
                        id: m.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        channel: m.get("channel").and_then(|v| v.as_str()).unwrap_or(channel).to_string(),
                        data: m.get("data").cloned().unwrap_or(Value::Null),
                        publisher_id: m.get("publisherId").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        timestamp: m.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0),
                        sequence: m.get("sequence").and_then(|v| v.as_u64()).unwrap_or(0),
                    });
                }
            }
        }
        result
    }
}

// ─── Push Notifications ─────────────────────────────────────

/// REST-based push notification client for wSocket.
///
/// # Example
/// ```no_run
/// let push = wsocket::PushClient::new("http://localhost:9001", "admin-token", "app-id");
/// push.register_fcm("device-token", "user1").await?;
/// push.send_to_member("user1", "Hello", Some("World")).await?;
/// ```
pub struct PushClient {
    base_url: String,
    token: String,
    app_id: String,
    http: reqwest::Client,
}

/// Push notification payload fields.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PushPayload {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl PushClient {
    /// Create a new push notification client.
    pub fn new(base_url: &str, token: &str, app_id: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            app_id: app_id.to_string(),
            http: reqwest::Client::new(),
        }
    }

    async fn api(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, Box<dyn std::error::Error>> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.request(method, &url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(format!("Push API error {}: {}", status.as_u16(), text).into());
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Register an FCM device token (Android).
    pub async fn register_fcm(&self, device_token: &str, member_id: &str) -> Result<String, Box<dyn std::error::Error>> {
        let res = self.api(reqwest::Method::POST, "/api/push/register", Some(serde_json::json!({
            "platform": "fcm", "memberId": member_id, "deviceToken": device_token
        }))).await?;
        Ok(res["subscriptionId"].as_str().unwrap_or("").to_string())
    }

    /// Register an APNs device token (iOS).
    pub async fn register_apns(&self, device_token: &str, member_id: &str) -> Result<String, Box<dyn std::error::Error>> {
        let res = self.api(reqwest::Method::POST, "/api/push/register", Some(serde_json::json!({
            "platform": "apns", "memberId": member_id, "deviceToken": device_token
        }))).await?;
        Ok(res["subscriptionId"].as_str().unwrap_or("").to_string())
    }

    /// Unregister push subscriptions for a member.
    pub async fn unregister(&self, member_id: &str, platform: Option<&str>) -> Result<u64, Box<dyn std::error::Error>> {
        let mut body = serde_json::json!({ "memberId": member_id });
        if let Some(p) = platform {
            body["platform"] = Value::String(p.to_string());
        }
        let res = self.api(reqwest::Method::DELETE, "/api/push/unregister", Some(body)).await?;
        Ok(res["removed"].as_u64().unwrap_or(0))
    }

    /// Send a push notification to a specific member.
    pub async fn send_to_member(&self, member_id: &str, title: &str, body: Option<&str>) -> Result<Value, Box<dyn std::error::Error>> {
        self.api(reqwest::Method::POST, "/api/push/send", Some(serde_json::json!({
            "memberId": member_id, "title": title, "body": body
        }))).await
    }

    /// Broadcast a push notification to all app subscribers.
    pub async fn broadcast(&self, title: &str, body: Option<&str>) -> Result<Value, Box<dyn std::error::Error>> {
        self.api(reqwest::Method::POST, "/api/push/broadcast", Some(serde_json::json!({
            "title": title, "body": body
        }))).await
    }
}

