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
            "🎯 사냥 보고 ({}탐색 → ✅{}포획 ⚖️{}처단{})",
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

/// 거래소 코드로 상세 조회 (market 힌트 우선, 없으면 NAS→NYS→AMS 순회)
async fn fetch_detail(api: &ApiHandle, ticker: &str, market_hint: Option<&str>) -> Result<OverseasDetail> {
    // market 힌트가 있으면 그 거래소를 먼저 시도
    let exchanges: Vec<&str> = if let Some(hint) = market_hint {
        let hint = hint.trim();
        if !hint.is_empty() && ["NAS", "NYS", "AMS"].contains(&hint) {
            let mut v = vec![hint];
            for e in &["NAS", "NYS", "AMS"] {
                if *e != hint { v.push(e); }
            }
            v
        } else {
            vec!["NAS", "NYS", "AMS"]
        }
    } else {
        vec!["NAS", "NYS", "AMS"]
    };

    for exch in &exchanges {
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

    // 0. 오래된 데이터 정리
    let retention = crate::storage::with_config(|c| c.watchlist.retention_days);
    let _ = db::cleanup_old_data(retention);

    // 1. 사냥 (Flash Lite 직접 추천)
    let hunt_results = gemini::hunt(http_client).await
        .context("사냥 실패")?;
    report.hunted = hunt_results.len();

    if hunt_results.is_empty() {
        return Ok(report);
    }

    // 2. 수집 (라운드로빈 — 1개씩 순차 처리)
    let pending = db::list_candidates(Some(CandidateStatus::Pending))
        .context("pending 후보 조회 실패")?;

    let mut collected_ids: Vec<i64> = Vec::new();
    for candidate in &pending {
        let hint = if candidate.market.is_empty() { None } else { Some(candidate.market.as_str()) };
        match fetch_detail(api, &candidate.ticker, hint).await {
            Ok(detail) => {
                let text = format_detail_for_gemini(&candidate.ticker, &detail);
                if let Err(e) = db::update_candidate_collected(candidate.id, &text) {
                    tracing::error!("수집 데이터 저장 실패 {}: {e:#}", candidate.ticker);
                    report.collect_failed += 1;
                } else {
                    collected_ids.push(candidate.id);
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

    // 3. 평가 (이번 사이클에서 수집한 것만 — 잔류 collected는 재평가에서 처리)
    let collected: Vec<_> = db::list_candidates(Some(CandidateStatus::Collected))
        .context("collected 후보 조회 실패")?
        .into_iter()
        .filter(|c| collected_ids.contains(&c.id))
        .collect();

    if collected.is_empty() {
        return Ok(report);
    }

    let combined_data: String = collected.iter()
        .map(|c| c.detail_text.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    let judge_results = gemini::judge(http_client, &combined_data).await
        .context("평가 실패")?;

    let (min_score, hunt_weight) = crate::storage::with_config(|c| {
        (c.watchlist.min_score, c.watchlist.hunt_weight)
    });

    for jr in &judge_results {
        let ticker = jr.ticker.to_uppercase();
        if let Some(candidate) = collected.iter().find(|c| c.ticker == ticker) {
            let hunt_s = candidate.hunt_score.unwrap_or(0.0);
            let final_score = hunt_s * hunt_weight + jr.score * (1.0 - hunt_weight);
            if let Err(e) = db::update_candidate_judge(candidate.id, final_score, &jr.verdict) {
                tracing::error!("{ticker} DB 업데이트 실패: {e:#}");
            } else if final_score < min_score {
                let reason = format!("처단: {:.0}점 < 기준 {:.0}점", final_score, min_score);
                let _ = db::add_blacklist(&ticker, &reason);
                let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                report.culled += 1;
            } else {
                report.survived += 1;
            }
        }
    }

    // 4. 도태 (상위 max_survivors만 유지)
    let max_survivors = crate::storage::with_config(|c| c.watchlist.max_survivors);
    let culled_excess = db::cull_excess_judged(max_survivors).unwrap_or(0);
    report.culled += culled_excess;

    tracing::info!(
        "사이클 완료: 사냥 {}개, 수집 {}개, 생존 {}개, 처단 {}개, 실패 {}개",
        report.hunted, report.collected, report.survived, report.culled, report.collect_failed
    );

    Ok(report)
}

/// 재평가 사이클: judged 후보를 재수집 → 재평가 → 도태
pub async fn run_reeval(
    api: &ApiHandle,
    http_client: &reqwest::Client,
) -> Result<RevalReport> {
    let mut report = RevalReport {
        target: 0,
        revived: 0,
        collected: 0,
        survived: 0,
        culled: 0,
        collect_failed: 0,
    };

    // 패자 부활 (재평가 전에 BL에서 복귀)
    let min_score = crate::storage::with_config(|c| c.watchlist.min_score);
    let revived = db::revive_near_misses(min_score).unwrap_or(0);
    report.revived = revived;

    // judged → pending 리셋 (재수집 대상으로)
    let reset_count = db::reset_judged_for_reeval()?;
    report.target = reset_count + revived;

    // 잔류 collected 체크 (이전 재평가 실패로 방치된 것)
    let stale_collected = db::count_candidates_by_status(CandidateStatus::Collected)?;
    if reset_count == 0 && revived == 0 && stale_collected == 0 {
        return Ok(report);
    }
    report.target += stale_collected;

    // 수집
    let pending = db::list_candidates(Some(CandidateStatus::Pending))
        .context("재평가 pending 조회 실패")?;

    for candidate in &pending {
        let hint = if candidate.market.is_empty() { None } else { Some(candidate.market.as_str()) };
        match fetch_detail(api, &candidate.ticker, hint).await {
            Ok(detail) => {
                let text = format_detail_for_gemini(&candidate.ticker, &detail);
                if let Err(e) = db::update_candidate_collected(candidate.id, &text) {
                    tracing::error!("재수집 저장 실패 {}: {e:#}", candidate.ticker);
                    report.collect_failed += 1;
                } else {
                    report.collected += 1;
                }
            }
            Err(e) => {
                tracing::warn!("재수집 실패 → BL: {}: {e:#}", candidate.ticker);
                let _ = db::add_blacklist(&candidate.ticker, "재수집 실패 (자동)");
                let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                report.collect_failed += 1;
            }
        }
    }

    // 평가 (배치 분할 — TPM 한도 회피)
    let collected = db::list_candidates(Some(CandidateStatus::Collected))
        .context("재평가 collected 조회 실패")?;

    if collected.is_empty() {
        return Ok(report);
    }

    let (min_score, hunt_weight, batch_size) = crate::storage::with_config(|c| {
        (c.watchlist.min_score, c.watchlist.hunt_weight, c.watchlist.candidate_count)
    });

    for (i, chunk) in collected.chunks(batch_size).enumerate() {
        if i > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }

        let combined_data: String = chunk.iter()
            .map(|c| c.detail_text.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");

        let judge_results = match gemini::judge(http_client, &combined_data).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("재평가 배치 {}/{} 실패 ({}개): {e:#}",
                    i + 1, collected.chunks(batch_size).len(), chunk.len());
                continue;
            }
        };

        for jr in &judge_results {
            let ticker = jr.ticker.to_uppercase();
            if let Some(candidate) = chunk.iter().find(|c| c.ticker == ticker) {
                let hunt_s = candidate.hunt_score.unwrap_or(0.0);
                let final_score = hunt_s * hunt_weight + jr.score * (1.0 - hunt_weight);
                if let Err(e) = db::update_candidate_judge(candidate.id, final_score, &jr.verdict) {
                    tracing::error!("{ticker} 재평가 DB 업데이트 실패: {e:#}");
                } else if final_score < min_score {
                    let reason = format!("재평가 처단: {:.0}점 < 기준 {:.0}점", final_score, min_score);
                    let _ = db::add_blacklist(&ticker, &reason);
                    let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                    report.culled += 1;
                } else {
                    report.survived += 1;
                }
            }
        }
    }

    // 도태
    let max_survivors = crate::storage::with_config(|c| c.watchlist.max_survivors);
    let culled_excess = db::cull_excess_judged(max_survivors).unwrap_or(0);
    report.culled += culled_excess;

    tracing::info!(
        "재평가 완료: 대상 {}개, 수집 {}개, 생존 {}개, 처단 {}개, 실패 {}개",
        report.target, report.collected, report.survived, report.culled, report.collect_failed
    );

    Ok(report)
}

/// 재평가 사이클 결과
pub struct RevalReport {
    pub target: usize,
    pub revived: usize,
    pub collected: usize,
    pub survived: usize,
    pub culled: usize,
    pub collect_failed: usize,
}

impl RevalReport {
    pub fn summary(&self) -> String {
        let err = if self.collect_failed == 0 { String::new() } else { format!(" ❗{}", self.collect_failed) };
        let rev = if self.revived == 0 { String::new() } else { format!(" 🔁{}발굴", self.revived) };
        format!(
            "🔄 재선별 보고 ({}마리{rev} → ✅{}포획 ⚖️{}처단{})",
            self.target, self.survived, self.culled, err,
        )
    }
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
        assert!(summary.contains("사냥 보고"));
        assert!(summary.contains("30탐색"));
        assert!(summary.contains("✅20포획"));
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

    #[test]
    fn reeval_report_summary() {
        let report = RevalReport {
            target: 50, revived: 0, collected: 45, survived: 30, culled: 15, collect_failed: 5,
        };
        let summary = report.summary();
        assert!(summary.contains("재선별 보고"));
        assert!(summary.contains("50마리"));
        assert!(summary.contains("✅30포획"));
        assert!(summary.contains("⚖️15처단"));
        assert!(summary.contains("❗5"));
    }

    #[test]
    fn reeval_report_no_errors() {
        let report = RevalReport {
            target: 10, revived: 0, collected: 10, survived: 8, culled: 2, collect_failed: 0,
        };
        let summary = report.summary();
        assert!(!summary.contains("❗"));
    }

    #[test]
    fn reeval_report_zero_target() {
        let report = RevalReport {
            target: 0, revived: 0, collected: 0, survived: 0, culled: 0, collect_failed: 0,
        };
        let summary = report.summary();
        assert!(summary.contains("0마리"));
        assert!(summary.contains("✅0포획"));
    }
}
