use anyhow::{Context, Result};

use super::actor::ActorContext;

/// 해외주식 현재가상세(HHDFS76200200) 응답의 t_rate(당일환율) 필드로 USD/KRW 환율 조회.
/// 별도의 환율 전용 엔드포인트가 없으므로 AAPL 시세 조회를 레퍼런스로 사용.
pub async fn get_usd_krw(ctx: &ActorContext) -> Result<f64> {
    let url = format!(
        "{}/uapi/overseas-price/v1/quotations/price-detail",
        ctx.config.base_url
    );

    let http_resp = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("HHDFS76200200")?)
        .query(&[("AUTH", ""), ("EXCD", "NAS"), ("SYMB", "AAPL")])
        .send()
        .await
        .context("Exchange rate request failed")?;

    let status = http_resp.status();
    let body = http_resp.text().await.context("Exchange rate body read failed")?;

    let resp: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("Exchange rate parse failed (HTTP {status}): {body}")
    })?;

    resp["output"]["t_rate"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| anyhow::anyhow!("t_rate not found in price-detail response: {body}"))
}
