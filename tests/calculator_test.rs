//! Property tests for price calculator module
//! Tests for Property 6 and 7

use proptest::prelude::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use standx_mm::calculator::PriceCalculator;

/// Property 6: Price Calculation Correctness
///
/// *For any* last_price > 0 and distance_bps in [0, 10000]:
/// - Buy price = floor(last_price * (1 - distance_bps/10000) / tick_size) * tick_size
/// - Sell price = ceil(last_price * (1 + distance_bps/10000) / tick_size) * tick_size
/// - Buy price < last_price < sell_price (when distance_bps > 0)
/// - Both prices are exact multiples of tick_size
///
/// **Validates: Requirements 4.2, 4.3, 4.4, 5.1, 5.2**
mod property_6_price_calculation {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// Feature: standx-maker-bot, Property 6: Price Calculation Correctness
        /// Tests that buy/sell prices are correctly calculated and aligned to tick size
        #[test]
        fn test_btc_price_calculation_correctness(
            // BTC prices typically range from 10000 to 200000
            last_price_cents in 1_000_000i64..20_000_000i64,
            // distance_bps from 1 to 1000 (0.01% to 10%)
            distance_bps in 1u32..1000u32,
        ) {
            // Feature: standx-maker-bot, Property 6: Price Calculation Correctness
            let last_price = Decimal::new(last_price_cents, 2); // Convert cents to dollars
            let calc = PriceCalculator::new("BTC-USD");
            let tick_size = dec!(0.01);

            let buy_price = calc.calculate_buy_price(last_price, distance_bps);
            let sell_price = calc.calculate_sell_price(last_price, distance_bps);

            // Property: Buy price < last_price (when distance_bps > 0)
            prop_assert!(
                buy_price < last_price,
                "Buy price {} should be less than last price {} with distance_bps {}",
                buy_price, last_price, distance_bps
            );

            // Property: Sell price > last_price (when distance_bps > 0)
            prop_assert!(
                sell_price > last_price,
                "Sell price {} should be greater than last price {} with distance_bps {}",
                sell_price, last_price, distance_bps
            );

            // Property: Both prices are exact multiples of tick_size
            prop_assert!(
                (buy_price % tick_size).is_zero(),
                "Buy price {} should be aligned to tick size {}",
                buy_price, tick_size
            );
            prop_assert!(
                (sell_price % tick_size).is_zero(),
                "Sell price {} should be aligned to tick size {}",
                sell_price, tick_size
            );

            // Property: Buy price is rounded DOWN (floor)
            // Expected: floor(last_price * (1 - distance_bps/10000) / tick_size) * tick_size
            let bps_multiplier = Decimal::new(distance_bps as i64, 4);
            let expected_raw_buy = last_price * (Decimal::ONE - bps_multiplier);
            let expected_buy = (expected_raw_buy / tick_size).floor() * tick_size;
            prop_assert_eq!(
                buy_price, expected_buy,
                "Buy price calculation mismatch for last_price={}, distance_bps={}",
                last_price, distance_bps
            );

            // Property: Sell price is rounded UP (ceil)
            // Expected: ceil(last_price * (1 + distance_bps/10000) / tick_size) * tick_size
            let expected_raw_sell = last_price * (Decimal::ONE + bps_multiplier);
            let expected_sell = (expected_raw_sell / tick_size).ceil() * tick_size;
            prop_assert_eq!(
                sell_price, expected_sell,
                "Sell price calculation mismatch for last_price={}, distance_bps={}",
                last_price, distance_bps
            );
        }

        /// Feature: standx-maker-bot, Property 6: Price Calculation Correctness (ETH)
        /// Tests price calculation for non-BTC symbols with 0.1 tick size
        #[test]
        fn test_eth_price_calculation_correctness(
            // ETH prices typically range from 1000 to 10000
            last_price_tenths in 10_000i64..100_000i64,
            // distance_bps from 1 to 1000 (0.01% to 10%)
            distance_bps in 1u32..1000u32,
        ) {
            // Feature: standx-maker-bot, Property 6: Price Calculation Correctness
            let last_price = Decimal::new(last_price_tenths, 1); // Convert tenths to dollars
            let calc = PriceCalculator::new("ETH-USD");
            let tick_size = dec!(0.1);

            let buy_price = calc.calculate_buy_price(last_price, distance_bps);
            let sell_price = calc.calculate_sell_price(last_price, distance_bps);

            // Property: Buy price < last_price (when distance_bps > 0)
            prop_assert!(
                buy_price < last_price,
                "Buy price {} should be less than last price {} with distance_bps {}",
                buy_price, last_price, distance_bps
            );

            // Property: Sell price > last_price (when distance_bps > 0)
            prop_assert!(
                sell_price > last_price,
                "Sell price {} should be greater than last price {} with distance_bps {}",
                sell_price, last_price, distance_bps
            );

            // Property: Both prices are exact multiples of tick_size
            prop_assert!(
                (buy_price % tick_size).is_zero(),
                "Buy price {} should be aligned to tick size {}",
                buy_price, tick_size
            );
            prop_assert!(
                (sell_price % tick_size).is_zero(),
                "Sell price {} should be aligned to tick size {}",
                sell_price, tick_size
            );

            // Property: Buy price is rounded DOWN (floor)
            let bps_multiplier = Decimal::new(distance_bps as i64, 4);
            let expected_raw_buy = last_price * (Decimal::ONE - bps_multiplier);
            let expected_buy = (expected_raw_buy / tick_size).floor() * tick_size;
            prop_assert_eq!(
                buy_price, expected_buy,
                "Buy price calculation mismatch for last_price={}, distance_bps={}",
                last_price, distance_bps
            );

            // Property: Sell price is rounded UP (ceil)
            let expected_raw_sell = last_price * (Decimal::ONE + bps_multiplier);
            let expected_sell = (expected_raw_sell / tick_size).ceil() * tick_size;
            prop_assert_eq!(
                sell_price, expected_sell,
                "Sell price calculation mismatch for last_price={}, distance_bps={}",
                last_price, distance_bps
            );
        }

        /// Feature: standx-maker-bot, Property 6: Price Calculation Correctness (Zero Distance)
        /// Tests that with 0 bps distance, buy and sell prices equal last_price (aligned to tick)
        #[test]
        fn test_zero_distance_prices_equal_last_price(
            last_price_cents in 1_000_000i64..20_000_000i64,
        ) {
            // Feature: standx-maker-bot, Property 6: Price Calculation Correctness
            let last_price = Decimal::new(last_price_cents, 2);
            let calc = PriceCalculator::new("BTC-USD");
            let tick_size = dec!(0.01);

            let buy_price = calc.calculate_buy_price(last_price, 0);
            let sell_price = calc.calculate_sell_price(last_price, 0);

            // With 0 distance, both prices should equal last_price aligned to tick
            let expected = (last_price / tick_size).floor() * tick_size;
            
            prop_assert_eq!(
                buy_price, expected,
                "Buy price with 0 bps should equal last_price aligned to tick"
            );
            // For sell with 0 distance, ceil of an already aligned price equals the price
            let expected_sell = (last_price / tick_size).ceil() * tick_size;
            prop_assert_eq!(
                sell_price, expected_sell,
                "Sell price with 0 bps should equal last_price aligned to tick"
            );
        }
    }
}
