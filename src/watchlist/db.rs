use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, FixedOffset};
use rusqlite::{params, Connection};

use super::models::{BlacklistEntry, Candidate, CandidateStatus, PromptRecord, PromptType};
use crate::models::portfolio::{Holding, Market, PortfolioStore};
use crate::models::signal::{Condition, Signal, SignalStore};

const DB_PATH: &str = "/opt/kkuepark/gidbcn/portfolio.db";

thread_local! {
    static DB_CONN: RefCell<Option<Connection>> = RefCell::new(None);
}

/// DB 초기화: 연결 + 테이블 생성
pub fn init_db() -> Result<()> {
    let conn = Connection::open(DB_PATH)
        .with_context(|| format!("SQLite 열기 실패: {DB_PATH}"))?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS candidates (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            ticker      TEXT NOT NULL,
            name        TEXT NOT NULL DEFAULT '',
            sector      TEXT NOT NULL DEFAULT '',
            reason      TEXT NOT NULL DEFAULT '',
            score       REAL,
            verdict     TEXT,
            status      TEXT NOT NULL DEFAULT 'pending',
            prompt_id   INTEGER,
            created_at  TEXT NOT NULL,
            judged_at   TEXT
        );

        CREATE TABLE IF NOT EXISTS blacklist (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            ticker   TEXT NOT NULL UNIQUE,
            reason   TEXT NOT NULL DEFAULT '',
            added_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prompts (
            type    TEXT PRIMARY KEY,
            content TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prompt_history (
            id               INTEGER PRIMARY KEY AUTOINCREMENT,
            prompt_type      TEXT NOT NULL,
            prompt_text      TEXT NOT NULL,
            response_text    TEXT NOT NULL DEFAULT '',
            model            TEXT NOT NULL DEFAULT '',
            tickers_extracted TEXT NOT NULL DEFAULT '',
            created_at       TEXT NOT NULL,
            status           TEXT NOT NULL DEFAULT 'success'
        );

        CREATE TABLE IF NOT EXISTS api_usage (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            api_name  TEXT NOT NULL,
            called_at TEXT NOT NULL,
            endpoint  TEXT NOT NULL DEFAULT '',
            success   INTEGER NOT NULL DEFAULT 1
        );

        CREATE INDEX IF NOT EXISTS idx_candidates_status ON candidates(status);
        CREATE INDEX IF NOT EXISTS idx_candidates_ticker ON candidates(ticker);
        CREATE INDEX IF NOT EXISTS idx_blacklist_ticker ON blacklist(ticker);
        CREATE INDEX IF NOT EXISTS idx_api_usage_date ON api_usage(called_at);

        CREATE TABLE IF NOT EXISTS holdings (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id      INTEGER NOT NULL,
            market       TEXT NOT NULL,
            symbol       TEXT NOT NULL,
            name         TEXT NOT NULL DEFAULT '',
            account      TEXT NOT NULL DEFAULT '',
            quantity     REAL NOT NULL,
            avg_price    REAL NOT NULL,
            added_at     TEXT NOT NULL,
            cached_price REAL,
            cached_at    TEXT,
            UNIQUE(user_id, symbol, account)
        );
        CREATE INDEX IF NOT EXISTS idx_holdings_user ON holdings(user_id);

        CREATE TABLE IF NOT EXISTS signals (
            id          TEXT PRIMARY KEY,
            user_id     INTEGER NOT NULL,
            symbol      TEXT NOT NULL,
            account     TEXT NOT NULL DEFAULT '',
            cond_type   TEXT NOT NULL,
            cond_value  REAL NOT NULL,
            active      INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_signals_user ON signals(user_id);
        CREATE INDEX IF NOT EXISTS idx_signals_active ON signals(active);",
    )?;

    DB_CONN.with(|c| *c.borrow_mut() = Some(conn));
    tracing::info!("watchlist DB initialized: {DB_PATH}");

    // JSON → SQLite 자동 마이그레이션
    migrate_json_portfolio()?;
    migrate_json_signals()?;

    Ok(())
}

fn with_db<F, R>(f: F) -> Result<R>
where
    F: FnOnce(&Connection) -> Result<R>,
{
    DB_CONN.with(|c| {
        let borrow = c.borrow();
        let conn = borrow.as_ref().context("watchlist DB not initialized")?;
        f(conn)
    })
}

// --- Candidates ---

pub fn insert_candidate(
    ticker: &str,
    name: &str,
    sector: &str,
    reason: &str,
    prompt_id: Option<i64>,
) -> Result<i64> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT INTO candidates (ticker, name, sector, reason, prompt_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![ticker, name, sector, reason, prompt_id, now],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn list_candidates(status: Option<CandidateStatus>) -> Result<Vec<Candidate>> {
    with_db(|conn| {
        let (sql, param): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match status {
            Some(s) => (
                "SELECT id, ticker, name, sector, reason, score, verdict, status, prompt_id, created_at, judged_at
                 FROM candidates WHERE status = ?1 ORDER BY score DESC, id DESC",
                vec![Box::new(s.as_str().to_string())],
            ),
            None => (
                "SELECT id, ticker, name, sector, reason, score, verdict, status, prompt_id, created_at, judged_at
                 FROM candidates ORDER BY score DESC, id DESC",
                vec![],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = param.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(Candidate {
                id: row.get(0)?,
                ticker: row.get(1)?,
                name: row.get(2)?,
                sector: row.get(3)?,
                reason: row.get(4)?,
                score: row.get(5)?,
                verdict: row.get(6)?,
                status: CandidateStatus::from_str(&row.get::<_, String>(7)?),
                prompt_id: row.get(8)?,
                created_at: row.get(9)?,
                judged_at: row.get(10)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
}

pub fn update_candidate_judge(id: i64, score: f64, verdict: &str) -> Result<()> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "UPDATE candidates SET score = ?1, verdict = ?2, status = 'judged', judged_at = ?3
             WHERE id = ?4",
            params![score, verdict, now, id],
        )?;
        Ok(())
    })
}

#[allow(dead_code)]
pub fn update_candidate_status(id: i64, status: CandidateStatus) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE candidates SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        Ok(())
    })
}

pub fn get_candidate_by_ticker(ticker: &str) -> Result<Option<Candidate>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ticker, name, sector, reason, score, verdict, status, prompt_id, created_at, judged_at
             FROM candidates WHERE ticker = ?1 ORDER BY id DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![ticker], |row| {
            Ok(Candidate {
                id: row.get(0)?,
                ticker: row.get(1)?,
                name: row.get(2)?,
                sector: row.get(3)?,
                reason: row.get(4)?,
                score: row.get(5)?,
                verdict: row.get(6)?,
                status: CandidateStatus::from_str(&row.get::<_, String>(7)?),
                prompt_id: row.get(8)?,
                created_at: row.get(9)?,
                judged_at: row.get(10)?,
            })
        })?;
        Ok(rows.next().and_then(|r| r.ok()))
    })
}

