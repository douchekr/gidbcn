use chrono::{FixedOffset, Utc};
use teloxide::prelude::*;

use crate::api::ApiHandle;
use crate::bot::formatter;
use crate::storage;

use super::price;

fn kst_now() -> chrono::DateTime<FixedOffset> {
    Utc::now().with_timezone(&FixedOffset::east_opt(9 * 3600).unwrap())
}

/// 특정 사용자의 활성 시그널을 순회하며 조건 평가. 발동 시 텔레그램 알림 전송.
pub async fn check_all_signals(api: &ApiHandle, bot: &Bot, user_id: i64) {
    let chat_id = ChatId(user_id);
    let mut portfolio = storage::load_portfolio(user_id);
    let mut signal_store = storage::load_signals(user_id);
    let mut any_triggered = false;
    let mut portfolio_updated = false;

    let active_indices: Vec<usize> = signal_store
        .signals
        .iter()
        .enumerate()
        .filter(|(_, s)| s.active)
        .map(|(i, _)| i)
        .collect();

    for idx in active_indices {
        let symbol = signal_store.signals[idx].symbol.clone();
        let sig_account = signal_store.signals[idx].account.clone();

        // account 지정 시 정확히 매칭, 미지정 시 첫 번째 매칭 holding 사용
        let (market, avg_price) = match portfolio.holdings.iter().find(|h| {
            h.symbol == symbol && (sig_account.is_empty() || h.account == sig_account)
        }) {
            Some(h) => (h.market, h.avg_price),
            None => continue,
        };

        // 해당 마켓이 현재 장중이 아니면 건너뜀
        if !market.is_open_now() {
            continue;
        }

        let price_data = match api.get_price_for_market(market, &symbol).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Signal check: failed to get price for {symbol}: {e:#}");
                continue;
            }
        };

        // 캐시 갱신
        if let Some(h) = portfolio.holdings.iter_mut().find(|h| {
            h.symbol == symbol && (sig_account.is_empty() || h.account == sig_account)
        }) {
            h.cached_price = Some(price_data.current_price);
            h.cached_at = Some(kst_now());
            portfolio_updated = true;
        }

        let triggered = price::evaluate(
            &signal_store.signals[idx].condition,
            price_data.current_price,
            Some(avg_price),
        );

        if triggered {
            let condition_desc = formatter::format_condition(&signal_store.signals[idx].condition, &market);
            let alert_msg = formatter::format_signal_alert(
                &market,
                &symbol,
                &price_data.name,
                &sig_account,
                &condition_desc,
                price_data.current_price,
                price_data.change_pct,
                Some(avg_price),
            );

            match bot.send_message(chat_id, &alert_msg).await {
                Ok(_) => {
                    tracing::info!("Signal triggered: {} {} (user {})", signal_store.signals[idx].id, condition_desc, user_id);
                    signal_store.signals[idx].active = false;
                    any_triggered = true;
                }
                Err(e) => {
                    // 전송 실패 시 active 유지 → 다음 주기에 재시도
                    tracing::error!("Failed to send alert for {} (user {}): {e} — keeping active for retry", signal_store.signals[idx].id, user_id);
                }
            }
        }
    }

    if any_triggered {
        if let Err(e) = storage::save_signals(user_id, &signal_store) {
            tracing::error!("Failed to save signals after trigger (user {}): {e:#}", user_id);
        }
    }

    if portfolio_updated {
        if let Err(e) = storage::save_portfolio(user_id, &portfolio) {
            tracing::warn!("Failed to save portfolio cache after signal check (user {}): {e:#}", user_id);
        }
    }
}
