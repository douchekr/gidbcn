use anyhow::{Context, Result};

use crate::models::messages::{DailyCandle, PriceData};

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

pub async fn get_daily_chart(
    ctx: &ActorContext,
    exchange: &str,
    symbol: &str,
    to_date: &str,
) -> Result<Vec<DailyCandle>> {
    let url = format!(
        "{}/uapi/overseas-price/v1/quotations/dailyprice",
        ctx.config.base_url
    );

    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("HHDFS76240000")?)
        .query(&[
            ("AUTH", ""),
            ("EXCD", exchange),
            ("SYMB", symbol),
            ("GUBN", "0"),
            ("BYMD", to_date),
            ("MODP", "1"),
        ])
        .send()
        .await
        .context("Overseas daily chart request failed")?
        .json()
        .await
        .context("Overseas daily chart parse failed")?;

    let items = resp["output2"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let candles = items
        .iter()
        .filter_map(|item| {
            let date = item["xymd"].as_str()?.to_string();
            if date.is_empty() {
                return None;
            }
            Some(DailyCandle {
                date,
                open: parse_f64(item["open"].as_str()),
                high: parse_f64(item["high"].as_str()),
                low: parse_f64(item["low"].as_str()),
                close: parse_f64(item["clos"].as_str()),
                volume: parse_u64(item["tvol"].as_str()),
            })
        })
        .collect();

    Ok(candles)
}

fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}

fn parse_u64(s: Option<&str>) -> u64 {
    s.and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
}
