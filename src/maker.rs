//! Maker strategy module
//!
//! Core market making logic and order management.
//! Implements event-driven market making with position control.

use anyhow::{anyhow, Result};
use rust_decimal::Decimal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::calculator::PriceCalculator;
use crate::config::Config;
use crate::http_client::{NewOrderRequest, OrderType, Side, StandXHttpClient, TimeInForce};
use crate::state::{OpenOrder, State};

/// Market maker strategy
///
/// Implements the core market making logic:
/// - Places buy/sell orders at configured distances from last price
/// - Monitors and cancels orders that are too close or too far
/// - Controls position size and implements reduction logic
/// - Event-driven: price updates trigger order checks
pub struct Maker {
    /// Configuration
    config: Config,
    /// HTTP client for API calls
    client: Arc<StandXHttpClient>,
    /// Shared state
    state: State,
    /// Price calculator for order prices
    calculator: PriceCalculator,
    /// Notification for pending price checks
    pending_check: Arc<Notify>,
    /// Running flag
    running: AtomicBool,
}

impl Maker {
    /// Create a new maker instance
    ///
    /// # Arguments
    /// * `config` - Bot configuration
    /// * `client` - HTTP client for API calls
    /// * `state` - Shared state manager
    pub fn new(config: Config, client: Arc<StandXHttpClient>, state: State) -> Self {
        let calculator = PriceCalculator::new(&config.symbol);
        Self {
            config,
            client,
            state,
            calculator,
            pending_check: Arc::new(Notify::new()),
            running: AtomicBool::new(false),
        }
    }

    /// Get the state reference
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Get the config reference
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Check if the maker is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Initialize state from exchange
    ///
    /// Syncs position and open orders from the exchange to local state.
    /// This should be called before starting the maker loop.
    ///
    /// # Requirements: 3.5
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing maker state from exchange...");

        // Query current positions
        let positions = self
            .client
            .query_positions(&self.config.symbol)
            .await
            .map_err(|e| anyhow!("Failed to query positions: {}", e))?;

        // Find position for our symbol
        let position = positions
            .iter()
            .find(|p| p.symbol == self.config.symbol);

        if let Some(pos) = position {
            self.state.update_position(pos.qty, pos.upnl);
            info!(
                "Synced position: qty={}, upnl={}",
                pos.qty, pos.upnl
            );
        } else {
            self.state.update_position(Decimal::ZERO, Decimal::ZERO);
            info!("No existing position found");
        }

        // Query open orders
        let open_orders = self
            .client
            .query_open_orders(&self.config.symbol)
            .await
            .map_err(|e| anyhow!("Failed to query open orders: {}", e))?;

        // Clear existing orders in state
        self.state.clear_all_orders();

        // Sync open orders to state (only track our mm- orders)
        for order in open_orders {
            if order.cl_ord_id.starts_with("mm-") {
                let open_order = OpenOrder {
                    cl_ord_id: order.cl_ord_id.clone(),
                    side: order.side,
                    price: order.price,
                    qty: order.qty,
                };
                self.state.set_order(order.side, Some(open_order));
                info!(
                    "Synced order: {} {} @ {} ({})",
                    order.side, order.qty, order.price, order.cl_ord_id
                );
            }
        }

        // Query current price
        let price_data = self
            .client
            .query_symbol_price(&self.config.symbol)
            .await
            .map_err(|e| anyhow!("Failed to query price: {}", e))?;

        self.state
            .update_price(price_data.last_price, self.config.volatility_window_sec);
        info!("Synced price: {}", price_data.last_price);

