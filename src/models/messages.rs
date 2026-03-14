use anyhow::Result;
use tokio::sync::oneshot;


#[derive(Debug)]
pub struct PriceData {
    pub name: String,
    pub current_price: f64,
    pub change_pct: f64,
}

#[derive(Debug)]
pub struct BondData {
    pub name: String,
    pub current_price: f64,
    pub change_pct: f64,
}

/// 해외주식 상세 (워치리스트 평가용)
#[derive(Debug, Clone)]
pub struct OverseasDetail {
    pub name: String,
    pub current_price: f64,
    pub change_pct: f64,
    pub market_cap: f64,
    pub per: f64,
    pub pbr: f64,
    pub eps: f64,
    pub bps: f64,
    pub shares: f64,
    pub volume: f64,
    pub volume_amount: f64,
    pub high_52w: f64,
    pub low_52w: f64,
    pub sector: String,
    pub prev_volume: f64,
}

pub enum ApiRequest {
    GetDomesticPrice {
        symbol: String,
        respond_to: oneshot::Sender<Result<PriceData>>,
    },
    GetOverseasPrice {
        exchange: String,
        symbol: String,
        respond_to: oneshot::Sender<Result<PriceData>>,
    },
    GetBondPrice {
        isin: String,
        respond_to: oneshot::Sender<Result<BondData>>,
    },
    GetExchangeRate {
        respond_to: oneshot::Sender<Result<f64>>,
    },
    GetStockName {
        prdt_type_cd: String,
        pdno: String,
        respond_to: oneshot::Sender<Result<String>>,
    },
    GetOverseasDetail {
        exchange: String,
        symbol: String,
        respond_to: oneshot::Sender<Result<OverseasDetail>>,
    },
}
