use anyhow::{Context, Result};

use crate::models::messages::PriceData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, symbol: &str) -> Result<PriceData> {
    // inquire-price: 날짜 파라미터 불필요 → 장 외 시간에도 안정적으로 동작.
    // 종목명(hts_kor_isnm) 미포함 → Holding.name 캐시 사용.
    let url = format!(
        "{}/uapi/domestic-stock/v1/quotations/inquire-price",
        ctx.config.base_url
    );

    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("FHKST01010100")?)
        .query(&[
            ("FID_COND_MRKT_DIV_CODE", "J"),
            ("FID_INPUT_ISCD", symbol),
        ])
        .send()
        .await
        .context("Domestic price request failed")?
        .json()
        .await
        .context("Domestic price parse failed")?;

    let output = &resp["output"];
    Ok(PriceData {
        name: String::new(),
        current_price: parse_f64(output["stck_prpr"].as_str()),
        change_pct: parse_f64(output["prdy_ctrt"].as_str()),
    })
}

fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}
