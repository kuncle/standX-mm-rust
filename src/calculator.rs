//! Price calculation module
//!
//! Handles precise price calculations using Decimal arithmetic.
//! Provides accurate order price calculations for market making,
//! avoiding floating-point precision issues.

use rust_decimal::Decimal;

/// Price calculator with tick size alignment.
///
/// Uses Decimal arithmetic to maintain precision in internal calculations,
/// then rounds to tick size for final prices.
pub struct PriceCalculator {
    tick_size: Decimal,
}

impl PriceCalculator {
    /// Create a new price calculator for the given symbol.
    ///
    /// # Arguments
    /// * `symbol` - Trading symbol (e.g., "BTC-USD", "ETH-USD")
    ///
    /// # Tick Sizes
    /// - BTC symbols: 0.01
    /// - Other symbols: 0.1
    pub fn new(symbol: &str) -> Self {
        let tick_size = if symbol.starts_with("BTC") {
            Decimal::new(1, 2) // 0.01
        } else {
            Decimal::new(1, 1) // 0.1
        };
        Self { tick_size }
    }

    /// Create a price calculator with a custom tick size.
    ///
    /// # Arguments
    /// * `tick_size` - Custom tick size for price alignment
    pub fn with_tick_size(tick_size: Decimal) -> Self {
        Self { tick_size }
    }

    /// Get the tick size.
    pub fn tick_size(&self) -> Decimal {
        self.tick_size
    }

    /// Calculate buy price (rounded DOWN to tick size).
    ///
    /// Buy price = last_price * (1 - distance_bps / 10000)
    /// Rounded DOWN to tick size (more conservative for buys).
    ///
    /// # Arguments
    /// * `last_price` - Current market price
    /// * `distance_bps` - Distance from last price in basis points (1 bps = 0.01%)
    ///
    /// # Returns
    /// Buy price aligned to tick size
    pub fn calculate_buy_price(&self, last_price: Decimal, distance_bps: u32) -> Decimal {
        // bps_multiplier = distance_bps / 10000
        let bps_multiplier = Decimal::new(distance_bps as i64, 4);
        // raw_price = last_price * (1 - bps_multiplier)
        let raw_price = last_price * (Decimal::ONE - bps_multiplier);
        self.align_to_tick(raw_price, false)
    }

    /// Calculate sell price (rounded UP to tick size).
    ///
    /// Sell price = last_price * (1 + distance_bps / 10000)
    /// Rounded UP to tick size (more conservative for sells).
    ///
    /// # Arguments
    /// * `last_price` - Current market price
    /// * `distance_bps` - Distance from last price in basis points (1 bps = 0.01%)
    ///
    /// # Returns
    /// Sell price aligned to tick size
    pub fn calculate_sell_price(&self, last_price: Decimal, distance_bps: u32) -> Decimal {
        // bps_multiplier = distance_bps / 10000
        let bps_multiplier = Decimal::new(distance_bps as i64, 4);
        // raw_price = last_price * (1 + bps_multiplier)
        let raw_price = last_price * (Decimal::ONE + bps_multiplier);
        self.align_to_tick(raw_price, true)
    }

    /// Align price to tick size with specified rounding.
    ///
    /// # Arguments
    /// * `price` - Raw price to align
    /// * `round_up` - If true, round up; if false, round down
    ///
    /// # Returns
    /// Price aligned to tick size
    fn align_to_tick(&self, price: Decimal, round_up: bool) -> Decimal {
        // Divide by tick size to get number of ticks
        let ticks = price / self.tick_size;

        // Round to integer number of ticks
        let rounded_ticks = if round_up {
            ticks.ceil()
        } else {
            ticks.floor()
        };

        // Multiply back by tick size
        rounded_ticks * self.tick_size
    }

    /// Format price for API submission.
    ///
    /// Determines decimal places from tick size and formats accordingly.
    ///
    /// # Arguments
    /// * `price` - Price to format
    ///
    /// # Returns
    /// String representation with correct decimal places
    pub fn format_price(&self, price: Decimal) -> String {
        let decimals = self.get_decimal_places();
        format!("{:.prec$}", price, prec = decimals)
    }

    /// Get the number of decimal places for this calculator's tick size.
    fn get_decimal_places(&self) -> usize {
        let tick_str = self.tick_size.to_string();
        if let Some(dot_pos) = tick_str.find('.') {
            tick_str[dot_pos + 1..].trim_end_matches('0').len().max(1)
        } else {
            0
        }
    }
}

