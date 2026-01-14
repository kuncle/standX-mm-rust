//! HTTP client module for StandX API
//!
//! Handles API communication with retry logic and rate limiting.

use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::auth::StandXAuth;

/// Base URL for StandX Perps API
const BASE_URL: &str = "https://perps.standx.com";

// ============================================================================
// Retry Configuration
// ============================================================================

/// Configuration for HTTP request retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30000,
        }
    }
}

impl RetryConfig {
    /// Calculate delay for a given attempt using exponential backoff
    ///
    /// Formula: min(base_delay * 2^attempt, max_delay)
    pub fn calculate_delay(&self, attempt: u32) -> u64 {
        // Prevent overflow: if attempt >= 64, the shift would overflow
        // In that case, just return max_delay since 2^64 would exceed any reasonable max
        if attempt >= 64 {
            return self.max_delay_ms;
        }
        let delay = self.base_delay_ms.saturating_mul(1u64 << attempt);
        delay.min(self.max_delay_ms)
    }

    /// Check if a status code is retryable
    /// 
    /// Retryable status codes: 429, 500, 502, 503, 504
    pub fn is_retryable_status(&self, status: u16) -> bool {
        matches!(status, 429 | 500 | 502 | 503 | 504)
    }
}

// ============================================================================
// Rate Limiter
// ============================================================================

/// Rate limiter that enforces a maximum number of requests per second
pub struct RateLimiter {
    /// Maximum requests per second
    requests_per_second: f64,
    /// Minimum interval between requests in milliseconds
    min_interval_ms: u64,
    /// Time of last request
    last_request_time: Option<Instant>,
    /// Lock for thread-safe access
    lock: Mutex<()>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(10.0)
    }
}

impl RateLimiter {
    /// Create a new rate limiter with specified requests per second
    pub fn new(requests_per_second: f64) -> Self {
        let min_interval_ms = (1000.0 / requests_per_second) as u64;
        Self {
            requests_per_second,
            min_interval_ms,
            last_request_time: None,
            lock: Mutex::new(()),
        }
    }

    /// Get the requests per second limit
    pub fn requests_per_second(&self) -> f64 {
        self.requests_per_second
    }

    /// Wait if necessary to respect rate limit
    ///
    /// Returns the time waited in milliseconds
    pub async fn wait(&mut self) -> u64 {
        let _guard = self.lock.lock().await;

        let now = Instant::now();

        if let Some(last_time) = self.last_request_time {
            let elapsed_ms = last_time.elapsed().as_millis() as u64;

            if elapsed_ms < self.min_interval_ms {
                let wait_ms = self.min_interval_ms - elapsed_ms;
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
                self.last_request_time = Some(Instant::now());
                return wait_ms;
            }
        }

        self.last_request_time = Some(now);
        0
    }

    /// Pause for a specified duration (used for 429 retry-after)
    pub async fn pause_for(&mut self, seconds: f64) {
        if seconds > 0.0 {
            warn!(
                "Rate limiter pausing for {:.1}s due to rate limit response",
                seconds
            );
            let _guard = self.lock.lock().await;
            tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
            self.last_request_time = Some(Instant::now());
        }
    }
}

// ============================================================================
// Data Types
// ============================================================================

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "buy"),
            Side::Sell => write!(f, "sell"),
        }
    }
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    Limit,
    Market,
}

impl std::fmt::Display for OrderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderType::Limit => write!(f, "limit"),
            OrderType::Market => write!(f, "market"),
        }
    }
}

/// Time in force for orders
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeInForce {
    /// Good till cancelled
    Gtc,
    /// Immediate or cancel
    Ioc,
    /// Fill or kill
    Fok,
}

impl Default for TimeInForce {
    fn default() -> Self {
        Self::Gtc
    }
}

/// New order request
#[derive(Debug, Clone, Serialize)]
pub struct NewOrderRequest {
    pub symbol: String,
    pub side: Side,
    #[serde(rename = "order_type")]
    pub order_type: OrderType,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(default)]
    pub time_in_force: TimeInForce,
    pub reduce_only: bool,
    pub cl_ord_id: String,
}

/// Order response from API
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    pub code: i32,
    pub message: Option<String>,
    pub id: Option<i64>,
}

