use anyhow::{Context, Result};

use crate::models::portfolio::PortfolioStore;
use crate::models::signal::SignalStore;

const PORTFOLIO_PATH: &str = "data/portfolio.json";
const SIGNALS_PATH: &str = "data/signals.json";

fn load_or_default<T: serde::de::DeserializeOwned + Default + serde::Serialize>(
    path: &str,
) -> T {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse {path}: {e}, using default");
            T::default()
        }),
        Err(_) => {
            tracing::info!("{path} not found, creating default");
            let val = T::default();
            if let Ok(json) = serde_json::to_string_pretty(&val) {
                let _ = std::fs::write(path, json);
            }
            val
        }
    }
}

fn save<T: serde::Serialize>(path: &str, data: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(data)?;
    std::fs::write(path, json).with_context(|| format!("Failed to write {path}"))?;
    Ok(())
}

// --- Portfolio ---

pub fn load_portfolio() -> PortfolioStore {
    load_or_default(PORTFOLIO_PATH)
}

pub fn save_portfolio(store: &PortfolioStore) -> Result<()> {
    save(PORTFOLIO_PATH, store)
}

// --- Signals ---

pub fn load_signals() -> SignalStore {
    load_or_default(SIGNALS_PATH)
}

pub fn save_signals(store: &SignalStore) -> Result<()> {
    save(SIGNALS_PATH, store)
}

