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
    let hour = kst.hour();
    let min = kst.minute();
    let hhmm = hour * 100 + min;

    // KRX: 09:00~15:30 KST
    let krx_open = hhmm >= 900 && hhmm <= 1530;

    // US: 22:30~05:00 KST (다음날)
    let us_open = hhmm >= 2230 || hhmm <= 500;

    krx_open || us_open
}
