use anyhow::{Context, Result};

use crate::models::messages::PriceData;

use super::actor::ActorContext;

/// (현재가 데이터, 당일환율 t_rate) 반환.
/// price-detail(HHDFS76200200)은 t_rate(당일환율)를 포함하므로 환율 캐싱에 활용.
pub async fn get_price(ctx: &ActorContext, exchange: &str, symbol: &str) -> Result<(PriceData, Option<f64>)> {
    let url = format!(
        "{}/uapi/overseas-price/v1/quotations/price-detail",
        ctx.config.base_url
    );

    let status;
    let body;
    {
        let http_resp = ctx
            .client
            .get(&url)
            .headers(ctx.common_headers("HHDFS76200200")?)
            .query(&[("AUTH", ""), ("EXCD", exchange), ("SYMB", symbol)])
            .send()
            .await
            .context("Overseas price request failed")?;
        status = http_resp.status();
        body = http_resp.text().await.context("Overseas price body read failed")?;
    }

    let resp: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("Overseas price parse failed (HTTP {status}): {body}")
    })?;

    let output = &resp["output"];
    let t_rate = output["t_rate"].as_str().and_then(|s| s.parse::<f64>().ok());

    Ok((
        PriceData {
            name: output["name"].as_str().unwrap_or("").to_string(),
            current_price: parse_f64(output["last"].as_str()),
            // price-detail에는 "rate" 없음. t_xrat = 원환산당일등락(%)
            change_pct: parse_f64(output["t_xrat"].as_str()),
        },
        t_rate,
    ))
}

fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}
