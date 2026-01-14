//! Configuration module
//!
//! Handles loading and validating configuration from YAML files.
//! Supports environment variable overrides for sensitive data like private keys.

use anyhow::{Context, Result};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub wallet: WalletConfig,
    pub symbol: String,
    #[serde(default = "default_order_distance_bps")]
    pub order_distance_bps: u32,
    #[serde(default = "default_cancel_distance_bps")]
    pub cancel_distance_bps: u32,
    #[serde(default = "default_rebalance_distance_bps")]
    pub rebalance_distance_bps: u32,
    #[serde(with = "rust_decimal::serde::float")]
    pub order_size_btc: Decimal,
    #[serde(with = "rust_decimal::serde::float")]
    pub max_position_btc: Decimal,
    #[serde(default = "default_volatility_window_sec")]
    pub volatility_window_sec: u64,
    #[serde(default = "default_volatility_threshold_bps")]
    pub volatility_threshold_bps: u32,
    #[serde(default = "default_min_balance_usd", with = "rust_decimal::serde::float")]
    pub min_balance_usd: Decimal,
    /// Take profit percentage (e.g., 5.0 = 5%)
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub take_profit_pct: Option<Decimal>,
    /// Stop loss percentage (e.g., 5.0 = 5%)
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub stop_loss_pct: Option<Decimal>,
    #[serde(default)]
    pub referral: ReferralConfig,
}

/// Wallet configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalletConfig {
    pub chain: String,
    #[serde(default)]
    pub private_key: String,
}

/// Referral configuration for transparency and user control
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReferralConfig {
    #[serde(default = "default_referral_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub code: String,
}

impl Default for ReferralConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            code: String::new(),
        }
    }
}

// Default value functions
fn default_order_distance_bps() -> u32 {
    10
}
fn default_cancel_distance_bps() -> u32 {
    5
}
fn default_rebalance_distance_bps() -> u32 {
    20
}
fn default_volatility_window_sec() -> u64 {
    5
}
fn default_volatility_threshold_bps() -> u32 {
    5
}
fn default_min_balance_usd() -> Decimal {
    Decimal::new(100, 0)
}
fn default_referral_enabled() -> bool {
    true
}

