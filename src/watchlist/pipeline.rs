use anyhow::{Context, Result};

use crate::api::actor::ApiHandle;
use crate::models::messages::OverseasDetail;

use super::{db, gemini, models::CandidateStatus};

/// 사냥 사이클 결과
pub struct CycleReport {
    pub hunted: usize,
    pub collected: usize,
    pub survived: usize,
    pub culled: usize,
    pub collect_failed: usize,
}

impl CycleReport {
    pub fn summary(&self) -> String {
        let err = if self.collect_failed == 0 { String::new() } else { format!(" ❗{}", self.collect_failed) };
        format!(
            "🎯 사냥완료 ({}후보 → ✅{}생존 ⚖️{}처단{})",
            self.hunted, self.survived, self.culled, err,
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

/// 전체 사이클: 사냥 → 수집(라운드로빈) → 평가
pub async fn run_cycle(
    api: &ApiHandle,
    http_client: &reqwest::Client,
) -> Result<CycleReport> {
    let mut report = CycleReport {
        hunted: 0,
        collected: 0,
        survived: 0,
        culled: 0,
        collect_failed: 0,
    };

    // 1. 사냥
    let candidates = gemini::hunt(http_client).await
        .context("사냥 실패")?;
    report.hunted = candidates.len();

    if candidates.is_empty() {
        return Ok(report);
    }

    // 2. 수집 (라운드로빈 — 1개씩 순차 처리)
    let pending = db::list_candidates(Some(CandidateStatus::Pending))
        .context("pending 후보 조회 실패")?;

    for candidate in &pending {
        match fetch_detail(api, &candidate.ticker).await {
            Ok(detail) => {
                let text = format_detail_for_gemini(&candidate.ticker, &detail);
                if let Err(e) = db::update_candidate_collected(candidate.id, &text) {
                    tracing::error!("수집 데이터 저장 실패 {}: {e:#}", candidate.ticker);
                    report.collect_failed += 1;
                } else {
                    report.collected += 1;
                }
            }
            Err(e) => {
                tracing::warn!("수집 실패 → BL: {}: {e:#}", candidate.ticker);
                let _ = db::add_blacklist(&candidate.ticker, "한투 API 조회 실패 (자동)");
                let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                report.collect_failed += 1;
            }
        }
    }

    // 3. 평가 (collected 모아서 Gemini 1콜)
    let collected = db::list_candidates(Some(CandidateStatus::Collected))
        .context("collected 후보 조회 실패")?;

    if collected.is_empty() {
        return Ok(report);
    }

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
        "사이클 완료: 사냥 {}개, 수집 {}개, 생존 {}개, 처단 {}개, 실패 {}개",
        report.hunted, report.collected, report.survived, report.culled, report.collect_failed
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
    }

    #[test]
    fn cycle_report_summary() {
        let report = CycleReport {
            hunted: 30, collected: 25, survived: 20, culled: 5, collect_failed: 5,
        };
        let summary = report.summary();
        assert!(summary.contains("사냥완료"));
        assert!(summary.contains("30후보"));
        assert!(summary.contains("✅20생존"));
        assert!(summary.contains("⚖️5처단"));
        assert!(summary.contains("❗5"));
    }

    #[test]
    fn cycle_report_no_errors() {
        let report = CycleReport {
            hunted: 10, collected: 10, survived: 8, culled: 2, collect_failed: 0,
        };
        let summary = report.summary();
        assert!(!summary.contains("❗"));
    }
}
