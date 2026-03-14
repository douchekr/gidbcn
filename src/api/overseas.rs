use anyhow::{Context, Result};

use crate::models::messages::{OverseasDetail, PriceData};

use super::actor::ActorContext;

/// (현재가 데이터, 당일환율 t_rate) 반환.
/// price-detail(HHDFS76200200)은 t_rate(당일환율)를 포함하므로 환율 캐싱에 활용.
pub async fn get_price(ctx: &ActorContext, exchange: &str, symbol: &str) -> Result<(PriceData, Option<f64>)> {
    let url = format!(
        "{}/uapi/overseas-price/v1/quotations/price-detail",
        ctx.config.base_url
    );

    let http_resp = ctx
        .send_with_retry(
            ctx.client
                .get(&url)
                .headers(ctx.common_headers("HHDFS76200200")?)
                .query(&[("AUTH", ""), ("EXCD", exchange), ("SYMB", symbol)]),
        )
        .await
        .context("Overseas price request failed")?;
    let status = http_resp.status();
    let body = http_resp.text().await.context("Overseas price body read failed")?;

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

/// 해외주식 상세 — 동일 엔드포인트에서 전체 필드 파싱
pub async fn get_detail(ctx: &ActorContext, exchange: &str, symbol: &str) -> Result<OverseasDetail> {
    let url = format!(
        "{}/uapi/overseas-price/v1/quotations/price-detail",
        ctx.config.base_url
    );

    let http_resp = ctx
        .send_with_retry(
            ctx.client
                .get(&url)
                .headers(ctx.common_headers("HHDFS76200200")?)
                .query(&[("AUTH", ""), ("EXCD", exchange), ("SYMB", symbol)]),
        )
        .await
        .context("Overseas detail request failed")?;
    let status = http_resp.status();
    let body = http_resp.text().await.context("Overseas detail body read failed")?;

    let resp: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("Overseas detail parse failed (HTTP {status}): {body}")
    })?;

    let o = &resp["output"];

    Ok(OverseasDetail {
        name: o["name"].as_str().unwrap_or("").to_string(),
        current_price: parse_f64(o["last"].as_str()),
        change_pct: parse_f64(o["t_xrat"].as_str()),
        market_cap: parse_f64(o["tomv"].as_str()),
        per: parse_f64(o["perx"].as_str()),
        pbr: parse_f64(o["pbrx"].as_str()),
        eps: parse_f64(o["epsx"].as_str()),
        bps: parse_f64(o["bpsx"].as_str()),
        shares: parse_f64(o["shar"].as_str()),
        volume: parse_f64(o["tvol"].as_str()),
        volume_amount: parse_f64(o["tamt"].as_str()),
        high_52w: parse_f64(o["h52p"].as_str()),
        low_52w: parse_f64(o["l52p"].as_str()),
        sector: o["e_icod"].as_str().unwrap_or("").to_string(),
        prev_volume: parse_f64(o["pvol"].as_str()),
    })
}

fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}