// --- Blacklist ---

pub fn add_blacklist(ticker: &str, reason: &str) -> Result<()> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT OR REPLACE INTO blacklist (ticker, reason, added_at) VALUES (?1, ?2, ?3)",
            params![ticker, reason, now],
        )?;
        Ok(())
    })
}

pub fn remove_blacklist(ticker: &str) -> Result<bool> {
    with_db(|conn| {
        let affected = conn.execute("DELETE FROM blacklist WHERE ticker = ?1", params![ticker])?;
        Ok(affected > 0)
    })
}

pub fn is_blacklisted(ticker: &str) -> Result<bool> {
    with_db(|conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM blacklist WHERE ticker = ?1",
            params![ticker],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    })
}

pub fn list_blacklist() -> Result<Vec<BlacklistEntry>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ticker, reason, added_at FROM blacklist ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BlacklistEntry {
                id: row.get(0)?,
                ticker: row.get(1)?,
                reason: row.get(2)?,
                added_at: row.get(3)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
}

// --- Prompts (사냥용 / 처단용) ---

pub fn get_prompt(prompt_type: PromptType) -> Result<Option<String>> {
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT content FROM prompts WHERE type = ?1")?;
        let mut rows = stmt.query_map(params![prompt_type.as_str()], |row| row.get(0))?;
        Ok(rows.next().and_then(|r| r.ok()))
    })
}

