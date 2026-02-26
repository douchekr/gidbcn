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
    GoldenCross { short_period: u32, long_period: u32 },
    DeadCross { short_period: u32, long_period: u32 },
    RsiAbove { threshold: f64 },
    RsiBelow { threshold: f64 },
    VolumeSurge { threshold_pct: f64 },
}

impl Condition {
    pub fn type_name(&self) -> &str {
        match self {
            Condition::PriceAbove { .. } => "price_above",
            Condition::PriceBelow { .. } => "price_below",
            Condition::ProfitAbove { .. } => "profit_above",
            Condition::ProfitBelow { .. } => "profit_below",
            Condition::GoldenCross { .. } => "golden_cross",
            Condition::DeadCross { .. } => "dead_cross",
            Condition::RsiAbove { .. } => "rsi_above",
            Condition::RsiBelow { .. } => "rsi_below",
            Condition::VolumeSurge { .. } => "volume_surge",
        }
    }

    pub fn needs_daily_chart(&self) -> bool {
        matches!(
            self,
            Condition::GoldenCross { .. }
                | Condition::DeadCross { .. }
                | Condition::RsiAbove { .. }
                | Condition::RsiBelow { .. }
                | Condition::VolumeSurge { .. }
        )
    }

    pub fn display_description(&self) -> String {
        match self {
            Condition::PriceAbove { target } => format!("가격 ≥ {target}"),
            Condition::PriceBelow { target } => format!("가격 ≤ {target}"),
            Condition::ProfitAbove { percentage } => format!("수익률 ≥ {percentage}%"),
            Condition::ProfitBelow { percentage } => format!("수익률 ≤ {percentage}%"),
            Condition::GoldenCross {
                short_period,
                long_period,
            } => format!("골든크로스 (MA{short_period}/{long_period})"),
            Condition::DeadCross {
                short_period,
                long_period,
            } => format!("데드크로스 (MA{short_period}/{long_period})"),
            Condition::RsiAbove { threshold } => format!("RSI ≥ {threshold}"),
            Condition::RsiBelow { threshold } => format!("RSI ≤ {threshold}"),
            Condition::VolumeSurge { threshold_pct } => {
                format!("거래량 ≥ 20일평균×{threshold_pct}%")
            }
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
    fn condition_serde_golden_cross() {
        let cond = Condition::GoldenCross {
            short_period: 5,
            long_period: 20,
        };
        let json = serde_json::to_string(&cond).unwrap();
        let parsed: Condition = serde_json::from_str(&json).unwrap();
        match parsed {
            Condition::GoldenCross {
                short_period,
                long_period,
            } => {
                assert_eq!(short_period, 5);
                assert_eq!(long_period, 20);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn condition_needs_daily_chart() {
        assert!(!Condition::PriceAbove { target: 100.0 }.needs_daily_chart());
        assert!(!Condition::ProfitBelow { percentage: -5.0 }.needs_daily_chart());
        assert!(Condition::GoldenCross { short_period: 5, long_period: 20 }.needs_daily_chart());
        assert!(Condition::RsiAbove { threshold: 70.0 }.needs_daily_chart());
        assert!(Condition::VolumeSurge { threshold_pct: 200.0 }.needs_daily_chart());
    }

    #[test]
    fn condition_display_description() {
        assert_eq!(
            Condition::PriceAbove { target: 80000.0 }.display_description(),
            "가격 ≥ 80000"
        );
        assert_eq!(
            Condition::RsiBelow { threshold: 30.0 }.display_description(),
            "RSI ≤ 30"
        );
    }

    #[test]
    fn next_signal_id_increments() {
        let mut store = SignalStore::default();
        assert_eq!(store.next_signal_id(), "s_001");
        assert_eq!(store.next_signal_id(), "s_002");
    }
}
