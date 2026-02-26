use anyhow::Result;
use tokio::sync::oneshot;

use super::portfolio::Market;

#[derive(Debug)]
pub struct PriceData {
    pub name: String,
    pub current_price: f64,
    pub change: f64,
    pub change_pct: f64,
    pub volume: u64,
}

#[derive(Debug)]
pub struct DailyCandle {
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
}

#[derive(Debug)]
pub struct BondData {
    pub current_price: f64,
    pub yield_rate: f64,
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
    GetDailyChart {
        market: Market,
        symbol: String,
        respond_to: oneshot::Sender<Result<Vec<DailyCandle>>>,
    },
    GetBondPrice {
        isin: String,
        respond_to: oneshot::Sender<Result<BondData>>,
    },
    GetExchangeRate {
        respond_to: oneshot::Sender<Result<f64>>,
    },
    RefreshToken,
}
