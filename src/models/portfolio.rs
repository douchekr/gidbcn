use chrono::{DateTime, FixedOffset, Timelike, Utc};
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

    /// 현재 KST 기준으로 해당 마켓이 장중인지 확인
    pub fn is_open_now(&self) -> bool {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = Utc::now().with_timezone(&kst);
        let hhmm = now.hour() * 100 + now.minute();
        match self {
            // KRX 채권: 09:00~15:30 KST
            Market::KRX | Market::BOND => hhmm >= 900 && hhmm <= 1530,
            // 미국 3개 거래소: 22:30~05:00 KST (자정 넘김)
            Market::NAS | Market::NYS | Market::AMS => hhmm >= 2230 || hhmm <= 500,
        }
    }

    pub fn exchange_code(&self) -> &str {
        match self {
            Market::NAS => "NAS",
            Market::NYS => "NYS",
            Market::AMS => "AMS",
            _ => "",
        }
    }

    /// 상품기본조회(CTPF1604R) PRDT_TYPE_CD
    pub fn product_type_code(&self) -> &str {
        match self {
            Market::KRX  => "300",
            Market::NAS  => "512",
            Market::NYS  => "513",
            Market::AMS  => "529",
            Market::BOND => "302",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holding {
    pub market: Market,
    pub symbol: String,
    #[serde(default)]
    pub name: String,
    pub quantity: f64,
    pub avg_price: f64,
    pub added_at: DateTime<FixedOffset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_at: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PortfolioStore {
    pub holdings: Vec<Holding>,
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
    fn market_exchange_code() {
        assert_eq!(Market::NAS.exchange_code(), "NAS");
        assert_eq!(Market::NYS.exchange_code(), "NYS");
        assert_eq!(Market::AMS.exchange_code(), "AMS");
        assert_eq!(Market::KRX.exchange_code(), "");
    }

    #[test]
    fn portfolio_serde_roundtrip() {
        let store = PortfolioStore {
            holdings: vec![Holding {
                market: Market::KRX,
                symbol: "005930".into(),
                name: "삼성전자".into(),
                quantity: 10.0,
                avg_price: 70000.0,
                added_at: chrono::Utc::now()
                    .with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap()),
                cached_price: None,
                cached_at: None,
            }],
        };
        let json = serde_json::to_string(&store).unwrap();
        let parsed: PortfolioStore = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.holdings.len(), 1);
        assert_eq!(parsed.holdings[0].symbol, "005930");
        assert_eq!(parsed.holdings[0].market, Market::KRX);
    }
}
