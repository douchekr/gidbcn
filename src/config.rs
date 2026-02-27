use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub kis_api: KisApiConfig,
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub log: LogConfig,
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
    /// 봇 오너의 텔레그램 chat_id. 0이면 미설정 (봇이 chat_id 안내 후 차단).
    #[serde(default)]
    pub owner_chat_id: i64,
    /// 봇 사용을 허용할 추가 chat_id 목록 (owner 제외)
    #[serde(default)]
    pub users: Vec<i64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    /// 로그 파일 저장 디렉토리
    pub dir: String,
    /// 보관할 최대 일수 (일별 롤링, 이 일수를 초과한 파일 자동 삭제)
    pub retain_days: u32,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            dir: "/opt/kkuepark/gidbcn".to_string(),
            retain_days: 7,
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