/// Position data
#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub symbol: String,
    #[serde(with = "rust_decimal::serde::float")]
    pub qty: Decimal,
    #[serde(with = "rust_decimal::serde::float")]
    pub entry_price: Decimal,
    #[serde(with = "rust_decimal::serde::float")]
    pub upnl: Decimal,
}

/// Balance data from /api/query_balance
#[derive(Debug, Clone, Deserialize)]
pub struct Balance {
    /// Total balance
    #[serde(with = "rust_decimal::serde::str")]
    pub balance: Decimal,
    /// Available balance for cross margin
    #[serde(with = "rust_decimal::serde::str")]
    pub cross_available: Decimal,
    /// Cross margin balance
    #[serde(with = "rust_decimal::serde::str")]
    pub cross_balance: Decimal,
    /// Cross margin used
    #[serde(with = "rust_decimal::serde::str")]
    pub cross_margin: Decimal,
    /// Cross margin unrealized PnL
    #[serde(with = "rust_decimal::serde::str")]
    pub cross_upnl: Decimal,
    /// Total equity
    #[serde(with = "rust_decimal::serde::str")]
    pub equity: Decimal,
    /// Isolated margin balance
    #[serde(with = "rust_decimal::serde::str")]
    pub isolated_balance: Decimal,
    /// Isolated margin unrealized PnL
    #[serde(with = "rust_decimal::serde::str")]
    pub isolated_upnl: Decimal,
    /// Locked balance
    #[serde(with = "rust_decimal::serde::str")]
    pub locked: Decimal,
    /// 24h PnL
    #[serde(with = "rust_decimal::serde::str")]
    pub pnl_24h: Decimal,
    /// Frozen PnL
    #[serde(with = "rust_decimal::serde::str")]
    pub pnl_freeze: Decimal,
    /// Total unrealized PnL
    #[serde(with = "rust_decimal::serde::str")]
    pub upnl: Decimal,
}

/// Open order data
#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    pub id: Option<i64>,
    pub cl_ord_id: String,
    pub symbol: String,
    pub side: Side,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub qty: Decimal,
    pub status: String,
}

/// Price data from API
#[derive(Debug, Clone, Deserialize)]
pub struct PriceData {
    pub symbol: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub last_price: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub mark_price: Decimal,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub index_price: Option<Decimal>,
}

// ============================================================================
// HTTP Client
// ============================================================================

/// HTTP client for StandX API
pub struct StandXHttpClient {
    auth: Arc<tokio::sync::RwLock<StandXAuth>>,
    client: reqwest::Client,
    rate_limiter: Mutex<RateLimiter>,
    retry_config: RetryConfig,
    base_url: String,
}

impl StandXHttpClient {
    /// Create a new HTTP client
    pub fn new(auth: Arc<tokio::sync::RwLock<StandXAuth>>) -> Self {
        Self::with_config(auth, RetryConfig::default(), RateLimiter::default())
    }