        info!("Maker initialization complete");
        Ok(())
    }

    /// Handle price update from WebSocket
    ///
    /// Updates state and notifies the maker loop to check orders.
    pub fn on_price_update(&self, price: Decimal) {
        self.state
            .update_price(price, self.config.volatility_window_sec);
        self.pending_check.notify_one();
    }

    /// Run the maker loop
    ///
    /// Event-driven loop that waits for price updates and checks orders.
    ///
    /// # Requirements: 5.1, 5.2, 5.5, 5.6
    pub async fn run(&self) -> Result<()> {
        info!("Starting maker loop for {}", self.config.symbol);
        self.running.store(true, Ordering::SeqCst);

        while self.running.load(Ordering::SeqCst) {
            // Wait for price update notification
            self.pending_check.notified().await;

            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            // Execute tick logic
            if let Err(e) = self.tick().await {
                error!("Tick error: {}", e);
            }
        }

        info!("Maker loop stopped");
        Ok(())
    }

    /// Stop the maker
    ///
    /// Sets running flag to false and notifies the loop.
    pub async fn stop(&self) {
        info!("Stopping maker...");
        self.running.store(false, Ordering::SeqCst);
        self.pending_check.notify_one();
    }

    /// Single tick of the maker loop
    ///
    /// Executes the following logic:
    /// 1. Check stop loss / take profit
    /// 2. Check if position exceeds limit (pause if so)
    /// 3. Check if position needs reduction
    /// 4. Cancel orders that are too close or too far
    /// 5. Check volatility (pause if too high)
    /// 6. Place missing orders
    async fn tick(&self) -> Result<()> {
        let last_price = match self.state.last_price() {
            Some(p) => p,
            None => {
                debug!("No price available, skipping tick");
                return Ok(());
            }
        };

        info!("Tick: last_price={}, position={}", last_price, self.state.position());

        // 1. Check stop loss / take profit - close position if triggered
        if self.check_stop_loss_take_profit().await? {
            return Ok(());
        }

        // 2. Check position limit
        if self.should_pause_market_making() {
            warn!(
                "Position {} exceeds max {}, pausing market making",
                self.state.position().abs(),
                self.config.max_position_btc
            );
            return Ok(());
        }

        // 3. Check and reduce position if needed
        if self.check_and_reduce_position().await? {
            // Position reduction was triggered, skip order placement this tick
            return Ok(());
        }

        // 4. Cancel orders that are too close or too far
        self.cancel_invalid_orders().await?;

        // 5. Check volatility
        let volatility = self.state.get_volatility_bps();
        let threshold = Decimal::new(self.config.volatility_threshold_bps as i64, 0);
        if volatility > threshold {
            info!(
                "Volatility {} bps exceeds threshold {} bps, skipping order placement",
                volatility, threshold
            );
            return Ok(());
        }

        // 6. Place missing orders
        self.place_missing_orders().await?;

        Ok(())
    }

    /// Cancel orders that are too close or too far from current price
    async fn cancel_invalid_orders(&self) -> Result<()> {
        let orders_to_cancel = self.state.get_orders_to_cancel(
            self.config.cancel_distance_bps,
            self.config.rebalance_distance_bps,
        );

        for order in orders_to_cancel {
            info!(
                "Cancelling order {} (price={}, side={:?})",
                order.cl_ord_id, order.price, order.side
            );

            match self.client.cancel_order(&order.cl_ord_id).await {
                Ok(_) => {
                    self.state.set_order(order.side, None);
                    info!("Order {} cancelled", order.cl_ord_id);
                }
                Err(e) => {
                    error!("Failed to cancel order {}: {}", order.cl_ord_id, e);
                }
            }
        }

        Ok(())
    }

    /// Place missing buy/sell orders
    ///
    /// # Requirements: 5.1, 5.2, 5.7
    async fn place_missing_orders(&self) -> Result<()> {
        let last_price = match self.state.last_price() {
            Some(p) => p,
            None => return Ok(()),
        };

        // Check balance before placing orders
        if !self.check_balance().await? {
            return Ok(());
        }

        let has_buy = self.state.has_order(Side::Buy);
        let has_sell = self.state.has_order(Side::Sell);
        
        debug!(
            "Checking orders: has_buy={}, has_sell={}, last_price={}",
            has_buy, has_sell, last_price
        );

        // Place buy order if missing
        if !has_buy {
            let buy_price = self
                .calculator
                .calculate_buy_price(last_price, self.config.order_distance_bps);
            info!("Placing buy order at price {}", buy_price);
            self.place_order(Side::Buy, buy_price, self.config.order_size_btc, false)
                .await?;
        }

        // Place sell order if missing
        if !has_sell {
            let sell_price = self
                .calculator
                .calculate_sell_price(last_price, self.config.order_distance_bps);
            info!("Placing sell order at price {}", sell_price);
            self.place_order(Side::Sell, sell_price, self.config.order_size_btc, false)
                .await?;
        }

        // Verify orders actually exist on exchange and sync state
        if !has_buy || !has_sell {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            self.sync_orders_from_exchange().await?;
        }

        Ok(())
    }

    /// Check if account has sufficient balance for trading
    async fn check_balance(&self) -> Result<bool> {
        let balance = self
            .client
            .query_balance()
            .await
            .map_err(|e| anyhow!("Failed to query balance: {}", e))?;

        // Use cross_available for checking available margin
        let available = balance.cross_available;

        info!("Account balance: available={} DUSD (equity={})", available, balance.equity);

        if available < self.config.min_balance_usd {
            warn!(
                "Insufficient balance: {} < {} DUSD, skipping order placement",
                available, self.config.min_balance_usd
            );
            return Ok(false);
        }

        Ok(true)
    }

    /// Sync orders from exchange to local state
    /// 
    /// Queries open orders and updates local state to match exchange reality.
    /// This ensures we don't have phantom orders in local state.
    async fn sync_orders_from_exchange(&self) -> Result<()> {
        let open_orders = self
            .client
            .query_open_orders(&self.config.symbol)
            .await
            .map_err(|e| anyhow!("Failed to query open orders: {}", e))?;

        let mut found_buy = false;
        let mut found_sell = false;

        for order in &open_orders {
            if order.cl_ord_id.starts_with("mm-") {
                debug!(
                    "Exchange order: {} {} @ {} ({})",
                    order.side, order.qty, order.price, order.cl_ord_id
                );
                match order.side {
                    Side::Buy => found_buy = true,
                    Side::Sell => found_sell = true,
                }
            }
        }

        // Sync state with exchange reality
        if !found_buy && self.state.has_order(Side::Buy) {
            warn!("Buy order not found on exchange (API returned success but order not created - possibly insufficient margin)");
            self.state.set_order(Side::Buy, None);
        }
        if !found_sell && self.state.has_order(Side::Sell) {
            warn!("Sell order not found on exchange (API returned success but order not created - possibly insufficient margin)");
            self.state.set_order(Side::Sell, None);
        }

        Ok(())
    }

    /// Place a single order
    async fn place_order(
        &self,
        side: Side,
        price: Decimal,
        qty: Decimal,
        reduce_only: bool,
    ) -> Result<()> {
        let cl_ord_id = Self::generate_order_id(&side.to_string());

        let request = NewOrderRequest {
            symbol: self.config.symbol.clone(),
            side,
            order_type: OrderType::Limit,
            qty,
            price,
            time_in_force: TimeInForce::Gtc,
            reduce_only,
            cl_ord_id: cl_ord_id.clone(),
        };

        info!(
            "Placing {} order: {} @ {} ({})",
            side, qty, price, cl_ord_id
        );

        match self.client.new_order(request).await {
            Ok(response) => {
                info!(
                    "Order API response: code={}, message={:?}, id={:?}",
                    response.code, response.message, response.id
                );
                // Accept order if code == 0 OR if we got an order id (some APIs return id without code)
                if response.code == 0 || response.id.is_some() {
                    let open_order = OpenOrder {
                        cl_ord_id: cl_ord_id.clone(),
                        side,
                        price,
                        qty,
                    };
                    self.state.set_order(side, Some(open_order));
                    info!("Order placed successfully: {} (exchange_id={:?})", cl_ord_id, response.id);
                } else {
                    error!(
                        "Order rejected by exchange: code={}, message={:?}",
                        response.code, response.message
                    );
                }
            }
            Err(e) => {
                error!("Failed to place order (HTTP error): {}", e);
            }
        }

        Ok(())
    }

    /// Generate a client order ID
    ///
    /// Format: "mm-{side}-{uuid8}"
    /// where uuid8 is the first 8 characters of a UUID v4
    ///
    /// # Arguments
    /// * `side` - Order side ("buy" or "sell")
    ///
    /// # Requirements: 6.1
    pub fn generate_order_id(side: &str) -> String {
        let uuid = Uuid::new_v4();
        let uuid_str = uuid.to_string().replace("-", "");
        let uuid8 = &uuid_str[..8];
        format!("mm-{}-{}", side.to_lowercase(), uuid8)
    }

    /// Check if position exceeds limit and market making should pause
    ///
    /// # Returns
    /// true if |position| >= max_position_btc
    ///
    /// # Requirements: 7.1
    pub fn should_pause_market_making(&self) -> bool {
        let position = self.state.position();
        position.abs() >= self.config.max_position_btc
    }

    /// Check and reduce position if needed
    ///
    /// Triggers reduction when:
    /// - Position > 70% of max
    /// - uPNL > 0
    ///
    /// Reduces to 50% of max position.
    ///
    /// # Returns
    /// true if reduction was triggered
    ///
    /// # Requirements: 7.2, 7.3
    async fn check_and_reduce_position(&self) -> Result<bool> {
        if let Some((reduce_qty, reduce_side)) = self.calculate_reduction() {
            info!(
                "Reducing position: {} {} (reduce_only market order)",
                reduce_side, reduce_qty
            );

            let side = if reduce_side == "sell" {
                Side::Sell
            } else {
                Side::Buy
            };

            // Place reduce-only market order
            let cl_ord_id = Self::generate_order_id(reduce_side);
            let request = NewOrderRequest {
                symbol: self.config.symbol.clone(),
                side,
                order_type: OrderType::Market,
                qty: reduce_qty,
                price: Decimal::ZERO, // Market order, price not used
                time_in_force: TimeInForce::Ioc,
                reduce_only: true,
                cl_ord_id: cl_ord_id.clone(),
            };

            match self.client.new_order(request).await {
                Ok(response) => {
                    if response.code == 0 {
                        info!("Position reduction order placed: {}", cl_ord_id);
                    } else {
                        error!(
                            "Position reduction rejected: code={}, message={:?}",
                            response.code, response.message
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to place reduction order: {}", e);
                }
            }

            return Ok(true);
        }

        Ok(false)
    }

    /// Check stop loss / take profit and close position if triggered
    ///
    /// Calculates PnL percentage based on position value at entry price.
    /// If take_profit_pct or stop_loss_pct is configured and triggered,
    /// closes the entire position with a reduce_only market order.
    ///
    /// # Returns
    /// true if position was closed due to stop loss or take profit
    async fn check_stop_loss_take_profit(&self) -> Result<bool> {
        let position = self.state.position();
        let upnl = self.state.upnl();
        let last_price = match self.state.last_price() {
            Some(p) => p,
            None => return Ok(false),
        };

        // No position, nothing to check
        if position == Decimal::ZERO {
            return Ok(false);
        }

        let abs_position = position.abs();
        
        // Calculate position value at current price (approximate entry value)
        let position_value = abs_position * last_price;
        
        // Avoid division by zero
        if position_value == Decimal::ZERO {
            return Ok(false);
        }

        // Calculate PnL percentage: upnl / position_value * 100
        let pnl_pct = (upnl / position_value) * Decimal::new(100, 0);

        // Check take profit
        if let Some(tp_pct) = self.config.take_profit_pct {
            if pnl_pct >= tp_pct {
                info!(
                    "Take profit triggered: PnL {:.2}% >= {:.2}%, closing position {}",
                    pnl_pct, tp_pct, abs_position
                );
                return self.close_position_market("take_profit").await;
            }
        }

        // Check stop loss (pnl_pct will be negative for losses)
        if let Some(sl_pct) = self.config.stop_loss_pct {
            let sl_threshold = -sl_pct; // Convert to negative for comparison
            if pnl_pct <= sl_threshold {
                warn!(
                    "Stop loss triggered: PnL {:.2}% <= -{:.2}%, closing position {}",
                    pnl_pct, sl_pct, abs_position
                );
                return self.close_position_market("stop_loss").await;
            }
        }

        Ok(false)
    }

    /// Close entire position with a reduce_only market order
    async fn close_position_market(&self, reason: &str) -> Result<bool> {
        let position = self.state.position();
        if position == Decimal::ZERO {
            return Ok(false);
        }

        let abs_position = position.abs();
        let close_side = if position > Decimal::ZERO {
            Side::Sell
        } else {
            Side::Buy
        };

        let cl_ord_id = Self::generate_order_id(&format!("{}-{}", reason, close_side));
        let request = NewOrderRequest {
            symbol: self.config.symbol.clone(),
            side: close_side,
            order_type: OrderType::Market,
            qty: abs_position,
            price: Decimal::ZERO,
            time_in_force: TimeInForce::Ioc,
            reduce_only: true,
            cl_ord_id: cl_ord_id.clone(),
        };

        info!(
            "Closing position due to {}: {} {} @ market",
            reason, close_side, abs_position
        );

        match self.client.new_order(request).await {
            Ok(response) => {
                if response.code == 0 {
                    info!("Position close order placed: {} ({})", cl_ord_id, reason);
                } else {
                    error!(
                        "Position close rejected: code={}, message={:?}",
                        response.code, response.message
                    );
                }
            }
            Err(e) => {
                error!("Failed to close position: {}", e);
            }
        }

        Ok(true)
    }

    /// Calculate position reduction if needed
    ///
    /// Returns (reduce_qty, reduce_side) if reduction is needed:
    /// - Position > 70% of max AND uPNL > 0
    /// - Reduce to 50% of max
    /// - Side is "sell" if position > 0, "buy" if position < 0
    ///
    /// # Requirements: 7.2
    pub fn calculate_reduction(&self) -> Option<(Decimal, &'static str)> {
        let position = self.state.position();
        let upnl = self.state.upnl();
        let max_position = self.config.max_position_btc;

        // 70% threshold
        let threshold = max_position * Decimal::new(70, 2); // 0.70
        // 50% target
        let target = max_position * Decimal::new(50, 2); // 0.50

        let abs_position = position.abs();

        // Check if position exceeds 70% and uPNL > 0
        if abs_position > threshold && upnl > Decimal::ZERO {
            // Calculate reduction amount
            let reduce_qty = abs_position - target;

            // Determine side: sell if long, buy if short
            let reduce_side = if position > Decimal::ZERO {
                "sell"
            } else {
                "buy"
            };

            Some((reduce_qty, reduce_side))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn create_test_config() -> Config {
        Config {
            wallet: crate::config::WalletConfig {
                chain: "bsc".to_string(),
                private_key: "0x1234".to_string(),
            },
            symbol: "BTC-USD".to_string(),
            order_distance_bps: 10,
            cancel_distance_bps: 5,
            rebalance_distance_bps: 20,
            order_size_btc: dec!(0.01),
            max_position_btc: dec!(0.1),
            volatility_window_sec: 5,
            volatility_threshold_bps: 5,
            min_balance_usd: dec!(99),
            take_profit_pct: None,
            stop_loss_pct: None,
            referral: crate::config::ReferralConfig::default(),
        }
    }

    #[test]
    fn test_generate_order_id_buy() {
        let id = Maker::generate_order_id("buy");
        assert!(id.starts_with("mm-buy-"));
        // "mm-buy-12345678" = 15 chars
        assert_eq!(id.len(), 15);
        
        // Check format with regex pattern
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "mm");
        assert_eq!(parts[1], "buy");
        assert_eq!(parts[2].len(), 8);
        // Check that uuid8 is hex
        assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_order_id_sell() {
        let id = Maker::generate_order_id("sell");
        assert!(id.starts_with("mm-sell-"));
        
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "mm");
        assert_eq!(parts[1], "sell");
        assert_eq!(parts[2].len(), 8);
        assert!(parts[2].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_order_id_unique() {
        let id1 = Maker::generate_order_id("buy");
        let id2 = Maker::generate_order_id("buy");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_should_pause_at_max_position() {
        let config = create_test_config();
        let state = State::new();
        
        // Position at max
        state.update_position(dec!(0.1), dec!(0));
        
        // Create maker without client (we won't use it in this test)
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        assert!(maker.should_pause_market_making());
    }

    #[test]
    fn test_should_not_pause_below_max() {
        let config = create_test_config();
        let state = State::new();
        
        // Position below max
        state.update_position(dec!(0.05), dec!(0));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        assert!(!maker.should_pause_market_making());
    }

    #[test]
    fn test_should_pause_negative_position() {
        let config = create_test_config();
        let state = State::new();
        
        // Negative position at max
        state.update_position(dec!(-0.1), dec!(0));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        assert!(maker.should_pause_market_making());
    }

    #[test]
    fn test_calculate_reduction_long_position() {
        let config = create_test_config(); // max_position = 0.1
        let state = State::new();
        
        // Position at 80% of max (0.08) with positive uPNL
        state.update_position(dec!(0.08), dec!(100));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        let result = maker.calculate_reduction();
        assert!(result.is_some());
        
        let (reduce_qty, reduce_side) = result.unwrap();
        // Reduce from 0.08 to 0.05 (50% of max)
        assert_eq!(reduce_qty, dec!(0.03));
        assert_eq!(reduce_side, "sell");
    }

    #[test]
    fn test_calculate_reduction_short_position() {
        let config = create_test_config();
        let state = State::new();
        
        // Short position at 80% of max with positive uPNL
        state.update_position(dec!(-0.08), dec!(100));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        let result = maker.calculate_reduction();
        assert!(result.is_some());
        
        let (reduce_qty, reduce_side) = result.unwrap();
        assert_eq!(reduce_qty, dec!(0.03));
        assert_eq!(reduce_side, "buy");
    }

    #[test]
    fn test_calculate_reduction_no_reduction_below_threshold() {
        let config = create_test_config();
        let state = State::new();
        
        // Position at 60% of max (below 70% threshold)
        state.update_position(dec!(0.06), dec!(100));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        assert!(maker.calculate_reduction().is_none());
    }

    #[test]
    fn test_calculate_reduction_no_reduction_negative_upnl() {
        let config = create_test_config();
        let state = State::new();
        
        // Position at 80% but negative uPNL
        state.update_position(dec!(0.08), dec!(-100));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        assert!(maker.calculate_reduction().is_none());
    }

    #[test]
    fn test_calculate_reduction_no_reduction_zero_upnl() {
        let config = create_test_config();
        let state = State::new();
        
        // Position at 80% but zero uPNL
        state.update_position(dec!(0.08), dec!(0));
        
        let auth = Arc::new(tokio::sync::RwLock::new(crate::auth::StandXAuth::new()));
        let client = Arc::new(StandXHttpClient::new(auth));
        let maker = Maker::new(config, client, state);
        
        assert!(maker.calculate_reduction().is_none());
    }
}
