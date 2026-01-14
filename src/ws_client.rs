//! WebSocket client module
//!
//! Handles WebSocket connections for market data and user updates.
//! Supports auto-reconnection with configurable delay.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{sleep, Instant};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
    MaybeTlsStream, WebSocketStream,
};
use tracing::{error, info, warn};

use crate::auth::StandXAuth;

/// Default WebSocket URLs
pub const MARKET_WS_URL: &str = "wss://perps.standx.com/ws-stream/v1";
pub const USER_WS_URL: &str = "wss://perps.standx.com/ws-api/v1";

/// Default reconnect delay in seconds
const DEFAULT_RECONNECT_DELAY_SECS: u64 = 5;

/// Heartbeat log interval in seconds
const HEARTBEAT_LOG_INTERVAL_SECS: u64 = 10;

// ============================================================================
// Data Types
// ============================================================================

/// Price data from WebSocket
#[derive(Debug, Clone)]
pub struct PriceData {
    pub symbol: String,
    pub last_price: Decimal,
    pub mark_price: Decimal,
    pub timestamp: u64,
}

/// Order update from WebSocket
#[derive(Debug, Clone, Deserialize)]
pub struct OrderUpdate {
    pub cl_ord_id: String,
    pub status: String,
    pub side: String,
    pub symbol: String,
}

/// Position update from WebSocket
#[derive(Debug, Clone)]
pub struct PositionUpdate {
    pub symbol: String,
    pub qty: Decimal,
    pub upnl: Decimal,
}

// ============================================================================
// WebSocket Message Types
// ============================================================================

/// Subscribe message
#[derive(Debug, Serialize)]
struct SubscribeMessage {
    subscribe: SubscribeParams,
}

#[derive(Debug, Serialize)]
struct SubscribeParams {
    channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
}

/// Pong response message
#[derive(Debug, Serialize)]
struct PongMessage {
    pong: u64,
}

/// Generic WebSocket message for parsing
#[derive(Debug, Deserialize)]
struct WsMessage {
    #[serde(default)]
    ping: Option<u64>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    code: Option<i32>,
}

/// Price channel data
#[derive(Debug, Deserialize)]
struct PriceChannelData {
    symbol: String,
    #[serde(alias = "lastPrice", alias = "last_price")]
    last_price: String,
    #[serde(alias = "markPrice", alias = "mark_price")]
    mark_price: String,
    #[serde(default)]
    timestamp: Option<u64>,
    /// Server timestamp in ISO 8601 format
    #[serde(default)]
    time: Option<String>,
}

/// Order channel data
#[derive(Debug, Deserialize)]
struct OrderChannelData {
    #[serde(rename = "clOrdId")]
    cl_ord_id: String,
    status: String,
    side: String,
    symbol: String,
}

/// Position channel data
#[derive(Debug, Deserialize)]
struct PositionChannelData {
    symbol: String,
    qty: String,
    #[serde(default)]
    upnl: Option<String>,
}

/// Auth request for user WebSocket
#[derive(Debug, Serialize)]
struct AuthRequest {
    session_id: String,
    request_id: String,
    method: String,
    params: String,
}

/// Auth params
#[derive(Debug, Serialize)]
struct AuthParams {
    token: String,
}

// ============================================================================
// Callback Types
// ============================================================================

/// Price callback type
pub type PriceCallback = Arc<dyn Fn(PriceData) + Send + Sync>;

/// Order callback type
pub type OrderCallback = Arc<dyn Fn(OrderUpdate) + Send + Sync>;

/// Position callback type
pub type PositionCallback = Arc<dyn Fn(PositionUpdate) + Send + Sync>;

/// Reconnect callback type
pub type ReconnectCallback = Arc<dyn Fn() + Send + Sync>;

// ============================================================================
// MarketWsClient
// ============================================================================

/// Market WebSocket client for price data
pub struct MarketWsClient {
    url: String,
    subscribed_symbols: Arc<RwLock<Vec<String>>>,
    reconnect_delay: Duration,
    running: Arc<AtomicBool>,
    msg_count: Arc<AtomicU64>,
    price_callbacks: Arc<RwLock<Vec<PriceCallback>>>,
    reconnect_callbacks: Arc<RwLock<Vec<ReconnectCallback>>>,
    ws_sender: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
}

