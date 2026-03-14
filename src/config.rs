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
    #[serde(default)]
    pub watchlist: WatchlistConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistConfig {
    pub gemini_api_key: String,
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    #[serde(default = "default_max_gemini_calls")]
    pub max_gemini_calls_per_day: usize,
    #[serde(default = "default_candidate_count")]
    pub candidate_count: usize,
}

impl Default for WatchlistConfig {
    fn default() -> Self {
        Self {
            gemini_api_key: String::new(),
            gemini_model: default_gemini_model(),
            max_gemini_calls_per_day: default_max_gemini_calls(),
            candidate_count: default_candidate_count(),
        }
    }
}

fn default_gemini_model() -> String { "gemini-2.5-flash".to_string() }
fn default_max_gemini_calls() -> usize { 250 }
fn default_candidate_count() -> usize { 30 }

/// 암호화 대상 민감 설정
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secrets {
    pub kis_api: KisApiConfig,
    pub watchlist: WatchlistConfig,
}

/// 봇 기동용 최소 설정 (평문 부분만)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootConfig {
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub log: LogConfig,
    /// 암호화된 민감 설정 (base64). 없으면 평문 모드.
    #[serde(default)]
    pub encrypted_secrets: Option<String>,
    // 평문 모드 하위 호환: 암호화 전에는 이 필드들이 존재
    #[serde(default)]
    pub kis_api: Option<KisApiConfig>,
    #[serde(default)]
    pub watchlist: Option<WatchlistConfig>,
}

impl BootConfig {
    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {path}"))?;
        let config: BootConfig =
            serde_json::from_str(&data).with_context(|| "Failed to parse config JSON")?;
        Ok(config)
    }

    /// 암호화 모드인지 (encrypted_secrets 필드 존재)
    pub fn is_encrypted(&self) -> bool {
        self.encrypted_secrets.is_some()
    }

    /// 평문 모드에서 전체 Config 구성 (마이그레이션 전 호환)
    pub fn into_plaintext_config(self) -> Result<Config> {
        let kis_api = self.kis_api.context("kis_api 섹션 없음 (평문 모드)")?;
        Ok(Config {
            kis_api,
            telegram: self.telegram,
            scheduler: self.scheduler,
            log: self.log,
            watchlist: self.watchlist.unwrap_or_default(),
        })
    }

    /// 암호화된 secrets를 복호화하여 전체 Config 구성
    pub fn decrypt_into_config(self, passphrase: &str) -> Result<Config> {
        let b64 = self.encrypted_secrets.context("encrypted_secrets 없음")?;
        let json = crate::crypto::decrypt_from_base64(&b64, passphrase)?;
        let secrets: Secrets = serde_json::from_str(&json)
            .context("복호화된 secrets JSON 파싱 실패")?;
        Ok(Config {
            kis_api: secrets.kis_api,
            telegram: self.telegram,
            scheduler: self.scheduler,
            log: self.log,
            watchlist: secrets.watchlist,
        })
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

    /// 민감 설정 추출
    pub fn extract_secrets(&self) -> Secrets {
        Secrets {
            kis_api: self.kis_api.clone(),
            watchlist: self.watchlist.clone(),
        }
    }

    /// 암호화하여 BootConfig 형태로 저장
    pub fn save_encrypted(&self, path: &str, passphrase: &str) -> Result<()> {
        let secrets = self.extract_secrets();
        let secrets_json = serde_json::to_string(&secrets)?;
        let encrypted_b64 = crate::crypto::encrypt_to_base64(&secrets_json, passphrase)?;

        // 평문 부분만 + encrypted_secrets
        let boot = serde_json::json!({
            "telegram": self.telegram,
            "scheduler": self.scheduler,
            "log": self.log,
            "encrypted_secrets": encrypted_b64,
        });

        let data = serde_json::to_string_pretty(&boot)?;
        std::fs::write(path, data)
            .with_context(|| format!("Failed to write encrypted config: {path}"))?;
        Ok(())
    }
}
