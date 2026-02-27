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

fn fmt_num(v: f64) -> String {
    let n = v as i64;
    let neg = n < 0;
    let s = n.unsigned_abs().to_string();
    let len = s.len();
    let mut result = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    if neg { format!("-{result}") } else { result }
}

impl Condition {
    pub fn display_description(&self) -> String {
        match self {
            Condition::PriceAbove { target } => format!("가격 ≥ {}", fmt_num(*target)),
            Condition::PriceBelow { target } => format!("가격 ≤ {}", fmt_num(*target)),
            Condition::ProfitAbove { percentage } => format!("수익률 ≥ {percentage}%"),
            Condition::ProfitBelow { percentage } => format!("수익률 ≤ {percentage}%"),
        }
    }
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
    fn condition_display_description() {
        assert_eq!(
            Condition::PriceAbove { target: 80000.0 }.display_description(),
            "가격 ≥ 80,000"
        );
        assert_eq!(
            Condition::ProfitBelow { percentage: -10.0 }.display_description(),
            "수익률 ≤ -10%"
        );
    }

    #[test]
    fn signal_store_default_empty() {
        let store = SignalStore::default();
        assert!(store.signals.is_empty());
    }
}
