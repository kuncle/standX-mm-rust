//! StandX Maker Bot Library
//!
//! A market making bot for StandX perpetual DEX.

pub mod auth;
pub mod calculator;
pub mod config;
pub mod http_client;
pub mod maker;
pub mod state;
pub mod ws_client;

// Re-exports for convenience
pub use auth::StandXAuth;
pub use calculator::PriceCalculator;
pub use config::Config;
pub use http_client::{RateLimiter, RetryConfig, Side, StandXHttpClient};
pub use maker::Maker;
pub use state::{OpenOrder, State};
