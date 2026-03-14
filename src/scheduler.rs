use chrono::{FixedOffset, Timelike, Utc};
use teloxide::prelude::*;
use tokio::time::{interval, Duration};

use crate::api::ApiHandle;
use crate::signal::engine;
use crate::storage;

pub async fn run_scheduler(
    api: ApiHandle,
    bot: Bot,
) {
    let interval_min = storage::with_config(|c| c.scheduler.signal_check_interval_minutes);
    let signal_interval = Duration::from_secs(interval_min * 60);
    let mut signal_tick = interval(signal_interval);

    tracing::info!("Scheduler started: signal check every {interval_min}min");

    loop {
        signal_tick.tick().await;
        if is_market_hours() {
            let user_ids = storage::list_user_ids();
            if user_ids.is_empty() {
                tracing::debug!("No users with portfolios, skipping signal check");
            } else {
                tracing::info!("Running signal check for {} users...", user_ids.len());
                for user_id in user_ids {
                    engine::check_all_signals(&api, &bot, user_id).await;
                }
            }
        }
    }
}

fn kst_now() -> chrono::DateTime<FixedOffset> {
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    Utc::now().with_timezone(&kst)
}

fn is_market_hours() -> bool {
    let kst = kst_now();
    is_market_hours_at(kst.hour(), kst.minute())
}

fn is_market_hours_at(hour: u32, min: u32) -> bool {
    let hhmm = hour * 100 + min;

    // KRX: 09:00~15:30 KST
    let krx_open = hhmm >= 900 && hhmm <= 1530;

    // US: 22:30~05:00 KST (다음날)
    let us_open = hhmm >= 2230 || hhmm <= 500;

    krx_open || us_open
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn krx_market_hours() {
        assert!(is_market_hours_at(9, 0));   // 09:00 개장
        assert!(is_market_hours_at(12, 30)); // 점심
        assert!(is_market_hours_at(15, 30)); // 15:30 마감
    }

    #[test]
    fn krx_outside_hours() {
        assert!(!is_market_hours_at(8, 59));  // 개장 전
        assert!(!is_market_hours_at(15, 31)); // 마감 후
    }

    #[test]
    fn us_market_hours_kst() {
        assert!(is_market_hours_at(22, 30)); // 22:30 개장
        assert!(is_market_hours_at(23, 0));
        assert!(is_market_hours_at(0, 0));   // 자정
        assert!(is_market_hours_at(3, 30));
        assert!(is_market_hours_at(5, 0));   // 05:00 마감
    }

    #[test]
    fn us_outside_hours_kst() {
        assert!(!is_market_hours_at(5, 1));   // 마감 후
        assert!(!is_market_hours_at(22, 29)); // 개장 전
    }

    #[test]
    fn gap_between_markets() {
        // KRX 마감 ~ US 개장 사이
        assert!(!is_market_hours_at(16, 0));
        assert!(!is_market_hours_at(20, 0));
        assert!(!is_market_hours_at(22, 0));
    }

    #[test]
    fn gap_morning() {
        // US 마감 ~ KRX 개장 사이
        assert!(!is_market_hours_at(6, 0));
        assert!(!is_market_hours_at(8, 0));
    }
}
