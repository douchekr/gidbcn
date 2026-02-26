use anyhow::Result;
use chrono::{FixedOffset, Utc};
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

use crate::api::ApiHandle;
use crate::bot::formatter;
use crate::models::messages::PriceData;
use crate::models::portfolio::{Holding, Market};
use crate::models::signal::{Condition, Signal};
use crate::storage;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum Command {
    #[command(description = "도움말")]
    Help,
    #[command(description = "포트폴리오: /port add|remove|edit|list|info|summary ...")]
    Port(String),
    #[command(description = "시그널: /signal add|list|remove|clear ...")]
    Signal(String),
    #[command(description = "시스템 상태")]
    Status,
    #[command(description = "핑")]
    Ping,
}

pub async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    api: ApiHandle,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    // 텔레그램 user_id 추출 (Private 채팅에서 user_id == chat_id)
    let user_id = match msg.from() {
        Some(user) => user.id.0 as i64,
        None => {
            bot.send_message(chat_id, "사용자 정보를 확인할 수 없습니다.").await?;
            return Ok(());
        }
    };

    let reply = match cmd {
        Command::Help => help_text(),
        Command::Ping => "pong".to_string(),
        Command::Port(args) => cmd_port(user_id, &args, &api).await,
        Command::Signal(args) => cmd_signal(user_id, &args),
        Command::Status => cmd_status(user_id),
    };

    bot.send_message(chat_id, reply).await?;
    Ok(())
}

fn kst_now() -> chrono::DateTime<FixedOffset> {
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    Utc::now().with_timezone(&kst)
}

fn help_text() -> String {
    "📋 명령어 목록\n\n\
     포트폴리오:\n\
     /port add [마켓] [종목코드] [수량] [매입가] [종목명]\n\
     /port remove [종목코드]\n\
     /port edit [종목코드] [수량] [매입가]\n\
     /port list — 전체 포트폴리오\n\
     /port info [종목코드] — 종목 상세\n\
     /port summary — 자산배분 요약\n\n\
     시그널:\n\
     /signal add [종목코드] [> 또는 <] [값 또는 수익률%]\n\
     /signal list — 전체 시그널\n\
     /signal remove [시그널ID]\n\
     /signal clear [종목코드]\n\n\
     시스템:\n\
     /status — 시스템 상태\n\
     /ping — 핑\n\n\
     마켓: KRX, NAS, NYS, AMS, BOND\n\
     조건: > [가격], < [가격], > [수익률%], < [수익률%]"
        .to_string()
}

// --- 포트폴리오 ---

async fn cmd_port(user_id: i64, args: &str, api: &ApiHandle) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let rest = parts.get(1..).unwrap_or(&[]).join(" ");
    match parts.first().copied() {
        Some("add")     => cmd_add(user_id, &rest),
        Some("remove")  => cmd_remove(user_id, &rest),
        Some("edit")    => cmd_edit(user_id, &rest),
        Some("list")    => cmd_list(user_id, api).await,
        Some("info")    => cmd_info(user_id, &rest, api).await,
        Some("summary") => cmd_summary(user_id, api).await,
        _ => "사용법:\n\
              /port add [마켓] [종목코드] [수량] [매입가] [종목명]\n\
              /port remove [종목코드]\n\
              /port edit [종목코드] [수량] [매입가]\n\
              /port list\n\
              /port info [종목코드]\n\
              /port summary"
            .to_string(),
    }
}

fn cmd_add(user_id: i64, args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() < 4 {
        return "사용법: /add [마켓] [종목코드] [수량] [매입가] [종목명]\n예: /add KRX 005930 10 70000 삼성전자".to_string();
    }

    let market = match Market::from_str(parts[0]) {
        Some(m) => m,
        None => return format!("알 수 없는 마켓: {}. (KRX/NAS/NYS/AMS/BOND)", parts[0]),
    };
    let symbol = parts[1].to_string();
    let quantity: f64 = match parts[2].parse() {
        Ok(v) => v,
        Err(_) => return "수량은 숫자여야 합니다.".to_string(),
    };
    let avg_price: f64 = match parts[3].parse() {
        Ok(v) => v,
        Err(_) => return "매입가는 숫자여야 합니다.".to_string(),
    };
    let name = if parts.len() > 4 {
        parts[4..].join(" ")
    } else {
        String::new()
    };

    let mut store = storage::load_portfolio(user_id);
    if store.holdings.iter().any(|h| h.symbol == symbol) {
        return format!("{symbol} 은(는) 이미 등록된 종목입니다. /edit 으로 수정하세요.");
    }

    store.holdings.push(Holding {
        market,
        symbol: symbol.clone(),
        name,
        quantity,
        avg_price,
        added_at: kst_now(),
        cached_price: None,
        cached_at: None,
    });

    if let Err(e) = storage::save_portfolio(user_id, &store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} ({market}) 추가 완료")
}