pub fn set_prompt(prompt_type: PromptType, content: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO prompts (type, content) VALUES (?1, ?2)",
            params![prompt_type.as_str(), content],
        )?;
        Ok(())
    })
}

// --- Prompt History ---

pub fn insert_prompt_history(
    prompt_type: PromptType,
    prompt_text: &str,
    response_text: &str,
    model: &str,
    tickers: &str,
    status: &str,
) -> Result<i64> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT INTO prompt_history (prompt_type, prompt_text, response_text, model, tickers_extracted, created_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![prompt_type.as_str(), prompt_text, response_text, model, tickers, now, status],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

pub fn list_prompt_history(limit: usize) -> Result<Vec<PromptRecord>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, prompt_type, prompt_text, response_text, model, tickers_extracted, created_at, status
             FROM prompt_history ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(PromptRecord {
                id: row.get(0)?,
                prompt_type: row.get(1)?,
                prompt_text: row.get(2)?,
                response_text: row.get(3)?,
                model: row.get(4)?,
                tickers_extracted: row.get(5)?,
                created_at: row.get(6)?,
                status: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
}

// --- API Usage ---

pub fn log_api_call(api_name: &str, endpoint: &str, success: bool) -> Result<()> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT INTO api_usage (api_name, called_at, endpoint, success) VALUES (?1, ?2, ?3, ?4)",
            params![api_name, now, endpoint, success as i32],
        )?;
        Ok(())
    })
}

pub fn gemini_calls_today() -> Result<usize> {
    with_db(|conn| {
        let today = today_prefix();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM api_usage WHERE api_name = 'gemini' AND called_at LIKE ?1",
            params![format!("{today}%")],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    })
}

// --- Holdings (portfolio) ---

pub fn load_holdings(user_id: i64) -> Result<PortfolioStore> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT market, symbol, name, account, quantity, avg_price, added_at, cached_price, cached_at
             FROM holdings WHERE user_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            let market_str: String = row.get(0)?;
            let added_at_str: String = row.get(6)?;
            let cached_at_str: Option<String> = row.get(8)?;
            Ok((market_str, row.get(1)?, row.get(2)?, row.get(3)?,
                row.get(4)?, row.get(5)?, added_at_str,
                row.get(7)?, cached_at_str))
        })?;

        let mut holdings = Vec::new();
        for r in rows {
            let (market_str, symbol, name, account, quantity, avg_price,
                 added_at_str, cached_price, cached_at_str): (String, String, String, String, f64, f64, String, Option<f64>, Option<String>) = r?;

            let market = Market::from_str(&market_str)
                .unwrap_or(Market::KRX);
            let added_at = DateTime::parse_from_rfc3339(&added_at_str)
                .unwrap_or_else(|_| chrono::Utc::now().with_timezone(&kst_offset()));
            let cached_at = cached_at_str.and_then(|s| DateTime::parse_from_rfc3339(&s).ok());

            holdings.push(Holding {
                market, symbol, name, account, quantity, avg_price,
                added_at, cached_price, cached_at,
            });
        }
        Ok(PortfolioStore { holdings })
    })
}