impl MarketWsClient {
    /// Create a new market WebSocket client
    pub fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            subscribed_symbols: Arc::new(RwLock::new(Vec::new())),
            reconnect_delay: Duration::from_secs(DEFAULT_RECONNECT_DELAY_SECS),
            running: Arc::new(AtomicBool::new(false)),
            msg_count: Arc::new(AtomicU64::new(0)),
            price_callbacks: Arc::new(RwLock::new(Vec::new())),
            reconnect_callbacks: Arc::new(RwLock::new(Vec::new())),
            ws_sender: Arc::new(Mutex::new(None)),
        }
    }

    /// Create with default market URL
    pub fn with_default_url() -> Self {
        Self::new(MARKET_WS_URL)
    }

    /// Set reconnect delay
    pub fn set_reconnect_delay(&mut self, delay: Duration) {
        self.reconnect_delay = delay;
    }

    /// Register a price callback
    pub async fn on_price<F>(&self, callback: F)
    where
        F: Fn(PriceData) + Send + Sync + 'static,
    {
        let mut callbacks = self.price_callbacks.write().await;
        callbacks.push(Arc::new(callback));
    }

    /// Register a reconnect callback
    pub async fn on_reconnect<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut callbacks = self.reconnect_callbacks.write().await;
        callbacks.push(Arc::new(callback));
    }

    /// Subscribe to price updates for a symbol
    pub async fn subscribe_price(&self, symbol: &str) -> Result<()> {
        // Add to subscribed symbols list
        {
            let mut symbols = self.subscribed_symbols.write().await;
            if !symbols.contains(&symbol.to_string()) {
                symbols.push(symbol.to_string());
            }
        }

        // Send subscribe message if connected
        if let Some(sender) = self.ws_sender.lock().await.as_ref() {
            let msg = SubscribeMessage {
                subscribe: SubscribeParams {
                    channel: "price".to_string(),
                    symbol: Some(symbol.to_string()),
                },
            };
            let json = serde_json::to_string(&msg)?;
            sender.send(Message::Text(json)).await.map_err(|e| anyhow!("Failed to send subscribe: {}", e))?;
            info!("Subscribed to price channel for {}", symbol);
        }

        Ok(())
    }

    /// Connect to the WebSocket server
    pub async fn connect(&self) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        info!("Connecting to market stream: {}", self.url);
        
        let (ws_stream, _) = connect_async(&self.url)
            .await
            .map_err(|e| anyhow!("Failed to connect to market stream: {}", e))?;
        
        info!("Market stream connected");
        Ok(ws_stream)
    }

    /// Run the message loop with auto-reconnection
    pub async fn run(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        let mut last_heartbeat = Instant::now();

        while self.running.load(Ordering::SeqCst) {
            // Connect
            let ws_stream = match self.connect().await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("Connection failed: {}", e);
                    if self.running.load(Ordering::SeqCst) {
                        info!("Reconnecting in {} seconds...", self.reconnect_delay.as_secs());
                        sleep(self.reconnect_delay).await;
                    }
                    continue;
                }
            };

            let (write, mut read) = ws_stream.split();
            
            // Create channel for sending messages
            let (tx, mut rx) = mpsc::channel::<Message>(100);
            *self.ws_sender.lock().await = Some(tx.clone());

            // Spawn write task
            let write = Arc::new(Mutex::new(write));
            let write_clone = write.clone();
            let write_task = tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    let mut w = write_clone.lock().await;
                    if let Err(e) = w.send(msg).await {
                        error!("Failed to send message: {}", e);
                        break;
                    }
                }
            });

            // Resubscribe to all symbols
            {
                let symbols = self.subscribed_symbols.read().await.clone();
                for symbol in symbols {
                    let msg = SubscribeMessage {
                        subscribe: SubscribeParams {
                            channel: "price".to_string(),
                            symbol: Some(symbol.clone()),
                        },
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = tx.send(Message::Text(json)).await;
                        info!("Resubscribed to price channel for {}", symbol);
                    }
                }
            }

            // Invoke reconnect callbacks (except on first connect)
            if self.msg_count.load(Ordering::SeqCst) > 0 {
                let callbacks = self.reconnect_callbacks.read().await;
                for callback in callbacks.iter() {
                    callback();
                }
            }

            // Message loop
            loop {
                if !self.running.load(Ordering::SeqCst) {
                    break;
                }

                match read.next().await {
                    Some(Ok(Message::Text(text))) => {
                        self.msg_count.fetch_add(1, Ordering::SeqCst);

                        // Log heartbeat
                        if last_heartbeat.elapsed() >= Duration::from_secs(HEARTBEAT_LOG_INTERVAL_SECS) {
                            info!(
                                "[Heartbeat] Market WS alive, {} msgs total",
                                self.msg_count.load(Ordering::SeqCst)
                            );
                            last_heartbeat = Instant::now();
                        }

                        // Parse message
                        match serde_json::from_str::<WsMessage>(&text) {
                            Ok(msg) => {
                                // Handle ping
                                if let Some(ping_val) = msg.ping {
                                    let pong = PongMessage { pong: ping_val };
                                    if let Ok(json) = serde_json::to_string(&pong) {
                                        let _ = tx.send(Message::Text(json)).await;
                                    }
                                    continue;
                                }

                                // Log all channel messages for debugging
                                if let Some(ref channel) = msg.channel {
                                    tracing::debug!("Received channel: {} - data: {:?}", channel, msg.data);
                                } else {
                                    // Log messages without channel for debugging
                                    tracing::trace!("Received non-channel message: {:?}", text);
                                }

                            // Handle price channel
                            if msg.channel.as_deref() == Some("price") {
                                if let Some(data) = msg.data {
                                    tracing::debug!("Price channel raw data: {:?}", data);
                                    match serde_json::from_value::<PriceChannelData>(data.clone()) {
                                        Ok(price_data) => {
                                            let price = PriceData {
                                                symbol: price_data.symbol.clone(),
                                                last_price: Decimal::from_str(&price_data.last_price)
                                                    .unwrap_or_default(),
                                                mark_price: Decimal::from_str(&price_data.mark_price)
                                                    .unwrap_or_default(),
                                                timestamp: price_data.timestamp.unwrap_or(0),
                                            };

                                            // Calculate latency from server time
                                            let latency_ms = if let Some(ref time_str) = price_data.time {
                                                calculate_latency_ms(time_str)
                                            } else {
                                                None
                                            };

                                            if let Some(latency) = latency_ms {
                                                info!(
                                                    "[Price] {} last={} mark={} latency={}ms",
                                                    price.symbol, price.last_price, price.mark_price, latency
                                                );
                                            } else {
                                                info!(
                                                    "[Price] {} last={} mark={}",
                                                    price.symbol, price.last_price, price.mark_price
                                                );
                                            }

                                            // Invoke callbacks
                                            let callbacks = self.price_callbacks.read().await;
                                            for callback in callbacks.iter() {
                                                callback(price.clone());
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!("Failed to parse price data: {} - {:?}", e, data);
                                        }
                                    }
                                }
                            }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse WS message: {} - {}", e, text);
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = tx.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        warn!("Market stream connection closed by server");
                        break;
                    }
                    Some(Err(e)) => {
                        error!("Market stream error: {}", e);
                        break;
                    }
                    None => {
                        warn!("Market stream connection ended");
                        break;
                    }
                    _ => {}
                }
            }

            // Cleanup
            *self.ws_sender.lock().await = None;
            write_task.abort();

            // Reconnect if still running
            if self.running.load(Ordering::SeqCst) {
                info!("Reconnecting in {} seconds...", self.reconnect_delay.as_secs());
                sleep(self.reconnect_delay).await;
            }
        }

        Ok(())
    }

    /// Stop the client
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get message count
    pub fn message_count(&self) -> u64 {
        self.msg_count.load(Ordering::SeqCst)
    }
}


