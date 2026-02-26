use anyhow::{Context, Result};

use super::actor::ActorContext;

pub async fn get_usd_krw(ctx: &ActorContext) -> Result<f64> {
    let url = format!(
        "{}/uapi/overseas-stock/v1/quotations/inquire-exchange-rate",
        ctx.config.base_url
    );

    // 환율 API는 별도 tr_id 불필요한 경우도 있으나, 일반적인 헤더 사용
    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("CTRP6504R")?)
        .send()
        .await
        .context("Exchange rate request failed")?
        .json()
        .await
        .context("Exchange rate parse failed")?;

    // 응답 구조에서 USD/KRW 환율 추출
    let items = resp["output"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    for item in items {
        if item["cur_cd"].as_str() == Some("USD") {
            if let Some(rate) = item["bkpr"].as_str().and_then(|s| s.parse::<f64>().ok()) {
                return Ok(rate);
            }
        }
    }

    anyhow::bail!("USD/KRW rate not found in response")
}
