use anyhow::{Context, Result};

use crate::api::actor::ApiHandle;
use crate::models::messages::OverseasDetail;

use super::{db, gemini, models::CandidateStatus};

/// 사냥 사이클 결과 (Gemini 추천만, 수집/감정 없음)
pub struct HuntReport {
    pub hunted: usize,
}

impl HuntReport {
    pub fn summary(&self) -> String {
        format!("🎯 사냥 보고 (🔍포착 +{})", self.hunted)
    }
}

/// 감정 사이클 결과
pub struct EvalReport {
    pub target: usize,
    pub revived: usize,
    pub collected: usize,
    pub survived: usize,
    pub culled: usize,
    pub collect_failed: usize,
}

impl EvalReport {
    pub fn summary(&self) -> String {
        let err = if self.collect_failed == 0 { String::new() } else { format!(" ❗{}", self.collect_failed) };
        let rev = if self.revived == 0 { String::new() } else { format!(" 🔁해제 +{}", self.revived) };
        format!(
            "🔄 감정 보고 (대상 {}마리{rev} → 📦수집 +{} → 🦎양피 +{} 🗡️척살 +{}{})",
            self.target, self.collected, self.survived, self.culled, err,
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

/// 수집 성공 후보 (감정 대상)
struct ReadyCandidate {
    id: i64,
    ticker: String,
    detail_text: String,
}

/// 사냥 사이클: Gemini 추천 → DB insert (수집/감정 안 함)
pub async fn run_hunt(
    http_client: &reqwest::Client,
) -> Result<HuntReport> {
    // 오래된 데이터 정리
    let retention = crate::storage::with_config(|c| c.watchlist.retention_days);
    let _ = db::cleanup_old_data(retention);

    let hunt_results = gemini::hunt(http_client).await
        .context("사냥 실패")?;

    let report = HuntReport { hunted: hunt_results.len() };

    tracing::info!("사냥 완료: {}개 포착", report.hunted);

    Ok(report)
}

/// 감정 사이클: 수집(pending+judged) → 감정(배치) → 도태
pub async fn run_evaluate(
    api: &ApiHandle,
    http_client: &reqwest::Client,
) -> Result<EvalReport> {
    let mut report = EvalReport {
        target: 0,
        revived: 0,
        collected: 0,
        survived: 0,
        culled: 0,
        collect_failed: 0,
    };

    // 1. 패자 부활
    let min_score = crate::storage::with_config(|c| c.watchlist.min_score);
    let revived = db::revive_near_misses(min_score).unwrap_or(0);
    report.revived = revived;

    // 2. 수집 대상: pending + judged
    let pending = db::list_candidates(Some(CandidateStatus::Pending))
        .context("pending 조회 실패")?;
    let judged = db::list_candidates(Some(CandidateStatus::Judged))
        .context("judged 조회 실패")?;

    report.target = pending.len() + judged.len();

    if report.target == 0 {
        return Ok(report);
    }

    // 3. KIS API 수집 → ready Vec (메모리)
    let mut ready: Vec<ReadyCandidate> = Vec::new();

    for candidate in &pending {
        let hint = if candidate.market.is_empty() { None } else { Some(candidate.market.as_str()) };
        match fetch_detail(api, &candidate.ticker, hint).await {
            Ok(detail) => {
                let text = format_detail_for_gemini(&candidate.ticker, &detail);
                let _ = db::update_detail_text(candidate.id, &text);
                ready.push(ReadyCandidate {
                    id: candidate.id,
                    ticker: candidate.ticker.clone(),
                    detail_text: text,
                });
                report.collected += 1;
            }
            Err(e) => {
                tracing::warn!("수집 실패 → BL: {}: {e:#}", candidate.ticker);
                let _ = db::add_blacklist(&candidate.ticker, "한투 API 조회 실패 (자동)");
                let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                report.collect_failed += 1;
            }
        }
    }

    for candidate in &judged {
        let hint = if candidate.market.is_empty() { None } else { Some(candidate.market.as_str()) };
        match fetch_detail(api, &candidate.ticker, hint).await {
            Ok(detail) => {
                let text = format_detail_for_gemini(&candidate.ticker, &detail);
                let _ = db::update_detail_text(candidate.id, &text);
                ready.push(ReadyCandidate {
                    id: candidate.id,
                    ticker: candidate.ticker.clone(),
                    detail_text: text,
                });
                report.collected += 1;
            }
            Err(e) => {
                // judged 재수집 실패 → 스킵 (기존 score 유지)
                tracing::warn!("재수집 실패 (스킵): {}: {e:#}", candidate.ticker);
                report.collect_failed += 1;
            }
        }
    }

    if ready.is_empty() {
        return Ok(report);
    }

    // 4. 감정 (배치 분할)
    let batch_size = crate::storage::with_config(|c| c.watchlist.candidate_count);
    let mut matched_ids: Vec<i64> = Vec::new();

    for (i, chunk) in ready.chunks(batch_size).enumerate() {
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
                tracing::warn!("감정 배치 {}/{} 실패 ({}개): {e:#}",
                    i + 1, ready.chunks(batch_size).len(), chunk.len());
                continue;
            }
        };

        for jr in &judge_results {
            let ticker = jr.ticker.to_uppercase();
            if let Some(rc) = chunk.iter().find(|c| c.ticker == ticker) {
                let score = jr.score;
                if let Err(e) = db::update_candidate_judge(rc.id, score, &jr.verdict) {
                    tracing::error!("{ticker} DB 업데이트 실패: {e:#}");
                } else if score < min_score {
                    let reason = format!("🗡️ 척살: {:.0}점 < 기준 {:.0}점", score, min_score);
                    let _ = db::add_blacklist(&ticker, &reason);
                    let _ = db::update_candidate_status(rc.id, CandidateStatus::Blacklisted);
                    report.culled += 1;
                } else {
                    report.survived += 1;
                }
                matched_ids.push(rc.id);
            }
        }
    }

    // 감정 미매칭 → pending 복귀하지 않음 (BL행)
    for rc in &ready {
        if !matched_ids.contains(&rc.id) {
            let _ = db::add_blacklist(&rc.ticker, "감정 누락 (자동)");
            let _ = db::update_candidate_status(rc.id, CandidateStatus::Blacklisted);
            report.culled += 1;
        }
    }

    // 5. 도태 (hunt_count 보너스 적용)
    let (max_survivors, hunt_count_weight) = crate::storage::with_config(|c| {
        (c.watchlist.max_survivors, c.watchlist.hunt_count_weight)
    });
    let culled_excess = db::cull_excess_judged(max_survivors, hunt_count_weight).unwrap_or(0);
    report.culled += culled_excess;
    report.survived = report.survived.saturating_sub(culled_excess);

    tracing::info!(
        "감정 완료: 대상 {}개, 수집 {}개, 생존 {}개, 척살 {}개, 실패 {}개",
        report.target, report.collected, report.survived, report.culled, report.collect_failed
    );

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::messages::OverseasDetail;

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
    fn hunt_report_summary() {
        let report = HuntReport { hunted: 30 };
        let summary = report.summary();
        assert!(summary.contains("사냥 보고"));
        assert!(summary.contains("포착 +30"));
    }

    #[test]
    fn eval_report_summary() {
        let report = EvalReport {
            target: 50, revived: 3, collected: 45, survived: 30, culled: 15, collect_failed: 5,
        };
        let summary = report.summary();
        assert!(summary.contains("감정 보고"));
        assert!(summary.contains("50마리"));
        assert!(summary.contains("해제 +3"));
        assert!(summary.contains("수집 +45"));
        assert!(summary.contains("🦎양피 +30"));
        assert!(summary.contains("🗡️척살 +15"));
        assert!(summary.contains("❗5"));
    }

    #[test]
    fn eval_report_no_errors() {
        let report = EvalReport {
            target: 10, revived: 0, collected: 10, survived: 8, culled: 2, collect_failed: 0,
        };
        let summary = report.summary();
        assert!(!summary.contains("❗"));
        assert!(!summary.contains("해제"));
    }

    #[test]
    fn eval_report_zero_target() {
        let report = EvalReport {
            target: 0, revived: 0, collected: 0, survived: 0, culled: 0, collect_failed: 0,
        };
        let summary = report.summary();
        assert!(summary.contains("0마리"));
    }
}
