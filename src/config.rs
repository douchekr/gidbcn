use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

/// 런타임 내부 Config (메모리용)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub watchlist: WatchlistConfig,
    #[serde(default = "default_kis_base_url")]
    pub kis_base_url: String,
    /// 민감 설정 (메모리에서만 평문, 디스크에서는 암호화 가능)
    pub secrets: Secrets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secrets {
    pub kis_app_key: String,
    pub kis_app_secret: String,
    pub kis_hts_id: String,
    #[serde(default)]
    pub kis_access_token: Option<String>,
    #[serde(default)]
    pub kis_expires_at: Option<DateTime<FixedOffset>>,
    pub gemini_api_key: String,
}

fn default_kis_base_url() -> String {
    "https://openapi.koreainvestment.com:9443".to_string()
}

impl Secrets {
    pub fn token_info(&self) -> Option<TokenInfo> {
        match (&self.kis_access_token, self.kis_expires_at) {
            (Some(tok), Some(exp)) if !tok.is_empty() => Some(TokenInfo {
                access_token: tok.clone(),
                expires_at: exp,
            }),
            _ => None,
        }
    }

    pub fn set_token(&mut self, token: &str, expires_at: DateTime<FixedOffset>) {
        self.kis_access_token = Some(token.to_string());
        self.kis_expires_at = Some(expires_at);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub access_token: String,
    pub expires_at: DateTime<FixedOffset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    #[serde(default)]
    pub owner_chat_id: i64,
    #[serde(default)]
    pub users: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub signal_check_interval_minutes: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self { signal_check_interval_minutes: 5 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    pub dir: String,
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
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    #[serde(default = "default_max_gemini_calls")]
    pub max_gemini_calls_per_day: usize,
    #[serde(default = "default_candidate_count")]
    pub candidate_count: usize,
    #[serde(default = "default_hunt_interval")]
    pub hunt_interval_minutes: u64,
    #[serde(default = "default_min_score")]
    pub min_score: f64,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl Default for WatchlistConfig {
    fn default() -> Self {
        Self {
            gemini_model: default_gemini_model(),
            max_gemini_calls_per_day: default_max_gemini_calls(),
            candidate_count: default_candidate_count(),
            hunt_interval_minutes: default_hunt_interval(),
            min_score: default_min_score(),
            retention_days: default_retention_days(),
        }
    }
}

fn default_gemini_model() -> String { "gemini-2.5-flash".to_string() }
fn default_max_gemini_calls() -> usize { 250 }
fn default_candidate_count() -> usize { 30 }
fn default_hunt_interval() -> u64 { 30 }
fn default_min_score() -> f64 { 60.0 }
fn default_retention_days() -> u32 { 100 }

// --- 디스크 포맷: BootConfig ---
// secrets 필드가 object면 평문, string이면 암호화

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootConfig {
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub watchlist: Option<WatchlistConfig>,
    #[serde(default = "default_kis_base_url")]
    pub kis_base_url: String,
    /// object(평문) 또는 string(암호화 blob)
    pub secrets: serde_json::Value,
    // 구버전 호환: kis_api 섹션이 있으면 마이그레이션
    #[serde(default)]
    pub kis_api: Option<serde_json::Value>,
    // 구버전 호환: encrypted_secrets가 있으면 마이그레이션
    #[serde(default)]
    pub encrypted_secrets: Option<String>,
}

impl BootConfig {
    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {path}"))?;
        let mut boot: BootConfig =
            serde_json::from_str(&data).with_context(|| "Failed to parse config JSON")?;

        // 구버전 마이그레이션: kis_api + watchlist.gemini_api_key → secrets
        if boot.secrets.is_null() {
            if let Some(ref old_enc) = boot.encrypted_secrets {
                // 구버전 암호화: encrypted_secrets → secrets (string)
                boot.secrets = serde_json::Value::String(old_enc.clone());
            } else if let Some(ref kis) = boot.kis_api {
                // 구버전 평문: kis_api → secrets (object)
                let gemini_key = boot.watchlist.as_ref()
                    .and_then(|w| {
                        // 구버전 watchlist에 gemini_api_key가 있을 수 있음
                        let v = serde_json::to_value(w).ok()?;
                        v.get("gemini_api_key")?.as_str().map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                boot.secrets = serde_json::json!({
                    "kis_app_key": kis.get("app_key").and_then(|v| v.as_str()).unwrap_or(""),
                    "kis_app_secret": kis.get("app_secret").and_then(|v| v.as_str()).unwrap_or(""),
                    "kis_base_url": kis.get("base_url").and_then(|v| v.as_str()).unwrap_or("https://openapi.koreainvestment.com:9443"),
                    "kis_hts_id": kis.get("hts_id").and_then(|v| v.as_str()).unwrap_or(""),
                    "gemini_api_key": gemini_key,
                });
                // 토큰 마이그레이션
                if let Some(token) = kis.get("token") {
                    if let Some(tok) = token.get("access_token").and_then(|v| v.as_str()) {
                        boot.secrets["kis_access_token"] = serde_json::Value::String(tok.to_string());
                    }
                    if let Some(exp) = token.get("expires_at") {
                        boot.secrets["kis_expires_at"] = exp.clone();
                    }
                }
            }
        }

        Ok(boot)
    }

    pub fn is_encrypted(&self) -> bool {
        self.secrets.is_string()
    }

    /// 평문 모드: secrets가 object
    pub fn into_plaintext_config(self) -> Result<Config> {
        let secrets: Secrets = serde_json::from_value(self.secrets)
            .context("secrets 파싱 실패 (평문 모드)")?;
        Ok(Config {
            telegram: self.telegram,
            scheduler: self.scheduler,
            log: self.log,
            watchlist: self.watchlist.unwrap_or_default(),
            kis_base_url: self.kis_base_url,
            secrets,
        })
    }

    /// 암호화 모드: secrets가 string (base64 blob)
    pub fn decrypt_into_config(self, passphrase: &str) -> Result<Config> {
        let b64 = self.secrets.as_str().context("secrets가 문자열이 아님")?;
        let json = crate::crypto::decrypt_from_base64(b64, passphrase)?;
        let secrets: Secrets = serde_json::from_str(&json)
            .context("복호화된 secrets 파싱 실패")?;
        Ok(Config {
            telegram: self.telegram,
            scheduler: self.scheduler,
            log: self.log,
            watchlist: self.watchlist.unwrap_or_default(),
            kis_base_url: self.kis_base_url,
            secrets,
        })
    }
}

/// API Actor용 KIS 설정 (하위 호환)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KisApiConfig {
    pub app_key: String,
    pub app_secret: String,
    pub base_url: String,
    pub hts_id: String,
    #[serde(default)]
    pub token: Option<TokenInfo>,
}

impl Config {
    /// API Actor용 KisApiConfig 조립
    pub fn to_kis_api_config(&self) -> KisApiConfig {
        KisApiConfig {
            app_key: self.secrets.kis_app_key.clone(),
            app_secret: self.secrets.kis_app_secret.clone(),
            base_url: self.kis_base_url.clone(),
            hts_id: self.secrets.kis_hts_id.clone(),
            token: self.secrets.token_info(),
        }
    }

    #[allow(dead_code)]
    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {path}"))?;
        let config: Config =
            serde_json::from_str(&data).with_context(|| "Failed to parse config JSON")?;
        Ok(config)
    }

    /// 평문 저장
    pub fn save(&self, path: &str) -> Result<()> {
        let boot = serde_json::json!({
            "telegram": self.telegram,
            "scheduler": self.scheduler,
            "log": self.log,
            "watchlist": self.watchlist,
            "kis_base_url": self.kis_base_url,
            "secrets": self.secrets,
        });
        let data = serde_json::to_string_pretty(&boot)?;
        std::fs::write(path, data)
            .with_context(|| format!("Failed to write config: {path}"))?;
        Ok(())
    }

    /// 암호화 저장: secrets → encrypted string
    pub fn save_encrypted(&self, path: &str, passphrase: &str) -> Result<()> {
        tracing::debug!(
            "save_encrypted: gemini_api_key={}, kis_app_key={}",
            if self.secrets.gemini_api_key.is_empty() { "EMPTY" } else { "SET" },
            if self.secrets.kis_app_key.is_empty() { "EMPTY" } else { "SET" },
        );
        let secrets_json = serde_json::to_string(&self.secrets)?;
        let encrypted_b64 = crate::crypto::encrypt_to_base64(&secrets_json, passphrase)?;

        let boot = serde_json::json!({
            "telegram": self.telegram,
            "scheduler": self.scheduler,
            "log": self.log,
            "watchlist": self.watchlist,
            "kis_base_url": self.kis_base_url,
            "secrets": encrypted_b64,
        });

        let data = serde_json::to_string_pretty(&boot)?;
        std::fs::write(path, data)
            .with_context(|| format!("Failed to write encrypted config: {path}"))?;
        Ok(())
    }

    /// 메모리에서 암호화 → BootConfig (파일 저장 없이)
    #[allow(dead_code)]
    pub fn encrypt_to_boot(&self, passphrase: &str) -> Result<BootConfig> {
        let secrets_json = serde_json::to_string(&self.secrets)?;
        let encrypted_b64 = crate::crypto::encrypt_to_base64(&secrets_json, passphrase)?;
        Ok(BootConfig {
            telegram: self.telegram.clone(),
            scheduler: self.scheduler.clone(),
            log: self.log.clone(),
            watchlist: Some(self.watchlist.clone()),
            kis_base_url: self.kis_base_url.clone(),
            secrets: serde_json::Value::String(encrypted_b64),
            kis_api: None,
            encrypted_secrets: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> Config {
        Config {
            telegram: TelegramConfig {
                bot_token: "123:ABC".to_string(),
                owner_chat_id: 42,
                users: vec![99],
            },
            scheduler: SchedulerConfig::default(),
            log: LogConfig::default(),
            watchlist: WatchlistConfig {
                min_score: 70.0,
                ..Default::default()
            },
            kis_base_url: default_kis_base_url(),
            secrets: Secrets {
                kis_app_key: "test_key_123".to_string(),
                kis_app_secret: "test_secret_456".to_string(),
                kis_hts_id: "testid".to_string(),
                kis_access_token: Some("tok_abc".to_string()),
                kis_expires_at: Some(chrono::Utc::now().with_timezone(
                    &FixedOffset::east_opt(9 * 3600).unwrap(),
                )),
                gemini_api_key: "gemini_key_789".to_string(),
            },
        }
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let config = make_test_config();
        let passphrase = "my-secret-pass";

        let boot = config.encrypt_to_boot(passphrase).unwrap();

        // 암호화 모드 확인
        assert!(boot.is_encrypted());
        assert!(boot.secrets.is_string());
        // watchlist 평문 보존
        let wl = boot.watchlist.as_ref().unwrap();
        assert_eq!(wl.min_score, 70.0);
        // telegram 평문
        assert_eq!(boot.telegram.bot_token, "123:ABC");

        // 복호화
        let restored = boot.decrypt_into_config(passphrase).unwrap();
        assert_eq!(restored.secrets.kis_app_key, "test_key_123");
        assert_eq!(restored.secrets.kis_app_secret, "test_secret_456");
        assert_eq!(restored.secrets.kis_access_token.as_deref(), Some("tok_abc"));
        assert_eq!(restored.secrets.gemini_api_key, "gemini_key_789");
        assert_eq!(restored.watchlist.min_score, 70.0);
        assert_eq!(restored.telegram.bot_token, "123:ABC");
    }

    #[test]
    fn wrong_passphrase_fails() {
        let config = make_test_config();
        let boot = config.encrypt_to_boot("correct-pass").unwrap();
        assert!(boot.decrypt_into_config("wrong-pass").is_err());
    }

    #[test]
    fn plaintext_boot() {
        let json = r#"{
            "telegram": { "bot_token": "t" },
            "secrets": {
                "kis_app_key": "k",
                "kis_app_secret": "s",
                "kis_hts_id": "h",
                "gemini_api_key": "g"
            }
        }"#;
        let boot: BootConfig = serde_json::from_str(json).unwrap();
        assert!(!boot.is_encrypted());

        let config = boot.into_plaintext_config().unwrap();
        assert_eq!(config.secrets.kis_app_key, "k");
        assert_eq!(config.secrets.gemini_api_key, "g");
    }

    #[test]
    fn encrypted_blob_no_plaintext_keys() {
        let config = make_test_config();
        let boot = config.encrypt_to_boot("pass123").unwrap();
        let blob = boot.secrets.as_str().unwrap();
        assert!(!blob.contains("test_key_123"));
        assert!(!blob.contains("test_secret_456"));
        assert!(!blob.contains("gemini_key_789"));
    }

    #[test]
    fn plaintext_with_kis_base_url() {
        let json = r#"{
            "telegram": { "bot_token": "t" },
            "kis_base_url": "https://custom.api.com",
            "secrets": {
                "kis_app_key": "k",
                "kis_app_secret": "s",
                "kis_hts_id": "h",
                "gemini_api_key": "g"
            }
        }"#;
        let boot: BootConfig = serde_json::from_str(json).unwrap();
        let config = boot.into_plaintext_config().unwrap();
        assert_eq!(config.kis_base_url, "https://custom.api.com");
        assert_eq!(config.secrets.kis_app_key, "k");
    }
}
