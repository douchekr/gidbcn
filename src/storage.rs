use anyhow::{Context, Result};

use crate::models::portfolio::PortfolioStore;
use crate::models::signal::SignalStore;

pub const DATA_DIR: &str = "/opt/kkuepark/gidbcn";
pub const CONFIG_PATH: &str = "/opt/kkuepark/gidbcn/config.json";

fn portfolio_path(user_id: i64) -> String {
    format!("{DATA_DIR}/portfolio_{user_id}.json")
}

fn signals_path(user_id: i64) -> String {
    format!("{DATA_DIR}/signals_{user_id}.json")
}

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

pub fn load_portfolio(user_id: i64) -> PortfolioStore {
    load_or_default(&portfolio_path(user_id))
}

pub fn save_portfolio(user_id: i64, store: &PortfolioStore) -> Result<()> {
    save(&portfolio_path(user_id), store)
}

// --- Signals ---

pub fn load_signals(user_id: i64) -> SignalStore {
    load_or_default(&signals_path(user_id))
}

pub fn save_signals(user_id: i64, store: &SignalStore) -> Result<()> {
    save(&signals_path(user_id), store)
}

// --- 전체 사용자 목록 (스케줄러용) ---
// portfolio_{user_id}.json 파일들을 스캔해서 user_id 목록 반환

pub fn list_user_ids() -> Vec<i64> {
    let entries = match std::fs::read_dir(DATA_DIR) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix("portfolio_") {
            if let Some(id_str) = rest.strip_suffix(".json") {
                if let Ok(id) = id_str.parse::<i64>() {
                    ids.push(id);
                }
            }
        }
    }
    ids
}
