use std::cell::RefCell;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, FixedOffset};
use rusqlite::{params, Connection};

use super::models::{Candidate, CandidateStatus, PendingEntry, PromptRecord, PromptType};
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
        "CREATE TABLE IF NOT EXISTS pending (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            ticker      TEXT NOT NULL UNIQUE,
            market      TEXT NOT NULL DEFAULT '',
            name        TEXT NOT NULL DEFAULT '',
            sector      TEXT NOT NULL DEFAULT '',
            reason      TEXT NOT NULL DEFAULT '',
            hunt_score  REAL,
            hunt_count  INTEGER NOT NULL DEFAULT 1,
            prompt_id   INTEGER,
            created_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS candidates (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            ticker        TEXT NOT NULL UNIQUE,
            market        TEXT NOT NULL DEFAULT '',
            name          TEXT NOT NULL DEFAULT '',
            sector        TEXT NOT NULL DEFAULT '',
            reason        TEXT NOT NULL DEFAULT '',
            hunt_score    REAL,
            hunt_count    INTEGER NOT NULL DEFAULT 1,
            score         REAL,
            verdict       TEXT,
            detail_text   TEXT NOT NULL DEFAULT '',
            status        TEXT NOT NULL DEFAULT 'judged',
            strike_count  INTEGER NOT NULL DEFAULT 0,
            judged_at     TEXT,
            created_at    TEXT NOT NULL
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
        CREATE INDEX IF NOT EXISTS idx_pending_ticker ON pending(ticker);
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

// ── Pending (사냥 버퍼) ──────────────────────────────────────────────

/// 사냥 결과 INSERT. ticker 충돌 시 hunt_count++ 및 메타데이터 갱신.
pub fn insert_pending(
    ticker: &str,
    market: &str,
    name: &str,
    sector: &str,
    reason: &str,
    hunt_score: f64,
    prompt_id: Option<i64>,
) -> Result<i64> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT INTO pending (ticker, market, name, sector, reason, hunt_score, hunt_count, prompt_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)
             ON CONFLICT(ticker) DO UPDATE SET
               hunt_count = pending.hunt_count + 1,
               name = excluded.name,
               sector = excluded.sector,
               reason = excluded.reason,
               hunt_score = excluded.hunt_score,
               market = excluded.market,
               prompt_id = excluded.prompt_id",
            params![ticker, market, name, sector, reason, hunt_score, prompt_id, now],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM pending WHERE ticker = ?1",
            params![ticker],
            |row| row.get(0),
        )?;
        Ok(id)
    })
}

pub fn list_pending() -> Result<Vec<PendingEntry>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ticker, market, name, sector, reason, hunt_score, hunt_count, created_at
             FROM pending ORDER BY hunt_score DESC, hunt_count DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PendingEntry {
                id: row.get(0)?,
                ticker: row.get(1)?,
                market: row.get(2)?,
                name: row.get(3)?,
                sector: row.get(4)?,
                reason: row.get(5)?,
                hunt_score: row.get(6)?,
                hunt_count: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    })
}

pub fn count_pending() -> Result<usize> {
    with_db(|conn| {
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM pending", [], |row| row.get(0))?;
        Ok(count as usize)
    })
}

/// pending에서 이미 candidates에 존재하는 ticker 제거 (졸업 정리)
pub fn cleanup_pending_graduated() -> Result<usize> {
    with_db(|conn| {
        let n = conn.execute(
            "DELETE FROM pending WHERE ticker IN (SELECT ticker FROM candidates)",
            [],
        )?;
        Ok(n)
    })
}

pub fn delete_pending(ticker: &str) -> Result<bool> {
    with_db(|conn| {
        let n = conn.execute("DELETE FROM pending WHERE ticker = ?1", params![ticker])?;
        Ok(n > 0)
    })
}

pub fn clear_all_pending() -> Result<usize> {
    with_db(|conn| {
        let n = conn.execute("DELETE FROM pending", [])?;
        Ok(n)
    })
}

/// pending 항목을 ticker로 조회
pub fn get_pending_by_ticker(ticker: &str) -> Result<Option<PendingEntry>> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ticker, market, name, sector, reason, hunt_score, hunt_count, created_at
             FROM pending WHERE ticker = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![ticker], |row| {
            Ok(PendingEntry {
                id: row.get(0)?,
                ticker: row.get(1)?,
                market: row.get(2)?,
                name: row.get(3)?,
                sector: row.get(4)?,
                reason: row.get(5)?,
                hunt_score: row.get(6)?,
                hunt_count: row.get(7)?,
                created_at: row.get(8)?,
            })
        })?;
        Ok(rows.next().and_then(|r| r.ok()))
    })
}

// ── Candidates (감정 완료) ───────────────────────────────────────────

