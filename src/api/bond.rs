use anyhow::{Context, Result};

use crate::models::messages::BondData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, isin: &str) -> Result<BondData> {
    let url = format!(
        "{}/uapi/domestic-bond/v1/quotations/inquire-price",
        ctx.config.base_url
    );

    let http_resp = ctx
        .send_with_retry(
            ctx.client
                .get(&url)
                .headers(ctx.common_headers("FHKBJ773400C0")?)
                .query(&[("FID_COND_MRKT_DIV_CODE", "B"), ("FID_INPUT_ISCD", isin)]),
        )
        .await
        .context("Bond price request failed")?;

    let status = http_resp.status();
    let body = http_resp.text().await.context("Bond price body read failed")?;
    let _resp: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("Bond price parse failed (HTTP {status}): {body}")
    })?;

    parse_price_response(&body)
}

pub fn parse_price_response(body: &str) -> Result<BondData> {
    let resp: serde_json::Value =
        serde_json::from_str(body).context("Bond price JSON parse failed")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normal_response() {
        let body = r#"{
            "output": {
                "hts_kor_isnm": "국고03250-2512",
                "bond_prpr": "7485",
                "prdy_ctrt": "0.12"
            },
            "rt_cd": "0"
        }"#;
        let data = parse_price_response(body).unwrap();
        assert_eq!(data.name, "국고03250-2512");
        assert_eq!(data.current_price, 7485.0);
        assert_eq!(data.change_pct, 0.12);
    }

    #[test]
    fn parse_empty_output() {
        let body = r#"{"output": {}, "rt_cd": "0"}"#;
        let data = parse_price_response(body).unwrap();
        assert_eq!(data.name, "");
        assert_eq!(data.current_price, 0.0);
    }

    #[test]
    fn parse_invalid_json_fails() {
        assert!(parse_price_response("{broken").is_err());
    }
}