fn cmd_remove(user_id: i64, args: &str) -> String {
    let symbol = args.trim();
    if symbol.is_empty() {
        return "사용법: /remove [종목코드]".to_string();
    }

    let mut store = storage::load_portfolio(user_id);
    let before = store.holdings.len();
    store.holdings.retain(|h| h.symbol != symbol);

    if store.holdings.len() == before {
        return format!("{symbol} 을(를) 찾을 수 없습니다.");
    }

    if let Err(e) = storage::save_portfolio(user_id, &store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} 삭제 완료")
}

fn cmd_edit(user_id: i64, args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() < 3 {
        return "사용법: /edit [종목코드] [수량] [매입가]".to_string();
    }

    let symbol = parts[0];
    let quantity: f64 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return "수량은 숫자여야 합니다.".to_string(),
    };
    let avg_price: f64 = match parts[2].parse() {
        Ok(v) => v,
        Err(_) => return "매입가는 숫자여야 합니다.".to_string(),
    };

    let mut store = storage::load_portfolio(user_id);
    let holding = match store.holdings.iter_mut().find(|h| h.symbol == symbol) {
        Some(h) => h,
        None => return format!("{symbol} 을(를) 찾을 수 없습니다."),
    };

    holding.quantity = quantity;
    holding.avg_price = avg_price;

    if let Err(e) = storage::save_portfolio(user_id, &store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} 수정 완료 (수량: {quantity}, 매입가: {avg_price})")
}

