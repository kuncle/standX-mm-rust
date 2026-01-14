//! Property tests for position management
//!
//! Tests for Property 10 and 11
//! **Validates: Requirements 7.1, 7.2**

use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use standx_mm::config::{Config, ReferralConfig, WalletConfig};
use standx_mm::http_client::StandXHttpClient;
use standx_mm::Maker;
use standx_mm::State;
use std::sync::Arc;

/// Helper to create a test config with specified max_position
fn create_test_config(max_position_btc: Decimal) -> Config {
    Config {
        wallet: WalletConfig {
            chain: "bsc".to_string(),
            private_key: "0x1234".to_string(),
        },
        symbol: "BTC-USD".to_string(),
        order_distance_bps: 10,
        cancel_distance_bps: 5,
        rebalance_distance_bps: 20,
        order_size_btc: dec!(0.01),
        max_position_btc,
        volatility_window_sec: 5,
        volatility_threshold_bps: 5,
        min_balance_usd: dec!(100),
        take_profit_pct: None,
        stop_loss_pct: None,
        referral: ReferralConfig::default(),
    }
}

/// Helper to create a maker instance for testing
fn create_test_maker(max_position_btc: Decimal, position: Decimal, upnl: Decimal) -> Maker {
    let config = create_test_config(max_position_btc);
    let state = State::new();
    state.update_position(position, upnl);

    let auth = Arc::new(tokio::sync::RwLock::new(standx_mm::StandXAuth::new()));
    let client = Arc::new(StandXHttpClient::new(auth));
    Maker::new(config, client, state)
}

/// Property 10: Position Limit Check
///
/// *For any* position value and max_position_btc, should_pause_market_making
/// returns true if and only if |position| >= max_position_btc.
///
/// **Feature: standx-maker-bot, Property 10: Position Limit Check**
/// **Validates: Requirements 7.1**
mod property_10_position_limit_check {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn test_pause_when_position_at_or_above_max(
            // max_position between 0.01 and 10.0 BTC
            max_position in 0.01f64..10.0,
            // position_ratio >= 1.0 (at or above max)
            position_ratio in 1.0f64..2.0,
        ) {
            // Feature: standx-maker-bot, Property 10: Position Limit Check
            // Validates: Requirements 7.1

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let should_pause = maker.should_pause_market_making();

            // |position| >= max_position_btc should return true
            prop_assert!(
                should_pause,
                "should_pause_market_making should return true when |position| ({}) >= max_position ({})",
                position.abs(), max_pos
            );
        }

        #[test]
        fn test_no_pause_when_position_below_max(
            // max_position between 0.01 and 10.0 BTC
            max_position in 0.01f64..10.0,
            // position_ratio < 1.0 (below max)
            position_ratio in 0.0f64..0.99,
        ) {
            // Feature: standx-maker-bot, Property 10: Position Limit Check
            // Validates: Requirements 7.1

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let should_pause = maker.should_pause_market_making();

            // |position| < max_position_btc should return false
            prop_assert!(
                !should_pause,
                "should_pause_market_making should return false when |position| ({}) < max_position ({})",
                position.abs(), max_pos
            );
        }

        #[test]
        fn test_pause_for_negative_position_at_or_above_max(
            // max_position between 0.01 and 10.0 BTC
            max_position in 0.01f64..10.0,
            // position_ratio >= 1.0 (at or above max, but negative)
            position_ratio in 1.0f64..2.0,
        ) {
            // Feature: standx-maker-bot, Property 10: Position Limit Check
            // Validates: Requirements 7.1

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            // Negative position (short)
            let position = -(max_pos * Decimal::from_f64_retain(position_ratio).unwrap());

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let should_pause = maker.should_pause_market_making();

            // |position| >= max_position_btc should return true (even for negative positions)
            prop_assert!(
                should_pause,
                "should_pause_market_making should return true for short position when |position| ({}) >= max_position ({})",
                position.abs(), max_pos
            );
        }

