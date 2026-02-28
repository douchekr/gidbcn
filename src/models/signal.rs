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
    fn condition_all_variants_serde() {
        let cases = vec![
            (Condition::PriceAbove { target: 100.0 }, "price_above"),
            (Condition::PriceBelow { target: 50.0 }, "price_below"),
            (Condition::ProfitAbove { percentage: 10.0 }, "profit_above"),
            (Condition::ProfitBelow { percentage: -5.0 }, "profit_below"),
        ];
        for (cond, expected_tag) in cases {
            let json = serde_json::to_string(&cond).unwrap();
            assert!(json.contains(expected_tag), "missing tag {expected_tag} in {json}");
            let parsed: Condition = serde_json::from_str(&json).unwrap();
            let re_json = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, re_json);
        }
    }

    #[test]
    fn signal_serde_with_account() {
        let kst = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let sig = Signal {
            id: "abc".into(),
            symbol: "005930".into(),
            account: "IRP".into(),
            condition: Condition::PriceAbove { target: 80000.0 },
            active: true,
            created_at: chrono::Utc::now().with_timezone(&kst),
        };
        let json = serde_json::to_string(&sig).unwrap();
        assert!(json.contains("\"account\":\"IRP\""));
        let parsed: Signal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.account, "IRP");
    }

    #[test]
    fn signal_serde_empty_account_skipped() {
        let kst = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let sig = Signal {
            id: "abc".into(),
            symbol: "005930".into(),
            account: String::new(),
            condition: Condition::PriceBelow { target: 60000.0 },
            active: false,
            created_at: chrono::Utc::now().with_timezone(&kst),
        };
        let json = serde_json::to_string(&sig).unwrap();
        // empty account → JSON에서 생략
        assert!(!json.contains("account"));
        // 역직렬화 시 default로 복원
        let parsed: Signal = serde_json::from_str(&json).unwrap();
        assert!(parsed.account.is_empty());
        assert!(!parsed.active);
    }

    #[test]
    fn signal_store_default_empty() {
        let store = SignalStore::default();
        assert!(store.signals.is_empty());
    }
}