async fn cmd_list(user_id: i64, api: &ApiHandle) -> String {
    let mut store = storage::load_portfolio(user_id);
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다. /add 로 종목을 추가하세요.".to_string();
    }

    let now = kst_now().format("%Y-%m-%d %H:%M").to_string();
    let mut msg = format!("📊 포트폴리오 현황\n{now} 기준\n");

    let usd_krw = api.get_exchange_rate().await.unwrap_or(1350.0);

    let mut domestic = Vec::new();
    let mut overseas = Vec::new();
    let mut bonds = Vec::new();
    let mut holdings_updated = false;
    let mut total_eval = 0.0f64;
    let mut total_cost = 0.0f64;
    let mut has_price = false;
    let mut has_cached = false;
    let mut failed_symbols: Vec<String> = Vec::new();

    for h in &mut store.holdings {
        match api.get_price_for_market(h.market, &h.symbol).await {
            Ok(price) => {
                if h.name.is_empty() && !price.name.is_empty() {
                    h.name = price.name.clone();
                }
                h.cached_price = Some(price.current_price);
                h.cached_at = Some(kst_now());
                holdings_updated = true;
                let eval = price.current_price * h.quantity;
                let cost = h.avg_price * h.quantity;
                match h.market {
                    Market::NAS | Market::NYS | Market::AMS => {
                        total_eval += eval * usd_krw;
                        total_cost += cost * usd_krw;
                    }
                    _ => {
                        total_eval += eval;
                        total_cost += cost;
                    }
                }
                has_price = true;
                let line = formatter::format_holding_line(h, &price, usd_krw);
                match h.market {
                    Market::KRX => domestic.push(line),
                    Market::NAS | Market::NYS | Market::AMS => overseas.push(line),
                    Market::BOND => bonds.push(line),
                }
            }
            Err(e) => {
                tracing::warn!("Failed to get price for {}: {e}", h.symbol);
                if let (Some(cp), Some(_)) = (h.cached_price, h.cached_at) {
                    // 캐시 가격 사용
                    let cached_price_data = PriceData {
                        name: h.name.clone(),
                        current_price: cp,
                        change: 0.0,
                        change_pct: 0.0,
                        volume: 0,
                    };
                    let eval = cp * h.quantity;
                    let cost = h.avg_price * h.quantity;
                    match h.market {
                        Market::NAS | Market::NYS | Market::AMS => {
                            total_eval += eval * usd_krw;
                            total_cost += cost * usd_krw;
                        }
                        _ => {
                            total_eval += eval;
                            total_cost += cost;
                        }
                    }
                    has_price = true;
                    has_cached = true;
                    let line = formatter::format_holding_line_cached(h, &cached_price_data, usd_krw);
                    match h.market {
                        Market::KRX => domestic.push(line),
                        Market::NAS | Market::NYS | Market::AMS => overseas.push(line),
                        Market::BOND => bonds.push(line),
                    }
                } else {
                    failed_symbols.push(h.symbol.clone());
                    let line = formatter::format_holding_line_no_price(h);
                    match h.market {
                        Market::KRX => domestic.push(line),
                        Market::NAS | Market::NYS | Market::AMS => overseas.push(line),
                        Market::BOND => bonds.push(line),
                    }
                }
            }
        }
    }

    if holdings_updated {
        if let Err(e) = storage::save_portfolio(user_id, &store) {
            tracing::warn!("Failed to save portfolio: {e}");
        }
    }

    if !domestic.is_empty() {
        msg.push_str("\n🇰🇷 국내\n");
        msg.push_str(&domestic.join("\n"));
    }
    if !overseas.is_empty() {
        msg.push_str("\n\n🇺🇸 미국\n");
        msg.push_str(&overseas.join("\n"));
    }
    if !bonds.is_empty() {
        msg.push_str("\n\n🏛 채권\n");
        msg.push_str(&bonds.join("\n"));
    }

    if has_price {
        let pnl = total_eval - total_cost;
        let pnl_pct = if total_cost > 0.0 { pnl / total_cost * 100.0 } else { 0.0 };
        let sign = if pnl >= 0.0 { "+" } else { "" };
        let partial = if !failed_symbols.is_empty() { " (일부 제외)" } else { "" };
        msg.push_str(&format!(
            "\n\n──────────\n💰 총 평가{partial}: {}원\n💵 총 손익: {sign}{}원 ({sign}{:.1}%)",
            formatter::fmt_quantity(total_eval),
            formatter::fmt_quantity(pnl),
            pnl_pct,
        ));
    }

    if has_cached {
        msg.push_str("\n* 직전 캐시 가격");
    }
    if !failed_symbols.is_empty() {
        msg.push_str(&format!(
            "\n⚠️ 시세 없음 (미포함): {}",
            failed_symbols.join(", ")
        ));
    }

    msg
}

async fn cmd_info(user_id: i64, args: &str, api: &ApiHandle) -> String {
    let symbol = args.trim();
    if symbol.is_empty() {
        return "사용법: /info [종목코드]".to_string();
    }

    let store = storage::load_portfolio(user_id);
    let holding = match store.holdings.iter().find(|h| h.symbol == symbol) {
        Some(h) => h,
        None => return format!("{symbol} 을(를) 포트폴리오에서 찾을 수 없습니다."),
    };

    let signal_store = storage::load_signals(user_id);
    let signals: Vec<&Signal> = signal_store
        .signals
        .iter()
        .filter(|s| s.symbol == symbol && s.active)
        .collect();

    match api.get_price_for_market(holding.market, symbol).await {
        Ok(price) => formatter::format_info(holding, &price, &signals),
        Err(e) => {
            tracing::warn!("Failed to get price for {symbol}: {e}");
            let display_name = if holding.name.is_empty() { "-" } else { &holding.name };
            let price_str = if let (Some(cp), Some(cat)) = (holding.cached_price, holding.cached_at) {
                format!("{}* (캐시 {})", formatter::fmt_price(&holding.market, cp), cat.format("%H:%M"))
            } else {
                "- (조회 불가)".to_string()
            };
            let mut msg = format!(
                "📈 {} {}\n매입가: {} × {}\n현재가: {}",
                holding.symbol, display_name,
                formatter::fmt_price(&holding.market, holding.avg_price),
                formatter::fmt_quantity(holding.quantity),
                price_str,
            );
            if !signals.is_empty() {
                msg.push_str("\n\n⚡ 설정된 시그널:");
                for s in &signals {
                    msg.push_str(&format!("\n• {} → 알림", s.condition.display_description()));
                }
            }
            msg
        }
    }
}