// ============================================================================
// UserWsClient
// ============================================================================

/// User WebSocket client for order/position updates
pub struct UserWsClient {
    url: String,
    auth: Arc<tokio::sync::RwLock<StandXAuth>>,
    reconnect_delay: Duration,
    running: Arc<AtomicBool>,
    session_id: Arc<RwLock<Option<String>>>,
    order_callbacks: Arc<RwLock<Vec<OrderCallback>>>,
    position_callbacks: Arc<RwLock<Vec<PositionCallback>>>,
    reconnect_callbacks: Arc<RwLock<Vec<ReconnectCallback>>>,
    ws_sender: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
}

impl UserWsClient {
    /// Create a new user WebSocket client
    pub fn new(url: &str, auth: Arc<tokio::sync::RwLock<StandXAuth>>) -> Self {
        Self {
            url: url.to_string(),
            auth,
            reconnect_delay: Duration::from_secs(DEFAULT_RECONNECT_DELAY_SECS),
            running: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            order_callbacks: Arc::new(RwLock::new(Vec::new())),
            position_callbacks: Arc::new(RwLock::new(Vec::new())),
            reconnect_callbacks: Arc::new(RwLock::new(Vec::new())),
            ws_sender: Arc::new(Mutex::new(None)),
        }
    }

    /// Create with default user URL
    pub fn with_default_url(auth: Arc<tokio::sync::RwLock<StandXAuth>>) -> Self {
        Self::new(USER_WS_URL, auth)
    }