        #[test]
        fn test_no_pause_for_negative_position_below_max(
            // max_position between 0.01 and 10.0 BTC
            max_position in 0.01f64..10.0,
            // position_ratio < 1.0 (below max, but negative)
            position_ratio in 0.0f64..0.99,
        ) {
            // Feature: standx-maker-bot, Property 10: Position Limit Check
            // Validates: Requirements 7.1

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            // Negative position (short)
            let position = -(max_pos * Decimal::from_f64_retain(position_ratio).unwrap());

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let should_pause = maker.should_pause_market_making();

            // |position| < max_position_btc should return false (even for negative positions)
            prop_assert!(
                !should_pause,
                "should_pause_market_making should return false for short position when |position| ({}) < max_position ({})",
                position.abs(), max_pos
            );
        }

        #[test]
        fn test_pause_exactly_at_max(
            // max_position between 0.01 and 10.0 BTC
            max_position in 0.01f64..10.0,
        ) {
            // Feature: standx-maker-bot, Property 10: Position Limit Check
            // Validates: Requirements 7.1

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            // Position exactly at max
            let position = max_pos;

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let should_pause = maker.should_pause_market_making();

            // |position| == max_position_btc should return true
            prop_assert!(
                should_pause,
                "should_pause_market_making should return true when position ({}) == max_position ({})",
                position, max_pos
            );
        }

        #[test]
        fn test_biconditional_property(
            // max_position between 0.01 and 10.0 BTC
            max_position in 0.01f64..10.0,
            // position can be anywhere from -2x to 2x max
            position_multiplier in -2.0f64..2.0,
        ) {
            // Feature: standx-maker-bot, Property 10: Position Limit Check
            // Validates: Requirements 7.1
            //
            // This test verifies the biconditional: should_pause returns true
            // if and only if |position| >= max_position_btc

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_multiplier).unwrap();

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let should_pause = maker.should_pause_market_making();

            let expected_pause = position.abs() >= max_pos;

            prop_assert_eq!(
                should_pause, expected_pause,
                "should_pause_market_making returned {} but expected {} for |position| ({}) vs max_position ({})",
                should_pause, expected_pause, position.abs(), max_pos
            );
        }
    }
}

/// Property 11: Position Reduction Calculation
///
/// *For any* position where |position| > max_position * 0.7 and uPNL > 0:
/// - Reduction should bring position to max_position * 0.5
/// - Reduce quantity = |position| - max_position * 0.5
/// - Reduce side = "sell" if position > 0, "buy" if position < 0
///
/// **Feature: standx-maker-bot, Property 11: Position Reduction Calculation**
/// **Validates: Requirements 7.2**
mod property_11_position_reduction_calculation {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn test_reduction_triggers_above_70_percent_with_positive_upnl(
            // max_position between 0.1 and 10.0 BTC
            max_position in 0.1f64..10.0,
            // position_ratio between 0.71 and 1.0 (above 70% threshold)
            position_ratio in 0.71f64..1.0,
            // upnl between 1 and 10000 USD (positive)
            upnl in 1.0f64..10000.0,
        ) {
            // Feature: standx-maker-bot, Property 11: Position Reduction Calculation
            // Validates: Requirements 7.2

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();
            let upnl_dec = Decimal::from_f64_retain(upnl).unwrap();

            let maker = create_test_maker(max_pos, position, upnl_dec);
            let result = maker.calculate_reduction();

            // Should trigger reduction
            prop_assert!(
                result.is_some(),
                "Reduction should trigger when position ({}) > 70% of max ({}) and uPNL ({}) > 0",
                position, max_pos, upnl_dec
            );

            let (reduce_qty, reduce_side) = result.unwrap();

            // Target is 50% of max
            let target = max_pos * Decimal::new(50, 2);
            let expected_reduce_qty = position - target;

            // Verify reduction quantity
            prop_assert!(
                (reduce_qty - expected_reduce_qty).abs() < Decimal::new(1, 10),
                "Reduce qty {} should equal position {} - target {} = {}",
                reduce_qty, position, target, expected_reduce_qty
            );

            // Verify side is "sell" for long position
            prop_assert_eq!(
                reduce_side, "sell",
                "Reduce side should be 'sell' for long position"
            );
        }