impl Config {
    /// Load configuration from a YAML file
    ///
    /// # Arguments
    /// * `path` - Path to the YAML configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - The loaded configuration or an error
    ///
    /// # Errors
    /// * If the file doesn't exist
    /// * If the file cannot be read
    /// * If the YAML is invalid or doesn't match the expected structure
    pub fn load(path: &str) -> Result<Self> {
        let path = Path::new(path);

        // Check if file exists
        if !path.exists() {
            anyhow::bail!("Config file not found: {}", path.display());
        }

        // Read file contents
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        // Parse YAML
        let config: Config = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Load configuration from a YAML string
    ///
    /// # Arguments
    /// * `yaml_str` - YAML string to parse
    ///
    /// # Returns
    /// * `Result<Self>` - The loaded configuration or an error
    pub fn from_yaml_str(yaml_str: &str) -> Result<Self> {
        let config: Config =
            serde_yaml::from_str(yaml_str).context("Failed to parse YAML configuration")?;
        Ok(config)
    }

    /// Serialize configuration to YAML string
    ///
    /// # Returns
    /// * `Result<String>` - The YAML string or an error
    pub fn to_yaml_string(&self) -> Result<String> {
        serde_yaml::to_string(self).context("Failed to serialize configuration to YAML")
    }

    /// Apply environment variable overrides
    ///
    /// Environment variables take priority over config file values:
    /// - `STANDX_PRIVATE_KEY`: Wallet private key (recommended for security)
    /// - `STANDX_CHAIN`: Chain type (optional)
    pub fn apply_env_overrides(&mut self) {
        // Override private key from environment variable
        if let Ok(private_key) = env::var("STANDX_PRIVATE_KEY") {
            if !private_key.is_empty() {
                self.wallet.private_key = private_key;
            }
        }

        // Override chain from environment variable
        if let Ok(chain) = env::var("STANDX_CHAIN") {
            if !chain.is_empty() {
                self.wallet.chain = chain;
            }
        }
    }

    /// Validate that the configuration has all required fields
    ///
    /// # Returns
    /// * `Result<()>` - Ok if valid, error with description if invalid
    pub fn validate(&self) -> Result<()> {
        // Validate private key exists
        if self.wallet.private_key.is_empty() {
            anyhow::bail!(
                "Private key not found. Please set STANDX_PRIVATE_KEY environment variable \
                 or provide 'private_key' in the config file wallet section."
            );
        }

        // Validate chain is supported
        let supported_chains = ["bsc", "solana"];
        if !supported_chains.contains(&self.wallet.chain.to_lowercase().as_str()) {
            anyhow::bail!(
                "Unsupported chain: {}. Supported chains: {:?}",
                self.wallet.chain,
                supported_chains
            );
        }

        // Validate symbol is not empty
        if self.symbol.is_empty() {
            anyhow::bail!("Symbol cannot be empty");
        }

        // Validate order parameters
        if self.order_size_btc <= Decimal::ZERO {
            anyhow::bail!("order_size_btc must be positive");
        }

        if self.max_position_btc <= Decimal::ZERO {
            anyhow::bail!("max_position_btc must be positive");
        }

        // Validate distance parameters make sense
        if self.cancel_distance_bps >= self.order_distance_bps {
            anyhow::bail!(
                "cancel_distance_bps ({}) must be less than order_distance_bps ({})",
                self.cancel_distance_bps,
                self.order_distance_bps
            );
        }

        if self.order_distance_bps >= self.rebalance_distance_bps {
            anyhow::bail!(
                "order_distance_bps ({}) must be less than rebalance_distance_bps ({})",
                self.order_distance_bps,
                self.rebalance_distance_bps
            );
        }

        Ok(())
    }

    /// Load configuration from file and apply environment overrides
    ///
    /// This is the recommended way to load configuration as it:
    /// 1. Loads from YAML file
    /// 2. Applies environment variable overrides
    /// 3. Validates the final configuration
    ///
    /// # Arguments
    /// * `path` - Path to the YAML configuration file
    ///
    /// # Returns
    /// * `Result<Self>` - The loaded and validated configuration
    pub fn load_and_validate(path: &str) -> Result<Self> {
        let mut config = Self::load(path)?;
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_parse_valid_yaml() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "0x1234567890abcdef"
symbol: BTC-USD
order_distance_bps: 10
cancel_distance_bps: 5
rebalance_distance_bps: 20
order_size_btc: 0.01
max_position_btc: 0.1
volatility_window_sec: 5
volatility_threshold_bps: 5
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        assert_eq!(config.wallet.chain, "bsc");
        assert_eq!(config.wallet.private_key, "0x1234567890abcdef");
        assert_eq!(config.symbol, "BTC-USD");
        assert_eq!(config.order_distance_bps, 10);
        assert_eq!(config.cancel_distance_bps, 5);
        assert_eq!(config.rebalance_distance_bps, 20);
        assert_eq!(config.order_size_btc, dec!(0.01));
        assert_eq!(config.max_position_btc, dec!(0.1));
        assert_eq!(config.volatility_window_sec, 5);
        assert_eq!(config.volatility_threshold_bps, 5);
    }

    #[test]
    fn test_default_values() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "0x1234"
symbol: ETH-USD
order_size_btc: 0.05
max_position_btc: 0.5
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        // Check defaults are applied
        assert_eq!(config.order_distance_bps, 10);
        assert_eq!(config.cancel_distance_bps, 5);
        assert_eq!(config.rebalance_distance_bps, 20);
        assert_eq!(config.volatility_window_sec, 5);
        assert_eq!(config.volatility_threshold_bps, 5);
        assert_eq!(config.min_balance_usd, dec!(100));
    }

    #[test]
    fn test_referral_config() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "0x1234"
symbol: BTC-USD
order_size_btc: 0.01
max_position_btc: 0.1
referral:
  enabled: true
  code: "mycode"
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        assert!(config.referral.enabled);
        assert_eq!(config.referral.code, "mycode");
    }

    #[test]
    fn test_referral_default() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "0x1234"