/// 감정 후 candidates에 upsert. pipeline이 judge 결과를 기록할 때 사용.
/// 이미 존재하면 hunt_count 합산 + 메타데이터/점수 갱신.
pub fn upsert_candidate(
    ticker: &str,
    market: &str,
    name: &str,
    sector: &str,
    reason: &str,
    hunt_score: f64,
    hunt_count: i64,
    score: f64,
    verdict: &str,
    detail_text: &str,
) -> Result<i64> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT INTO candidates (ticker, market, name, sector, reason, hunt_score, hunt_count, score, verdict, detail_text, status, strike_count, judged_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'judged', 0, ?11, ?11)
             ON CONFLICT(ticker) DO UPDATE SET
               market = excluded.market,
               name = excluded.name,
               sector = excluded.sector,
               reason = excluded.reason,
               hunt_score = excluded.hunt_score,
               hunt_count = candidates.hunt_count + excluded.hunt_count,
               score = excluded.score,
               verdict = excluded.verdict,
               detail_text = excluded.detail_text,
               status = 'judged',
               judged_at = excluded.judged_at",
            params![ticker, market, name, sector, reason, hunt_score, hunt_count, score, verdict, detail_text, now],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM candidates WHERE ticker = ?1",
            params![ticker],
            |row| row.get(0),
        )?;
        Ok(id)
    })
}

pub fn list_candidates(status: Option<CandidateStatus>) -> Result<Vec<Candidate>> {
    with_db(|conn| {
        let (sql, param): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match status {
            Some(s) => (
                "SELECT id, ticker, market, name, sector, reason, hunt_score, hunt_count,
                        score, verdict, detail_text, status, strike_count, judged_at, created_at
                 FROM candidates WHERE status = ?1 ORDER BY score DESC, id DESC",
                vec![Box::new(s.as_str().to_string())],
            ),
            None => (
                "SELECT id, ticker, market, name, sector, reason, hunt_score, hunt_count,
                        score, verdict, detail_text, status, strike_count, judged_at, created_at
                 FROM candidates ORDER BY score DESC, id DESC",
                vec![],
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), row_to_candidate)?;
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

pub fn update_detail_text(id: i64, detail_text: &str) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE candidates SET detail_text = ?1 WHERE id = ?2",
            params![detail_text, id],
        )?;
        Ok(())
    })
}

pub fn clear_candidate_score(id: i64) -> Result<()> {
    with_db(|conn| {
        conn.execute(
            "UPDATE candidates SET score = NULL, verdict = NULL WHERE id = ?1",
            params![id],
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
            "SELECT id, ticker, market, name, sector, reason, hunt_score, hunt_count,
                    score, verdict, detail_text, status, strike_count, judged_at, created_at
             FROM candidates WHERE ticker = ?1 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![ticker], row_to_candidate)?;
        Ok(rows.next().and_then(|r| r.ok()))
    })
}

#[allow(dead_code)]
pub fn count_candidates_by_status(status: CandidateStatus) -> Result<usize> {
    with_db(|conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM candidates WHERE status = ?1",
            params![status.as_str()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    })
}

pub fn clear_candidates_by_status(status: CandidateStatus) -> Result<usize> {
    with_db(|conn| {
        let n = conn.execute(
            "DELETE FROM candidates WHERE status = ?1",
            params![status.as_str()],
        )?;
        Ok(n)
    })
}

/// candidates 행을 파싱하는 헬퍼
fn row_to_candidate(row: &rusqlite::Row) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        id: row.get(0)?,
        ticker: row.get(1)?,
        market: row.get(2)?,
        name: row.get(3)?,
        sector: row.get(4)?,
        reason: row.get(5)?,
        hunt_score: row.get(6)?,
        hunt_count: row.get(7)?,
        score: row.get(8)?,
        verdict: row.get(9)?,
        detail_text: row.get(10)?,
        status: CandidateStatus::from_str(&row.get::<_, String>(11)?),
        strike_count: row.get(12)?,
        judged_at: row.get(13)?,
        created_at: row.get(14)?,
    })
}

// ── Blacklist (candidates 테이블 활용) ───────────────────────────────

/// BL 여부 확인: candidates에서 status='blacklisted'인지
pub fn is_blacklisted(ticker: &str) -> Result<bool> {
    with_db(|conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM candidates WHERE ticker = ?1 AND status = 'blacklisted'",
            params![ticker],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    })
}

/// BL 추가/갱신: candidates에 INSERT (신규) 또는 UPDATE (기존)
/// 기존 judged → BL 전환 시 strike_count+1, status 변경
pub fn add_blacklist(ticker: &str, reason: &str) -> Result<()> {
    with_db(|conn| {
        let now = now_iso();
        conn.execute(
            "INSERT INTO candidates (ticker, verdict, status, strike_count, created_at)
             VALUES (?1, ?2, 'blacklisted', 1, ?3)
             ON CONFLICT(ticker) DO UPDATE SET
               verdict = excluded.verdict,
               strike_count = candidates.strike_count + 1,
               status = 'blacklisted'",
            params![ticker, reason, now],
        )?;
        Ok(())
    })
}

/// BL 해제: candidates에서 status='blacklisted'인 행 DELETE
pub fn remove_blacklist(ticker: &str) -> Result<bool> {
    with_db(|conn| {
        let n = conn.execute(
            "DELETE FROM candidates WHERE ticker = ?1 AND status = 'blacklisted'",
            params![ticker],
        )?;
        Ok(n > 0)
    })
}

