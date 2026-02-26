use anyhow::{Context, Result};

use crate::models::messages::{DailyCandle, PriceData};

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, symbol: &str) -> Result<PriceData> {
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
        name: output["hts_kor_isnm"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        current_price: parse_f64(output["stck_prpr"].as_str()),
        change: parse_f64(output["prdy_vrss"].as_str()),
        change_pct: parse_f64(output["prdy_ctrt"].as_str()),
        volume: parse_u64(output["acml_vol"].as_str()),
    })
}

pub async fn get_daily_chart(
    ctx: &ActorContext,
    symbol: &str,
    from: &str,
    to: &str,
) -> Result<Vec<DailyCandle>> {
    let url = format!(
        "{}/uapi/domestic-stock/v1/quotations/inquire-daily-itemchartprice",
        ctx.config.base_url
    );

    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("FHKST03010100")?)
        .query(&[
            ("FID_COND_MRKT_DIV_CODE", "J"),
            ("FID_INPUT_ISCD", symbol),
            ("FID_INPUT_DATE_1", from),
            ("FID_INPUT_DATE_2", to),
            ("FID_PERIOD_DIV_CODE", "D"),
            ("FID_ORG_ADJ_PRC", "0"),
        ])
        .send()
        .await
        .context("Domestic daily chart request failed")?
        .json()
        .await
        .context("Domestic daily chart parse failed")?;

    let items = resp["output2"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let candles = items
        .iter()
        .filter_map(|item| {
            let date = item["stck_bsop_date"].as_str()?.to_string();
            if date.is_empty() {
                return None;
            }
            Some(DailyCandle {
                date,
                open: parse_f64(item["stck_oprc"].as_str()),
                high: parse_f64(item["stck_hgpr"].as_str()),
                low: parse_f64(item["stck_lwpr"].as_str()),
                close: parse_f64(item["stck_clpr"].as_str()),
                volume: parse_u64(item["acml_vol"].as_str()),
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
