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
    #[serde(default)]
    pub name: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_from_str() {
        assert_eq!(Market::from_str("KRX"), Some(Market::KRX));
        assert_eq!(Market::from_str("nas"), Some(Market::NAS));
        assert_eq!(Market::from_str("Nys"), Some(Market::NYS));
        assert_eq!(Market::from_str("BOND"), Some(Market::BOND));
        assert_eq!(Market::from_str("INVALID"), None);
    }

    #[test]
    fn market_is_domestic() {
        assert!(Market::KRX.is_domestic());
        assert!(Market::BOND.is_domestic());
        assert!(!Market::NAS.is_domestic());
        assert!(!Market::NYS.is_domestic());
        assert!(!Market::AMS.is_domestic());
    }

    #[test]
    fn market_exchange_code() {
        assert_eq!(Market::NAS.exchange_code(), "NAS");
        assert_eq!(Market::NYS.exchange_code(), "NYS");
        assert_eq!(Market::AMS.exchange_code(), "AMS");
        assert_eq!(Market::KRX.exchange_code(), "");
    }

    #[test]
    fn next_holding_id_increments() {
        let mut store = PortfolioStore::default();
        assert_eq!(store.next_holding_id(), "h_001");
        assert_eq!(store.next_holding_id(), "h_002");
        assert_eq!(store.next_holding_id(), "h_003");
    }

    #[test]
    fn portfolio_serde_roundtrip() {
        let store = PortfolioStore {
            next_id: 2,
            holdings: vec![Holding {
                id: "h_001".into(),
                market: Market::KRX,
                symbol: "005930".into(),
                name: "삼성전자".into(),
                quantity: 10.0,
                avg_price: 70000.0,
                added_at: chrono::Utc::now()
                    .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap()),
            }],
        };
        let json = serde_json::to_string(&store).unwrap();
        let parsed: PortfolioStore = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.next_id, 2);
        assert_eq!(parsed.holdings.len(), 1);
        assert_eq!(parsed.holdings[0].symbol, "005930");
        assert_eq!(parsed.holdings[0].market, Market::KRX);
    }
}