/// BL 목록: candidates에서 status='blacklisted' 조회
pub fn list_blacklist() -> Result<Vec<Candidate>> {
    list_candidates(Some(CandidateStatus::Blacklisted))
}

/// BL 전체 삭제
pub fn clear_all_blacklist() -> Result<usize> {
    clear_candidates_by_status(CandidateStatus::Blacklisted)
}

// ── 재평가 ───────────────────────────────────────────────────────────

/// judged 중 점수 상위 max_survivors 외 나머지를 척살 (BL 전환 + strike_count++)
/// effective = score + ln(1+hunt_count) * hunt_count_weight
pub fn cull_excess_judged(max_survivors: usize, hunt_count_weight: f64) -> Result<usize> {
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ticker, score, hunt_count FROM candidates WHERE status = 'judged'",
        )?;
        let mut all: Vec<(i64, String, f64, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, f64>(2).unwrap_or(0.0),
                    row.get::<_, i64>(3).unwrap_or(1),
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        // effective score 내림차순 정렬
        all.sort_by(|a, b| {
            let eff_a = a.2 + (a.3 as f64).ln_1p() * hunt_count_weight;
            let eff_b = b.2 + (b.3 as f64).ln_1p() * hunt_count_weight;
            eff_b
                .partial_cmp(&eff_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if all.len() <= max_survivors {
            return Ok(0);
        }

        let victims: Vec<(i64, String, f64)> = all
            .iter()
            .skip(max_survivors)
            .map(|x| (x.0, x.1.clone(), x.2))
            .collect();

        for (id, _ticker, score) in &victims {
            let bl_reason = format!(
                "\u{1f5e1}\u{fe0f} 척살: {:.0}점 (상위 {max_survivors}위 밖)",
                score
            );
            conn.execute(
                "UPDATE candidates SET status = 'blacklisted', strike_count = strike_count + 1, verdict = ?1
                 WHERE id = ?2",
                params![bl_reason, id],
            )?;
        }

        let culled = victims.len();
        if culled > 0 {
            tracing::info!("척살: {culled}개 (상위 {max_survivors}개 유지)");
        }
        Ok(culled)
    })
}

/// 패자 부활: BL 중 score >= threshold (= min_score*0.9) 이고 strike_count < 3인 후보를
/// pending으로 이동 (hunt_count 유지), candidates에서 DELETE.
pub fn revive_near_misses(min_score: f64) -> Result<usize> {
    let threshold = min_score * 0.9;
    with_db(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ticker, market, name, sector, reason, hunt_score, hunt_count, created_at
             FROM candidates
             WHERE status = 'blacklisted'
               AND score IS NOT NULL
               AND score >= ?1
               AND strike_count < 3",
        )?;
        let targets: Vec<(i64, String, String, String, String, String, Option<f64>, i64, String)> =
            stmt.query_map(params![threshold], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let now = now_iso();
        for (_id, ticker, market, name, sector, reason, hunt_score, hunt_count, created_at) in
            &targets
        {
            // INSERT INTO pending (hunt_count 유지)
            conn.execute(
                "INSERT INTO pending (ticker, market, name, sector, reason, hunt_score, hunt_count, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(ticker) DO UPDATE SET
                   hunt_count = pending.hunt_count + excluded.hunt_count,
                   hunt_score = excluded.hunt_score",
                params![ticker, market, name, sector, reason, hunt_score, hunt_count, created_at],
            )?;
            // DELETE from candidates
            conn.execute(
                "DELETE FROM candidates WHERE ticker = ?1",
                params![ticker],
            )?;
            let _ = now; // suppress unused warning
        }

        if !targets.is_empty() {
            tracing::info!(
                "패자 부활: {}개 (threshold: {:.0}점, 삼진아웃 제외)",
                targets.len(),
                threshold
            );
        }
        Ok(targets.len())
    })
}

// ── Prompts (사냥용 / 감정용) ────────────────────────────────────────

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

// ── Prompt History ───────────────────────────────────────────────────

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

// ── API Usage ────────────────────────────────────────────────────────

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

pub fn hunt_calls_today() -> Result<usize> {
    with_db(|conn| {
        let today = today_prefix();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM api_usage WHERE api_name = 'gemini' AND endpoint = 'hunt' AND called_at LIKE ?1",
            params![format!("{today}%")],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    })
}

pub fn judge_calls_today() -> Result<usize> {
    with_db(|conn| {
        let today = today_prefix();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM api_usage WHERE api_name = 'gemini' AND endpoint = 'judge' AND called_at LIKE ?1",
            params![format!("{today}%")],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    })
}

// ── Retention (오래된 데이터 정리) ───────────────────────────────────

