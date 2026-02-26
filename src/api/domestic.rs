use anyhow::{Context, Result};

use crate::models::messages::PriceData;

use super::actor::ActorContext;

pub async fn get_price(ctx: &ActorContext, symbol: &str) -> Result<PriceData> {
    // inquire-price 엔드포인트에는 hts_kor_isnm(종목명) 필드가 없음.
    // inquire-daily-itemchartprice의 output1은 현재가 + 종목명을 모두 포함.
    let url = format!(
        "{}/uapi/domestic-stock/v1/quotations/inquire-daily-itemchartprice",
        ctx.config.base_url
    );

    let kst = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
    let today = chrono::Utc::now()
        .with_timezone(&kst)
        .format("%Y%m%d")
        .to_string();

    let resp: serde_json::Value = ctx
        .client
        .get(&url)
        .headers(ctx.common_headers("FHKST03010100")?)
        .query(&[
            ("FID_COND_MRKT_DIV_CODE", "J"),
            ("FID_INPUT_ISCD", symbol),
            ("FID_INPUT_DATE_1", today.as_str()),
            ("FID_INPUT_DATE_2", today.as_str()),
            ("FID_PERIOD_DIV_CODE", "D"),
            ("FID_ORG_ADJ_PRC", "0"),
        ])
        .send()
        .await
        .context("Domestic price request failed")?
        .json()
        .await
        .context("Domestic price parse failed")?;

    let output = &resp["output1"];
    Ok(PriceData {
        name: output["hts_kor_isnm"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        current_price: parse_f64(output["stck_prpr"].as_str()),
        change_pct: parse_f64(output["prdy_ctrt"].as_str()),
    })
}


fn parse_f64(s: Option<&str>) -> f64 {
    s.and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0)
}
