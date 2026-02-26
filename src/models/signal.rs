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

impl Condition {
    pub fn type_name(&self) -> &str {
        match self {
            Condition::PriceAbove { .. } => "price_above",
            Condition::PriceBelow { .. } => "price_below",
            Condition::ProfitAbove { .. } => "profit_above",
            Condition::ProfitBelow { .. } => "profit_below",
        }
    }

    pub fn display_description(&self) -> String {
        match self {
            Condition::PriceAbove { target } => format!("가격 ≥ {target}"),
            Condition::PriceBelow { target } => format!("가격 ≤ {target}"),
            Condition::ProfitAbove { percentage } => format!("수익률 ≥ {percentage}%"),
            Condition::ProfitBelow { percentage } => format!("수익률 ≤ {percentage}%"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    pub symbol: String,
    pub condition: Condition,
    pub active: bool,
    pub created_at: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalStore {
    pub next_id: u64,
    pub signals: Vec<Signal>,
}

impl Default for SignalStore {
    fn default() -> Self {
        Self {
            next_id: 1,
            signals: Vec::new(),
        }
    }
}

impl SignalStore {
    pub fn next_signal_id(&mut self) -> String {
        let id = format!("s_{:03}", self.next_id);
        self.next_id += 1;
        id
    }
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
            "가격 ≥ 80000"
        );
        assert_eq!(
            Condition::ProfitBelow { percentage: -10.0 }.display_description(),
            "수익률 ≤ -10%"
        );
    }

    #[test]
    fn next_signal_id_increments() {
        let mut store = SignalStore::default();
        assert_eq!(store.next_signal_id(), "s_001");
        assert_eq!(store.next_signal_id(), "s_002");
    }
}
