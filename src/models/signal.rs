use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    PriceAbove { target: f64 },
    PriceBelow { target: f64 },
    ProfitAbove { percentage: f64 },
    ProfitBelow { percentage: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    pub symbol: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub account: String,
    pub condition: Condition,
    pub active: bool,
    pub created_at: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignalStore {
    pub signals: Vec<Signal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_serde_roundtrip() {
        let cond = Condition::PriceAbove { target: 80000.0 };
        let json = serde_json::to_string(&cond).unwrap();
        assert!(json.contains("price_above"));
        let parsed: Condition = serde_json::from_str(&json).unwrap();
        match parsed {
            Condition::PriceAbove { target } => assert_eq!(target, 80000.0),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn signal_store_default_empty() {
        let store = SignalStore::default();
        assert!(store.signals.is_empty());
    }
}
