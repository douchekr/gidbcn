use chrono::{FixedOffset, Timelike, Utc};
use teloxide::prelude::*;
use tokio::time::{interval, Duration};

use crate::api::ApiHandle;
use crate::config::SchedulerConfig;
use crate::signal::engine;
use crate::storage;

pub async fn run_scheduler(
    api: ApiHandle,
    config: SchedulerConfig,
    bot: Bot,
) {
    let signal_interval = Duration::from_secs(config.signal_check_interval_minutes * 60);
    let mut signal_tick = interval(signal_interval);

    // 환율/토큰 체크는 1분 간격으로 시간 확인
    let mut minute_tick = interval(Duration::from_secs(60));

    let mut last_exchange_update: Option<chrono::NaiveTime> = None;

    tracing::info!(
        "Scheduler started: signal check every {}min",
        config.signal_check_interval_minutes
    );

    loop {
        tokio::select! {
            _ = signal_tick.tick() => {
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
            _ = minute_tick.tick() => {
                let kst = kst_now();
                let time = kst.time();

                // 환율 조회: 08:50, 15:40
                let should_update_exchange =
                    (time.hour() == 8 && time.minute() == 50)
                    || (time.hour() == 15 && time.minute() == 40);

                if should_update_exchange {
                    let current_time = time;
                    let already_done = last_exchange_update
                        .map(|t| t.hour() == current_time.hour() && t.minute() == current_time.minute())
                        .unwrap_or(false);

                    if !already_done {
                        tracing::info!("Updating exchange rate...");
                        match api.get_exchange_rate().await {
                            Ok(rate) => {
                                tracing::info!("USD/KRW = {rate}");
                            }
                            Err(e) => tracing::warn!("Exchange rate update failed: {e}"),
                        }
                        last_exchange_update = Some(current_time);
                    }
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