pub fn save_holdings(user_id: i64, store: &PortfolioStore) -> Result<()> {
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM holdings WHERE user_id = ?1", params![user_id])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO holdings (user_id, market, symbol, name, account, quantity, avg_price, added_at, cached_price, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for h in &store.holdings {
                let cached_at_str = h.cached_at.map(|dt| dt.to_rfc3339());
                stmt.execute(params![
                    user_id,
                    h.market.to_string(),
                    h.symbol,
                    h.name,
                    h.account,
                    h.quantity,
                    h.avg_price,
                    h.added_at.to_rfc3339(),
                    h.cached_price,
                    cached_at_str,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
}

pub fn list_holding_user_ids() -> Result<Vec<i64>> {
    with_db(|conn| {
        let mut stmt = conn.prepare("SELECT DISTINCT user_id FROM holdings")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
}

// --- Signals ---

fn condition_to_parts(cond: &Condition) -> (&'static str, f64) {
    match cond {
        Condition::PriceAbove { target } => ("price_above", *target),
        Condition::PriceBelow { target } => ("price_below", *target),
        Condition::ProfitAbove { percentage } => ("profit_above", *percentage),
        Condition::ProfitBelow { percentage } => ("profit_below", *percentage),
    }
}

fn parts_to_condition(cond_type: &str, cond_value: f64) -> Result<Condition> {
    match cond_type {
        "price_above" => Ok(Condition::PriceAbove { target: cond_value }),
        "price_below" => Ok(Condition::PriceBelow { target: cond_value }),
        "profit_above" => Ok(Condition::ProfitAbove { percentage: cond_value }),
        "profit_below" => Ok(Condition::ProfitBelow { percentage: cond_value }),
        other => bail!("unknown condition type: {other}"),
    }
}

pub fn load_signals_db(user_id: i64) -> Result<SignalStore> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, symbol, account, cond_type, cond_value, active, created_at
             FROM signals WHERE user_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut signals = Vec::new();
        for r in rows {
            let (id, symbol, account, cond_type, cond_value, active, created_at_str) = r?;
            let condition = parts_to_condition(&cond_type, cond_value)
                .unwrap_or(Condition::PriceAbove { target: cond_value });
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .unwrap_or_else(|_| chrono::Utc::now().with_timezone(&kst_offset()));
            signals.push(Signal { id, symbol, account, condition, active, created_at });
        }
        Ok(SignalStore { signals })
    })
}

pub fn save_signals_db(user_id: i64, store: &SignalStore) -> Result<()> {
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM signals WHERE user_id = ?1", params![user_id])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO signals (id, user_id, symbol, account, cond_type, cond_value, active, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for s in &store.signals {
                let (cond_type, cond_value) = condition_to_parts(&s.condition);
                stmt.execute(params![
                    s.id,
                    user_id,
                    s.symbol,
                    s.account,
                    cond_type,
                    cond_value,
                    s.active,
                    s.created_at.to_rfc3339(),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
}

// --- JSON → SQLite 마이그레이션 ---

fn kst_offset() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).unwrap()
}

const PORTFOLIO_JSON: &str = "/opt/kkuepark/gidbcn/portfolio.json";
const SIGNALS_JSON: &str = "/opt/kkuepark/gidbcn/signals.json";

fn migrate_json_portfolio() -> Result<()> {
    // holdings 테이블이 비어있고 JSON 파일이 있으면 임포트
    let count: i64 = with_db(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM holdings", [], |row| row.get(0))?)
    })?;
    if count > 0 {
        return Ok(());
    }

    let json_str = match std::fs::read_to_string(PORTFOLIO_JSON) {
        Ok(s) => s,
        Err(_) => return Ok(()), // 파일 없으면 스킵
    };

    let db: HashMap<String, PortfolioStore> = match serde_json::from_str(&json_str) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("portfolio.json 파싱 실패 (마이그레이션 스킵): {e}");
            return Ok(());
        }
    };

    let mut total = 0usize;
    for (user_id_str, store) in &db {
        let user_id: i64 = match user_id_str.parse() {
            Ok(id) => id,
            Err(_) => continue,
        };
        save_holdings(user_id, store)?;
        total += store.holdings.len();
    }

    if total > 0 {
        tracing::info!("portfolio.json → SQLite 마이그레이션 완료: {total}건 ({} 사용자)", db.len());
    }
    Ok(())
}

fn migrate_json_signals() -> Result<()> {
    let count: i64 = with_db(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM signals", [], |row| row.get(0))?)
    })?;
    if count > 0 {
        return Ok(());
    }

    let json_str = match std::fs::read_to_string(SIGNALS_JSON) {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    let db: HashMap<String, SignalStore> = match serde_json::from_str(&json_str) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("signals.json 파싱 실패 (마이그레이션 스킵): {e}");
            return Ok(());
        }
    };

    let mut total = 0usize;
    for (user_id_str, store) in &db {
        let user_id: i64 = match user_id_str.parse() {
            Ok(id) => id,
            Err(_) => continue,
        };
        save_signals_db(user_id, store)?;
        total += store.signals.len();
    }

    if total > 0 {
        tracing::info!("signals.json → SQLite 마이그레이션 완료: {total}건 ({} 사용자)", db.len());
    }
    Ok(())
}

