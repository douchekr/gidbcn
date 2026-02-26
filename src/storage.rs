use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::models::portfolio::PortfolioStore;
use crate::models::signal::SignalStore;

pub const DATA_DIR: &str = "/opt/kkuepark/gidbcn";
pub const CONFIG_PATH: &str = "/opt/kkuepark/gidbcn/config.json";
pub const PORTFOLIO_PATH: &str = "/opt/kkuepark/gidbcn/portfolio.json";
pub const SIGNALS_PATH: &str = "/opt/kkuepark/gidbcn/signals.json";

type PortfolioDb = HashMap<String, PortfolioStore>;
type SignalDb = HashMap<String, SignalStore>;

fn load_db<T: serde::de::DeserializeOwned + Default + serde::Serialize>(path: &str) -> HashMap<String, T> {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse {path}: {e}, using empty db");
            HashMap::new()
        }),
        Err(_) => HashMap::new(),
    }
}

fn save_db<T: serde::Serialize>(path: &str, db: &HashMap<String, T>) -> Result<()> {
    let json = serde_json::to_string_pretty(db)?;
    std::fs::write(path, json).with_context(|| format!("Failed to write {path}"))?;
    Ok(())
}

// --- Portfolio ---

pub fn load_portfolio(user_id: i64) -> PortfolioStore {
    let db: PortfolioDb = load_db(PORTFOLIO_PATH);
    db.get(&user_id.to_string()).cloned().unwrap_or_default()
}

pub fn save_portfolio(user_id: i64, store: &PortfolioStore) -> Result<()> {
    let mut db: PortfolioDb = load_db(PORTFOLIO_PATH);
    db.insert(user_id.to_string(), store.clone());
    save_db(PORTFOLIO_PATH, &db)
}

// --- Signals ---

pub fn load_signals(user_id: i64) -> SignalStore {
    let db: SignalDb = load_db(SIGNALS_PATH);
    db.get(&user_id.to_string()).cloned().unwrap_or_default()
}

pub fn save_signals(user_id: i64, store: &SignalStore) -> Result<()> {
    let mut db: SignalDb = load_db(SIGNALS_PATH);
    db.insert(user_id.to_string(), store.clone());
    save_db(SIGNALS_PATH, &db)
}

// --- 전체 사용자 목록 (스케줄러용) ---
// portfolio.json의 키 목록에서 user_id 반환

pub fn list_user_ids() -> Vec<i64> {
    let db: PortfolioDb = load_db(PORTFOLIO_PATH);
    db.keys().filter_map(|k| k.parse::<i64>().ok()).collect()
}
