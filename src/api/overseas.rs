use anyhow::{Context, Result};

use crate::models::messages::PriceData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, exchange: &str, symbol: &str) -> Result<PriceData> {
    let url = format!(
        "{}/uapi/overseas-price/v1/quotations/price",
        ctx.config.base_url
    );

    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("HHDFS00000300")?)
        .query(&[("AUTH", ""), ("EXCD", exchange), ("SYMB", symbol)])
        .send()
        .await
        .context("Overseas price request failed")?
        .json()
        .await
        .context("Overseas price parse failed")?;

    let output = &resp["output"];
    Ok(PriceData {
        name: output["name"].as_str().unwrap_or("").to_string(),
        current_price: parse_f64(output["last"].as_str()),
        change: parse_f64(output["diff"].as_str()),
        change_pct: parse_f64(output["rate"].as_str()),
        volume: parse_u64(output["tvol"].as_str()),
    })
}


fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}

fn parse_u64(s: Option<&str>) -> u64 {
    s.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
}
