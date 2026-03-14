use anyhow::{Context, Result};

use crate::api::actor::ApiHandle;
use crate::models::messages::OverseasDetail;

use super::{db, gemini, models::CandidateStatus};

/// 사냥 결과
pub struct HuntReport {
    pub hunted: usize,
}

/// 평가 결과
pub struct EvaluateReport {
    pub survived: usize,
    pub culled: usize,
}

impl EvaluateReport {
    pub fn summary(&self) -> String {
        let total = self.survived + self.culled;
        format!(
            "⚖️ 평가완료 ({}개 → ✅{}생존 ⚖️{}처단)",
            total, self.survived, self.culled,
        )
    }
}

/// OverseasDetail을 Gemini에 넘길 텍스트로 변환
fn format_detail_for_gemini(ticker: &str, d: &OverseasDetail) -> String {
    format!(
        "Ticker: {ticker}\n\
         Name: {name}\n\
         Price: ${price:.2} ({change:+.2}%)\n\
         Market Cap: {mcap}\n\
         PER: {per}, PBR: {pbr}\n\
         EPS: {eps}, BPS: {bps}\n\
         Shares Outstanding: {shares}\n\
         Volume: {vol} (prev: {pvol})\n\
         52W High: ${h52:.2}, Low: ${l52:.2}\n\
         Sector: {sector}\n",
        name = d.name,
        price = d.current_price,
        change = d.change_pct,
        mcap = d.market_cap,
        per = d.per,
        pbr = d.pbr,
        eps = d.eps,
        bps = d.bps,
        shares = d.shares,
        vol = d.volume,
        pvol = d.prev_volume,
        h52 = d.high_52w,
        l52 = d.low_52w,
        sector = d.sector,
    )
}

/// 거래소 코드 추정 (NAS → NYS → AMS 순회)
async fn fetch_detail(api: &ApiHandle, ticker: &str) -> Result<OverseasDetail> {
    for exch in &["NAS", "NYS", "AMS"] {
        match api.get_overseas_detail(exch, ticker).await {
            Ok(detail) if detail.current_price > 0.0 => return Ok(detail),
            _ => continue,
        }
    }
    anyhow::bail!("{ticker}: 모든 거래소에서 조회 실패")
}

// === A: 사냥 ===

pub async fn run_hunt(http_client: &reqwest::Client) -> Result<HuntReport> {
    let candidates = gemini::hunt(http_client).await
        .context("사냥 실패")?;
    Ok(HuntReport { hunted: candidates.len() })
}

// === B: 수집 (라운드로빈 — 1개씩) ===

/// pending 중 가장 오래된 1개를 수집. 성공 → collected, 실패 → BL.
/// 수집할 게 없으면 None 리턴.
pub async fn collect_one(api: &ApiHandle) -> Option<(String, bool)> {
    let pending = db::list_candidates(Some(CandidateStatus::Pending)).ok()?;
    let candidate = pending.first()?;
    let ticker = candidate.ticker.clone();
    let id = candidate.id;

    match fetch_detail(api, &ticker).await {
        Ok(detail) => {
            let text = format_detail_for_gemini(&ticker, &detail);
            if let Err(e) = db::update_candidate_collected(id, &text) {
                tracing::error!("수집 데이터 저장 실패 {ticker}: {e:#}");
                return Some((ticker, false));
            }
            tracing::debug!("수집 완료: {ticker}");
            Some((ticker, true))
        }
        Err(e) => {
            tracing::warn!("수집 실패 → BL: {ticker}: {e:#}");
            let _ = db::add_blacklist(&ticker, "한투 API 조회 실패 (자동)");
            let _ = db::update_candidate_status(id, CandidateStatus::Blacklisted);
            Some((ticker, false))
        }
    }
}

// === C: 평가 ===

pub async fn run_evaluate(http_client: &reqwest::Client) -> Result<EvaluateReport> {
    let mut report = EvaluateReport { survived: 0, culled: 0 };

    let collected = db::list_candidates(Some(CandidateStatus::Collected))
        .context("collected 후보 조회 실패")?;

    if collected.is_empty() {
        return Ok(report);
    }

    // detail_text 합쳐서 Gemini에 전달
    let combined_data: String = collected.iter()
        .map(|c| c.detail_text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let judge_results = gemini::judge(http_client, &combined_data).await
        .context("평가 실패")?;

    let min_score = crate::storage::with_config(|c| c.watchlist.min_score);

    for jr in &judge_results {
        let ticker = jr.ticker.to_uppercase();
        if let Some(candidate) = collected.iter().find(|c| c.ticker == ticker) {
            if let Err(e) = db::update_candidate_judge(candidate.id, jr.score, &jr.verdict) {
                tracing::error!("{ticker} DB 업데이트 실패: {e:#}");
            } else if jr.score < min_score {
                let reason = format!("처단: {:.0}점 < 기준 {:.0}점", jr.score, min_score);
                let _ = db::add_blacklist(&ticker, &reason);
                let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                report.culled += 1;
            } else {
                report.survived += 1;
            }
        }
    }

    tracing::info!(
        "평가 완료: 생존 {}개, 처단 {}개",
        report.survived, report.culled
    );

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_detail_text() {
        let detail = OverseasDetail {
            name: "SoundHound AI".to_string(),
            current_price: 4.52,
            change_pct: 3.21,
            market_cap: 1500000000.0,
            per: 0.0,
            pbr: 8.5,
            eps: -0.32,
            bps: 0.53,
            shares: 250000000.0,
            volume: 12500000.0,
            volume_amount: 56250000.0,
            high_52w: 10.25,
            low_52w: 1.80,
            sector: "Technology".to_string(),
            prev_volume: 9800000.0,
        };

        let text = format_detail_for_gemini("SOUN", &detail);
        assert!(text.contains("Ticker: SOUN"));
        assert!(text.contains("Name: SoundHound AI"));
        assert!(text.contains("Price: $4.52"));
        assert!(text.contains("PBR: 8.5"));
        assert!(text.contains("Sector: Technology"));
        assert!(text.contains("52W High: $10.25, Low: $1.80"));
    }

    #[test]
    fn evaluate_report_summary() {
        let report = EvaluateReport { survived: 8, culled: 2 };
        let summary = report.summary();
        assert!(summary.contains("평가완료"));
        assert!(summary.contains("✅8생존"));
        assert!(summary.contains("⚖️2처단"));
    }
}
