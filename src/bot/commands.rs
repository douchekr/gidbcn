use anyhow::Result;
use chrono::{FixedOffset, Utc};
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

use crate::api::ApiHandle;
use crate::bot::formatter;
use crate::models::portfolio::{Holding, Market};
use crate::models::signal::{Condition, Signal};
use crate::storage;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "도움말")]
    Help,
    #[command(description = "종목 추가: /add [마켓] [종목코드] [수량] [매입가]")]
    Add(String),
    #[command(description = "종목 삭제: /remove [종목코드]")]
    Remove(String),
    #[command(description = "종목 수정: /edit [종목코드] [수량] [매입가]")]
    Edit(String),
    #[command(description = "포트폴리오 현황")]
    List,
    #[command(description = "종목 상세: /info [종목코드]")]
    Info(String),
    #[command(description = "포트폴리오 요약")]
    Summary,
    #[command(description = "시그널 설정: /signal [종목코드] [조건] [파라미터...]")]
    Signal(String),
    #[command(description = "시그널 목록")]
    SignalList,
    #[command(description = "시그널 삭제: /signal_remove [시그널ID]")]
    SignalRemove(String),
    #[command(description = "종목 시그널 전체 삭제: /signal_clear [종목코드]")]
    SignalClear(String),
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
    let reply = match cmd {
        Command::Help => help_text(),
        Command::Ping => "pong".to_string(),
        Command::Add(args) => cmd_add(&args),
        Command::Remove(args) => cmd_remove(&args),
        Command::Edit(args) => cmd_edit(&args),
        Command::List => cmd_list(&api).await,
        Command::Info(args) => cmd_info(&args, &api).await,
        Command::Summary => cmd_summary(&api).await,
        Command::Signal(args) => cmd_signal(&args),
        Command::SignalList => cmd_signal_list(),
        Command::SignalRemove(args) => cmd_signal_remove(&args),
        Command::SignalClear(args) => cmd_signal_clear(&args),
        Command::Status => cmd_status(),
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
     /add [마켓] [종목코드] [수량] [매입가]\n\
     /remove [종목코드]\n\
     /edit [종목코드] [수량] [매입가]\n\
     /list — 전체 포트폴리오\n\
     /info [종목코드] — 종목 상세\n\
     /summary — 자산배분 요약\n\n\
     시그널:\n\
     /signal [종목코드] [조건] [파라미터...]\n\
     /signal_list — 전체 시그널\n\
     /signal_remove [시그널ID]\n\
     /signal_clear [종목코드]\n\n\
     시스템:\n\
     /status — 시스템 상태\n\
     /ping — 핑\n\n\
     마켓: KRX, NAS, NYS, AMS, BOND\n\
     조건: price_above, price_below, profit_above, profit_below,\n\
     golden_cross, dead_cross, rsi_above, rsi_below, volume_surge"
        .to_string()
}

// --- 포트폴리오 ---

fn cmd_add(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() < 4 {
        return "사용법: /add [마켓] [종목코드] [수량] [매입가]\n예: /add KRX 005930 10 70000".to_string();
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

    let mut store = storage::load_portfolio();
    if store.holdings.iter().any(|h| h.symbol == symbol) {
        return format!("{symbol} 은(는) 이미 등록된 종목입니다. /edit 으로 수정하세요.");
    }

    let id = store.next_holding_id();
    store.holdings.push(Holding {
        id: id.clone(),
        market,
        symbol: symbol.clone(),
        quantity,
        avg_price,
        added_at: kst_now(),
    });

    if let Err(e) = storage::save_portfolio(&store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} ({market}) 추가 완료 [{id}]")
}

fn cmd_remove(args: &str) -> String {
    let symbol = args.trim();
    if symbol.is_empty() {
        return "사용법: /remove [종목코드]".to_string();
    }

    let mut store = storage::load_portfolio();
    let before = store.holdings.len();
    store.holdings.retain(|h| h.symbol != symbol);

    if store.holdings.len() == before {
        return format!("{symbol} 을(를) 찾을 수 없습니다.");
    }

    if let Err(e) = storage::save_portfolio(&store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} 삭제 완료")
}

fn cmd_edit(args: &str) -> String {
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

    let mut store = storage::load_portfolio();
    let holding = match store.holdings.iter_mut().find(|h| h.symbol == symbol) {
        Some(h) => h,
        None => return format!("{symbol} 을(를) 찾을 수 없습니다."),
    };

    holding.quantity = quantity;
    holding.avg_price = avg_price;

    if let Err(e) = storage::save_portfolio(&store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} 수정 완료 (수량: {quantity}, 매입가: {avg_price})")
}

async fn cmd_list(api: &ApiHandle) -> String {
    let store = storage::load_portfolio();
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다. /add 로 종목을 추가하세요.".to_string();
    }

    let now = kst_now().format("%Y-%m-%d %H:%M").to_string();
    let mut msg = format!("📊 포트폴리오 현황\n{now} 기준\n");

    let usd_krw = api.get_exchange_rate().await.unwrap_or(1350.0);

    let mut domestic = Vec::new();
    let mut overseas = Vec::new();
    let mut bonds = Vec::new();

    for h in &store.holdings {
        let price = match api.get_price_for_market(h.market, &h.symbol).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to get price for {}: {e}", h.symbol);
                continue;
            }
        };
        let line = formatter::format_holding_line(h, &price, usd_krw);
        match h.market {
            Market::KRX => domestic.push(line),
            Market::NAS | Market::NYS | Market::AMS => overseas.push(line),
            Market::BOND => bonds.push(line),
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

    msg
}

