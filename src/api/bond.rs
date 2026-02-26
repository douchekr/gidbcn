use anyhow::{Context, Result};

use crate::models::messages::BondData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, isin: &str) -> Result<BondData> {
    let url = format!(
        "{}/uapi/domestic-bond/v1/quotations/inquire-price",
        ctx.config.base_url
    );

    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("FHKBJ773000C0")?)
        .query(&[
            ("FID_COND_MRKT_DIV_CODE", "B"),
            ("FID_INPUT_ISCD", isin),
        ])
        .send()
        .await
        .context("Bond price request failed")?
        .json()
        .await
        .context("Bond price parse failed")?;

    let output = &resp["output"];
    Ok(BondData {
        current_price: parse_f64(output["bond_prpr"].as_str()),
    })
}

fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}
