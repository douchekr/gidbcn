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
    let _status = http_resp.status();
    let body = http_resp.text().await.context("Overseas price body read failed")?;

    parse_price_response(&body)
}

pub fn parse_price_response(body: &str) -> Result<(PriceData, Option<f64>)> {
    let resp: serde_json::Value =
        serde_json::from_str(body).context("Overseas price JSON parse failed")?;
    let output = &resp["output"];
    let t_rate = output["t_rate"].as_str().and_then(|s| s.parse::<f64>().ok());
    Ok((
        PriceData {
            name: output["name"].as_str().unwrap_or("").to_string(),
            current_price: parse_f64(output["last"].as_str()),
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
    let _status = http_resp.status();
    let body = http_resp.text().await.context("Overseas detail body read failed")?;

    parse_detail_response(&body)
}

pub fn parse_detail_response(body: &str) -> Result<OverseasDetail> {
    let resp: serde_json::Value =
        serde_json::from_str(body).context("Overseas detail JSON parse failed")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // 한투 API 스펙 문서의 TSLA 응답 예시
    const TSLA_RESPONSE: &str = r#"{
        "output": {
            "rsym": "DNASTSLA",
            "zdiv": "4",
            "curr": "USD",
            "vnit": "1",
            "open": "257.2600",
            "high": "259.0794",
            "low": "242.0100",
            "last": "245.0100",
            "base": "258.0800",
            "pvol": "108861698",
            "pamt": "28090405673",
            "uplp": "0.0000",
            "dnlp": "0.0000",
            "h52p": "313.8000",
            "h52d": "20220921",
            "l52p": "101.8100",
            "l52d": "20230106",
            "perx": "69.51",
            "pbrx": "15.21",
            "epsx": "3.52",
            "bpsx": "16.11",
            "shar": "3173990000",
            "mcap": "3000000",
            "tomv": "777659289900",
            "t_xprc": "323658",
            "t_xdif": "17265",
            "t_xrat": "-5.06",
            "p_xprc": "0",
            "p_xdif": "0",
            "p_xrat": " 0.00",
            "t_rate": "1321.00",
            "p_rate": "",
            "t_xsgn": "5",
            "p_xsng": "3",
            "e_ordyn": "매매 가능",
            "e_hogau": "0.0100",
            "e_icod": "자동차",
            "e_parp": "0.0000",
            "tvol": "132541640",
            "tamt": "32907071789",
            "etyp_nm": "",
            "name": "TESLA INC"
        },
        "rt_cd": "0",
        "msg_cd": "MCA00000",
        "msg1": "정상처리 되었습니다."
    }"#;

    #[test]
    fn parse_price_from_spec_example() {
        let (price, t_rate) = parse_price_response(TSLA_RESPONSE).unwrap();
        assert_eq!(price.name, "TESLA INC");
        assert_eq!(price.current_price, 245.01);
        assert_eq!(price.change_pct, -5.06);
        assert_eq!(t_rate, Some(1321.0));
    }

    #[test]
    fn parse_detail_from_spec_example() {
        let detail = parse_detail_response(TSLA_RESPONSE).unwrap();
        assert_eq!(detail.name, "TESLA INC");
        assert_eq!(detail.current_price, 245.01);
        assert_eq!(detail.per, 69.51);
        assert_eq!(detail.pbr, 15.21);
        assert_eq!(detail.eps, 3.52);
        assert_eq!(detail.bps, 16.11);
        assert_eq!(detail.shares, 3173990000.0);
        assert_eq!(detail.market_cap, 777659289900.0);
        assert_eq!(detail.volume, 132541640.0);
        assert_eq!(detail.prev_volume, 108861698.0);
        assert_eq!(detail.high_52w, 313.80);
        assert_eq!(detail.low_52w, 101.81);
        assert_eq!(detail.sector, "자동차");
    }

    #[test]
    fn parse_price_empty_output() {
        let body = r#"{"output": {}, "rt_cd": "0"}"#;
        let (price, t_rate) = parse_price_response(body).unwrap();
        assert_eq!(price.current_price, 0.0);
        assert_eq!(price.name, "");
        assert!(t_rate.is_none());
    }

    #[test]
    fn parse_detail_empty_output() {
        let body = r#"{"output": {}, "rt_cd": "0"}"#;
        let detail = parse_detail_response(body).unwrap();
        assert_eq!(detail.current_price, 0.0);
        assert_eq!(detail.sector, "");
    }

    #[test]
    fn parse_invalid_json_fails() {
        assert!(parse_price_response("not json").is_err());
        assert!(parse_detail_response("").is_err());
    }

    #[test]
    fn parse_null_output() {
        let body = r#"{"output": null, "rt_cd": "1", "msg1": "종목코드 오류"}"#;
        let (price, _) = parse_price_response(body).unwrap();
        assert_eq!(price.current_price, 0.0);
    }
}
