use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{FixedOffset, Timelike, Utc};
use teloxide::prelude::*;
use tokio::sync::Notify;
use tokio::time::{interval, Duration};

use crate::api::ApiHandle;
use crate::signal::engine;
use crate::storage;
use crate::watchlist::{db as wdb, models::PromptType, pipeline};

pub async fn run_scheduler(
    api: ApiHandle,
    bot: Bot,
    discovery_enabled: Arc<AtomicBool>,
    discovery_trigger: Arc<Notify>,
) {
    let interval_min = storage::with_config(|c| c.scheduler.signal_check_interval_minutes);
    let signal_interval = Duration::from_secs(interval_min * 60);
    let mut signal_tick = interval(signal_interval);

    let hunt_min = storage::with_config(|c| c.watchlist.hunt_interval_minutes);
    let mut hunt_tick = interval(Duration::from_secs(hunt_min * 60));

    // 감정 사이클: 1분마다 체크, KST 02:00 / 14:00 하루 2회
    let mut eval_tick = interval(Duration::from_secs(60));
    let mut last_eval_slot = String::new();

    tracing::info!("Scheduler started: signal {}min, hunt {}min", interval_min, hunt_min);

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
            _ = hunt_tick.tick() => {
                if discovery_enabled.load(Ordering::SeqCst) && prompts_configured() && !hunt_exhausted() {
                    run_hunt_cycle(&bot).await;
                }
            }
            _ = discovery_trigger.notified() => {
                if !hunt_exhausted() {
                    run_hunt_cycle(&bot).await;
                }
            }
            _ = eval_tick.tick() => {
                if discovery_enabled.load(Ordering::SeqCst) && prompts_configured() {
                    let kst = kst_now();
                    let hour = kst.hour();
                    // KST 02:00 / 14:00 하루 2회
                    if (hour == 2 || hour == 14) && !judge_exhausted() {
                        let slot = format!("{}-{:02}", kst.format("%Y-%m-%d"), hour);
                        if last_eval_slot != slot {
                            last_eval_slot = slot;
                            run_eval_cycle(&api, &bot).await;
                        }
                    }
                }
            }
        }
    }
}

fn hunt_exhausted() -> bool {
    let max = storage::with_config(|c| c.watchlist.max_hunt_calls_per_day);
    wdb::hunt_calls_today().unwrap_or(0) >= max
}

fn judge_exhausted() -> bool {
    let max = storage::with_config(|c| c.watchlist.max_judge_calls_per_day);
    wdb::judge_calls_today().unwrap_or(0) >= max
}

fn prompts_configured() -> bool {
    wdb::get_prompt(PromptType::Hunt).ok().flatten().is_some()
        && wdb::get_prompt(PromptType::Judge).ok().flatten().is_some()
}

async fn run_hunt_cycle(bot: &Bot) {
    tracing::info!("Running hunt cycle...");
    let client = reqwest::Client::new();
    let owner_id = storage::with_config(|c| c.telegram.owner_chat_id);

    match pipeline::run_hunt(&client).await {
        Ok(report) => {
            let msg = report.summary();
            tracing::info!("{msg}");
            if owner_id != 0 {
                let _ = bot.send_message(ChatId(owner_id), &msg).await;
            }
        }
        Err(e) => {
            tracing::error!("Hunt cycle failed: {e:#}");
            if owner_id != 0 {
                let _ = bot.send_message(ChatId(owner_id), format!("❌ 사냥 실패: {e:#}")).await;
            }
        }
    }
}

async fn run_eval_cycle(api: &ApiHandle, bot: &Bot) {
    tracing::info!("Running evaluate cycle...");
    let client = reqwest::Client::new();
    let owner_id = storage::with_config(|c| c.telegram.owner_chat_id);

    match pipeline::run_evaluate(api, &client).await {
        Ok(report) => {
            let msg = report.summary();
            tracing::info!("{msg}");
            if owner_id != 0 {
                let _ = bot.send_message(ChatId(owner_id), &msg).await;
            }
        }
        Err(e) => {
            tracing::error!("Evaluate cycle failed: {e:#}");
            if owner_id != 0 {
                let _ = bot.send_message(ChatId(owner_id), format!("❌ 감정 실패: {e:#}")).await;
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
    let krx_open = hhmm >= 900 && hhmm <= 1530;
    let us_open = hhmm >= 2230 || hhmm <= 500;
    krx_open || us_open
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn krx_market_hours() {
        assert!(is_market_hours_at(9, 0));
        assert!(is_market_hours_at(12, 30));
        assert!(is_market_hours_at(15, 30));
    }

    #[test]
    fn krx_outside_hours() {
        assert!(!is_market_hours_at(8, 59));
        assert!(!is_market_hours_at(15, 31));
    }

    #[test]
    fn us_market_hours_kst() {
        assert!(is_market_hours_at(22, 30));
        assert!(is_market_hours_at(23, 0));
        assert!(is_market_hours_at(0, 0));
        assert!(is_market_hours_at(3, 30));
        assert!(is_market_hours_at(5, 0));
    }

    #[test]
    fn us_outside_hours_kst() {
        assert!(!is_market_hours_at(5, 1));
        assert!(!is_market_hours_at(22, 29));
    }

    #[test]
    fn gap_between_markets() {
        assert!(!is_market_hours_at(16, 0));
        assert!(!is_market_hours_at(20, 0));
        assert!(!is_market_hours_at(22, 0));
    }

    #[test]
    fn gap_morning() {
        assert!(!is_market_hours_at(6, 0));
        assert!(!is_market_hours_at(8, 0));
    }
}