/// retention_days보다 오래된 pending + candidates + prompt_history + api_usage 삭제
pub fn cleanup_old_data(retention_days: u32) -> Result<usize> {
    with_db(|conn| {
        let cutoff = chrono::Utc::now()
            .checked_sub_signed(chrono::Duration::days(retention_days as i64))
            .unwrap_or_else(chrono::Utc::now)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();

        let mut total = 0usize;

        let n = conn.execute(
            "DELETE FROM pending WHERE created_at < ?1",
            params![cutoff],
        )?;
        total += n;

        let n = conn.execute(
            "DELETE FROM candidates WHERE created_at < ?1",
            params![cutoff],
        )?;
        total += n;

        let n = conn.execute(
            "DELETE FROM prompt_history WHERE created_at < ?1",
            params![cutoff],
        )?;
        total += n;

        let n = conn.execute(
            "DELETE FROM api_usage WHERE called_at < ?1",
            params![cutoff],
        )?;
        total += n;

        if total > 0 {
            tracing::info!("데이터 정리: {total}건 삭제 (기준: {retention_days}일)");
        }

        Ok(total)
    })
}

// ── Holdings (portfolio) ─────────────────────────────────────────────

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
            Ok((
                market_str,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                added_at_str,
                row.get(7)?,
                cached_at_str,
            ))
        })?;

        let mut holdings = Vec::new();
        for r in rows {
            let (
                market_str,
                symbol,
                name,
                account,
                quantity,
                avg_price,
                added_at_str,
                cached_price,
                cached_at_str,
            ): (
                String,
                String,
                String,
                String,
                f64,
                f64,
                String,
                Option<f64>,
                Option<String>,
            ) = r?;

            let market = Market::from_str(&market_str).unwrap_or(Market::KRX);
            let added_at = DateTime::parse_from_rfc3339(&added_at_str)
                .unwrap_or_else(|_| chrono::Utc::now().with_timezone(&kst_offset()));
            let cached_at = cached_at_str.and_then(|s| DateTime::parse_from_rfc3339(&s).ok());

            holdings.push(Holding {
                market,
                symbol,
                name,
                account,
                quantity,
                avg_price,
                added_at,
                cached_price,
                cached_at,
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

// ── Signals ──────────────────────────────────────────────────────────

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
        "profit_above" => Ok(Condition::ProfitAbove {
            percentage: cond_value,
        }),
        "profit_below" => Ok(Condition::ProfitBelow {
            percentage: cond_value,
        }),
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
            signals.push(Signal {
                id,
                symbol,
                account,
                condition,
                active,
                created_at,
            });
        }
        Ok(SignalStore { signals })
    })
}

pub fn save_signals_db(user_id: i64, store: &SignalStore) -> Result<()> {
    with_db(|conn| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM signals WHERE user_id = ?1",
            params![user_id],
        )?;
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

// ── Helpers ──────────────────────────────────────────────────────────

fn kst_offset() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).unwrap()
}

/// Google AI Studio 일일 쿼터 리셋 기준: 태평양시간 (고정 UTC-8, 서머타임 무시)
fn pacific_now() -> chrono::DateTime<chrono::FixedOffset> {
    let pt = chrono::FixedOffset::west_opt(8 * 3600).unwrap();
    chrono::Utc::now().with_timezone(&pt)
}

