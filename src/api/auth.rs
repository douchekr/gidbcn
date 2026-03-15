use anyhow::{Context, Result};
use chrono::{Duration, FixedOffset, Utc};
use serde::Deserialize;

use crate::config::{KisApiConfig, TokenInfo};

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    access_token_token_expired: String, // "YYYY-MM-DD HH:MM:SS" KST
}

pub async fn issue_token(client: &reqwest::Client, config: &KisApiConfig) -> Result<TokenInfo> {
    let url = format!("{}/oauth2/tokenP", config.base_url);

    let body = serde_json::json!({
        "grant_type": "client_credentials",
        "appkey": config.app_key,
        "appsecret": config.app_secret,
    });

    let raw: serde_json::Value = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Token request failed")?
        .json()
        .await
        .context("Token response parse failed")?;

    tracing::debug!("Token response: {raw}");

    let resp: TokenResponse = serde_json::from_value(raw)
        .context("Token response field mismatch")?;

    // 한투 API는 "YYYY-MM-DD HH:MM:SS" (KST) 형식으로 만료시간 반환
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    let expires_at = chrono::NaiveDateTime::parse_from_str(
        &resp.access_token_token_expired,
        "%Y-%m-%d %H:%M:%S",
    )
    .map(|naive| naive.and_local_timezone(kst).unwrap())
    .unwrap_or_else(|_| {
        // 파싱 실패 시 24시간 후로 설정
        Utc::now().with_timezone(&kst) + Duration::hours(24)
    });

    Ok(TokenInfo {
        access_token: resp.access_token,
        expires_at,
    })
}

pub fn token_needs_refresh(token: &Option<TokenInfo>) -> bool {
    match token {
        None => true,
        Some(info) => {
            let kst = FixedOffset::east_opt(9 * 3600).unwrap();
            let now = Utc::now().with_timezone(&kst);
            // 만료 1시간 전에 갱신
            now >= info.expires_at - Duration::hours(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_token(hours_from_now: i64) -> Option<TokenInfo> {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let expires_at = Utc::now().with_timezone(&kst) + Duration::hours(hours_from_now);
        Some(TokenInfo {
            access_token: "test_token".to_string(),
            expires_at,
        })
    }

    #[test]
    fn no_token_needs_refresh() {
        assert!(token_needs_refresh(&None));
    }

    #[test]
    fn fresh_token_no_refresh() {
        // 12시간 후 만료 → 갱신 불필요
        assert!(!token_needs_refresh(&make_token(12)));
    }

    #[test]
    fn expiring_soon_needs_refresh() {
        // 30분 후 만료 → 1시간 이내이므로 갱신 필요
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let expires_at = Utc::now().with_timezone(&kst) + Duration::minutes(30);
        let token = Some(TokenInfo {
            access_token: "test".to_string(),
            expires_at,
        });
        assert!(token_needs_refresh(&token));
    }

    #[test]
    fn already_expired_needs_refresh() {
        // 이미 만료됨
        assert!(token_needs_refresh(&make_token(-1)));
    }

    #[test]
    fn exactly_one_hour_boundary() {
        // 정확히 1시간 후 만료 → 경계값, 갱신 필요
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let expires_at = Utc::now().with_timezone(&kst) + Duration::hours(1);
        let token = Some(TokenInfo {
            access_token: "test".to_string(),
            expires_at,
        });
        assert!(token_needs_refresh(&token));
    }

    #[test]
    fn just_over_one_hour_no_refresh() {
        // 1시간 1분 후 만료 → 아직 여유
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let expires_at = Utc::now().with_timezone(&kst) + Duration::minutes(61);
        let token = Some(TokenInfo {
            access_token: "test".to_string(),
            expires_at,
        });
        assert!(!token_needs_refresh(&token));
    }
}
