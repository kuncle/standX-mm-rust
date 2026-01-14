//! State management module
//!
//! Thread-safe state management for the maker bot.
//! Tracks prices, positions, and open orders with RwLock for concurrent access.

use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::http_client::Side;

/// Thread-safe state manager
///
/// Provides concurrent access to bot state including prices, positions, and orders.
/// Uses RwLock to allow multiple readers or a single writer.
#[derive(Clone)]
pub struct State {
    inner: Arc<RwLock<StateInner>>,
}

/// Internal state data
struct StateInner {
    /// Last known price
    last_price: Option<Decimal>,
    /// Price window for volatility calculation: (timestamp_ms, price)
    price_window: Vec<(u64, Decimal)>,
    /// Current position quantity
    position: Decimal,
    /// Unrealized PnL
    upnl: Decimal,
    /// Open orders by side (at most one per side)
    open_orders: HashMap<Side, Option<OpenOrder>>,
}

/// Open order information
#[derive(Debug, Clone)]
pub struct OpenOrder {
    /// Client order ID
    pub cl_ord_id: String,
    /// Order side (Buy/Sell)
    pub side: Side,
    /// Order price
    pub price: Decimal,
    /// Order quantity
    pub qty: Decimal,
}

impl State {
    /// Create a new state manager
    pub fn new() -> Self {
        let mut open_orders = HashMap::new();
        open_orders.insert(Side::Buy, None);
        open_orders.insert(Side::Sell, None);

        Self {
            inner: Arc::new(RwLock::new(StateInner {
                last_price: None,
                price_window: Vec::new(),
                position: Decimal::ZERO,
                upnl: Decimal::ZERO,
                open_orders,
            })),
        }
    }

    /// Get the last price
    pub fn last_price(&self) -> Option<Decimal> {
        self.inner.read().unwrap().last_price
    }

    /// Get the current position
    pub fn position(&self) -> Decimal {
        self.inner.read().unwrap().position
    }

    /// Get the unrealized PnL
    pub fn upnl(&self) -> Decimal {
        self.inner.read().unwrap().upnl
    }

    /// Get current timestamp in milliseconds
    fn current_timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// Update price and maintain sliding window
    ///
    /// # Arguments
    /// * `price` - New price to add
    /// * `window_sec` - Window size in seconds for volatility calculation
    pub fn update_price(&self, price: Decimal, window_sec: u64) {
        let now_ms = Self::current_timestamp_ms();
        let cutoff_ms = now_ms.saturating_sub(window_sec * 1000);

        let mut inner = self.inner.write().unwrap();
        inner.last_price = Some(price);

        // Add new price to window
        inner.price_window.push((now_ms, price));

        // Remove prices outside the window
        inner.price_window.retain(|(ts, _)| *ts >= cutoff_ms);
    }

    /// Update price with explicit timestamp (for testing)
    ///
    /// # Arguments
    /// * `price` - New price to add
    /// * `timestamp_ms` - Timestamp in milliseconds
    /// * `window_sec` - Window size in seconds for volatility calculation
    pub fn update_price_with_timestamp(&self, price: Decimal, timestamp_ms: u64, window_sec: u64) {
        let cutoff_ms = timestamp_ms.saturating_sub(window_sec * 1000);

        let mut inner = self.inner.write().unwrap();
        inner.last_price = Some(price);

        // Add new price to window
        inner.price_window.push((timestamp_ms, price));

        // Remove prices outside the window
        inner.price_window.retain(|(ts, _)| *ts >= cutoff_ms);
    }

    /// Calculate volatility in basis points (bps)
    ///
    /// Volatility = (max_price - min_price) / last_price * 10000
    ///
    /// # Returns
    /// Volatility in bps, or Decimal::ZERO if fewer than 2 prices in window
    /// (allowing orders to be placed when there's insufficient data)
    pub fn get_volatility_bps(&self) -> Decimal {
        let inner = self.inner.read().unwrap();

        // Need at least 2 prices to calculate volatility
        // Return 0 (low volatility) if insufficient data to allow order placement
        if inner.price_window.len() < 2 {
            return Decimal::ZERO;
        }

        let last_price = match inner.last_price {
            Some(p) if p > Decimal::ZERO => p,
            _ => return Decimal::ZERO,
        };

        // Find min and max prices in window
        let (min_price, max_price) = inner.price_window.iter().fold(
            (Decimal::MAX, Decimal::MIN),
            |(min, max), (_, price)| (min.min(*price), max.max(*price)),
        );

        // Calculate volatility: (max - min) / last_price * 10000
        let price_range = max_price - min_price;
        (price_range / last_price) * Decimal::new(10000, 0)
    }