fn now_iso() -> String {
    pacific_now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

fn today_prefix() -> String {
    pacific_now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ticker TEXT NOT NULL UNIQUE,
                market TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL DEFAULT '',
                sector TEXT NOT NULL DEFAULT '',
                reason TEXT NOT NULL DEFAULT '',
                hunt_score REAL,
                hunt_count INTEGER NOT NULL DEFAULT 1,
                prompt_id INTEGER,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS candidates (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ticker TEXT NOT NULL UNIQUE,
                market TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL DEFAULT '',
                sector TEXT NOT NULL DEFAULT '',
                reason TEXT NOT NULL DEFAULT '',
                hunt_score REAL,
                hunt_count INTEGER NOT NULL DEFAULT 1,
                score REAL,
                verdict TEXT,
                detail_text TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'judged',
                strike_count INTEGER NOT NULL DEFAULT 0,
                judged_at TEXT,
                created_at TEXT NOT NULL
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
        )
        .unwrap();
        DB_CONN.with(|c| *c.borrow_mut() = Some(conn));
    }

    // ── Pending CRUD ─────────────────────────────────────────────────

    #[test]
    fn pending_insert_and_list() {
        setup_test_db();
        let id = insert_pending("SOUN", "NAS", "SoundHound", "AI", "voice platform", 75.0, None).unwrap();
        assert!(id > 0);

        let list = list_pending().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ticker, "SOUN");
        assert_eq!(list[0].market, "NAS");
        assert_eq!(list[0].hunt_count, 1);
        assert_eq!(list[0].hunt_score, Some(75.0));
    }

    #[test]
    fn pending_upsert_increments_count() {
        setup_test_db();
        let id1 = insert_pending("SOUN", "NAS", "SoundHound", "AI", "voice", 75.0, None).unwrap();
        let id2 = insert_pending("SOUN", "NAS", "SoundHound AI", "AI/Voice", "updated", 80.0, None).unwrap();
        assert_eq!(id1, id2);

        let entry = get_pending_by_ticker("SOUN").unwrap().unwrap();
        assert_eq!(entry.hunt_count, 2);
        assert_eq!(entry.reason, "updated");
        assert_eq!(entry.name, "SoundHound AI");
        assert_eq!(entry.hunt_score, Some(80.0));
    }

    #[test]
    fn pending_count_and_delete() {
        setup_test_db();
        insert_pending("AAA", "NAS", "A", "T", "r", 0.0, None).unwrap();
        insert_pending("BBB", "NYS", "B", "T", "r", 0.0, None).unwrap();
        assert_eq!(count_pending().unwrap(), 2);

        assert!(delete_pending("AAA").unwrap());
        assert_eq!(count_pending().unwrap(), 1);

        assert!(!delete_pending("NOPE").unwrap());
    }

    #[test]
    fn pending_cleanup_graduated() {
        setup_test_db();
        insert_pending("SOUN", "NAS", "SoundHound", "AI", "voice", 75.0, None).unwrap();
        insert_pending("GEVO", "NAS", "Gevo", "Energy", "fuel", 60.0, None).unwrap();

        // SOUN을 candidates에도 넣음 (졸업)
        upsert_candidate("SOUN", "NAS", "SoundHound", "AI", "voice", 75.0, 1, 85.0, "buy", "").unwrap();

        let cleaned = cleanup_pending_graduated().unwrap();
        assert_eq!(cleaned, 1);

        let remaining = list_pending().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].ticker, "GEVO");
    }

    #[test]
    fn pending_get_not_found() {
        setup_test_db();
        assert!(get_pending_by_ticker("NOPE").unwrap().is_none());
    }

    // ── Blacklist via candidates ─────────────────────────────────────

    #[test]
    fn blacklist_crud() {
        setup_test_db();
        assert!(!is_blacklisted("SCAM").unwrap());

        add_blacklist("SCAM", "fraud history").unwrap();
        assert!(is_blacklisted("SCAM").unwrap());

        let list = list_blacklist().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ticker, "SCAM");
        assert_eq!(list[0].status, CandidateStatus::Blacklisted);
        assert_eq!(list[0].strike_count, 1);

        assert!(remove_blacklist("SCAM").unwrap());
        assert!(!is_blacklisted("SCAM").unwrap());
    }

    #[test]
    fn blacklist_strike_count_increments() {
        setup_test_db();
        add_blacklist("SCAM", "first").unwrap();
        add_blacklist("SCAM", "second").unwrap();
        add_blacklist("SCAM", "third").unwrap();

        let c = get_candidate_by_ticker("SCAM").unwrap().unwrap();
        assert_eq!(c.strike_count, 3);
        assert_eq!(c.verdict.as_deref(), Some("third"));
    }

    #[test]
    fn blacklist_from_judged_candidate() {
        setup_test_db();
        // 먼저 judged candidate 생성
        upsert_candidate("FAIL", "NYS", "FailCo", "Junk", "bad", 0.0, 1, 30.0, "sell", "").unwrap();
        assert!(!is_blacklisted("FAIL").unwrap());

        // BL 전환
        add_blacklist("FAIL", "manual bl").unwrap();
        assert!(is_blacklisted("FAIL").unwrap());

        let c = get_candidate_by_ticker("FAIL").unwrap().unwrap();
        assert_eq!(c.status, CandidateStatus::Blacklisted);
        assert_eq!(c.strike_count, 1); // 기존 0 + 1
    }

    // ── Candidate Judge Flow ─────────────────────────────────────────

    #[test]
    fn candidate_upsert_and_query() {
        setup_test_db();
        let id = upsert_candidate(
            "AAPL", "NAS", "Apple", "Tech", "solid", 80.0, 1, 85.0, "strong buy", "detail",
        ).unwrap();

        let c = get_candidate_by_ticker("AAPL").unwrap().unwrap();
        assert_eq!(c.id, id);
        assert_eq!(c.status, CandidateStatus::Judged);
        assert_eq!(c.score, Some(85.0));
        assert_eq!(c.verdict.as_deref(), Some("strong buy"));
        assert_eq!(c.detail_text, "detail");
        assert_eq!(c.hunt_count, 1);
        assert_eq!(c.strike_count, 0);
    }

    #[test]
    fn candidate_upsert_merges_hunt_count() {
        setup_test_db();
        upsert_candidate("SOUN", "NAS", "SoundHound", "AI", "v1", 75.0, 3, 78.0, "buy", "d1").unwrap();
        upsert_candidate("SOUN", "NAS", "SoundHound AI", "AI", "v2", 80.0, 2, 90.0, "strong buy", "d2").unwrap();

        let c = get_candidate_by_ticker("SOUN").unwrap().unwrap();
        assert_eq!(c.hunt_count, 5); // 3 + 2
        assert_eq!(c.score, Some(90.0));
        assert_eq!(c.verdict.as_deref(), Some("strong buy"));
        assert_eq!(c.detail_text, "d2");
    }

    #[test]
    fn candidate_update_judge() {
        setup_test_db();
        let id = upsert_candidate("TSLA", "NAS", "Tesla", "EV", "growth", 0.0, 1, 70.0, "hold", "").unwrap();
        update_candidate_judge(id, 95.0, "strong buy").unwrap();

        let c = get_candidate_by_ticker("TSLA").unwrap().unwrap();
        assert_eq!(c.score, Some(95.0));
        assert_eq!(c.verdict.as_deref(), Some("strong buy"));
        assert_eq!(c.status, CandidateStatus::Judged);
    }

    #[test]
    fn candidate_detail_text_update() {
        setup_test_db();
        let id = upsert_candidate("TSLA", "NAS", "Tesla", "EV", "growth", 0.0, 1, 70.0, "hold", "").unwrap();
        update_detail_text(id, "Price: $8.50\nPER: 12").unwrap();

        let c = get_candidate_by_ticker("TSLA").unwrap().unwrap();
        assert!(c.detail_text.contains("Price: $8.50"));
    }

    #[test]
    fn candidate_status_update() {
        setup_test_db();
        let id = upsert_candidate("FAIL", "NYS", "FailCo", "Junk", "bad", 0.0, 1, 30.0, "sell", "").unwrap();
        update_candidate_status(id, CandidateStatus::Blacklisted).unwrap();

        let c = get_candidate_by_ticker("FAIL").unwrap().unwrap();
        assert_eq!(c.status, CandidateStatus::Blacklisted);
    }

    #[test]
    fn candidate_clear_score() {
        setup_test_db();
        let id = upsert_candidate("X", "NAS", "X", "T", "r", 0.0, 1, 80.0, "ok", "").unwrap();
        clear_candidate_score(id).unwrap();

        let c = get_candidate_by_ticker("X").unwrap().unwrap();
        assert!(c.score.is_none());
        assert!(c.verdict.is_none());
    }

    #[test]
    fn candidate_not_found() {
        setup_test_db();
        assert!(get_candidate_by_ticker("NOPE").unwrap().is_none());
    }

    #[test]
    fn list_candidates_by_status() {
        setup_test_db();
        upsert_candidate("AAA", "NAS", "A Co", "Tech", "r1", 0.0, 1, 70.0, "ok", "").unwrap();
        let id2 = upsert_candidate("BBB", "NYS", "B Co", "Fin", "r2", 0.0, 1, 90.0, "good", "").unwrap();
        update_candidate_status(id2, CandidateStatus::Blacklisted).unwrap();

        let judged = list_candidates(Some(CandidateStatus::Judged)).unwrap();
        assert_eq!(judged.len(), 1);
        assert_eq!(judged[0].ticker, "AAA");

        let bl = list_candidates(Some(CandidateStatus::Blacklisted)).unwrap();
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0].ticker, "BBB");

        let all = list_candidates(None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn count_and_clear_candidates() {
        setup_test_db();
        upsert_candidate("J1", "NAS", "J1", "T", "r", 0.0, 1, 80.0, "ok", "").unwrap();
        upsert_candidate("J2", "NYS", "J2", "T", "r", 0.0, 1, 75.0, "ok", "").unwrap();
        add_blacklist("BL1", "bad").unwrap();

        assert_eq!(count_candidates_by_status(CandidateStatus::Judged).unwrap(), 2);
        assert_eq!(count_candidates_by_status(CandidateStatus::Blacklisted).unwrap(), 1);

        let n = clear_candidates_by_status(CandidateStatus::Judged).unwrap();
        assert_eq!(n, 2);
        assert_eq!(count_candidates_by_status(CandidateStatus::Judged).unwrap(), 0);
        // BL은 그대로
        assert_eq!(count_candidates_by_status(CandidateStatus::Blacklisted).unwrap(), 1);
    }

    #[test]
    fn clear_all_blacklist_works() {
        setup_test_db();
        add_blacklist("BL1", "r1").unwrap();
        add_blacklist("BL2", "r2").unwrap();
        upsert_candidate("J1", "NAS", "J1", "T", "r", 0.0, 1, 80.0, "ok", "").unwrap();

        let n = clear_all_blacklist().unwrap();
        assert_eq!(n, 2);
        assert!(!is_blacklisted("BL1").unwrap());
        // judged는 그대로
        assert!(get_candidate_by_ticker("J1").unwrap().is_some());
    }

    // ── Cull Excess Judged ───────────────────────────────────────────

    #[test]
    fn cull_excess_judged_keeps_top_n() {
        setup_test_db();
        for (ticker, market, score) in &[
            ("T1", "NAS", 90.0),
            ("T2", "NAS", 80.0),
            ("T3", "NYS", 70.0),
            ("T4", "AMS", 60.0),
            ("T5", "NAS", 50.0),
        ] {
            upsert_candidate(ticker, market, ticker, "Sec", "reason", 0.0, 1, *score, "v", "").unwrap();
        }

        let culled = cull_excess_judged(3, 0.0).unwrap();
        assert_eq!(culled, 2);

        let judged = list_candidates(Some(CandidateStatus::Judged)).unwrap();
        assert_eq!(judged.len(), 3);
        let tickers: Vec<&str> = judged.iter().map(|c| c.ticker.as_str()).collect();
        assert!(tickers.contains(&"T1"));
        assert!(tickers.contains(&"T2"));
        assert!(tickers.contains(&"T3"));

        // 척살된 것들은 BL (candidates 테이블 내)
        assert!(is_blacklisted("T4").unwrap());
        assert!(is_blacklisted("T5").unwrap());

        // strike_count 확인
        let t4 = get_candidate_by_ticker("T4").unwrap().unwrap();
        assert_eq!(t4.strike_count, 1);
    }

    #[test]
    fn cull_excess_judged_with_bonus_reorders() {
        setup_test_db();
        // T1: score=70, count=10 → effective = 70 + ln(11)*3 ≈ 77.19
        // T2: score=75, count=1  → effective = 75 + ln(2)*3  ≈ 77.08
        // T3: score=60, count=1  → effective = 60 + ln(2)*3  ≈ 62.08
        upsert_candidate("T1", "NAS", "T1", "Sec", "reason", 0.0, 10, 70.0, "v", "").unwrap();
        upsert_candidate("T2", "NAS", "T2", "Sec", "reason", 0.0, 1, 75.0, "v", "").unwrap();
        upsert_candidate("T3", "NAS", "T3", "Sec", "reason", 0.0, 1, 60.0, "v", "").unwrap();

        let culled = cull_excess_judged(2, 3.0).unwrap();
        assert_eq!(culled, 1);
        assert!(is_blacklisted("T3").unwrap());

        // T1 survived (lower raw score but higher effective)
        let c1 = get_candidate_by_ticker("T1").unwrap().unwrap();
        assert_eq!(c1.status, CandidateStatus::Judged);
    }

    #[test]
    fn cull_excess_judged_no_op_under_limit() {
        setup_test_db();
        upsert_candidate("SOLO", "NAS", "Solo", "Tech", "r", 0.0, 1, 75.0, "ok", "").unwrap();

        let culled = cull_excess_judged(50, 0.0).unwrap();
        assert_eq!(culled, 0);
    }

    #[test]
    fn cull_excess_judged_empty() {
        setup_test_db();
        let culled = cull_excess_judged(10, 0.0).unwrap();
        assert_eq!(culled, 0);
    }

    // ── Revive Near Misses ───────────────────────────────────────────

    #[test]
    fn revive_near_misses_basic() {
        setup_test_db();
        let min_score = 60.0;
        // threshold = 54.0

        // 55점 — threshold(54) 이상 → 부활 대상
        upsert_candidate("NEAR", "NAS", "Near", "T", "r", 70.0, 2, 55.0, "close", "").unwrap();
        update_candidate_status(
            get_candidate_by_ticker("NEAR").unwrap().unwrap().id,
            CandidateStatus::Blacklisted,
        ).unwrap();
        // strike_count는 add_blacklist이 아니라 status만 바꿨으므로 0. 수동으로 +1
        with_db(|conn| {
            conn.execute("UPDATE candidates SET strike_count = 1 WHERE ticker = 'NEAR'", [])?;
            Ok(())
        }).unwrap();

        // 40점 — threshold(54) 미만 → 부활 불가
        upsert_candidate("FAR", "NAS", "Far", "T", "r", 50.0, 1, 40.0, "bad", "").unwrap();
        add_blacklist("FAR", "bad score").unwrap();

        // score 없음 → 부활 불가
        with_db(|conn| {
            conn.execute(
                "INSERT INTO candidates (ticker, status, strike_count, created_at) VALUES ('DEAD', 'blacklisted', 1, '2025-01-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        }).unwrap();

        let revived = revive_near_misses(min_score).unwrap();
        assert_eq!(revived, 1);

        // NEAR: pending으로 이동, candidates에서 삭제
        let pending = get_pending_by_ticker("NEAR").unwrap();
        assert!(pending.is_some());
        assert_eq!(pending.unwrap().hunt_count, 2); // hunt_count 유지
        assert!(get_candidate_by_ticker("NEAR").unwrap().is_none());

        // FAR: 그대로 BL
        assert!(is_blacklisted("FAR").unwrap());
        // DEAD: 그대로 BL
        assert!(is_blacklisted("DEAD").unwrap());
    }

    #[test]
    fn revive_three_strikes_out() {
        setup_test_db();
        let min_score = 60.0;

        upsert_candidate("RETRY", "NAS", "Retry", "T", "r", 0.0, 1, 58.0, "close", "").unwrap();
        // strike_count = 3 (삼진아웃)
        add_blacklist("RETRY", "strike 1").unwrap();
        add_blacklist("RETRY", "strike 2").unwrap();
        add_blacklist("RETRY", "strike 3").unwrap();

        let revived = revive_near_misses(min_score).unwrap();
        assert_eq!(revived, 0);
        assert!(is_blacklisted("RETRY").unwrap());
    }

    // ── Prompts ──────────────────────────────────────────────────────

    #[test]
    fn prompt_set_get() {
        setup_test_db();
        assert!(get_prompt(PromptType::Hunt).unwrap().is_none());
        set_prompt(PromptType::Hunt, "find me stocks").unwrap();
        assert_eq!(
            get_prompt(PromptType::Hunt).unwrap().unwrap(),
            "find me stocks"
        );
        set_prompt(PromptType::Hunt, "updated prompt").unwrap();
        assert_eq!(
            get_prompt(PromptType::Hunt).unwrap().unwrap(),
            "updated prompt"
        );
    }

    // ── API Usage ────────────────────────────────────────────────────

    #[test]
    fn hunt_judge_calls_count() {
        setup_test_db();
        assert_eq!(hunt_calls_today().unwrap(), 0);
        assert_eq!(judge_calls_today().unwrap(), 0);
        log_api_call("gemini", "hunt", true).unwrap();
        log_api_call("gemini", "hunt", true).unwrap();
        log_api_call("gemini", "judge", true).unwrap();
        assert_eq!(hunt_calls_today().unwrap(), 2);
        assert_eq!(judge_calls_today().unwrap(), 1);
    }

    #[test]
    fn hunt_judge_calls_isolated() {
        setup_test_db();
        log_api_call("gemini", "hunt", true).unwrap();
        log_api_call("gemini", "judge", true).unwrap();
        log_api_call("gemini", "judge", true).unwrap();
        assert_eq!(hunt_calls_today().unwrap(), 1);
        assert_eq!(judge_calls_today().unwrap(), 2);
    }

    #[test]
    fn api_usage_log_success_and_failure() {
        setup_test_db();
        log_api_call("gemini", "hunt", true).unwrap();
        log_api_call("gemini", "hunt", false).unwrap();
        log_api_call("gemini", "judge", true).unwrap();
        assert_eq!(hunt_calls_today().unwrap(), 2);
        assert_eq!(judge_calls_today().unwrap(), 1);
    }

    // ── Prompt History ───────────────────────────────────────────────

    #[test]
    fn prompt_history_insert_and_list() {
        setup_test_db();
        let id = insert_prompt_history(
            PromptType::Hunt,
            "prompt text",
            "response text",
            "gemma-3-27b-it",
            "SOUN,GEVO",
            "success",
        )
        .unwrap();
        assert!(id > 0);

        let history = list_prompt_history(10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].prompt_type, "hunt");
        assert_eq!(history[0].model, "gemma-3-27b-it");
        assert_eq!(history[0].tickers_extracted, "SOUN,GEVO");
    }

    // ── Holdings ─────────────────────────────────────────────────────

    #[test]
    fn holdings_crud() {
        setup_test_db();
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = chrono::Utc::now().with_timezone(&kst);

        let store = load_holdings(123).unwrap();
        assert!(store.holdings.is_empty());

        let store = PortfolioStore {
            holdings: vec![
                Holding {
                    market: Market::KRX,
                    symbol: "005930".into(),
                    name: "삼성전자".into(),
                    account: String::new(),
                    quantity: 10.0,
                    avg_price: 70000.0,
                    added_at: now,
                    cached_price: Some(72000.0),
                    cached_at: Some(now),
                },
                Holding {
                    market: Market::NAS,
                    symbol: "AAPL".into(),
                    name: "Apple".into(),
                    account: "IRP".into(),
                    quantity: 5.0,
                    avg_price: 180.5,
                    added_at: now,
                    cached_price: None,
                    cached_at: None,
                },
            ],
        };
        save_holdings(123, &store).unwrap();

        let loaded = load_holdings(123).unwrap();
        assert_eq!(loaded.holdings.len(), 2);
        assert_eq!(loaded.holdings[0].symbol, "005930");
        assert_eq!(loaded.holdings[0].market, Market::KRX);
        assert_eq!(loaded.holdings[0].cached_price, Some(72000.0));
        assert_eq!(loaded.holdings[1].symbol, "AAPL");
        assert_eq!(loaded.holdings[1].account, "IRP");

        let other = load_holdings(456).unwrap();
        assert!(other.holdings.is_empty());

        let ids = list_holding_user_ids().unwrap();
        assert_eq!(ids, vec![123]);

        let updated = PortfolioStore {
            holdings: vec![loaded.holdings[1].clone()],
        };
        save_holdings(123, &updated).unwrap();
        let reloaded = load_holdings(123).unwrap();
        assert_eq!(reloaded.holdings.len(), 1);
        assert_eq!(reloaded.holdings[0].symbol, "AAPL");
    }

    // ── Signals ──────────────────────────────────────────────────────

    #[test]
    fn signals_crud() {
        setup_test_db();
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = chrono::Utc::now().with_timezone(&kst);

        let store = load_signals_db(123).unwrap();
        assert!(store.signals.is_empty());

        let store = SignalStore {
            signals: vec![
                Signal {
                    id: "sig-1".into(),
                    symbol: "005930".into(),
                    account: String::new(),
                    condition: Condition::PriceAbove { target: 80000.0 },
                    active: true,
                    created_at: now,
                },
                Signal {
                    id: "sig-2".into(),
                    symbol: "AAPL".into(),
                    account: "IRP".into(),
                    condition: Condition::ProfitBelow { percentage: -5.0 },
                    active: false,
                    created_at: now,
                },
            ],
        };
        save_signals_db(123, &store).unwrap();

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
