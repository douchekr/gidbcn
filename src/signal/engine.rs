use teloxide::prelude::*;

use crate::api::ApiHandle;
use crate::bot::formatter;
use crate::models::signal::Condition;
use crate::storage;

use super::{price, technical, volume};

/// 모든 활성 시그널을 순회하며 조건 평가. 발동 시 텔레그램 알림 전송.
pub async fn check_all_signals(api: &ApiHandle, bot: &Bot, chat_id: ChatId) {
    let portfolio = storage::load_portfolio();
    let mut signal_store = storage::load_signals();
    let mut alert_log = storage::load_alert_log();
    let mut any_triggered = false;

    // 활성 시그널만 처리
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

        // 포트폴리오에서 해당 종목 찾기
        let holding = portfolio.holdings.iter().find(|h| h.symbol == *symbol);
        let market = match holding {
            Some(h) => h.market,
            None => continue, // 포트폴리오에 없으면 스킵
        };

        // 현재가 조회
        let price_data = match api.get_price_for_market(market, symbol).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Signal check: failed to get price for {symbol}: {e}");
                continue;
            }
        };

        let avg_price = holding.map(|h| h.avg_price);
        let triggered = if signal.condition.needs_daily_chart() {
            // 일봉 기반 시그널
            match api.get_daily_chart(market, symbol).await {
                Ok(candles) => match &signal.condition {
                    Condition::VolumeSurge { .. } => {
                        volume::evaluate(&signal.condition, &candles, price_data.volume)
                    }
                    _ => technical::evaluate(&signal.condition, &candles),
                },
                Err(e) => {
                    tracing::warn!("Signal check: failed to get chart for {symbol}: {e}");
                    false
                }
            }
        } else {
            price::evaluate(&signal.condition, price_data.current_price, avg_price)
        };

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

            // 텔레그램 전송
            let success = match bot.send_message(chat_id, &alert_msg).await {
                Ok(_) => {
                    tracing::info!("Signal triggered: {} {}", signal.id, condition_desc);
                    true
                }
                Err(e) => {
                    tracing::error!("Failed to send alert for {}: {e}", signal.id);
                    false
                }
            };

            // 알림 기록
            let alert_id = alert_log.next_alert_id();
            alert_log.alerts.push(crate::models::AlertRecord {
                id: alert_id,
                signal_id: signal.id.clone(),
                symbol: symbol.clone(),
                condition_type: signal.condition.type_name().to_string(),
                trigger_value: price_data.current_price,
                message: alert_msg,
                sent_at: chrono::Utc::now().with_timezone(
                    &chrono::FixedOffset::east_opt(9 * 3600).unwrap(),
                ),
                success,
            });

            // 1회성 발동 → 비활성화
            signal_store.signals[idx].active = false;
        }
    }

    if any_triggered {
        if let Err(e) = storage::save_signals(&signal_store) {
            tracing::error!("Failed to save signals after trigger: {e}");
        }
        if let Err(e) = storage::save_alert_log(&alert_log) {
            tracing::error!("Failed to save alert log: {e}");
        }
    }
}
