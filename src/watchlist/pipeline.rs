use anyhow::{Context, Result};

use crate::api::actor::ApiHandle;
use crate::models::messages::OverseasDetail;

use super::{db, gemini, models::CandidateStatus};

/// 디스커버리 사이클 결과
pub struct CycleReport {
    pub hunted: usize,
    pub detailed: usize,
    pub survived: usize,
    pub culled: usize,
    pub errors: Vec<String>,
}

impl CycleReport {
    pub fn summary(&self) -> String {
        let evaluated = self.survived + self.culled;
        let mut lines = vec![
            format!("🎯 사냥: {}개 후보", self.hunted),
            format!("📊 데이터 수집: {}개", self.detailed),
            format!("✅ 생존: {}개 / ⚖️ 처단: {}개 ({}개 평가)", self.survived, self.culled, evaluated),
        ];
        if !self.errors.is_empty() {
            lines.push(format!("⚠️ 오류: {}건", self.errors.len()));
            for e in &self.errors {
                lines.push(format!("  - {e}"));
            }
        }
        lines.join("\n")
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

/// 거래소 코드 추정 (대부분 NAS, 없으면 NYS, AMS 순회)
async fn fetch_detail(api: &ApiHandle, ticker: &str) -> Result<OverseasDetail> {
    // NAS 먼저 시도, 실패하면 NYS, AMS
    for exch in &["NAS", "NYS", "AMS"] {
        match api.get_overseas_detail(exch, ticker).await {
            Ok(detail) if detail.current_price > 0.0 => return Ok(detail),
            _ => continue,
        }
    }
    anyhow::bail!("{ticker}: 모든 거래소에서 조회 실패")
}

/// 전체 디스커버리 사이클 실행
///
/// 1. 사냥: Gemini → 후보 목록
/// 2. 데이터 수집: 한투 API → 종목 상세
/// 3. 처단: Gemini + 실데이터 → 점수/판결
/// 4. DB 업데이트
pub async fn run_discovery_cycle(
    api: &ApiHandle,
    http_client: &reqwest::Client,
) -> Result<CycleReport> {
    let mut report = CycleReport {
        hunted: 0,
        detailed: 0,
        survived: 0,
        culled: 0,
        errors: Vec::new(),
    };

    // 1. 사냥
    let candidates = match gemini::hunt(http_client).await {
        Ok(c) => c,
        Err(e) => {
            return Err(e.context("사냥 실패"));
        }
    };
    report.hunted = candidates.len();

    if candidates.is_empty() {
        return Ok(report);
    }

    // 2. 한투 API로 상세 데이터 수집
    let mut detail_texts = Vec::new();
    let mut detail_candidate_ids = Vec::new();

    // pending 상태의 후보를 DB에서 가져옴 (방금 사냥으로 저장된 것 포함)
    let pending = db::list_candidates(Some(CandidateStatus::Pending))
        .context("pending 후보 조회 실패")?;

    for candidate in &pending {
        match fetch_detail(api, &candidate.ticker).await {
            Ok(detail) => {
                detail_texts.push(format_detail_for_gemini(&candidate.ticker, &detail));
                detail_candidate_ids.push(candidate.id);
                report.detailed += 1;
            }
            Err(e) => {
                let msg = format!("{}: {e}", candidate.ticker);
                tracing::warn!("데이터 수집 실패 → 블랙리스트: {msg}");
                let _ = db::add_blacklist(&candidate.ticker, "한투 API 조회 실패 (자동)");
                let _ = db::update_candidate_status(candidate.id, CandidateStatus::Blacklisted);
                report.errors.push(msg);
            }
        }
    }

    if detail_texts.is_empty() {
        report.errors.push("데이터 수집된 종목 없음".to_string());
        return Ok(report);
    }

    // 3. 처단: 수집한 데이터를 Gemini에게 평가 요청
    let combined_data = detail_texts.join("\n---\n");
    let judge_results = match gemini::judge(http_client, &combined_data).await {
        Ok(r) => r,
        Err(e) => {
            report.errors.push(format!("처단 실패: {e}"));
            return Ok(report);
        }
    };

    // 4. DB 업데이트 + 기준 점수 미달 → 블랙리스트 (처단)
    let min_score = crate::storage::with_config(|c| c.watchlist.min_score);
    for jr in &judge_results {
        let ticker = jr.ticker.to_uppercase();
        if let Some(candidate) = pending.iter().find(|c| c.ticker == ticker) {
            if let Err(e) = db::update_candidate_judge(candidate.id, jr.score, &jr.verdict) {
                report.errors.push(format!("{ticker} DB 업데이트 실패: {e}"));
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
        "디스커버리 사이클 완료: 사냥 {}개, 데이터 {}개, 생존 {}개, 처단 {}개",
        report.hunted, report.detailed, report.survived, report.culled
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
    fn cycle_report_summary() {
        let report = CycleReport {
            hunted: 30,
            detailed: 28,
            survived: 20,
            culled: 5,
            errors: vec!["XYZ: 조회 실패".to_string()],
        };
        let summary = report.summary();
        assert!(summary.contains("사냥: 30개"));
        assert!(summary.contains("데이터 수집: 28개"));
        assert!(summary.contains("생존: 20개"));
        assert!(summary.contains("처단: 5개"));
        assert!(summary.contains("25개 평가"));
        assert!(summary.contains("오류: 1건"));
    }

    #[test]
    fn cycle_report_no_errors() {
        let report = CycleReport {
            hunted: 10,
            detailed: 10,
            survived: 8,
            culled: 2,
            errors: Vec::new(),
        };
        let summary = report.summary();
        assert!(!summary.contains("오류"));
        assert!(summary.contains("생존: 8개"));
        assert!(summary.contains("처단: 2개"));
    }
}