    /// Create a new HTTP client with custom configuration
    pub fn with_config(
        auth: Arc<tokio::sync::RwLock<StandXAuth>>,
        retry_config: RetryConfig,
        rate_limiter: RateLimiter,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            auth,
            client,
            rate_limiter: Mutex::new(rate_limiter),
            retry_config,
            base_url: BASE_URL.to_string(),
        }
    }

    /// Set a custom base URL (useful for testing)
    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.to_string();
    }

    /// Build headers from auth headers map
    fn build_headers(&self, auth_headers: std::collections::HashMap<String, String>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (key, value) in auth_headers {
            if let (Ok(name), Ok(val)) = (
                HeaderName::try_from(key.as_str()),
                HeaderValue::from_str(&value),
            ) {
                headers.insert(name, val);
            }
        }
        headers
    }

    /// Check if an error is retryable
    fn is_retryable_error(&self, error: &reqwest::Error) -> bool {
        // Timeout and connection errors are retryable
        if error.is_timeout() || error.is_connect() {
            return true;
        }

        // Check status code if available
        if let Some(status) = error.status() {
            return self.retry_config.is_retryable_status(status.as_u16());
        }

        false
    }

    /// Extract retry-after duration from response headers
    fn get_retry_after(&self, response: &reqwest::Response) -> Option<f64> {
        if response.status().as_u16() != 429 {
            return None;
        }

        response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<f64>().ok())
    }

    /// Execute a GET request with retry logic
    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        params: &[(&str, &str)],
        require_auth: bool,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);

        for attempt in 0..=self.retry_config.max_retries {
            // Rate limiting
            {
                let mut limiter = self.rate_limiter.lock().await;
                limiter.wait().await;
            }

            // Build headers
            let headers = if require_auth {
                let auth = self.auth.read().await;
                self.build_headers(auth.get_auth_headers(""))
            } else {
                HeaderMap::new()
            };

            debug!("GET {} (attempt {})", path, attempt + 1);

            let result = self
                .client
                .get(&url)
                .headers(headers)
                .query(params)
                .send()
                .await;

            match result {
                Ok(response) => {
                    let status = response.status();

                    // Handle rate limiting
                    if status.as_u16() == 429 {
                        if let Some(retry_after) = self.get_retry_after(&response) {
                            let mut limiter = self.rate_limiter.lock().await;
                            limiter.pause_for(retry_after).await;
                        }

                        if attempt < self.retry_config.max_retries {
                            continue;
                        }
                    }

                    // Check for retryable status codes
                    if self.retry_config.is_retryable_status(status.as_u16()) {
                        if attempt < self.retry_config.max_retries {
                            let delay = self.retry_config.calculate_delay(attempt);
                            warn!(
                                "Retryable status {} for {} (attempt {}/{}). Retrying in {}ms...",
                                status,
                                path,
                                attempt + 1,
                                self.retry_config.max_retries + 1,
                                delay
                            );
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            continue;
                        }
                    }

                    if !status.is_success() {
                        let error_text = response.text().await.unwrap_or_default();
                        error!("API error {} for {}: {}", status, path, error_text);
                        return Err(anyhow!("API error {}: {}", status, error_text));
                    }

                    if attempt > 0 {
                        info!("Request to {} succeeded after {} retry(ies)", path, attempt);
                    }

                    return response
                        .json::<T>()
                        .await
                        .map_err(|e| anyhow!("Failed to parse response: {}", e));
                }
                Err(e) => {
                    if self.is_retryable_error(&e) && attempt < self.retry_config.max_retries {
                        let delay = self.retry_config.calculate_delay(attempt);
                        warn!(
                            "Retryable error for {} (attempt {}/{}): {}. Retrying in {}ms...",
                            path,
                            attempt + 1,
                            self.retry_config.max_retries + 1,
                            e,
                            delay
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }

                    error!("Request failed for {}: {}", path, e);
                    return Err(anyhow!("Request failed: {}", e));
                }
            }
        }

        Err(anyhow!(
            "All {} retries exhausted for {}",
            self.retry_config.max_retries,
            path
        ))
    }

    /// Execute a POST request with retry logic
    async fn post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        payload: &impl Serialize,
        sign: bool,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let payload_str = serde_json::to_string(payload)?;

        for attempt in 0..=self.retry_config.max_retries {
            // Rate limiting
            {
                let mut limiter = self.rate_limiter.lock().await;
                limiter.wait().await;
            }

            // Build headers
            let headers = {
                let auth = self.auth.read().await;
                if sign {
                    self.build_headers(auth.get_auth_headers(&payload_str))
                } else {
                    self.build_headers(auth.get_auth_headers(""))
                }
            };

            debug!("POST {} (attempt {}): {}", path, attempt + 1, payload_str);

            let start_time = Instant::now();
            let result = self
                .client
                .post(&url)
                .headers(headers)
                .body(payload_str.clone())
                .send()
                .await;

            match result {
                Ok(response) => {
                    let latency_ms = start_time.elapsed().as_millis();
                    let status = response.status();

                    // Handle rate limiting
                    if status.as_u16() == 429 {
                        if let Some(retry_after) = self.get_retry_after(&response) {
                            let mut limiter = self.rate_limiter.lock().await;
                            limiter.pause_for(retry_after).await;
                        }

                        if attempt < self.retry_config.max_retries {
                            continue;
                        }
                    }

                    // Check for retryable status codes
                    if self.retry_config.is_retryable_status(status.as_u16()) {
                        if attempt < self.retry_config.max_retries {
                            let delay = self.retry_config.calculate_delay(attempt);
                            warn!(
                                "Retryable status {} for {} (attempt {}/{}). Retrying in {}ms...",
                                status,
                                path,
                                attempt + 1,
                                self.retry_config.max_retries + 1,
                                delay
                            );
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            continue;
                        }
                    }

                    if !status.is_success() {
                        let error_text = response.text().await.unwrap_or_default();
                        error!("API error {} for {}: {}", status, path, error_text);
                        return Err(anyhow!("API error {}: {}", status, error_text));
                    }

                    info!("[Latency] {} responded in {}ms", path, latency_ms);

                    if attempt > 0 {
                        info!("Request to {} succeeded after {} retry(ies)", path, attempt);
                    }

                    // Get raw response text first for debugging
                    let response_text = response.text().await
                        .map_err(|e| anyhow!("Failed to read response body: {}", e))?;
                    
                    debug!("Raw API response for {}: {}", path, response_text);

                    return serde_json::from_str::<T>(&response_text)
                        .map_err(|e| anyhow!("Failed to parse response: {} (body: {})", e, response_text));
                }
                Err(e) => {
                    if self.is_retryable_error(&e) && attempt < self.retry_config.max_retries {
                        let delay = self.retry_config.calculate_delay(attempt);
                        warn!(
                            "Retryable error for {} (attempt {}/{}): {}. Retrying in {}ms...",
                            path,
                            attempt + 1,
                            self.retry_config.max_retries + 1,
                            e,
                            delay
                        );
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        continue;
                    }

                    error!("Request failed for {}: {}", path, e);
                    return Err(anyhow!("Request failed: {}", e));
                }
            }
        }

        Err(anyhow!(
            "All {} retries exhausted for {}",
            self.retry_config.max_retries,
            path
        ))
    }


    // ========================================================================
    // API Endpoint Methods
    // ========================================================================

    /// Place a new order
    pub async fn new_order(&self, order: NewOrderRequest) -> Result<OrderResponse> {
        self.post("/api/new_order", &order, true).await
    }

    /// Cancel an order by client order ID
    pub async fn cancel_order(&self, cl_ord_id: &str) -> Result<()> {
        #[derive(Serialize)]
        struct CancelRequest {
            cl_ord_id: String,
        }

        let request = CancelRequest {
            cl_ord_id: cl_ord_id.to_string(),
        };

        let _response: serde_json::Value = self.post("/api/cancel_order", &request, true).await?;
        Ok(())
    }

    /// Cancel multiple orders by client order IDs
    pub async fn cancel_orders(&self, cl_ord_ids: Vec<String>) -> Result<()> {
        if cl_ord_ids.is_empty() {
            return Ok(());
        }

        #[derive(Serialize)]
        struct CancelOrdersRequest {
            cl_ord_id_list: Vec<String>,
        }

        let request = CancelOrdersRequest {
            cl_ord_id_list: cl_ord_ids,
        };

        let _response: serde_json::Value = self.post("/api/cancel_orders", &request, true).await?;
        Ok(())
    }

    /// Query positions for a symbol
    pub async fn query_positions(&self, symbol: &str) -> Result<Vec<Position>> {
        #[derive(Deserialize)]
        struct PositionsResponse {
            #[serde(default)]
            result: Vec<Position>,
        }

        // Try to parse as object with result field first
        let params = [("symbol", symbol)];
        let response: serde_json::Value = self.get("/api/query_positions", &params, true).await?;

        // Handle both array and object responses
        if response.is_array() {
            let positions: Vec<Position> = serde_json::from_value(response)?;
            Ok(positions)
        } else {
            let parsed: PositionsResponse = serde_json::from_value(response)?;
            Ok(parsed.result)
        }
    }

    /// Query account balance
    pub async fn query_balance(&self) -> Result<Balance> {
        self.get("/api/query_balance", &[], true).await
    }

    /// Query open orders for a symbol
    pub async fn query_open_orders(&self, symbol: &str) -> Result<Vec<Order>> {
        #[derive(Deserialize)]
        struct OrdersResponse {
            #[serde(default)]
            result: Vec<Order>,
        }

        let params = [("symbol", symbol)];
        let response: serde_json::Value = self.get("/api/query_open_orders", &params, true).await?;

        // Handle both array and object responses
        if response.is_array() {
            let orders: Vec<Order> = serde_json::from_value(response)?;
            Ok(orders)
        } else {
            let parsed: OrdersResponse = serde_json::from_value(response)?;
            Ok(parsed.result)
        }
    }

    /// Query current price for a symbol
    pub async fn query_symbol_price(&self, symbol: &str) -> Result<PriceData> {
        let params = [("symbol", symbol)];
        self.get("/api/query_symbol_price", &params, false).await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 30000);
    }

    #[test]
    fn test_calculate_delay_exponential_backoff() {
        let config = RetryConfig::default();

        // Attempt 0: 1000 * 2^0 = 1000
        assert_eq!(config.calculate_delay(0), 1000);

        // Attempt 1: 1000 * 2^1 = 2000
        assert_eq!(config.calculate_delay(1), 2000);

        // Attempt 2: 1000 * 2^2 = 4000
        assert_eq!(config.calculate_delay(2), 4000);

        // Attempt 3: 1000 * 2^3 = 8000
        assert_eq!(config.calculate_delay(3), 8000);
    }

    #[test]
    fn test_calculate_delay_respects_max() {
        let config = RetryConfig {
            max_retries: 10,
            base_delay_ms: 1000,
            max_delay_ms: 5000,
            ..Default::default()
        };

        // Attempt 3: 1000 * 2^3 = 8000, but max is 5000
        assert_eq!(config.calculate_delay(3), 5000);

        // Attempt 10: would be huge, but capped at 5000
        assert_eq!(config.calculate_delay(10), 5000);
    }

    #[test]
    fn test_is_retryable_status() {
        let config = RetryConfig::default();

        // Retryable status codes
        assert!(config.is_retryable_status(429));
        assert!(config.is_retryable_status(500));
        assert!(config.is_retryable_status(502));
        assert!(config.is_retryable_status(503));
        assert!(config.is_retryable_status(504));

        // Non-retryable status codes
        assert!(!config.is_retryable_status(200));
        assert!(!config.is_retryable_status(400));
        assert!(!config.is_retryable_status(401));
        assert!(!config.is_retryable_status(403));
        assert!(!config.is_retryable_status(404));
    }

    #[test]
    fn test_rate_limiter_default() {
        let limiter = RateLimiter::default();
        assert_eq!(limiter.requests_per_second(), 10.0);
        assert_eq!(limiter.min_interval_ms, 100);
    }

    #[test]
    fn test_rate_limiter_custom() {
        let limiter = RateLimiter::new(5.0);
        assert_eq!(limiter.requests_per_second(), 5.0);
        assert_eq!(limiter.min_interval_ms, 200);
    }

    #[test]
    fn test_side_display() {
        assert_eq!(format!("{}", Side::Buy), "buy");
        assert_eq!(format!("{}", Side::Sell), "sell");
    }

    #[test]
    fn test_order_type_display() {
        assert_eq!(format!("{}", OrderType::Limit), "limit");
        assert_eq!(format!("{}", OrderType::Market), "market");
    }

    #[test]
    fn test_new_order_request_serialization() {
        let request = NewOrderRequest {
            symbol: "BTC-USD".to_string(),
            side: Side::Buy,
            order_type: OrderType::Limit,
            qty: Decimal::new(1, 2), // 0.01
            price: Decimal::new(50000, 0),
            time_in_force: TimeInForce::Gtc,
            reduce_only: false,
            cl_ord_id: "mm-buy-12345678".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"symbol\":\"BTC-USD\""));
        assert!(json.contains("\"side\":\"buy\""));
        assert!(json.contains("\"order_type\":\"limit\""));
        assert!(json.contains("\"qty\":\"0.01\""));
        assert!(json.contains("\"price\":\"50000\""));
        assert!(json.contains("\"cl_ord_id\":\"mm-buy-12345678\""));
        assert!(json.contains("\"reduce_only\":false"));
    }

    #[tokio::test]
    async fn test_rate_limiter_wait_first_request() {
        let mut limiter = RateLimiter::new(10.0);

        // First request should not wait
        let waited = limiter.wait().await;
        assert_eq!(waited, 0);
    }

    #[tokio::test]
    async fn test_rate_limiter_wait_enforces_interval() {
        let mut limiter = RateLimiter::new(100.0); // 10ms interval

        // First request
        limiter.wait().await;

        // Second request immediately should wait
        let start = Instant::now();
        limiter.wait().await;
        let elapsed = start.elapsed();

        // Should have waited approximately 10ms (allow some tolerance)
        assert!(elapsed.as_millis() >= 5);
    }
}
