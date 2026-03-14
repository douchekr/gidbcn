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
    /// 암호화 모드에서는 Secrets에 저장. 평문 모드에서는 여기에 직접.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gemini_api_key: String,
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    #[serde(default = "default_max_gemini_calls")]
    pub max_gemini_calls_per_day: usize,
    #[serde(default = "default_candidate_count")]
    pub candidate_count: usize,
    #[serde(default = "default_discovery_interval")]
    pub discovery_interval_hours: u64,
    #[serde(default = "default_min_score")]
    pub min_score: f64,
}

impl Default for WatchlistConfig {
    fn default() -> Self {
        Self {
            gemini_api_key: String::new(),
            gemini_model: default_gemini_model(),
            max_gemini_calls_per_day: default_max_gemini_calls(),
            candidate_count: default_candidate_count(),
            discovery_interval_hours: default_discovery_interval(),
            min_score: default_min_score(),
        }
    }
}

fn default_gemini_model() -> String { "gemini-2.5-flash".to_string() }
fn default_max_gemini_calls() -> usize { 250 }
fn default_candidate_count() -> usize { 30 }
fn default_discovery_interval() -> u64 { 1 }
fn default_min_score() -> f64 { 60.0 }

/// 암호화 대상 민감 설정 (플랫 구조)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secrets {
    pub kis_app_key: String,
    pub kis_app_secret: String,
    pub kis_base_url: String,
    pub kis_hts_id: String,
    #[serde(default)]
    pub kis_access_token: Option<String>,
    #[serde(default)]
    pub kis_expires_at: Option<DateTime<FixedOffset>>,
    pub gemini_api_key: String,
}

/// 봇 기동용 최소 설정 (평문 부분만)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootConfig {
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub watchlist: Option<WatchlistConfig>,
    /// 암호화된 민감 설정 (base64). 없으면 평문 모드.
    #[serde(default)]
    pub encrypted_secrets: Option<String>,
    // 평문 모드 하위 호환: 암호화 전에는 이 필드가 존재
    #[serde(default)]
    pub kis_api: Option<KisApiConfig>,
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

        // Secrets → KisApiConfig 복원
        let kis_api = KisApiConfig {
            app_key: secrets.kis_app_key,
            app_secret: secrets.kis_app_secret,
            base_url: secrets.kis_base_url,
            hts_id: secrets.kis_hts_id,
            token: match (secrets.kis_access_token, secrets.kis_expires_at) {
                (Some(tok), Some(exp)) if !tok.is_empty() => Some(TokenInfo {
                    access_token: tok,
                    expires_at: exp,
                }),
                _ => None,
            },
        };

        // watchlist: 평문 설정 + gemini_api_key 복원
        let mut watchlist = self.watchlist.unwrap_or_default();
        watchlist.gemini_api_key = secrets.gemini_api_key;

        Ok(Config {
            kis_api,
            telegram: self.telegram,
            scheduler: self.scheduler,
            log: self.log,
            watchlist,
        })
    }
}

impl Config {
    #[allow(dead_code)]
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

    /// 민감 설정을 플랫 Secrets로 추출
    fn extract_secrets(&self) -> Secrets {
        let (access_token, expires_at) = match &self.kis_api.token {
            Some(t) => (Some(t.access_token.clone()), Some(t.expires_at)),
            None => (None, None),
        };
        Secrets {
            kis_app_key: self.kis_api.app_key.clone(),
            kis_app_secret: self.kis_api.app_secret.clone(),
            kis_base_url: self.kis_api.base_url.clone(),
            kis_hts_id: self.kis_api.hts_id.clone(),
            kis_access_token: access_token,
            kis_expires_at: expires_at,
            gemini_api_key: self.watchlist.gemini_api_key.clone(),
        }
    }

    /// 암호화하여 BootConfig 형태로 저장
    pub fn save_encrypted(&self, path: &str, passphrase: &str) -> Result<()> {
        let secrets = self.extract_secrets();
        tracing::debug!(
            "save_encrypted: gemini_api_key={}, kis_app_key={}",
            if secrets.gemini_api_key.is_empty() { "EMPTY" } else { "SET" },
            if secrets.kis_app_key.is_empty() { "EMPTY" } else { "SET" },
        );
        let secrets_json = serde_json::to_string(&secrets)?;
        let encrypted_b64 = crate::crypto::encrypt_to_base64(&secrets_json, passphrase)?;

        // watchlist 평문 (gemini_api_key 제외 — skip_serializing_if = is_empty)
        let mut watchlist_plain = self.watchlist.clone();
        watchlist_plain.gemini_api_key = String::new();

        let boot = serde_json::json!({
            "telegram": self.telegram,
            "scheduler": self.scheduler,
            "log": self.log,
            "watchlist": watchlist_plain,
            "encrypted_secrets": encrypted_b64,
        });

        let data = serde_json::to_string_pretty(&boot)?;
        std::fs::write(path, data)
            .with_context(|| format!("Failed to write encrypted config: {path}"))?;
        Ok(())
    }