    /// Update position
    ///
    /// # Arguments
    /// * `qty` - New position quantity
    /// * `upnl` - Unrealized PnL
    pub fn update_position(&self, qty: Decimal, upnl: Decimal) {
        let mut inner = self.inner.write().unwrap();
        inner.position = qty;
        inner.upnl = upnl;
    }

    /// Set an order for a side
    ///
    /// # Arguments
    /// * `side` - Order side (Buy/Sell)
    /// * `order` - Order to set, or None to clear
    pub fn set_order(&self, side: Side, order: Option<OpenOrder>) {
        let mut inner = self.inner.write().unwrap();
        inner.open_orders.insert(side, order);
    }

    /// Get an order for a side
    ///
    /// # Arguments
    /// * `side` - Order side to query
    ///
    /// # Returns
    /// Clone of the order if exists, None otherwise
    pub fn get_order(&self, side: Side) -> Option<OpenOrder> {
        let inner = self.inner.read().unwrap();
        inner.open_orders.get(&side).and_then(|o| o.clone())
    }

    /// Check if there's an order for a side
    ///
    /// # Arguments
    /// * `side` - Order side to check
    ///
    /// # Returns
    /// true if an order exists for the side
    pub fn has_order(&self, side: Side) -> bool {
        let inner = self.inner.read().unwrap();
        inner
            .open_orders
            .get(&side)
            .map(|o| o.is_some())
            .unwrap_or(false)
    }

    /// Clear all orders
    pub fn clear_all_orders(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.open_orders.insert(Side::Buy, None);
        inner.open_orders.insert(Side::Sell, None);
    }

    /// Get orders that need to be cancelled based on distance from last price
    ///
    /// An order should be cancelled if:
    /// - distance < cancel_bps (too close to price)
    /// - distance > rebalance_bps (too far from price)
    ///
    /// # Arguments
    /// * `cancel_bps` - Minimum distance in bps (cancel if closer)
    /// * `rebalance_bps` - Maximum distance in bps (cancel if farther)
    ///
    /// # Returns
    /// Vector of orders that should be cancelled
    pub fn get_orders_to_cancel(&self, cancel_bps: u32, rebalance_bps: u32) -> Vec<OpenOrder> {
        let inner = self.inner.read().unwrap();

        let last_price = match inner.last_price {
            Some(p) if p > Decimal::ZERO => p,
            _ => return Vec::new(),
        };

        let cancel_bps_dec = Decimal::new(cancel_bps as i64, 0);
        let rebalance_bps_dec = Decimal::new(rebalance_bps as i64, 0);

        let mut orders_to_cancel = Vec::new();

        for order_opt in inner.open_orders.values() {
            if let Some(order) = order_opt {
                // Calculate distance in bps: |order_price - last_price| / last_price * 10000
                let price_diff = if order.price > last_price {
                    order.price - last_price
                } else {
                    last_price - order.price
                };
                let distance_bps = (price_diff / last_price) * Decimal::new(10000, 0);

                // Cancel if too close or too far
                if distance_bps < cancel_bps_dec || distance_bps > rebalance_bps_dec {
                    orders_to_cancel.push(order.clone());
                }
            }
        }

        orders_to_cancel
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_new_state() {
        let state = State::new();
        assert!(state.last_price().is_none());
        assert_eq!(state.position(), Decimal::ZERO);
        assert_eq!(state.upnl(), Decimal::ZERO);
        assert!(!state.has_order(Side::Buy));
        assert!(!state.has_order(Side::Sell));
    }

    #[test]
    fn test_update_position() {
        let state = State::new();
        state.update_position(dec!(1.5), dec!(100));
        assert_eq!(state.position(), dec!(1.5));
        assert_eq!(state.upnl(), dec!(100));
    }

    #[test]
    fn test_set_and_get_order() {
        let state = State::new();

        let buy_order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(99900),
            qty: dec!(0.01),
        };

        state.set_order(Side::Buy, Some(buy_order.clone()));

        assert!(state.has_order(Side::Buy));
        assert!(!state.has_order(Side::Sell));

        let retrieved = state.get_order(Side::Buy).unwrap();
        assert_eq!(retrieved.cl_ord_id, "mm-buy-12345678");
        assert_eq!(retrieved.price, dec!(99900));
    }

    #[test]
    fn test_clear_order() {
        let state = State::new();

        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(99900),
            qty: dec!(0.01),
        };

        state.set_order(Side::Buy, Some(order));
        assert!(state.has_order(Side::Buy));