        #[test]
        fn test_reduction_triggers_for_short_position(
            max_position in 0.1f64..10.0,
            position_ratio in 0.71f64..1.0,
            upnl in 1.0f64..10000.0,
        ) {
            // Feature: standx-maker-bot, Property 11: Position Reduction Calculation
            // Validates: Requirements 7.2

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            // Negative position (short)
            let position = -(max_pos * Decimal::from_f64_retain(position_ratio).unwrap());
            let upnl_dec = Decimal::from_f64_retain(upnl).unwrap();

            let maker = create_test_maker(max_pos, position, upnl_dec);
            let result = maker.calculate_reduction();

            prop_assert!(
                result.is_some(),
                "Reduction should trigger for short position"
            );

            let (reduce_qty, reduce_side) = result.unwrap();

            // Target is 50% of max
            let target = max_pos * Decimal::new(50, 2);
            let expected_reduce_qty = position.abs() - target;

            prop_assert!(
                (reduce_qty - expected_reduce_qty).abs() < Decimal::new(1, 10),
                "Reduce qty {} should equal |position| {} - target {} = {}",
                reduce_qty, position.abs(), target, expected_reduce_qty
            );

            // Verify side is "buy" for short position
            prop_assert_eq!(
                reduce_side, "buy",
                "Reduce side should be 'buy' for short position"
            );
        }

        #[test]
        fn test_no_reduction_below_70_percent(
            max_position in 0.1f64..10.0,
            // position_ratio between 0.0 and 0.69 (below 70% threshold)
            position_ratio in 0.0f64..0.69,
            upnl in 1.0f64..10000.0,
        ) {
            // Feature: standx-maker-bot, Property 11: Position Reduction Calculation
            // Validates: Requirements 7.2

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();
            let upnl_dec = Decimal::from_f64_retain(upnl).unwrap();

            let maker = create_test_maker(max_pos, position, upnl_dec);
            let result = maker.calculate_reduction();

            prop_assert!(
                result.is_none(),
                "No reduction should trigger when position ({}) <= 70% of max ({})",
                position, max_pos
            );
        }

        #[test]
        fn test_no_reduction_with_negative_upnl(
            max_position in 0.1f64..10.0,
            position_ratio in 0.71f64..1.0,
            // Negative uPNL
            upnl in -10000.0f64..-0.01,
        ) {
            // Feature: standx-maker-bot, Property 11: Position Reduction Calculation
            // Validates: Requirements 7.2

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();
            let upnl_dec = Decimal::from_f64_retain(upnl).unwrap();

            let maker = create_test_maker(max_pos, position, upnl_dec);
            let result = maker.calculate_reduction();

            prop_assert!(
                result.is_none(),
                "No reduction should trigger when uPNL ({}) <= 0",
                upnl_dec
            );
        }

        #[test]
        fn test_no_reduction_with_zero_upnl(
            max_position in 0.1f64..10.0,
            position_ratio in 0.71f64..1.0,
        ) {
            // Feature: standx-maker-bot, Property 11: Position Reduction Calculation
            // Validates: Requirements 7.2

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();

            let maker = create_test_maker(max_pos, position, Decimal::ZERO);
            let result = maker.calculate_reduction();

            prop_assert!(
                result.is_none(),
                "No reduction should trigger when uPNL = 0"
            );
        }

        #[test]
        fn test_reduction_quantity_is_positive(
            max_position in 0.1f64..10.0,
            position_ratio in 0.71f64..1.0,
            upnl in 1.0f64..10000.0,
        ) {
            // Feature: standx-maker-bot, Property 11: Position Reduction Calculation
            // Validates: Requirements 7.2

            let max_pos = Decimal::from_f64_retain(max_position).unwrap();
            let position = max_pos * Decimal::from_f64_retain(position_ratio).unwrap();
            let upnl_dec = Decimal::from_f64_retain(upnl).unwrap();

            let maker = create_test_maker(max_pos, position, upnl_dec);

            if let Some((reduce_qty, _)) = maker.calculate_reduction() {
                prop_assert!(
                    reduce_qty > Decimal::ZERO,
                    "Reduce quantity {} should be positive",
                    reduce_qty
                );
            }
        }
    }
}
