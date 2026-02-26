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
}