        state.set_order(Side::Buy, None);
        assert!(!state.has_order(Side::Buy));
    }

    #[test]
    fn test_clear_all_orders() {
        let state = State::new();

        let buy_order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(99900),
            qty: dec!(0.01),
        };

        let sell_order = OpenOrder {
            cl_ord_id: "mm-sell-87654321".to_string(),
            side: Side::Sell,
            price: dec!(100100),
            qty: dec!(0.01),
        };

        state.set_order(Side::Buy, Some(buy_order));
        state.set_order(Side::Sell, Some(sell_order));

        assert!(state.has_order(Side::Buy));
        assert!(state.has_order(Side::Sell));

        state.clear_all_orders();

        assert!(!state.has_order(Side::Buy));
        assert!(!state.has_order(Side::Sell));
    }

    #[test]
    fn test_update_price_with_timestamp() {
        let state = State::new();

        // Add prices with explicit timestamps
        state.update_price_with_timestamp(dec!(100000), 1000, 5);
        state.update_price_with_timestamp(dec!(100100), 2000, 5);
        state.update_price_with_timestamp(dec!(99900), 3000, 5);

        assert_eq!(state.last_price(), Some(dec!(99900)));
    }

    #[test]
    fn test_price_window_cleanup() {
        let state = State::new();

        // Add prices at different timestamps
        state.update_price_with_timestamp(dec!(100000), 1000, 5); // Will be removed
        state.update_price_with_timestamp(dec!(100100), 2000, 5); // Will be removed
        state.update_price_with_timestamp(dec!(99900), 7000, 5); // cutoff = 7000 - 5000 = 2000

        // Only prices >= 2000 should remain
        let inner = state.inner.read().unwrap();
        assert_eq!(inner.price_window.len(), 2); // 2000 and 7000
    }

    #[test]
    fn test_volatility_insufficient_prices() {
        let state = State::new();

        // No prices - should return 0 to allow order placement
        assert_eq!(state.get_volatility_bps(), Decimal::ZERO);

        // One price - should return 0 to allow order placement
        state.update_price_with_timestamp(dec!(100000), 1000, 5);
        assert_eq!(state.get_volatility_bps(), Decimal::ZERO);
    }

    #[test]
    fn test_volatility_calculation() {
        let state = State::new();

        // Add prices: min=99900, max=100100, last=100000
        state.update_price_with_timestamp(dec!(100000), 1000, 10);
        state.update_price_with_timestamp(dec!(100100), 2000, 10);
        state.update_price_with_timestamp(dec!(99900), 3000, 10);
        state.update_price_with_timestamp(dec!(100000), 4000, 10);

        // Volatility = (100100 - 99900) / 100000 * 10000 = 200 / 100000 * 10000 = 20 bps
        let volatility = state.get_volatility_bps();
        assert_eq!(volatility, dec!(20));
    }

    #[test]
    fn test_orders_to_cancel_too_close() {
        let state = State::new();

        // Set last price
        state.update_price_with_timestamp(dec!(100000), 1000, 5);

        // Order at 99950 = 5 bps away
        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(99950),
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        // cancel_bps=6, rebalance_bps=20 -> 5 bps < 6 bps, should cancel
        let to_cancel = state.get_orders_to_cancel(6, 20);
        assert_eq!(to_cancel.len(), 1);
        assert_eq!(to_cancel[0].cl_ord_id, "mm-buy-12345678");
    }

    #[test]
    fn test_orders_to_cancel_too_far() {
        let state = State::new();

        // Set last price
        state.update_price_with_timestamp(dec!(100000), 1000, 5);

        // Order at 97500 = 250 bps away
        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(97500),
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        // cancel_bps=5, rebalance_bps=20 -> 250 bps > 20 bps, should cancel
        let to_cancel = state.get_orders_to_cancel(5, 20);
        assert_eq!(to_cancel.len(), 1);
    }

    #[test]
    fn test_orders_to_cancel_in_range() {
        let state = State::new();

        // Set last price
        state.update_price_with_timestamp(dec!(100000), 1000, 5);

        // Order at 99900 = 10 bps away
        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(99900),
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        // cancel_bps=5, rebalance_bps=20 -> 10 bps is in range, should NOT cancel
        let to_cancel = state.get_orders_to_cancel(5, 20);
        assert!(to_cancel.is_empty());
    }

    #[test]
    fn test_orders_to_cancel_no_last_price() {
        let state = State::new();

        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: dec!(99900),
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        // No last price, should return empty
        let to_cancel = state.get_orders_to_cancel(5, 20);
        assert!(to_cancel.is_empty());
    }
}