async fn cmd_summary(user_id: i64, api: &ApiHandle) -> String {
    let store = storage::load_portfolio(user_id);
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다.".to_string();
    }

    let usd_krw = api.get_exchange_rate().await.unwrap_or(1350.0);

    let mut domestic_val = 0.0f64;
    let mut overseas_val = 0.0f64;
    let mut bond_val = 0.0f64;
    let mut total_cost = 0.0f64;
    let mut has_cached = false;
    let mut failed_symbols: Vec<String> = Vec::new();

    for h in &store.holdings {
        let current_price = match api.get_price_for_market(h.market, &h.symbol).await {
            Ok(p) => p.current_price,
            Err(e) => {
                tracing::warn!("Summary: failed to get price for {}: {e}", h.symbol);
                if let Some(cp) = h.cached_price {
                    has_cached = true;
                    cp
                } else {
                    failed_symbols.push(h.symbol.clone());
                    continue;
                }
            }
        };
        let eval = current_price * h.quantity;
        let cost = h.avg_price * h.quantity;

        match h.market {
            Market::KRX => {
                domestic_val += eval;
                total_cost += cost;
            }
            Market::NAS | Market::NYS | Market::AMS => {
                overseas_val += eval * usd_krw;
                total_cost += cost * usd_krw;
            }
            Market::BOND => {
                bond_val += eval;
                total_cost += cost;
            }
        }
    }

    let total = domestic_val + overseas_val + bond_val;
    if total == 0.0 && !failed_symbols.is_empty() {
        return format!(
            "⚠️ 시세 조회에 실패했습니다.\n실패 종목: {}",
            failed_symbols.join(", ")
        );
    }

    let pnl = total - total_cost;
    let pnl_pct = if total_cost > 0.0 {
        pnl / total_cost * 100.0
    } else {
        0.0
    };

    let fmt_pct = |v: f64| {
        if total > 0.0 {
            format!("{:.0}%", v / total * 100.0)
        } else {
            "0%".to_string()
        }
    };

    let sign = if pnl >= 0.0 { "+" } else { "" };
    let partial = if failed_symbols.is_empty() { "" } else { " (일부 제외)" };

    let mut msg = format!(
        "📊 포트폴리오 요약{partial}\n\
         🇰🇷 국내: {}원 ({})\n\
         🇺🇸 미국: {}원 ({})\n\
         🏛 채권: {}원 ({})\n\
         ──────────\n\
         💰 총 평가: {}원\n\
         💵 총 손익: {sign}{}원 ({sign}{:.1}%)",
        formatter::fmt_int(domestic_val), fmt_pct(domestic_val),
        formatter::fmt_int(overseas_val), fmt_pct(overseas_val),
        formatter::fmt_int(bond_val),     fmt_pct(bond_val),
        formatter::fmt_int(total),
        formatter::fmt_int(pnl),
        pnl_pct,
    );

    if has_cached {
        msg.push_str("\n* 직전 캐시 가격");
    }
    if !failed_symbols.is_empty() {
        msg.push_str(&format!(
            "\n⚠️ 시세 없음 (제외됨): {}",
            failed_symbols.join(", ")
        ));
    }

    msg
}

// --- 시그널 ---

