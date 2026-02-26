use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Market {
    KRX,
    NAS,
    NYS,
    AMS,
    BOND,
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Market::KRX => write!(f, "KRX"),
            Market::NAS => write!(f, "NAS"),
            Market::NYS => write!(f, "NYS"),
            Market::AMS => write!(f, "AMS"),
            Market::BOND => write!(f, "BOND"),
        }
    }
}

impl Market {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "KRX" => Some(Market::KRX),
            "NAS" => Some(Market::NAS),
            "NYS" => Some(Market::NYS),
            "AMS" => Some(Market::AMS),
            "BOND" => Some(Market::BOND),
            _ => None,
        }
    }

    pub fn is_domestic(&self) -> bool {
        matches!(self, Market::KRX | Market::BOND)
    }

    pub fn exchange_code(&self) -> &str {
        match self {
            Market::NAS => "NAS",
            Market::NYS => "NYS",
            Market::AMS => "AMS",
            _ => "",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holding {
    pub id: String,
    pub market: Market,
    pub symbol: String,
    pub quantity: f64,
    pub avg_price: f64,
    pub added_at: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioStore {
    pub next_id: u64,
    pub holdings: Vec<Holding>,
}

impl Default for PortfolioStore {
    fn default() -> Self {
        Self {
            next_id: 1,
            holdings: Vec::new(),
        }
    }
}

impl PortfolioStore {
    pub fn next_holding_id(&mut self) -> String {
        let id = format!("h_{:03}", self.next_id);
        self.next_id += 1;
        id
    }
}
