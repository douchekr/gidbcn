use std::cell::RefCell;

use anyhow::Result;

use crate::config::Config;
use crate::models::portfolio::PortfolioStore;
use crate::models::signal::SignalStore;
use crate::watchlist::db as wdb;

pub const DATA_DIR: &str = "/opt/kkuepark/gidbcn";
pub const CONFIG_PATH: &str = "/opt/kkuepark/gidbcn/config.json";

// --- Config 인메모리 싱글턴 (current_thread 런타임 → 단일 스레드) ---

thread_local! {
    static IN_MEMORY_CONFIG: RefCell<Option<Config>> = RefCell::new(None);
    static PASSPHRASE: RefCell<Option<String>> = RefCell::new(None);
}

/// 시작 시 1회 호출. 파일에서 로드한 config를 메모리에 적재.
pub fn init_config(config: Config) {
    IN_MEMORY_CONFIG.with(|c| *c.borrow_mut() = Some(config));
}

/// 암호화 모드: 패스프레이즈 저장 (unlock 시 호출)
pub fn set_passphrase(passphrase: &str) {
    PASSPHRASE.with(|p| *p.borrow_mut() = Some(passphrase.to_string()));
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
/// 암호화 모드면 save_encrypted, 평문이면 save.
pub fn update_config<F>(f: F) -> Result<()>
where
    F: FnOnce(&mut Config),
{
    IN_MEMORY_CONFIG.with(|c| {
        let mut borrow = c.borrow_mut();
        let config = borrow.as_mut().expect("Config not initialized");
        f(config);

        PASSPHRASE.with(|p| {
            let p_borrow = p.borrow();
            if let Some(pass) = p_borrow.as_ref() {
                tracing::debug!(
                    "update_config: saving encrypted (gemini_api_key={})",
                    if config.secrets.gemini_api_key.is_empty() { "EMPTY" } else { "SET" }
                );
                config.save_encrypted(CONFIG_PATH, pass)
            } else {
                config.save(CONFIG_PATH)
            }
        })
    })
}

// --- Portfolio (SQLite) ---

pub fn load_portfolio(user_id: i64) -> PortfolioStore {
    wdb::load_holdings(user_id).unwrap_or_else(|e| {
        tracing::warn!("Failed to load portfolio for {user_id}: {e:#}");
        PortfolioStore::default()
    })
}

pub fn save_portfolio(user_id: i64, store: &PortfolioStore) -> Result<()> {
    wdb::save_holdings(user_id, store)
}

// --- Signals (SQLite) ---

pub fn load_signals(user_id: i64) -> SignalStore {
    wdb::load_signals_db(user_id).unwrap_or_else(|e| {
        tracing::warn!("Failed to load signals for {user_id}: {e:#}");
        SignalStore::default()
    })
}

pub fn save_signals(user_id: i64, store: &SignalStore) -> Result<()> {
    wdb::save_signals_db(user_id, store)
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

pub fn list_user_ids() -> Vec<i64> {
    wdb::list_holding_user_ids().unwrap_or_else(|e| {
        tracing::warn!("Failed to list user ids: {e:#}");
        vec![]
    })
}
