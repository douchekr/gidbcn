use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub kis_api: KisApiConfig,
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub exchange_rate: ExchangeRateCache,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KisApiConfig {
    pub app_key: String,
    pub app_secret: String,
    pub base_url: String,
    pub hts_id: String,
    #[serde(default)]
    pub token: Option<TokenInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub access_token: String,
    pub expires_at: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub chat_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRateCache {
    pub usd_krw: f64,
    pub updated_at: Option<DateTime<FixedOffset>>,
}

impl Default for ExchangeRateCache {
    fn default() -> Self {
        Self {
            usd_krw: 1350.0,
            updated_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub signal_check_interval_minutes: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            signal_check_interval_minutes: 5,
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {path}"))?;
        let config: Config =
            serde_json::from_str(&data).with_context(|| "Failed to parse config JSON")?;
        Ok(config)
    }

    pub fn save(&self, path: &str) -> Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)
            .with_context(|| format!("Failed to write config: {path}"))?;
        Ok(())
    }
}