async fn cmd_info(args: &str, api: &ApiHandle) -> String {
    let symbol = args.trim();
    if symbol.is_empty() {
        return "사용법: /info [종목코드]".to_string();
    }

    let store = storage::load_portfolio();
    let holding = match store.holdings.iter().find(|h| h.symbol == symbol) {
        Some(h) => h,
        None => return format!("{symbol} 을(를) 포트폴리오에서 찾을 수 없습니다."),
    };

    let price = match api.get_price_for_market(holding.market, symbol).await {
        Ok(p) => p,
        Err(e) => return format!("시세 조회 실패: {e}"),
    };

    let signal_store = storage::load_signals();
    let signals: Vec<&Signal> = signal_store
        .signals
        .iter()
        .filter(|s| s.symbol == symbol && s.active)
        .collect();

    formatter::format_info(holding, &price, &signals)
}

async fn cmd_summary(api: &ApiHandle) -> String {
    let store = storage::load_portfolio();
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다.".to_string();
    }

    let usd_krw = api.get_exchange_rate().await.unwrap_or(1350.0);

    let mut domestic_val = 0.0f64;
    let mut overseas_val = 0.0f64;
    let mut bond_val = 0.0f64;
    let mut total_cost = 0.0f64;

    for h in &store.holdings {
        let price = match api.get_price_for_market(h.market, &h.symbol).await {
            Ok(p) => p,
            Err(_) => continue,
        };
        let eval = price.current_price * h.quantity;
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

    format!(
        "📊 포트폴리오 요약\n\
         🇰🇷 국내: {:.0}원 ({})\n\
         🇺🇸 미국: {:.0}원 ({})\n\
         🏛 채권: {:.0}원 ({})\n\
         ──────────\n\
         💰 총 평가: {:.0}원\n\
         💵 총 손익: {sign}{:.0}원 ({sign}{:.1}%)",
        domestic_val,
        fmt_pct(domestic_val),
        overseas_val,
        fmt_pct(overseas_val),
        bond_val,
        fmt_pct(bond_val),
        total,
        pnl,
        pnl_pct,
    )
}

// --- 시그널 ---

fn cmd_signal(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() < 3 {
        return "사용법: /signal [종목코드] [조건타입] [파라미터...]\n\
                예: /signal 005930 price_above 80000\n\
                예: /signal TSLA golden_cross 5 20"
            .to_string();
    }

    let symbol = parts[0];
    let condition = match parse_condition(parts[1], &parts[2..]) {
        Ok(c) => c,
        Err(e) => return e,
    };

    let mut store = storage::load_signals();
    let id = store.next_signal_id();
    store.signals.push(Signal {
        id: id.clone(),
        symbol: symbol.to_string(),
        condition: condition.clone(),
        active: true,
        created_at: kst_now(),
    });

    if let Err(e) = storage::save_signals(&store) {
        return format!("저장 실패: {e}");
    }

    format!(
        "✅ 시그널 설정 완료 [{id}]\n{symbol}: {}",
        condition.display_description()
    )
}

fn cmd_signal_list() -> String {
    let store = storage::load_signals();
    if store.signals.is_empty() {
        return "설정된 시그널이 없습니다.".to_string();
    }

    let mut msg = "⚡ 시그널 목록\n".to_string();
    for s in &store.signals {
        let status = if s.active { "🟢" } else { "⚫" };
        msg.push_str(&format!(
            "\n{status} [{}] {} — {}",
            s.id,
            s.symbol,
            s.condition.display_description()
        ));
    }
    msg
}

