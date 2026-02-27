use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::models::portfolio::PortfolioStore;
use crate::models::signal::SignalStore;

pub const DATA_DIR: &str = "/opt/kkuepark/gidbcn";
pub const CONFIG_PATH: &str = "/opt/kkuepark/gidbcn/config.json";
pub const PORTFOLIO_PATH: &str = "/opt/kkuepark/gidbcn/portfolio.json";
pub const SIGNALS_PATH: &str = "/opt/kkuepark/gidbcn/signals.json";

// --- Config 인메모리 싱글턴 (current_thread 런타임 → 단일 스레드) ---

thread_local! {
    static IN_MEMORY_CONFIG: RefCell<Option<Config>> = RefCell::new(None);
}

/// 시작 시 1회 호출. 파일에서 로드한 config를 메모리에 적재.
pub fn init_config(config: Config) {
    IN_MEMORY_CONFIG.with(|c| *c.borrow_mut() = Some(config));
}

/// 메모리 config 읽기 전용.
pub fn with_config<F, R>(f: F) -> R
where
    F: FnOnce(&Config) -> R,
{
    IN_MEMORY_CONFIG.with(|c| {
        let borrow = c.borrow();
        f(borrow.as_ref().expect("Config not initialized"))
    })
}

/// 메모리 config 수정 + 파일 저장.
pub fn update_config<F>(f: F) -> Result<()>
where
    F: FnOnce(&mut Config),
{
    IN_MEMORY_CONFIG.with(|c| {
        let mut borrow = c.borrow_mut();
        let config = borrow.as_mut().expect("Config not initialized");
        f(config);
        config.save(CONFIG_PATH)
    })
}

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

// --- 허용 사용자 (config 인메모리) ---

pub fn load_allowed_users() -> Vec<i64> {
    with_config(|c| c.telegram.users.clone())
}

pub fn save_allowed_users(users: &[i64]) -> Result<()> {
    update_config(|c| {
        c.telegram.users = users.to_vec();
    })
}

// --- 전체 사용자 목록 (스케줄러용) ---
// portfolio.json의 키 목록에서 user_id 반환

pub fn list_user_ids() -> Vec<i64> {
    let db: PortfolioDb = load_db(PORTFOLIO_PATH);
    db.keys().filter_map(|k| k.parse::<i64>().ok()).collect()
}
