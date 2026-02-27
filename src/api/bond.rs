use anyhow::{Context, Result};

use crate::models::messages::BondData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, isin: &str) -> Result<BondData> {
    let url = format!(
        "{}/uapi/domestic-bond/v1/quotations/inquire-price",
        ctx.config.base_url
    );

    let http_resp = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("FHKBJ773400C0")?)
        .query(&[
            ("FID_COND_MRKT_DIV_CODE", "B"),
            ("FID_INPUT_ISCD", isin),
        ])
        .send()
        .await
        .context("Bond price request failed")?;

    let status = http_resp.status();
    let body = http_resp.text().await.context("Bond price body read failed")?;
    let resp: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("Bond price parse failed (HTTP {status}): {body}")
    })?;

    let output = &resp["output"];
    Ok(BondData {
        name: output["hts_kor_isnm"].as_str().unwrap_or("").to_string(),
        current_price: parse_f64(output["bond_prpr"].as_str()),
        change_pct: parse_f64(output["prdy_ctrt"].as_str()),
    })
}

fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}