fn cmd_signal_remove(args: &str) -> String {
    let signal_id = args.trim();
    if signal_id.is_empty() {
        return "사용법: /signal_remove [시그널ID]".to_string();
    }

    let mut store = storage::load_signals();
    let before = store.signals.len();
    store.signals.retain(|s| s.id != signal_id);

    if store.signals.len() == before {
        return format!("{signal_id} 을(를) 찾을 수 없습니다.");
    }

    if let Err(e) = storage::save_signals(&store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ 시그널 {signal_id} 삭제 완료")
}

fn cmd_signal_clear(args: &str) -> String {
    let symbol = args.trim();
    if symbol.is_empty() {
        return "사용법: /signal_clear [종목코드]".to_string();
    }

    let mut store = storage::load_signals();
    let before = store.signals.len();
    store.signals.retain(|s| s.symbol != symbol);
    let removed = before - store.signals.len();

    if removed == 0 {
        return format!("{symbol}에 설정된 시그널이 없습니다.");
    }

    if let Err(e) = storage::save_signals(&store) {
        return format!("저장 실패: {e}");
    }

    format!("✅ {symbol} 시그널 {removed}개 삭제 완료")
}

fn cmd_status() -> String {
    let portfolio = storage::load_portfolio();
    let signals = storage::load_signals();
    let active = signals.signals.iter().filter(|s| s.active).count();
    let alerts = storage::load_alert_log();

    format!(
        "📊 시스템 상태\n\
         종목 수: {}\n\
         시그널: {} (활성 {})\n\
         알림 기록: {}건",
        portfolio.holdings.len(),
        signals.signals.len(),
        active,
        alerts.alerts.len(),
    )
}

fn parse_condition(cond_type: &str, params: &[&str]) -> Result<Condition, String> {
    match cond_type {
        "price_above" => {
            let target = parse_param_f64(params, 0, "target")?;
            Ok(Condition::PriceAbove { target })
        }
        "price_below" => {
            let target = parse_param_f64(params, 0, "target")?;
            Ok(Condition::PriceBelow { target })
        }
        "profit_above" => {
            let percentage = parse_param_f64(params, 0, "percentage")?;
            Ok(Condition::ProfitAbove { percentage })
        }
        "profit_below" => {
            let percentage = parse_param_f64(params, 0, "percentage")?;
            Ok(Condition::ProfitBelow { percentage })
        }
        "golden_cross" => {
            let short = parse_param_u32(params, 0, "short_period")?;
            let long = parse_param_u32(params, 1, "long_period")?;
            Ok(Condition::GoldenCross {
                short_period: short,
                long_period: long,
            })
        }
        "dead_cross" => {
            let short = parse_param_u32(params, 0, "short_period")?;
            let long = parse_param_u32(params, 1, "long_period")?;
            Ok(Condition::DeadCross {
                short_period: short,
                long_period: long,
            })
        }
        "rsi_above" => {
            let threshold = parse_param_f64(params, 0, "threshold")?;
            Ok(Condition::RsiAbove { threshold })
        }
        "rsi_below" => {
            let threshold = parse_param_f64(params, 0, "threshold")?;
            Ok(Condition::RsiBelow { threshold })
        }
        "volume_surge" => {
            let threshold_pct = parse_param_f64(params, 0, "threshold_pct")?;
            Ok(Condition::VolumeSurge { threshold_pct })
        }
        _ => Err(format!(
            "알 수 없는 조건: {cond_type}\n\
             사용 가능: price_above, price_below, profit_above, profit_below, \
             golden_cross, dead_cross, rsi_above, rsi_below, volume_surge"
        )),
    }
}

fn parse_param_f64(params: &[&str], idx: usize, name: &str) -> Result<f64, String> {
    params
        .get(idx)
        .ok_or_else(|| format!("{name} 파라미터가 필요합니다."))?
        .parse::<f64>()
        .map_err(|_| format!("{name}은(는) 숫자여야 합니다."))
}

fn parse_param_u32(params: &[&str], idx: usize, name: &str) -> Result<u32, String> {
    params
        .get(idx)
        .ok_or_else(|| format!("{name} 파라미터가 필요합니다."))?
        .parse::<u32>()
        .map_err(|_| format!("{name}은(는) 정수여야 합니다."))
}