// --- Helpers ---

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn today_prefix() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS candidates (
                id INTEGER PRIMARY KEY AUTOINCREMENT, ticker TEXT NOT NULL,
                name TEXT NOT NULL DEFAULT '', sector TEXT NOT NULL DEFAULT '',
                reason TEXT NOT NULL DEFAULT '', score REAL, verdict TEXT,
                status TEXT NOT NULL DEFAULT 'pending', prompt_id INTEGER,
                created_at TEXT NOT NULL, judged_at TEXT
            );
            CREATE TABLE IF NOT EXISTS blacklist (
                id INTEGER PRIMARY KEY AUTOINCREMENT, ticker TEXT NOT NULL UNIQUE,
                reason TEXT NOT NULL DEFAULT '', added_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prompts (
                type TEXT PRIMARY KEY, content TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS prompt_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT, prompt_type TEXT NOT NULL,
                prompt_text TEXT NOT NULL, response_text TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '', tickers_extracted TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'success'
            );
            CREATE TABLE IF NOT EXISTS api_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT, api_name TEXT NOT NULL,
                called_at TEXT NOT NULL, endpoint TEXT NOT NULL DEFAULT '',
                success INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS holdings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL, market TEXT NOT NULL,
                symbol TEXT NOT NULL, name TEXT NOT NULL DEFAULT '',
                account TEXT NOT NULL DEFAULT '', quantity REAL NOT NULL,
                avg_price REAL NOT NULL, added_at TEXT NOT NULL,
                cached_price REAL, cached_at TEXT,
                UNIQUE(user_id, symbol, account)
            );
            CREATE TABLE IF NOT EXISTS signals (
                id TEXT PRIMARY KEY, user_id INTEGER NOT NULL,
                symbol TEXT NOT NULL, account TEXT NOT NULL DEFAULT '',
                cond_type TEXT NOT NULL, cond_value REAL NOT NULL,
                active INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL
            );",
        ).unwrap();
        DB_CONN.with(|c| *c.borrow_mut() = Some(conn));
    }

    #[test]
    fn blacklist_crud() {
        setup_test_db();
        assert!(!is_blacklisted("SCAM").unwrap());
        add_blacklist("SCAM", "fraud history").unwrap();
        assert!(is_blacklisted("SCAM").unwrap());
        let list = list_blacklist().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ticker, "SCAM");
        assert!(remove_blacklist("SCAM").unwrap());
        assert!(!is_blacklisted("SCAM").unwrap());
    }

    #[test]
    fn candidate_insert_and_judge() {
        setup_test_db();
        let id = insert_candidate("AAPL", "Apple", "Tech", "solid fundamentals", None).unwrap();
        let c = get_candidate_by_ticker("AAPL").unwrap().unwrap();
        assert_eq!(c.status, CandidateStatus::Pending);
        assert!(c.score.is_none());

        update_candidate_judge(id, 85.0, "strong buy").unwrap();
        let c = get_candidate_by_ticker("AAPL").unwrap().unwrap();
        assert_eq!(c.status, CandidateStatus::Judged);
        assert_eq!(c.score, Some(85.0));
        assert_eq!(c.verdict.as_deref(), Some("strong buy"));
    }

    #[test]
    fn prompt_set_get() {
        setup_test_db();
        assert!(get_prompt(PromptType::Hunt).unwrap().is_none());
        set_prompt(PromptType::Hunt, "find me stocks").unwrap();
        assert_eq!(get_prompt(PromptType::Hunt).unwrap().unwrap(), "find me stocks");
        set_prompt(PromptType::Hunt, "updated prompt").unwrap();
        assert_eq!(get_prompt(PromptType::Hunt).unwrap().unwrap(), "updated prompt");
    }

    #[test]
    fn gemini_calls_count() {
        setup_test_db();
        assert_eq!(gemini_calls_today().unwrap(), 0);
        log_api_call("gemini", "generateContent", true).unwrap();
        log_api_call("gemini", "generateContent", true).unwrap();
        assert_eq!(gemini_calls_today().unwrap(), 2);
    }

    #[test]
    fn holdings_crud() {
        setup_test_db();
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = chrono::Utc::now().with_timezone(&kst);

        // 빈 상태
        let store = load_holdings(123).unwrap();
        assert!(store.holdings.is_empty());

        // 저장
        let store = PortfolioStore {
            holdings: vec![
                Holding {
                    market: Market::KRX, symbol: "005930".into(), name: "삼성전자".into(),
                    account: String::new(), quantity: 10.0, avg_price: 70000.0,
                    added_at: now, cached_price: Some(72000.0), cached_at: Some(now),
                },
                Holding {
                    market: Market::NAS, symbol: "AAPL".into(), name: "Apple".into(),
                    account: "IRP".into(), quantity: 5.0, avg_price: 180.5,
                    added_at: now, cached_price: None, cached_at: None,
                },
            ],
        };
        save_holdings(123, &store).unwrap();

        // 로드 + 검증
        let loaded = load_holdings(123).unwrap();
        assert_eq!(loaded.holdings.len(), 2);
        assert_eq!(loaded.holdings[0].symbol, "005930");
        assert_eq!(loaded.holdings[0].market, Market::KRX);
        assert_eq!(loaded.holdings[0].cached_price, Some(72000.0));
        assert_eq!(loaded.holdings[1].symbol, "AAPL");
        assert_eq!(loaded.holdings[1].account, "IRP");

        // 다른 유저는 비어있음
        let other = load_holdings(456).unwrap();
        assert!(other.holdings.is_empty());

        // user_ids
        let ids = list_holding_user_ids().unwrap();
        assert_eq!(ids, vec![123]);

        // 덮어쓰기 (삼성전자 삭제, 애플만 남김)
        let updated = PortfolioStore {
            holdings: vec![loaded.holdings[1].clone()],
        };
        save_holdings(123, &updated).unwrap();
        let reloaded = load_holdings(123).unwrap();
        assert_eq!(reloaded.holdings.len(), 1);
        assert_eq!(reloaded.holdings[0].symbol, "AAPL");
    }

    #[test]
    fn signals_crud() {
        setup_test_db();
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = chrono::Utc::now().with_timezone(&kst);

        // 빈 상태
        let store = load_signals_db(123).unwrap();
        assert!(store.signals.is_empty());

        // 저장
        let store = SignalStore {
            signals: vec![
                Signal {
                    id: "sig-1".into(), symbol: "005930".into(), account: String::new(),
                    condition: Condition::PriceAbove { target: 80000.0 },
                    active: true, created_at: now,
                },
                Signal {
                    id: "sig-2".into(), symbol: "AAPL".into(), account: "IRP".into(),
                    condition: Condition::ProfitBelow { percentage: -5.0 },
                    active: false, created_at: now,
                },
            ],
        };
        save_signals_db(123, &store).unwrap();

        // 로드 + 검증
        let loaded = load_signals_db(123).unwrap();
        assert_eq!(loaded.signals.len(), 2);
        assert_eq!(loaded.signals[0].id, "sig-1");
        assert!(loaded.signals[0].active);
        match &loaded.signals[0].condition {
            Condition::PriceAbove { target } => assert_eq!(*target, 80000.0),
            _ => panic!("wrong condition variant"),
        }
        assert_eq!(loaded.signals[1].id, "sig-2");
        assert!(!loaded.signals[1].active);
        assert_eq!(loaded.signals[1].account, "IRP");
        match &loaded.signals[1].condition {
            Condition::ProfitBelow { percentage } => assert_eq!(*percentage, -5.0),
            _ => panic!("wrong condition variant"),
        }
    }

    #[test]
    fn condition_roundtrip() {
        let cases = vec![
            (Condition::PriceAbove { target: 100.0 }, "price_above", 100.0),
            (Condition::PriceBelow { target: 50.0 }, "price_below", 50.0),
            (Condition::ProfitAbove { percentage: 10.0 }, "profit_above", 10.0),
            (Condition::ProfitBelow { percentage: -5.0 }, "profit_below", -5.0),
        ];
        for (cond, expected_type, expected_value) in cases {
            let (ct, cv) = condition_to_parts(&cond);
            assert_eq!(ct, expected_type);
            assert_eq!(cv, expected_value);
            let restored = parts_to_condition(ct, cv).unwrap();
            let (ct2, cv2) = condition_to_parts(&restored);
            assert_eq!(ct2, expected_type);
            assert_eq!(cv2, expected_value);
        }
    }
}
