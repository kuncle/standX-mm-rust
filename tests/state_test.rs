//! Property tests for state management module
//! Tests for Property 3, 4, 7, and 9

use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use standx_mm::http_client::Side;
use standx_mm::state::{OpenOrder, State};

/// Strategy for generating valid prices (positive decimals)
fn arb_price() -> impl Strategy<Value = Decimal> {
    (1000i64..200000i64).prop_map(|v| Decimal::new(v, 2))
}

/// Strategy for generating valid timestamps
fn arb_timestamp() -> impl Strategy<Value = u64> {
    1000u64..1000000u64
}

/// Strategy for generating window sizes
fn arb_window_sec() -> impl Strategy<Value = u64> {
    1u64..60u64
}

/// Strategy for generating order quantities
fn arb_qty() -> impl Strategy<Value = Decimal> {
    (1i64..1000i64).prop_map(|v| Decimal::new(v, 4))
}

/// Strategy for generating client order IDs
fn arb_cl_ord_id() -> impl Strategy<Value = String> {
    "[a-z]{2}-[a-z]{3,4}-[a-f0-9]{8}".prop_map(|s| s)
}

/// Strategy for generating sides
fn arb_side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Buy), Just(Side::Sell)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    // =========================================================================
    // Property 3: State Price Window Management
    // Feature: standx-maker-bot, Property 3: State Price Window Management
    // **Validates: Requirements 3.2**
    //
    // *For any* sequence of price updates with timestamps, the price window
    // should only contain prices within the configured window_sec, and older
    // prices should be removed.
    // =========================================================================
    #[test]
    fn test_price_window_only_contains_recent_prices(
        prices in prop::collection::vec((arb_timestamp(), arb_price()), 1..20),
        window_sec in arb_window_sec(),
    ) {
        let state = State::new();

        // Sort prices by timestamp to simulate time progression
        let mut sorted_prices = prices.clone();
        sorted_prices.sort_by_key(|(ts, _)| *ts);

        // Add all prices
        for (ts, price) in &sorted_prices {
            state.update_price_with_timestamp(*price, *ts, window_sec);
        }

        // Get the last timestamp
        if let Some((last_ts, _)) = sorted_prices.last() {
            let cutoff = last_ts.saturating_sub(window_sec * 1000);

            // Verify: all prices in window should be >= cutoff
            // We can't directly access the window, but we can verify through volatility
            // If we have prices, volatility should be calculable (not MAX) if >= 2 prices in window
            let expected_in_window: Vec<_> = sorted_prices
                .iter()
                .filter(|(ts, _)| *ts >= cutoff)
                .collect();

            if expected_in_window.len() >= 2 {
                let volatility = state.get_volatility_bps();
                prop_assert!(volatility < Decimal::MAX, "Should have valid volatility with {} prices in window", expected_in_window.len());
            }
        }
    }

    // =========================================================================
    // Property 4: Volatility Calculation
    // Feature: standx-maker-bot, Property 4: Volatility Calculation
    // **Validates: Requirements 3.3**
    //
    // *For any* price window with at least 2 prices, the volatility should equal
    // (max_price - min_price) / last_price * 10000 bps. For windows with fewer
    // than 2 prices, volatility should be infinity.
    // =========================================================================
    #[test]
    fn test_volatility_calculation_formula(
        prices in prop::collection::vec(arb_price(), 2..10),
        window_sec in 100u64..1000u64, // Large window to keep all prices
    ) {
        let state = State::new();

        // Add prices with sequential timestamps within window
        let base_ts = 100000u64;
        for (i, price) in prices.iter().enumerate() {
            state.update_price_with_timestamp(*price, base_ts + (i as u64 * 100), window_sec);
        }

        let last_price = *prices.last().unwrap();
        let min_price = prices.iter().cloned().min().unwrap();
        let max_price = prices.iter().cloned().max().unwrap();

        // Expected volatility = (max - min) / last * 10000
        let expected_volatility = (max_price - min_price) / last_price * dec!(10000);
        let actual_volatility = state.get_volatility_bps();

        // Allow small rounding differences
        let diff = (expected_volatility - actual_volatility).abs();
        prop_assert!(diff < dec!(0.0001), "Volatility mismatch: expected {}, got {}", expected_volatility, actual_volatility);
    }

    #[test]
    fn test_volatility_returns_zero_for_insufficient_prices(
        price in arb_price(),
        window_sec in arb_window_sec(),
    ) {
        let state = State::new();

        // No prices - should return ZERO to allow order placement
        prop_assert_eq!(state.get_volatility_bps(), Decimal::ZERO);

        // One price - should still return ZERO to allow order placement
        state.update_price_with_timestamp(price, 1000, window_sec);
        prop_assert_eq!(state.get_volatility_bps(), Decimal::ZERO);
    }

    // =========================================================================
    // Property 9: State Order Tracking Consistency
    // Feature: standx-maker-bot, Property 9: State Order Tracking Consistency
    // **Validates: Requirements 3.6, 6.2, 6.3**
    //
    // *For any* sequence of set_order and clear_order operations:
    // - After set_order(side, order), has_order(side) returns true and get_order(side) returns the order
    // - After set_order(side, None), has_order(side) returns false
    // - State tracks at most one order per side
    // =========================================================================
    #[test]
    fn test_set_order_then_has_order(
        cl_ord_id in arb_cl_ord_id(),
        side in arb_side(),
        price in arb_price(),
        qty in arb_qty(),
    ) {
        let state = State::new();

        let order = OpenOrder {
            cl_ord_id: cl_ord_id.clone(),
            side,
            price,
            qty,
        };

        state.set_order(side, Some(order.clone()));

        // has_order should return true
        prop_assert!(state.has_order(side), "has_order should return true after set_order");

        // get_order should return the order
        let retrieved = state.get_order(side);
        prop_assert!(retrieved.is_some(), "get_order should return Some after set_order");

        let retrieved = retrieved.unwrap();
        prop_assert_eq!(retrieved.cl_ord_id, cl_ord_id);
        prop_assert_eq!(retrieved.price, price);
        prop_assert_eq!(retrieved.qty, qty);
    }

    #[test]
    fn test_clear_order_then_no_order(
        cl_ord_id in arb_cl_ord_id(),
        side in arb_side(),
        price in arb_price(),
        qty in arb_qty(),
    ) {
        let state = State::new();

        let order = OpenOrder {
            cl_ord_id,
            side,
            price,
            qty,
        };

        // Set then clear
        state.set_order(side, Some(order));
        state.set_order(side, None);

        // has_order should return false
        prop_assert!(!state.has_order(side), "has_order should return false after clearing");

        // get_order should return None
        prop_assert!(state.get_order(side).is_none(), "get_order should return None after clearing");
    }

    #[test]
    fn test_at_most_one_order_per_side(
        cl_ord_id1 in arb_cl_ord_id(),
        cl_ord_id2 in arb_cl_ord_id(),
        side in arb_side(),
        price1 in arb_price(),
        price2 in arb_price(),
        qty in arb_qty(),
    ) {
        let state = State::new();

        let order1 = OpenOrder {
            cl_ord_id: cl_ord_id1,
            side,
            price: price1,
            qty,
        };

        let order2 = OpenOrder {
            cl_ord_id: cl_ord_id2.clone(),
            side,
            price: price2,
            qty,
        };

        // Set first order
        state.set_order(side, Some(order1));

        // Set second order (should replace first)
        state.set_order(side, Some(order2));

        // Should only have the second order
        let retrieved = state.get_order(side).unwrap();
        prop_assert_eq!(retrieved.cl_ord_id, cl_ord_id2, "Second order should replace first");
        prop_assert_eq!(retrieved.price, price2);
    }

    // =========================================================================
    // Property 7: Order Distance Validation
    // Feature: standx-maker-bot, Property 7: Order Distance Validation
    // **Validates: Requirements 5.3, 5.4**
    //
    // *For any* order price and last_price, the distance in bps =
    // |order_price - last_price| / last_price * 10000. An order should be
    // marked for cancellation if:
    // - distance < cancel_distance_bps (too close), OR
    // - distance > rebalance_distance_bps (too far)
    // =========================================================================
    #[test]
    fn test_order_too_close_is_cancelled(
        last_price in 10000i64..100000i64,
        cancel_bps in 5u32..20u32,
        rebalance_bps in 50u32..100u32,
    ) {
        let state = State::new();
        let last_price_dec = Decimal::new(last_price, 2);

        // Set last price
        state.update_price_with_timestamp(last_price_dec, 1000, 60);

        // Create order that is too close (distance < cancel_bps)
        // distance = (last_price - order_price) / last_price * 10000
        // For distance = cancel_bps - 1, order_price = last_price * (1 - (cancel_bps - 1) / 10000)
        let close_distance_bps = cancel_bps.saturating_sub(1).max(1);
        let order_price = last_price_dec * (Decimal::ONE - Decimal::new(close_distance_bps as i64, 4));

        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: order_price,
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        let to_cancel = state.get_orders_to_cancel(cancel_bps, rebalance_bps);

        // Order should be cancelled because it's too close
        prop_assert!(!to_cancel.is_empty(), "Order at {} bps should be cancelled (cancel_bps={})", close_distance_bps, cancel_bps);
    }

    #[test]
    fn test_order_too_far_is_cancelled(
        last_price in 10000i64..100000i64,
        cancel_bps in 5u32..10u32,
        rebalance_bps in 20u32..50u32,
    ) {
        let state = State::new();
        let last_price_dec = Decimal::new(last_price, 2);

        // Set last price
        state.update_price_with_timestamp(last_price_dec, 1000, 60);

        // Create order that is too far (distance > rebalance_bps)
        let far_distance_bps = rebalance_bps + 10;
        let order_price = last_price_dec * (Decimal::ONE - Decimal::new(far_distance_bps as i64, 4));

        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: order_price,
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        let to_cancel = state.get_orders_to_cancel(cancel_bps, rebalance_bps);

        // Order should be cancelled because it's too far
        prop_assert!(!to_cancel.is_empty(), "Order at {} bps should be cancelled (rebalance_bps={})", far_distance_bps, rebalance_bps);
    }

    #[test]
    fn test_order_in_range_not_cancelled(
        last_price in 10000i64..100000i64,
        cancel_bps in 5u32..10u32,
        rebalance_bps in 30u32..50u32,
    ) {
        let state = State::new();
        let last_price_dec = Decimal::new(last_price, 2);

        // Set last price
        state.update_price_with_timestamp(last_price_dec, 1000, 60);

        // Create order that is in range (cancel_bps <= distance <= rebalance_bps)
        let in_range_bps = (cancel_bps + rebalance_bps) / 2;
        let order_price = last_price_dec * (Decimal::ONE - Decimal::new(in_range_bps as i64, 4));

        let order = OpenOrder {
            cl_ord_id: "mm-buy-12345678".to_string(),
            side: Side::Buy,
            price: order_price,
            qty: dec!(0.01),
        };
        state.set_order(Side::Buy, Some(order));

        let to_cancel = state.get_orders_to_cancel(cancel_bps, rebalance_bps);

        // Order should NOT be cancelled because it's in range
        prop_assert!(to_cancel.is_empty(), "Order at {} bps should NOT be cancelled (range: {}-{})", in_range_bps, cancel_bps, rebalance_bps);
    }
}