    /// 메모리에서 암호화 → BootConfig (파일 저장 없이)
    #[allow(dead_code)]
    pub fn encrypt_to_boot(&self, passphrase: &str) -> Result<BootConfig> {
        let secrets = self.extract_secrets();
        let secrets_json = serde_json::to_string(&secrets)?;
        let encrypted_b64 = crate::crypto::encrypt_to_base64(&secrets_json, passphrase)?;

        let mut watchlist_plain = self.watchlist.clone();
        watchlist_plain.gemini_api_key = String::new();

        Ok(BootConfig {
            telegram: self.telegram.clone(),
            scheduler: self.scheduler.clone(),
            log: self.log.clone(),
            watchlist: Some(watchlist_plain),
            encrypted_secrets: Some(encrypted_b64),
            kis_api: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> Config {
        Config {
            kis_api: KisApiConfig {
                app_key: "test_key_123".to_string(),
                app_secret: "test_secret_456".to_string(),
                base_url: "https://api.example.com".to_string(),
                hts_id: "testid".to_string(),
                token: Some(TokenInfo {
                    access_token: "tok_abc".to_string(),
                    expires_at: chrono::Utc::now().with_timezone(
                        &FixedOffset::east_opt(9 * 3600).unwrap(),
                    ),
                }),
            },
            telegram: TelegramConfig {
                bot_token: "123:ABC".to_string(),
                owner_chat_id: 42,
                users: vec![99],
            },
            scheduler: SchedulerConfig::default(),
            log: LogConfig::default(),
            watchlist: WatchlistConfig {
                gemini_api_key: "gemini_key_789".to_string(),
                min_score: 70.0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn encrypt_decrypt_config_roundtrip() {
        let config = make_test_config();
        let passphrase = "my-secret-pass";

        let boot = config.encrypt_to_boot(passphrase).unwrap();

        // 암호화 모드 확인
        assert!(boot.is_encrypted());
        assert!(boot.kis_api.is_none());
        // watchlist 평문 존재 (키 제외)
        let wl = boot.watchlist.as_ref().unwrap();
        assert!(wl.gemini_api_key.is_empty());
        assert_eq!(wl.min_score, 70.0);
        // 평문 부분 보존
        assert_eq!(boot.telegram.bot_token, "123:ABC");

        // 복호화
        let restored = boot.decrypt_into_config(passphrase).unwrap();
        assert_eq!(restored.kis_api.app_key, "test_key_123");
        assert_eq!(restored.kis_api.app_secret, "test_secret_456");
        assert_eq!(restored.kis_api.token.as_ref().unwrap().access_token, "tok_abc");
        assert_eq!(restored.watchlist.gemini_api_key, "gemini_key_789");
        assert_eq!(restored.watchlist.min_score, 70.0);
        assert_eq!(restored.telegram.bot_token, "123:ABC");
    }

    #[test]
    fn wrong_passphrase_fails_decryption() {
        let config = make_test_config();
        let boot = config.encrypt_to_boot("correct-pass").unwrap();
        assert!(boot.decrypt_into_config("wrong-pass").is_err());
    }

    #[test]
    fn plaintext_boot_compat() {
        // 암호화 전 config.json 형태를 BootConfig로 파싱
        let json = r#"{
            "kis_api": { "app_key": "k", "app_secret": "s", "base_url": "u", "hts_id": "h" },
            "telegram": { "bot_token": "t" },
            "watchlist": { "gemini_api_key": "g" }
        }"#;
        let boot: BootConfig = serde_json::from_str(json).unwrap();
        assert!(!boot.is_encrypted());

        let config = boot.into_plaintext_config().unwrap();
        assert_eq!(config.kis_api.app_key, "k");
        assert_eq!(config.watchlist.gemini_api_key, "g");
    }

    #[test]
    fn encrypted_secrets_not_contain_plaintext_keys() {
        let config = make_test_config();
        let boot = config.encrypt_to_boot("pass123").unwrap();

        let blob = boot.encrypted_secrets.as_ref().unwrap();
        assert!(!blob.contains("test_key_123"));
        assert!(!blob.contains("test_secret_456"));
        assert!(!blob.contains("gemini_key_789"));
    }

    #[test]
    fn secrets_flat_structure() {
        let config = make_test_config();
        let secrets = config.extract_secrets();
        assert_eq!(secrets.kis_app_key, "test_key_123");
        assert_eq!(secrets.kis_app_secret, "test_secret_456");
        assert_eq!(secrets.kis_base_url, "https://api.example.com");
        assert_eq!(secrets.kis_hts_id, "testid");
        assert_eq!(secrets.kis_access_token.as_deref(), Some("tok_abc"));
        assert_eq!(secrets.gemini_api_key, "gemini_key_789");
    }

    #[test]
    fn watchlist_plaintext_no_key() {
        let config = make_test_config();
        let boot = config.encrypt_to_boot("pass").unwrap();
        // 평문 watchlist에 gemini_api_key 없음
        let wl_json = serde_json::to_string(boot.watchlist.as_ref().unwrap()).unwrap();
        assert!(!wl_json.contains("gemini_key_789"));
        assert!(wl_json.contains("gemini-2.5-flash"));
    }
}
