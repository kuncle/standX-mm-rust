//! Property tests for order management
//!
//! Tests for Property 8: Client Order ID Format
//! **Validates: Requirements 6.1**

use proptest::prelude::*;
use regex::Regex;
use standx_mm::Maker;

/// Property 8: Client Order ID Format
///
/// *For any* generated client order ID for side "buy" or "sell",
/// the ID should match the regex pattern `^mm-(buy|sell)-[a-f0-9]{8}$`.
///
/// **Feature: standx-maker-bot, Property 8: Client Order ID Format**
/// **Validates: Requirements 6.1**
mod property_8_client_order_id_format {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn test_order_id_format_buy(
            // Generate random iterations to test uniqueness
            _iteration in 0u32..100
        ) {
            // Feature: standx-maker-bot, Property 8: Client Order ID Format
            // Validates: Requirements 6.1
            
            let id = Maker::generate_order_id("buy");
            
            // Check format matches pattern: mm-buy-[a-f0-9]{8}
            let pattern = Regex::new(r"^mm-buy-[a-f0-9]{8}$").unwrap();
            prop_assert!(
                pattern.is_match(&id),
                "Order ID '{}' does not match expected pattern mm-buy-[a-f0-9]{{8}}",
                id
            );
            
            // Verify length
            prop_assert_eq!(id.len(), 15, "Order ID should be 15 characters");
            
            // Verify prefix
            prop_assert!(id.starts_with("mm-buy-"), "Order ID should start with 'mm-buy-'");
        }

        #[test]
        fn test_order_id_format_sell(
            _iteration in 0u32..100
        ) {
            // Feature: standx-maker-bot, Property 8: Client Order ID Format
            // Validates: Requirements 6.1
            
            let id = Maker::generate_order_id("sell");
            
            // Check format matches pattern: mm-sell-[a-f0-9]{8}
            let pattern = Regex::new(r"^mm-sell-[a-f0-9]{8}$").unwrap();
            prop_assert!(
                pattern.is_match(&id),
                "Order ID '{}' does not match expected pattern mm-sell-[a-f0-9]{{8}}",
                id
            );
            
            // Verify length
            prop_assert_eq!(id.len(), 16, "Order ID should be 16 characters");
            
            // Verify prefix
            prop_assert!(id.starts_with("mm-sell-"), "Order ID should start with 'mm-sell-'");
        }

        #[test]
        fn test_order_id_uniqueness(
            _iteration in 0u32..100
        ) {
            // Feature: standx-maker-bot, Property 8: Client Order ID Format
            // Validates: Requirements 6.1
            
            // Generate two IDs and verify they are unique
            let id1 = Maker::generate_order_id("buy");
            let id2 = Maker::generate_order_id("buy");
            
            prop_assert_ne!(
                id1, id2,
                "Generated order IDs should be unique"
            );
        }

        #[test]
        fn test_order_id_side_parameter(
            side in prop_oneof![Just("buy"), Just("sell"), Just("BUY"), Just("SELL")]
        ) {
            // Feature: standx-maker-bot, Property 8: Client Order ID Format
            // Validates: Requirements 6.1
            
            let id = Maker::generate_order_id(side);
            
            // Should always produce lowercase side in the ID
            let expected_side = side.to_lowercase();
            let expected_prefix = format!("mm-{}-", expected_side);
            
            prop_assert!(
                id.starts_with(&expected_prefix),
                "Order ID '{}' should start with '{}'",
                id, expected_prefix
            );
            
            // The uuid8 part should be 8 hex characters
            let uuid_part = &id[expected_prefix.len()..];
            prop_assert_eq!(uuid_part.len(), 8, "UUID part should be 8 characters");
            prop_assert!(
                uuid_part.chars().all(|c| c.is_ascii_hexdigit()),
                "UUID part '{}' should only contain hex digits",
                uuid_part
            );
        }
    }
}
