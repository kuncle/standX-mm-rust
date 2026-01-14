//! StandX Maker Bot - Rust Implementation
//!
//! A market making bot for StandX perpetual DEX.
//! Earns Maker Points by placing limit orders on both sides of the order book.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::signal;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

use standx_mm::{
    Config, Maker, StandXAuth, StandXHttpClient, State,
    ws_client::{MarketWsClient, UserWsClient, MARKET_WS_URL, USER_WS_URL},
};

/// Default config file path
const DEFAULT_CONFIG_PATH: &str = "config.yaml";

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    init_logging();

    info!("StandX Maker Bot starting...");

    // Load configuration
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());

    info!("Loading configuration from: {}", config_path);
    let config = Config::load_and_validate(&config_path)
        .with_context(|| format!("Failed to load config from {}", config_path))?;

    info!("Configuration loaded successfully");
    info!("  Symbol: {}", config.symbol);
    info!("  Order distance: {} bps", config.order_distance_bps);
    info!("  Order size: {} BTC", config.order_size_btc);
    info!("  Max position: {} BTC", config.max_position_btc);

    // Create shutdown channel
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // Initialize authentication
    info!("Initializing authentication...");
    let mut auth = StandXAuth::new();
    auth.authenticate(&config.wallet.chain, &config.wallet.private_key)
        .await
        .context("Failed to authenticate with StandX")?;
    info!("Authentication successful");

    let auth = Arc::new(RwLock::new(auth));

    // Initialize HTTP client
    let http_client = Arc::new(StandXHttpClient::new(auth.clone()));

    // Initialize state
    let state = State::new();

    // Initialize maker
    let maker = Arc::new(Maker::new(config.clone(), http_client.clone(), state.clone()));

    // Initialize maker state from exchange
    info!("Syncing state from exchange...");
    maker.initialize().await.context("Failed to initialize maker state")?;
    info!("State sync complete");

    // Create WebSocket clients
    let market_ws = Arc::new(MarketWsClient::new(MARKET_WS_URL));
    let user_ws = Arc::new(UserWsClient::new(USER_WS_URL, auth.clone()));

    // Register price callback
    let maker_for_price = maker.clone();
    let symbol = config.symbol.clone();
    market_ws.on_price(move |price_data| {
        if price_data.symbol == symbol {
            maker_for_price.on_price_update(price_data.last_price);
        }
    }).await;

    // Register order callback
    let state_for_order = state.clone();
    user_ws.on_order(move |order_update| {
        // Clear order from state if filled, cancelled, or rejected
        let status = order_update.status.to_lowercase();
        if status == "filled" || status == "cancelled" || status == "rejected" {
            let side = if order_update.side.to_lowercase() == "buy" {
                standx_mm::http_client::Side::Buy
            } else {
                standx_mm::http_client::Side::Sell
            };
            state_for_order.set_order(side, None);
            info!("Order {} status: {}", order_update.cl_ord_id, order_update.status);
        }
    }).await;

    // Register position callback
    let state_for_position = state.clone();
    user_ws.on_position(move |position_update| {
        state_for_position.update_position(position_update.qty, position_update.upnl);
    }).await;

    // Register reconnect callbacks
    market_ws.on_reconnect(move || {
        info!("Market WebSocket reconnected, will resync state");
    }).await;

    user_ws.on_reconnect(move || {
        info!("User WebSocket reconnected");
    }).await;

    // Subscribe to price channel
    market_ws.subscribe_price(&config.symbol).await?;

    // Spawn tasks
    let market_ws_clone = market_ws.clone();
    let market_ws_task = tokio::spawn(async move {
        if let Err(e) = market_ws_clone.run().await {
            error!("Market WebSocket error: {}", e);
        }
    });

    let user_ws_clone = user_ws.clone();
    let user_ws_task = tokio::spawn(async move {
        if let Err(e) = user_ws_clone.run().await {
            error!("User WebSocket error: {}", e);
        }
    });

    let maker_clone = maker.clone();
    let maker_task = tokio::spawn(async move {
        if let Err(e) = maker_clone.run().await {
            error!("Maker error: {}", e);
        }
    });

    info!("StandX Maker Bot is running");
    info!("Press Ctrl+C to stop");

    // Wait for shutdown signal
    let shutdown_result = wait_for_shutdown().await;

    info!("Shutdown signal received, stopping...");

    // Stop all components
    market_ws.stop();
    user_ws.stop();
    maker.stop().await;

    // Cancel all open orders on shutdown
    info!("Cancelling all open orders...");
    if let Err(e) = cancel_all_orders(&http_client, &state).await {
        error!("Failed to cancel orders on shutdown: {}", e);
    }

    // Send shutdown signal
    let _ = shutdown_tx.send(());

    // Wait for tasks to complete (with timeout)
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        async {
            let _ = market_ws_task.await;
            let _ = user_ws_task.await;
            let _ = maker_task.await;
        }
    ).await;

    info!("StandX Maker Bot stopped");

    shutdown_result
}

/// Initialize logging with tracing
fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,standx_mm=debug"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_span_events(FmtSpan::NONE)
        .init();
}

/// Wait for shutdown signal (SIGINT or SIGTERM)
async fn wait_for_shutdown() -> Result<()> {
    #[cfg(unix)]
    {
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt())?;
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;

        tokio::select! {
            _ = sigint.recv() => {
                info!("Received SIGINT");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM");
            }
        }
    }

    #[cfg(not(unix))]
    {
        signal::ctrl_c().await?;
        info!("Received Ctrl+C");
    }

    Ok(())
}

/// Cancel all open orders on shutdown
async fn cancel_all_orders(client: &StandXHttpClient, state: &State) -> Result<()> {
    let mut cl_ord_ids = Vec::new();

    if let Some(order) = state.get_order(standx_mm::http_client::Side::Buy) {
        cl_ord_ids.push(order.cl_ord_id);
    }
    if let Some(order) = state.get_order(standx_mm::http_client::Side::Sell) {
        cl_ord_ids.push(order.cl_ord_id);
    }

    if !cl_ord_ids.is_empty() {
        info!("Cancelling {} orders: {:?}", cl_ord_ids.len(), cl_ord_ids);
        client.cancel_orders(cl_ord_ids).await?;
        state.clear_all_orders();
        info!("All orders cancelled");
    } else {
        info!("No open orders to cancel");
    }

    Ok(())
}