    /// Set reconnect delay
    pub fn set_reconnect_delay(&mut self, delay: Duration) {
        self.reconnect_delay = delay;
    }

    /// Register an order callback
    pub async fn on_order<F>(&self, callback: F)
    where
        F: Fn(OrderUpdate) + Send + Sync + 'static,
    {
        let mut callbacks = self.order_callbacks.write().await;
        callbacks.push(Arc::new(callback));
    }

    /// Register a position callback
    pub async fn on_position<F>(&self, callback: F)
    where
        F: Fn(PositionUpdate) + Send + Sync + 'static,
    {
        let mut callbacks = self.position_callbacks.write().await;
        callbacks.push(Arc::new(callback));
    }

    /// Register a reconnect callback
    pub async fn on_reconnect<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        let mut callbacks = self.reconnect_callbacks.write().await;
        callbacks.push(Arc::new(callback));
    }

    /// Connect to the WebSocket server
    async fn connect_internal(&self) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        info!("Connecting to user stream: {}", self.url);
        
        let (ws_stream, _) = connect_async(&self.url)
            .await
            .map_err(|e| anyhow!("Failed to connect to user stream: {}", e))?;
        
        // Generate new session ID
        let session_id = uuid::Uuid::new_v4().to_string();
        *self.session_id.write().await = Some(session_id);
        
        info!("User stream connected");
        Ok(ws_stream)
    }

    /// Authenticate the WebSocket connection
    async fn authenticate(&self, sender: &mpsc::Sender<Message>) -> Result<()> {
        let auth = self.auth.read().await;
        let token = auth.token().ok_or_else(|| anyhow!("Not authenticated - no token available"))?;
        let session_id = self.session_id.read().await.clone()
            .ok_or_else(|| anyhow!("No session ID"))?;

        let auth_params = AuthParams {
            token: token.to_string(),
        };
        let params_json = serde_json::to_string(&auth_params)?;

        let auth_msg = AuthRequest {
            session_id,
            request_id: uuid::Uuid::new_v4().to_string(),
            method: "auth:login".to_string(),
            params: params_json,
        };

        let json = serde_json::to_string(&auth_msg)?;
        sender.send(Message::Text(json)).await
            .map_err(|e| anyhow!("Failed to send auth message: {}", e))?;
        
        info!("User stream authentication sent");
        Ok(())
    }

    /// Subscribe to order and position channels
    async fn subscribe(&self, sender: &mpsc::Sender<Message>) -> Result<()> {
        // Subscribe to order channel
        let order_sub = SubscribeMessage {
            subscribe: SubscribeParams {
                channel: "order".to_string(),
                symbol: None,
            },
        };
        let json = serde_json::to_string(&order_sub)?;
        sender.send(Message::Text(json)).await
            .map_err(|e| anyhow!("Failed to subscribe to order channel: {}", e))?;
        info!("Subscribed to order channel");

        // Subscribe to position channel
        let position_sub = SubscribeMessage {
            subscribe: SubscribeParams {
                channel: "position".to_string(),
                symbol: None,
            },
        };
        let json = serde_json::to_string(&position_sub)?;
        sender.send(Message::Text(json)).await
            .map_err(|e| anyhow!("Failed to subscribe to position channel: {}", e))?;
        info!("Subscribed to position channel");

        Ok(())
    }

    /// Run the message loop with auto-reconnection
    pub async fn run(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        let mut first_connect = true;

        while self.running.load(Ordering::SeqCst) {
            // Connect
            let ws_stream = match self.connect_internal().await {
                Ok(stream) => stream,
                Err(e) => {
                    error!("User stream connection failed: {}", e);
                    if self.running.load(Ordering::SeqCst) {
                        info!("Reconnecting user stream in {} seconds...", self.reconnect_delay.as_secs());
                        sleep(self.reconnect_delay).await;
                    }
                    continue;
                }
            };

            let (write, mut read) = ws_stream.split();
            
            // Create channel for sending messages
            let (tx, mut rx) = mpsc::channel::<Message>(100);
            *self.ws_sender.lock().await = Some(tx.clone());

            // Spawn write task
            let write = Arc::new(Mutex::new(write));
            let write_clone = write.clone();
            let write_task = tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    let mut w = write_clone.lock().await;
                    if let Err(e) = w.send(msg).await {
                        error!("Failed to send message: {}", e);
                        break;
                    }
                }
            });

            // Authenticate
            if let Err(e) = self.authenticate(&tx).await {
                error!("Authentication failed: {}", e);
                *self.ws_sender.lock().await = None;
                write_task.abort();
                if self.running.load(Ordering::SeqCst) {
                    sleep(self.reconnect_delay).await;
                }
                continue;
            }

            // Wait for auth response
            let auth_timeout = tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(Ok(Message::Text(text))) = read.next().await {
                    if let Ok(msg) = serde_json::from_str::<WsMessage>(&text) {
                        if msg.code == Some(0) {
                            return true;
                        } else if msg.code.is_some() {
                            error!("Auth failed with code: {:?}", msg.code);
                            return false;
                        }
                    }
                }
                false
            }).await;

            let authenticated = match auth_timeout {
                Ok(true) => {
                    info!("User stream authenticated");
                    true
                }
                Ok(false) => {
                    error!("User stream authentication failed");
                    *self.ws_sender.lock().await = None;
                    write_task.abort();
                    if self.running.load(Ordering::SeqCst) {
                        sleep(self.reconnect_delay).await;
                    }
                    continue;
                }
                Err(_) => {
                    error!("User stream authentication timeout");
                    *self.ws_sender.lock().await = None;
                    write_task.abort();
                    if self.running.load(Ordering::SeqCst) {
                        sleep(self.reconnect_delay).await;
                    }
                    continue;
                }
            };

            // Subscribe to channels
            if authenticated {
                if let Err(e) = self.subscribe(&tx).await {
                    error!("Failed to subscribe: {}", e);
                }
            }

            // Invoke reconnect callbacks (except on first connect)
            if !first_connect {
                let callbacks = self.reconnect_callbacks.read().await;
                for callback in callbacks.iter() {
                    callback();
                }
            }
            first_connect = false;

            // Message loop
            loop {
                if !self.running.load(Ordering::SeqCst) {
                    break;
                }

                match read.next().await {
                    Some(Ok(Message::Text(text))) => {
                        // Parse message
                        if let Ok(msg) = serde_json::from_str::<WsMessage>(&text) {
                            // Handle ping
                            if let Some(ping_val) = msg.ping {
                                let pong = PongMessage { pong: ping_val };
                                if let Ok(json) = serde_json::to_string(&pong) {
                                    let _ = tx.send(Message::Text(json)).await;
                                }
                                continue;
                            }

                            // Handle order channel
                            if msg.channel.as_deref() == Some("order") {
                                if let Some(ref data) = msg.data {
                                    if let Ok(order_data) = serde_json::from_value::<OrderChannelData>(data.clone()) {
                                        let order = OrderUpdate {
                                            cl_ord_id: order_data.cl_ord_id,
                                            status: order_data.status,
                                            side: order_data.side,
                                            symbol: order_data.symbol,
                                        };

                                        // Invoke callbacks
                                        let callbacks = self.order_callbacks.read().await;
                                        for callback in callbacks.iter() {
                                            callback(order.clone());
                                        }
                                    }
                                }
                            }

                            // Handle position channel
                            if msg.channel.as_deref() == Some("position") {
                                if let Some(ref data) = msg.data {
                                    if let Ok(pos_data) = serde_json::from_value::<PositionChannelData>(data.clone()) {
                                        let position = PositionUpdate {
                                            symbol: pos_data.symbol,
                                            qty: Decimal::from_str(&pos_data.qty).unwrap_or_default(),
                                            upnl: pos_data.upnl
                                                .and_then(|s| Decimal::from_str(&s).ok())
                                                .unwrap_or_default(),
                                        };

                                        // Invoke callbacks
                                        let callbacks = self.position_callbacks.read().await;
                                        for callback in callbacks.iter() {
                                            callback(position.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = tx.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) => {
                        warn!("User stream connection closed by server");
                        break;
                    }
                    Some(Err(e)) => {
                        error!("User stream error: {}", e);
                        break;
                    }
                    None => {
                        warn!("User stream connection ended");
                        break;
                    }
                    _ => {}
                }
            }

            // Cleanup
            *self.ws_sender.lock().await = None;
            write_task.abort();

            // Reconnect if still running
            if self.running.load(Ordering::SeqCst) {
                info!("Reconnecting user stream in {} seconds...", self.reconnect_delay.as_secs());
                sleep(self.reconnect_delay).await;
            }
        }

        Ok(())
    }

    /// Stop the client
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Tests
// ============================================================================

/// Calculate latency in milliseconds from server timestamp to now
///
/// # Arguments
/// * `time_str` - ISO 8601 timestamp string (e.g., "2026-01-14T02:42:41.514972396Z")
///
/// # Returns
/// Latency in milliseconds, or None if parsing fails
fn calculate_latency_ms(time_str: &str) -> Option<i64> {
    // Parse ISO 8601 timestamp
    let server_time: DateTime<Utc> = time_str.parse().ok()?;
    let now = Utc::now();
    
    // Calculate difference in milliseconds
    let duration = now.signed_duration_since(server_time);
    Some(duration.num_milliseconds())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_market_ws_client_new() {
        let client = MarketWsClient::new("wss://test.example.com/ws");
        assert!(!client.is_running());
        assert_eq!(client.message_count(), 0);
    }

    #[test]
    fn test_market_ws_client_with_default_url() {
        let client = MarketWsClient::with_default_url();
        assert_eq!(client.url, MARKET_WS_URL);
    }

    #[tokio::test]
    async fn test_user_ws_client_new() {
        let auth = Arc::new(tokio::sync::RwLock::new(StandXAuth::new()));
        let client = UserWsClient::new("wss://test.example.com/ws", auth);
        assert!(!client.is_running());
    }

    #[tokio::test]
    async fn test_user_ws_client_with_default_url() {
        let auth = Arc::new(tokio::sync::RwLock::new(StandXAuth::new()));
        let client = UserWsClient::with_default_url(auth);
        assert_eq!(client.url, USER_WS_URL);
    }

    #[test]
    fn test_price_data_parsing() {
        let json = r#"{"symbol":"BTC-USD","lastPrice":"50000.50","markPrice":"50001.00"}"#;
        let data: PriceChannelData = serde_json::from_str(json).unwrap();
        
        assert_eq!(data.symbol, "BTC-USD");
        assert_eq!(data.last_price, "50000.50");
        assert_eq!(data.mark_price, "50001.00");
    }

    #[test]
    fn test_order_data_parsing() {
        let json = r#"{"clOrdId":"mm-buy-12345678","status":"new","side":"buy","symbol":"BTC-USD"}"#;
        let data: OrderChannelData = serde_json::from_str(json).unwrap();
        
        assert_eq!(data.cl_ord_id, "mm-buy-12345678");
        assert_eq!(data.status, "new");
        assert_eq!(data.side, "buy");
        assert_eq!(data.symbol, "BTC-USD");
    }

    #[test]
    fn test_position_data_parsing() {
        let json = r#"{"symbol":"BTC-USD","qty":"0.5","upnl":"100.50"}"#;
        let data: PositionChannelData = serde_json::from_str(json).unwrap();
        
        assert_eq!(data.symbol, "BTC-USD");
        assert_eq!(data.qty, "0.5");
        assert_eq!(data.upnl, Some("100.50".to_string()));
    }

    #[test]
    fn test_subscribe_message_serialization() {
        let msg = SubscribeMessage {
            subscribe: SubscribeParams {
                channel: "price".to_string(),
                symbol: Some("BTC-USD".to_string()),
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"channel\":\"price\""));
        assert!(json.contains("\"symbol\":\"BTC-USD\""));
    }

    #[test]
    fn test_pong_message_serialization() {
        let msg = PongMessage { pong: 12345 };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"pong":12345}"#);
    }

    #[test]
    fn test_ws_message_parsing_ping() {
        let json = r#"{"ping":12345}"#;
        let msg: WsMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.ping, Some(12345));
    }

    #[test]
    fn test_ws_message_parsing_channel() {
        let json = r#"{"channel":"price","data":{"symbol":"BTC-USD"}}"#;
        let msg: WsMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.channel, Some("price".to_string()));
        assert!(msg.data.is_some());
    }

    #[tokio::test]
    async fn test_market_ws_subscribe_price_adds_symbol() {
        let client = MarketWsClient::new("wss://test.example.com/ws");
        
        // Subscribe without connection (should just add to list)
        let _ = client.subscribe_price("BTC-USD").await;
        
        let symbols = client.subscribed_symbols.read().await;
        assert!(symbols.contains(&"BTC-USD".to_string()));
    }

    #[tokio::test]
    async fn test_market_ws_on_price_callback() {
        let client = MarketWsClient::new("wss://test.example.com/ws");
        let received = Arc::new(AtomicBool::new(false));
        let received_clone = received.clone();
        
        client.on_price(move |_price| {
            received_clone.store(true, Ordering::SeqCst);
        }).await;
        
        let callbacks = client.price_callbacks.read().await;
        assert_eq!(callbacks.len(), 1);
    }

    #[tokio::test]
    async fn test_user_ws_on_order_callback() {
        let auth = Arc::new(tokio::sync::RwLock::new(StandXAuth::new()));
        let client = UserWsClient::new("wss://test.example.com/ws", auth);
        
        client.on_order(|_order| {
            // Callback registered
        }).await;
        
        let callbacks = client.order_callbacks.read().await;
        assert_eq!(callbacks.len(), 1);
    }

    #[tokio::test]
    async fn test_user_ws_on_position_callback() {
        let auth = Arc::new(tokio::sync::RwLock::new(StandXAuth::new()));
        let client = UserWsClient::new("wss://test.example.com/ws", auth);
        
        client.on_position(|_position| {
            // Callback registered
        }).await;
        
        let callbacks = client.position_callbacks.read().await;
        assert_eq!(callbacks.len(), 1);
    }

    #[test]
    fn test_auth_request_serialization() {
        let auth_params = AuthParams {
            token: "test_token".to_string(),
        };
        let params_json = serde_json::to_string(&auth_params).unwrap();
        
        let auth_msg = AuthRequest {
            session_id: "session-123".to_string(),
            request_id: "request-456".to_string(),
            method: "auth:login".to_string(),
            params: params_json,
        };
        
        let json = serde_json::to_string(&auth_msg).unwrap();
        assert!(json.contains("\"method\":\"auth:login\""));
        assert!(json.contains("\"session_id\":\"session-123\""));
    }
}
