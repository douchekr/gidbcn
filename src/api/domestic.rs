use anyhow::{Context, Result};

use crate::models::messages::PriceData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, symbol: &str) -> Result<PriceData> {
    // inquire-daily-itemchartprice(FHKST03010100)는 장마감 후 이상동작 확인 → 미사용.
    // inquire-price(FHKST01010100): 날짜 파라미터 불필요, 장 내외 모두 안정적으로 동작.
    // 종목명(hts_kor_isnm) 미포함 → Holding.name 캐시 사용.
    let url = format!(
        "{}/uapi/domestic-stock/v1/quotations/inquire-price",
        ctx.config.base_url
    );

    let http_resp = ctx
        .send_with_retry(
            ctx.client
                .get(&url)
                .headers(ctx.common_headers("FHKST01010100")?)
                .query(&[("FID_COND_MRKT_DIV_CODE", "J"), ("FID_INPUT_ISCD", symbol)]),
        )
        .await
        .context("Domestic price request failed")?;

    let status = http_resp.status();
    let body = http_resp.text().await.context("Domestic price body read failed")?;

    let _resp: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("Domestic price parse failed (HTTP {status}): {body}")
    })?;

    tracing::debug!("Domestic price ({symbol}) HTTP {status}: {body}");

    parse_price_response(&body)
}

pub fn parse_price_response(body: &str) -> Result<PriceData> {
    let resp: serde_json::Value =
        serde_json::from_str(body).context("Domestic price JSON parse failed")?;
    let output = &resp["output"];
    Ok(PriceData {
        name: String::new(),
        current_price: parse_f64(output["stck_prpr"].as_str()),
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
                "stck_prpr": "72500",
                "prdy_ctrt": "-1.23"
            },
            "rt_cd": "0",
            "msg_cd": "MCA00000",
            "msg1": "정상처리 되었습니다."
        }"#;
        let data = parse_price_response(body).unwrap();
        assert_eq!(data.current_price, 72500.0);
        assert_eq!(data.change_pct, -1.23);
    }

    #[test]
    fn parse_empty_output() {
        let body = r#"{"output": {}, "rt_cd": "0"}"#;
        let data = parse_price_response(body).unwrap();
        assert_eq!(data.current_price, 0.0);
        assert_eq!(data.change_pct, 0.0);
    }

    #[test]
    fn parse_invalid_json_fails() {
        assert!(parse_price_response("not json").is_err());
    }

    #[test]
    fn parse_error_response() {
        // API 에러 시에도 output이 있을 수 있음
        let body = r#"{
            "output": null,
            "rt_cd": "1",
            "msg_cd": "EGW00123",
            "msg1": "종목코드 오류"
        }"#;
        let data = parse_price_response(body).unwrap();
        assert_eq!(data.current_price, 0.0);
    }
}