fn cmd_signal(user_id: i64, args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    match parts.first().copied() {
        Some("list") => cmd_signal_list(user_id),
        Some("remove") => cmd_signal_remove(user_id, parts.get(1).copied().unwrap_or("")),
        Some("clear") => cmd_signal_clear(user_id, parts.get(1).copied().unwrap_or("")),
        Some("add") => {
            if parts.len() < 4 {
                return "사용법: /signal add [종목코드] [> 또는 <] [값 또는 수익률%]\n\
                        예: /signal add 005930 > 80000\n\
                        예: /signal add 005930 > 10%"
                    .to_string();
            }
            let symbol = parts[1];
            let condition = match parse_condition(parts[2], &parts[3..]) {
                Ok(c) => c,
                Err(e) => return e,
            };
            let mut store = storage::load_signals(user_id);
            let id = store.next_signal_id();
            store.signals.push(Signal {
                id: id.clone(),
                symbol: symbol.to_string(),
                condition: condition.clone(),
                active: true,
                created_at: kst_now(),
            });
            if let Err(e) = storage::save_signals(user_id, &store) {
                return format!("저장 실패: {e}");
            }
            format!("✅ 시그널 설정 완료 [{id}]\n{symbol}: {}", condition.display_description())
        }
        _ => "사용법:\n\
              /signal add [종목코드] [> 또는 <] [값 또는 수익률%]\n\
              /signal list\n\
              /signal remove [시그널ID]\n\
              /signal clear [종목코드]"
            .to_string(),
    }
}

fn cmd_signal_list(user_id: i64) -> String {
    let store = storage::load_signals(user_id);
    if store.signals.is_empty() {
        return "설정된 시그널이 없습니다.".to_string();
    }

    let portfolio = storage::load_portfolio(user_id);

    let mut msg = "⚡ 시그널 목록\n".to_string();
    for s in &store.signals {
        let status = if s.active { "🟢" } else { "⚫" };
        let name = portfolio.holdings.iter()
            .find(|h| h.symbol == s.symbol)
            .map(|h| h.name.as_str())
            .unwrap_or("");
        let display = if name.is_empty() {
            s.symbol.clone()
        } else {
            format!("{} {}", s.symbol, name)
        };
        msg.push_str(&format!(
            "\n{status} [{}] {} — {}",
            s.id,
            display,
            s.condition.display_description()
        ));
    }
    msg
}

fn cmd_signal_remove(user_id: i64, args: &str) -> String {
    let signal_id = args.trim();
    if signal_id.is_empty() {
        return "사용법: /signal_remove [시그널ID]".to_string();
    }

    let mut store = storage::load_signals(user_id);
    let before = store.signals.len();
    store.signals.retain(|s| s.id != signal_id);

    if store.signals.len() == before {
        return format!("{signal_id} 을(를) 찾을 수 없습니다.");
    }

    if let Err(e) = storage::save_signals(user_id, &store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ 시그널 {signal_id} 삭제 완료")
}

fn cmd_signal_clear(user_id: i64, args: &str) -> String {
    let symbol = args.trim();
    if symbol.is_empty() {
        return "사용법: /signal_clear [종목코드]".to_string();
    }

    let mut store = storage::load_signals(user_id);
    let before = store.signals.len();
    store.signals.retain(|s| s.symbol != symbol);
    let removed = before - store.signals.len();

    if removed == 0 {
        return format!("{symbol}에 설정된 시그널이 없습니다.");
    }

    if let Err(e) = storage::save_signals(user_id, &store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} 시그널 {removed}개 삭제 완료")
}

fn cmd_status(user_id: i64) -> String {
    let portfolio = storage::load_portfolio(user_id);
    let signals = storage::load_signals(user_id);
    let active = signals.signals.iter().filter(|s| s.active).count();
    format!(
        "📊 시스템 상태\n\
         종목 수: {}\n\
         시그널: {} (활성 {})",
        portfolio.holdings.len(),
        signals.signals.len(),
        active,
    )
}

fn parse_condition(cond_type: &str, params: &[&str]) -> Result<Condition, String> {
    let value_str = params.get(0).ok_or("값을 입력하세요.")?;
    let is_percent = value_str.ends_with('%');
    let num_str = if is_percent { value_str.trim_end_matches('%') } else { value_str };
    let num: f64 = num_str.parse().map_err(|_| format!("잘못된 값: {value_str}"))?;

    match (cond_type, is_percent) {
        (">", false) => Ok(Condition::PriceAbove { target: num }),
        ("<", false) => Ok(Condition::PriceBelow { target: num }),
        (">", true)  => Ok(Condition::ProfitAbove { percentage: num }),
        ("<", true)  => Ok(Condition::ProfitBelow { percentage: num }),
        _ => Err(format!(
            "알 수 없는 조건: {cond_type}\n\
             사용 가능: > [가격], < [가격], > [수익률%], < [수익률%]"
        )),
    }
}

