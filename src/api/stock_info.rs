use anyhow::{Context, Result};

use super::actor::ActorContext;

/// 상품기본조회 (CTPF1604R) — 종목명(prdt_abrv_name) 조회
/// PRDT_TYPE_CD: 300=KRX, 512=NAS, 513=NYS, 529=AMS, 302=BOND
pub async fn get_stock_name(ctx: &ActorContext, prdt_type_cd: &str, pdno: &str) -> Result<String> {
    let url = format!(
        "{}/uapi/domestic-stock/v1/quotations/search-info",
        ctx.config.base_url
    );

    let http_resp = ctx
        .send_with_retry(
            ctx.client
                .get(&url)
                .headers(ctx.common_headers("CTPF1604R")?)
                .query(&[("PRDT_TYPE_CD", prdt_type_cd), ("PDNO", pdno)]),
        )
        .await
        .context("Stock info request failed")?;
    let body = http_resp.text().await.context("Stock info body read failed")?;
    let resp: serde_json::Value =
        serde_json::from_str(&body).context("Stock info parse failed")?;

    let name = resp["output"]["prdt_abrv_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    Ok(name)
}
