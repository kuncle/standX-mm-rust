//! Property tests for configuration module
//! Tests for Property 12: Config YAML Round-Trip
//!
//! **Feature: standx-maker-bot, Property 12: Config YAML Round-Trip**
//! **Validates: Requirements 8.1, 8.2**

use proptest::prelude::*;
use rust_decimal::Decimal;
use standx_mm::config::{Config, ReferralConfig, WalletConfig};

/// Strategy to generate valid chain names
fn chain_strategy() -> impl Strategy<Value = String> {
    prop_oneof![Just("bsc".to_string()), Just("solana".to_string()),]
}

/// Strategy to generate non-empty strings for private keys
fn private_key_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9]{8,64}".prop_map(|s| format!("0x{}", s))
}

/// Strategy to generate valid trading symbols
fn symbol_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("BTC-USD".to_string()),
        Just("ETH-USD".to_string()),
        Just("SOL-USD".to_string()),
    ]
}

/// Strategy to generate valid referral codes (alphanumeric, can be empty)
fn referral_code_strategy() -> impl Strategy<Value = String> {
    prop_oneof![Just("".to_string()), "[a-zA-Z0-9]{1,16}".prop_map(|s| s),]
}

/// Strategy to generate valid Decimal values for order sizes (positive, reasonable range)
fn order_size_strategy() -> impl Strategy<Value = Decimal> {
    (1u64..1000u64).prop_map(|n| Decimal::new(n as i64, 3)) // 0.001 to 0.999
}

/// Strategy to generate valid Decimal values for max position (positive, reasonable range)
fn max_position_strategy() -> impl Strategy<Value = Decimal> {
    (1u64..100u64).prop_map(|n| Decimal::new(n as i64, 1)) // 0.1 to 9.9
}

/// Strategy to generate valid Decimal values for min balance (positive)
fn min_balance_strategy() -> impl Strategy<Value = Decimal> {
    (10u64..10000u64).prop_map(|n| Decimal::new(n as i64, 0)) // 10 to 9999
}

/// Strategy to generate valid distance bps values that satisfy constraints:
/// cancel_distance_bps < order_distance_bps < rebalance_distance_bps
fn distance_bps_strategy() -> impl Strategy<Value = (u32, u32, u32)> {
    (1u32..50u32).prop_flat_map(|cancel| {
        ((cancel + 1)..100u32).prop_flat_map(move |order| {
            ((order + 1)..200u32).prop_map(move |rebalance| (cancel, order, rebalance))
        })
    })
}

/// Strategy to generate valid WalletConfig
fn wallet_config_strategy() -> impl Strategy<Value = WalletConfig> {
    (chain_strategy(), private_key_strategy()).prop_map(|(chain, private_key)| WalletConfig {
        chain,
        private_key,
    })
}

/// Strategy to generate valid ReferralConfig
fn referral_config_strategy() -> impl Strategy<Value = ReferralConfig> {
    (any::<bool>(), referral_code_strategy()).prop_map(|(enabled, code)| ReferralConfig {
        enabled,
        code,
    })
}

/// Strategy to generate valid Config
fn config_strategy() -> impl Strategy<Value = Config> {
    (
        wallet_config_strategy(),
        symbol_strategy(),
        distance_bps_strategy(),
        order_size_strategy(),
        max_position_strategy(),
        1u64..60u64,       // volatility_window_sec
        1u32..100u32,      // volatility_threshold_bps
        min_balance_strategy(),
        referral_config_strategy(),
    )
        .prop_map(
            |(
                wallet,
                symbol,
                (cancel_distance_bps, order_distance_bps, rebalance_distance_bps),
                order_size_btc,
                max_position_btc,
                volatility_window_sec,
                volatility_threshold_bps,
                min_balance_usd,
                referral,
            )| {
                Config {
                    wallet,
                    symbol,
                    order_distance_bps,
                    cancel_distance_bps,
                    rebalance_distance_bps,
                    order_size_btc,
                    max_position_btc,
                    volatility_window_sec,
                    volatility_threshold_bps,
                    min_balance_usd,
                    take_profit_pct: None,
                    stop_loss_pct: None,
                    referral,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: standx-maker-bot, Property 12: Config YAML Round-Trip**
    ///
    /// *For any* valid Config struct, serializing to YAML and deserializing back
    /// should produce an equivalent Config.
    ///
    /// **Validates: Requirements 8.1, 8.2**
    #[test]
    fn test_config_yaml_round_trip(config in config_strategy()) {
        // Serialize to YAML
        let yaml_str = config.to_yaml_string()
            .expect("Config should serialize to YAML");

        // Deserialize back
        let parsed = Config::from_yaml_str(&yaml_str)
            .expect("YAML should deserialize back to Config");

        // Verify all fields match
        prop_assert_eq!(&config.wallet.chain, &parsed.wallet.chain,
            "wallet.chain mismatch");
        prop_assert_eq!(&config.wallet.private_key, &parsed.wallet.private_key,
            "wallet.private_key mismatch");
        prop_assert_eq!(&config.symbol, &parsed.symbol,
            "symbol mismatch");
        prop_assert_eq!(config.order_distance_bps, parsed.order_distance_bps,
            "order_distance_bps mismatch");
        prop_assert_eq!(config.cancel_distance_bps, parsed.cancel_distance_bps,
            "cancel_distance_bps mismatch");
        prop_assert_eq!(config.rebalance_distance_bps, parsed.rebalance_distance_bps,
            "rebalance_distance_bps mismatch");
        prop_assert_eq!(config.order_size_btc, parsed.order_size_btc,
            "order_size_btc mismatch");
        prop_assert_eq!(config.max_position_btc, parsed.max_position_btc,
            "max_position_btc mismatch");
        prop_assert_eq!(config.volatility_window_sec, parsed.volatility_window_sec,
            "volatility_window_sec mismatch");
        prop_assert_eq!(config.volatility_threshold_bps, parsed.volatility_threshold_bps,
            "volatility_threshold_bps mismatch");
        prop_assert_eq!(config.min_balance_usd, parsed.min_balance_usd,
            "min_balance_usd mismatch");
        prop_assert_eq!(config.referral.enabled, parsed.referral.enabled,
            "referral.enabled mismatch");
        prop_assert_eq!(&config.referral.code, &parsed.referral.code,
            "referral.code mismatch");

        // Also verify using PartialEq
        prop_assert_eq!(config, parsed, "Config round-trip failed");
    }
}