symbol: BTC-USD
order_size_btc: 0.01
max_position_btc: 0.1
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        assert!(config.referral.enabled);
        assert_eq!(config.referral.code, "");
    }

    #[test]
    fn test_validate_missing_private_key() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: ""
symbol: BTC-USD
order_size_btc: 0.01
max_position_btc: 0.1
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Private key"));
    }

    #[test]
    fn test_validate_unsupported_chain() {
        let yaml = r#"
wallet:
  chain: ethereum
  private_key: "0x1234"
symbol: BTC-USD
order_size_btc: 0.01
max_position_btc: 0.1
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported chain"));
    }

    #[test]
    fn test_validate_invalid_distances() {
        // cancel_distance_bps >= order_distance_bps
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "0x1234"
symbol: BTC-USD
order_distance_bps: 10
cancel_distance_bps: 10
order_size_btc: 0.01
max_position_btc: 0.1
"#;

        let config = Config::from_yaml_str(yaml).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cancel_distance_bps"));
    }

    #[test]
    fn test_yaml_round_trip() {
        let original = Config {
            wallet: WalletConfig {
                chain: "bsc".to_string(),
                private_key: "0x1234567890".to_string(),
            },
            symbol: "BTC-USD".to_string(),
            order_distance_bps: 10,
            cancel_distance_bps: 5,
            rebalance_distance_bps: 20,
            order_size_btc: dec!(0.01),
            max_position_btc: dec!(0.1),
            volatility_window_sec: 5,
            volatility_threshold_bps: 5,
            min_balance_usd: dec!(100),
            take_profit_pct: Some(dec!(5)),
            stop_loss_pct: Some(dec!(5)),
            referral: ReferralConfig {
                enabled: true,
                code: "test".to_string(),
            },
        };

        let yaml_str = original.to_yaml_string().unwrap();
        let parsed = Config::from_yaml_str(&yaml_str).unwrap();

        assert_eq!(original.wallet.chain, parsed.wallet.chain);
        assert_eq!(original.wallet.private_key, parsed.wallet.private_key);
        assert_eq!(original.symbol, parsed.symbol);
        assert_eq!(original.order_distance_bps, parsed.order_distance_bps);
        assert_eq!(original.cancel_distance_bps, parsed.cancel_distance_bps);
        assert_eq!(original.rebalance_distance_bps, parsed.rebalance_distance_bps);
        assert_eq!(original.order_size_btc, parsed.order_size_btc);
        assert_eq!(original.max_position_btc, parsed.max_position_btc);
        assert_eq!(original.volatility_window_sec, parsed.volatility_window_sec);
        assert_eq!(
            original.volatility_threshold_bps,
            parsed.volatility_threshold_bps
        );
        assert_eq!(original.min_balance_usd, parsed.min_balance_usd);
        assert_eq!(original.take_profit_pct, parsed.take_profit_pct);
        assert_eq!(original.stop_loss_pct, parsed.stop_loss_pct);
        assert_eq!(original.referral.enabled, parsed.referral.enabled);
        assert_eq!(original.referral.code, parsed.referral.code);
    }

    #[test]
    fn test_env_override_private_key() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "original_key"
symbol: BTC-USD
order_size_btc: 0.01
max_position_btc: 0.1
"#;

        let mut config = Config::from_yaml_str(yaml).unwrap();

        // Set environment variable
        env::set_var("STANDX_PRIVATE_KEY", "env_key");

        config.apply_env_overrides();

        assert_eq!(config.wallet.private_key, "env_key");

        // Clean up
        env::remove_var("STANDX_PRIVATE_KEY");
    }

    #[test]
    fn test_env_override_chain() {
        let yaml = r#"
wallet:
  chain: bsc
  private_key: "0x1234"
symbol: BTC-USD
order_size_btc: 0.01
max_position_btc: 0.1
"#;

        let mut config = Config::from_yaml_str(yaml).unwrap();

        // Set environment variable
        env::set_var("STANDX_CHAIN", "solana");

        config.apply_env_overrides();

        assert_eq!(config.wallet.chain, "solana");

        // Clean up
        env::remove_var("STANDX_CHAIN");
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = Config::load("nonexistent_config.yaml");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
