use teloxide::prelude::*;

use crate::api::ApiHandle;
use crate::bot::formatter;
use crate::storage;

use super::price;

/// 특정 사용자의 활성 시그널을 순회하며 조건 평가. 발동 시 텔레그램 알림 전송.
pub async fn check_all_signals(api: &ApiHandle, bot: &Bot, user_id: i64) {
    let chat_id = ChatId(user_id);
    let portfolio = storage::load_portfolio(user_id);
    let mut signal_store = storage::load_signals(user_id);
    let mut any_triggered = false;

    let active_indices: Vec<usize> = signal_store
        .signals
        .iter()
        .enumerate()
        .filter(|(_, s)| s.active)
        .map(|(i, _)| i)
        .collect();

    for idx in active_indices {
        let signal = &signal_store.signals[idx];
        let symbol = &signal.symbol;

        let holding = portfolio.holdings.iter().find(|h| h.symbol == *symbol);
        let market = match holding {
            Some(h) => h.market,
            None => continue,
        };

        let price_data = match api.get_price_for_market(market, symbol).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Signal check: failed to get price for {symbol}: {e}");
                continue;
            }
        };

        let avg_price = holding.map(|h| h.avg_price);
        let triggered = price::evaluate(&signal.condition, price_data.current_price, avg_price);

        if triggered {
            any_triggered = true;
            let condition_desc = signal.condition.display_description();
            let alert_msg = formatter::format_signal_alert(
                symbol,
                &price_data.name,
                &condition_desc,
                price_data.current_price,
                price_data.change_pct,
                avg_price,
            );

            match bot.send_message(chat_id, &alert_msg).await {
                Ok(_) => {
                    tracing::info!("Signal triggered: {} {} (user {})", signal.id, condition_desc, user_id);
                }
                Err(e) => {
                    tracing::error!("Failed to send alert for {} (user {}): {e}", signal.id, user_id);
                }
            }

            signal_store.signals[idx].active = false;
        }
    }

    if any_triggered {
        if let Err(e) = storage::save_signals(user_id, &signal_store) {
            tracing::error!("Failed to save signals after trigger (user {}): {e}", user_id);
        }
    }
}