/// Get tick size for a symbol.
///
/// # Arguments
/// * `symbol` - Trading symbol (e.g., "BTC-USD", "ETH-USD")
///
/// # Returns
/// Tick size as Decimal
pub fn get_tick_size(symbol: &str) -> Decimal {
    if symbol.starts_with("BTC") {
        Decimal::new(1, 2) // 0.01
    } else {
        Decimal::new(1, 1) // 0.1
    }
}

/// Get number of decimal places for a symbol's price.
///
/// # Arguments
/// * `symbol` - Trading symbol
///
/// # Returns
/// Number of decimal places
pub fn get_price_decimals(symbol: &str) -> usize {
    if symbol.starts_with("BTC") {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_new_btc_symbol() {
        let calc = PriceCalculator::new("BTC-USD");
        assert_eq!(calc.tick_size(), dec!(0.01));
    }

    #[test]
    fn test_new_eth_symbol() {
        let calc = PriceCalculator::new("ETH-USD");
        assert_eq!(calc.tick_size(), dec!(0.1));
    }

    #[test]
    fn test_calculate_buy_price_basic() {
        let calc = PriceCalculator::new("BTC-USD");
        // 100000 * (1 - 10/10000) = 100000 * 0.999 = 99900
        let buy_price = calc.calculate_buy_price(dec!(100000), 10);
        assert_eq!(buy_price, dec!(99900.00));
    }

    #[test]
    fn test_calculate_sell_price_basic() {
        let calc = PriceCalculator::new("BTC-USD");
        // 100000 * (1 + 10/10000) = 100000 * 1.001 = 100100
        let sell_price = calc.calculate_sell_price(dec!(100000), 10);
        assert_eq!(sell_price, dec!(100100.00));
    }

    #[test]
    fn test_buy_price_rounds_down() {
        let calc = PriceCalculator::new("BTC-USD");
        // 99999.99 * (1 - 10/10000) = 99999.99 * 0.999 = 99899.99001
        // Should round DOWN to 99899.99
        let buy_price = calc.calculate_buy_price(dec!(99999.99), 10);
        assert_eq!(buy_price, dec!(99899.99));
    }

    #[test]
    fn test_sell_price_rounds_up() {
        let calc = PriceCalculator::new("BTC-USD");
        // 99999.99 * (1 + 10/10000) = 99999.99 * 1.001 = 100099.98999
        // Should round UP to 100099.99
        let sell_price = calc.calculate_sell_price(dec!(99999.99), 10);
        assert_eq!(sell_price, dec!(100099.99));
    }

    #[test]
    fn test_tick_alignment_eth() {
        let calc = PriceCalculator::new("ETH-USD");
        // 3500.55 * (1 - 10/10000) = 3500.55 * 0.999 = 3497.04945
        // Should round DOWN to 3497.0
        let buy_price = calc.calculate_buy_price(dec!(3500.55), 10);
        assert_eq!(buy_price, dec!(3497.0));

        // 3500.55 * (1 + 10/10000) = 3500.55 * 1.001 = 3504.05055
        // Should round UP to 3504.1
        let sell_price = calc.calculate_sell_price(dec!(3500.55), 10);
        assert_eq!(sell_price, dec!(3504.1));
    }

    #[test]
    fn test_zero_distance() {
        let calc = PriceCalculator::new("BTC-USD");
        let buy_price = calc.calculate_buy_price(dec!(50000), 0);
        let sell_price = calc.calculate_sell_price(dec!(50000), 0);
        // With 0 bps, both should equal the last price (aligned to tick)
        assert_eq!(buy_price, dec!(50000.00));
        assert_eq!(sell_price, dec!(50000.00));
    }

    #[test]
    fn test_format_price_btc() {
        let calc = PriceCalculator::new("BTC-USD");
        assert_eq!(calc.format_price(dec!(99999.99)), "99999.99");
        assert_eq!(calc.format_price(dec!(100000)), "100000.00");
    }

    #[test]
    fn test_format_price_eth() {
        let calc = PriceCalculator::new("ETH-USD");
        assert_eq!(calc.format_price(dec!(3500.5)), "3500.5");
        assert_eq!(calc.format_price(dec!(3500)), "3500.0");
    }

    #[test]
    fn test_get_tick_size() {
        assert_eq!(get_tick_size("BTC-USD"), dec!(0.01));
        assert_eq!(get_tick_size("ETH-USD"), dec!(0.1));
        assert_eq!(get_tick_size("SOL-USD"), dec!(0.1));
    }

    #[test]
    fn test_get_price_decimals() {
        assert_eq!(get_price_decimals("BTC-USD"), 2);
        assert_eq!(get_price_decimals("ETH-USD"), 1);
    }
}
