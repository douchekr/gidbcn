use std::cell::RefCell;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use super::models::{BlacklistEntry, Candidate, CandidateStatus, PromptRecord, PromptType};

const DB_PATH: &str = "/opt/kkuepark/gidbcn/watchlist.db";

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
        CREATE INDEX IF NOT EXISTS idx_api_usage_date ON api_usage(called_at);",
    )?;

    DB_CONN.with(|c| *c.borrow_mut() = Some(conn));
    tracing::info!("watchlist DB initialized: {DB_PATH}");
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
}
