use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRecord {
    pub id: String,
    pub signal_id: String,
    pub symbol: String,
    pub condition_type: String,
    pub trigger_value: f64,
    pub message: String,
    pub sent_at: DateTime<FixedOffset>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertLog {
    pub next_id: u64,
    pub alerts: Vec<AlertRecord>,
}

impl Default for AlertLog {
    fn default() -> Self {
        Self {
            next_id: 1,
            alerts: Vec::new(),
        }
    }
}

impl AlertLog {
    pub fn next_alert_id(&mut self) -> String {
        let id = format!("a_{:03}", self.next_id);
        self.next_id += 1;
        id
    }
}
