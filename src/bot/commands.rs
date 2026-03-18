use anyhow::Result;
use chrono::{FixedOffset, Utc};
use teloxide::prelude::*;
use teloxide::types::InputFile;
use teloxide::utils::command::BotCommands;

use std::sync::Arc;

use crate::api::ApiHandle;
use crate::bot::formatter;
use crate::config::BootConfig;
use crate::models::messages::PriceData;
use crate::models::portfolio::{Holding, Market};
use crate::models::signal::{Condition, Signal};
use crate::watchlist::{db as wdb, models::PromptType};
use uuid::Uuid;
use crate::storage;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum Command {
    #[command(description = "봇 시작")]
    Start,
    #[command(description = "도움말")]
    Help,
    #[command(description = "포트폴리오: /port add|rm|e|ls|i|sum|ex ...")]
    Port(String),
    #[command(description = "포트폴리오 단축 (/p)")]
    P(String),
    #[command(description = "시그널: /signal add|ls|rm|cls|prn ...")]
    Signal(String),
    #[command(description = "시그널 단축 (/s)")]
    S(String),
    #[command(description = "시스템 상태")]
    Status,
    #[command(description = "시스템 상태 단축 (/st)")]
    St,
    #[command(description = "핑")]
    Ping,
    #[command(description = "사용자 관리 (오너 전용): /user add|rm|ls")]
    User(String),
    #[command(description = "워치리스트: /watch run|ls|pending|info|bl|budget|prompt|history")]
    Watch(String),
    #[command(description = "워치리스트 단축 (/w)")]
    W(String),
    #[command(description = "잠금 해제 (오너 전용)")]
    Unlock(String),
    #[command(description = "설정 암호화 (오너 전용)")]
    Encrypt(String),
}

pub async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    api: ApiHandle,
    discovery_enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    discovery_trigger: std::sync::Arc<tokio::sync::Notify>,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    let user_id = match &msg.from {
        Some(user) => user.id.0 as i64,
        None => {
            bot.send_message(chat_id, "사용자 정보를 확인할 수 없습니다.").await?;
            return Ok(());
        }
    };

    let owner_chat_id = storage::with_config(|c| c.telegram.owner_chat_id);

    // 접근 제어
    let effective_owner = if owner_chat_id == 0 {
        // 오너 미설정: 첫 메시지 발신자를 오너로 자동 등록
        if let Err(e) = storage::update_config(|c| {
            c.telegram.owner_chat_id = user_id;
        }) {
            bot.send_message(chat_id, format!("⚠️ 오너 등록 실패: {e:#}")).await?;
            return Ok(());
        }
        tracing::info!("Owner auto-registered: {user_id}");
        bot.send_message(chat_id, format!(
            "✅ 봇 오너로 등록되었습니다. (chat_id: {user_id})"
        )).await?;
        // 평문 모드 경고
        if !storage::is_encrypted() {
            bot.send_message(chat_id,
                "⚠️ 평문 모드로 실행 중입니다.\n\
                 config.json에 API 키가 평문으로 저장되어 있어요.\n\n\
                 /encrypt [패스프레이즈] 로 암호화하세요."
            ).await?;
        }
        user_id
    } else {
        owner_chat_id
    };

    let is_owner = user_id == effective_owner;
    let is_allowed = is_owner || storage::with_config(|c| c.telegram.users.contains(&user_id));

    if !is_allowed {
        bot.send_message(chat_id, format!(
            "접근 권한이 없습니다.\n(chat_id: {user_id})"
        )).await?;
        return Ok(());
    }

    // export: CSV 파일로 전송
    if let Command::Port(ref args) | Command::P(ref args) = cmd {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.first().copied() == Some("export") || parts.first().copied() == Some("ex") {
            let rest = parts.get(1..).unwrap_or(&[]).join(" ").to_uppercase();
            let csv = cmd_export(user_id, &rest);
            if csv.starts_with('\u{FEFF}') {
                // CSV 데이터 → 파일 전송
                let fname = chrono::Local::now().format("portfolio_%Y%m%d_%H%M.csv").to_string();
                let file = InputFile::memory(csv.into_bytes()).file_name(fname);
                bot.send_document(chat_id, file).await?;
            } else {
                // 에러 메시지 → 텍스트 전송
                bot.send_message(chat_id, csv).await?;
            }
            return Ok(());
        }
    }

    // /w export: CSV 파일로 전송
    if let Command::Watch(ref args) | Command::W(ref args) = cmd {
        let parts: Vec<&str> = args.split_whitespace().collect();
        if parts.first().copied() == Some("export") || parts.first().copied() == Some("ex") {
            if !is_owner {
                bot.send_message(chat_id, "이 명령어는 봇 오너만 사용할 수 있습니다.").await?;
                return Ok(());
            }
            let csv = cmd_watch_export();
            if csv.starts_with('\u{FEFF}') {
                let fname = chrono::Local::now().format("watchlist_%Y%m%d_%H%M.csv").to_string();
                let file = InputFile::memory(csv.into_bytes()).file_name(fname);
                bot.send_document(chat_id, file).await?;
            } else {
                bot.send_message(chat_id, csv).await?;
            }
            return Ok(());
        }
    }

    let reply = match cmd {
        Command::Start | Command::Help => help_text(),
        Command::Ping => "pong".to_string(),
        Command::Port(args) | Command::P(args) => cmd_port(user_id, &args, &api).await,
        Command::Signal(args) | Command::S(args) => cmd_signal(user_id, &args),
        Command::Watch(args) | Command::W(args) => {
            if !is_owner {
                "이 명령어는 봇 오너만 사용할 수 있습니다.".to_string()
            } else {
                cmd_watchlist(&args, &api, &discovery_enabled, &discovery_trigger).await
            }
        }
        Command::Status | Command::St => cmd_status(user_id),
        Command::User(args) => {
            if !is_owner {
                "이 명령어는 봇 오너만 사용할 수 있습니다.".to_string()
            } else {
                cmd_user(&args)
            }
        }
        Command::Unlock(_) => "이미 잠금 해제 상태입니다.".to_string(),
        Command::Encrypt(args) => {
            if !is_owner {
                "이 명령어는 봇 오너만 사용할 수 있습니다.".to_string()
            } else {
                cmd_encrypt(&bot, chat_id, msg.id, &args).await
            }
        }
    };

    if reply.contains("<pre>") {
        bot.send_message(chat_id, reply)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?;
    } else {
        bot.send_message(chat_id, reply).await?;
    }
    Ok(())
}

fn kst_now() -> chrono::DateTime<FixedOffset> {
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    Utc::now().with_timezone(&kst)
}

fn help_text() -> String {
    "📋 명령어 목록\n\n\
     포트폴리오 (/port 또는 /p):\n\
     /p add|a [마켓] [종목코드] [수량] [매입가] [@계좌]\n\
     /p rm [종목코드|*] [@계좌]\n\
     /p e [종목코드] [수량] [매입가] [@계좌]\n\
     /p ls [@계좌] — 전체 포트폴리오\n\
     /p i [종목코드] [@계좌] — 종목 상세\n\
     /p sum — 자산배분 요약\n\
     /p ex [@계좌] — 구글 시트 붙여넣기용\n\n\
     시그널 (/signal 또는 /s):\n\
     /s add|a [종목코드] [> 또는 <] [값 또는 수익률%] [@계좌]\n\
     /s ls — 전체 시그널\n\
     /s rm [번호]  ← 여러 개: /s rm 1 2\n\
     /s cls [종목코드] [@계좌]\n\
     /s prn — 비활성 시그널 전체 삭제\n\n\
     시스템:\n\
     /status|st — 시스템 상태\n\
     /ping — 핑\n\n\
     워치리스트 (/watch 또는 /w):\n\
     /w run — 디스커버리 사이클\n\
     /w ls — 평가 완료 종목\n\
     /w pending — 대기 중 후보\n\
     /w info [TICKER] — 종목 상세\n\
     /w bl — 블랙리스트 관리\n\
     /w budget — Gemini 사용량\n\
     /w prompt hunt|judge show|set\n\
     /w hist — 호출 이력\n\n\
     사용자 관리 (/user, 오너 전용):\n\
     /user add|a [chat_id]\n\
     /user rm [chat_id]\n\
     /user ls\n\n\
     마켓: KRX, NAS, NYS, AMS, BOND, CART\n\
     조건: > [가격], < [가격], > [수익률%], < [수익률%]\n\n\
     📂 계좌 구분 (@계좌):\n\
     동일 종목을 여러 계좌(IRP, 일반 등)에 나눠 보유할 때 사용.\n\
     /p a KRX 005930 10 70000 @IRP\n\
     /s a 005930 > 80000 @IRP\n\
     • @계좌 미지정 시 — 종목이 1개 계좌에만 있으면 자동 적용\n\
     • @계좌 미지정 시 — 여러 계좌에 있으면 계좌 지정 요청\n\n\
     ※ BOND 수량/매입가 단위:\n\
       수량 = 액면가 1,000원 단위 (예: 50000 → 액면 5,000만원)\n\
       매입가 = 10,000원 액면 기준 가격 (예: 7435)\n\n\
     ※ CART (수동 관리):\n\
       /p a CART 비트코인 2 50000000 @코인 =55000000\n\
       /p e 비트코인 2 50000000 =55000000 @코인\n\
       이름=종목코드. =현재가·@계좌는 순서 자유. 생략 시 매입가 사용.\n\
       시세 자동 조회·시그널 불가."
        .to_string()
}

/// args에서 @계좌, =현재가 프리픽스 토큰을 추출, 나머지 반환
/// 순서 무관 — @, = 중 어떤 것이 먼저 와도 동일하게 파싱
fn extract_options<'a>(parts: &[&'a str]) -> (Vec<&'a str>, String, Option<&'a str>) {
    let mut rest = Vec::new();
    let mut account = String::new();
    let mut current_price_str: Option<&'a str> = None;
    for &p in parts {
        if let Some(a) = p.strip_prefix('@') {
            account = a.to_string();
        } else if let Some(cp) = p.strip_prefix('=') {
            current_price_str = Some(cp);
        } else {
            rest.push(p);
        }
    }
    (rest, account, current_price_str)
}

/// @계좌만 추출 (=현재가 무시) — 대부분의 커맨드용
fn extract_account<'a>(parts: &[&'a str]) -> (Vec<&'a str>, String) {
    let (rest, account, _) = extract_options(parts);
    (rest, account)
}

fn account_tag(account: &str) -> String {
    if account.is_empty() { String::new() } else { format!(" [@{account}]") }
}

// --- 포트폴리오 ---

async fn cmd_port(user_id: i64, args: &str, api: &ApiHandle) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    // 서브커맨드 이후 인자: 종목코드 등 대문자 정규화 (한글/숫자는 영향 없음)
    let rest = parts.get(1..).unwrap_or(&[]).join(" ").to_uppercase();
    match parts.first().copied() {
        Some("add") | Some("a")        => cmd_add(user_id, &rest, api).await,
        Some("remove") | Some("rm")    => cmd_remove(user_id, &rest),
        Some("edit") | Some("e")       => cmd_edit(user_id, &rest),
        Some("list") | Some("ls")      => cmd_list(user_id, &rest, api).await,
        Some("info") | Some("i")       => cmd_info(user_id, &rest, api).await,
        Some("summary") | Some("sum")  => cmd_summary(user_id, api).await,
        Some("export") | Some("ex")    => cmd_export(user_id, &rest),
        _ => "사용법:\n\
              /p add (a) [마켓] [종목코드] [수량] [매입가] [@계좌]\n\
              /p rm [종목코드|*] [@계좌]\n\
              /p e [종목코드] [수량] [매입가] [@계좌]\n\
              /p ls [@계좌]\n\
              /p i [종목코드] [@계좌]\n\
              /p sum\n\
              /p ex [@계좌]"
            .to_string(),
    }
}

async fn cmd_add(user_id: i64, args: &str, api: &ApiHandle) -> String {
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (parts, account, cp_str) = extract_options(&raw);
    if parts.len() < 4 {
        return "사용법: /port add [마켓] [종목코드] [수량] [매입가] [@계좌]\n예: /port add KRX 005930 10 70000 @IRP".to_string();
    }

    let market = match Market::from_str(parts[0]) {
        Some(m) => m,
        None => return format!("알 수 없는 마켓: {}. (KRX/NAS/NYS/AMS/BOND/CART)", parts[0]),
    };

    let is_cart = market == Market::CART;
    if !is_cart && cp_str.is_some() {
        return "=현재가는 CART 마켓 종목만 사용할 수 있습니다.".to_string();
    }

    let symbol = parts[1].to_string();
    let quantity: f64 = match parts[2].parse() {
        Ok(v) => v,
        Err(_) => return "수량은 숫자여야 합니다.".to_string(),
    };
    let avg_price: f64 = match parts[3].parse() {
        Ok(v) => v,
        Err(_) => return "매입가는 숫자여야 합니다.".to_string(),
    };

    // CART: =현재가 프리픽스 파싱 (생략 시 매입가) + 종목명 = symbol
    let (name, cached_price, cached_at) = if is_cart {
        let cp = if let Some(s) = cp_str {
            match s.parse::<f64>() {
                Ok(v) => v,
                Err(_) => return "현재가는 숫자여야 합니다. (예: =55000000)".to_string(),
            }
        } else {
            avg_price
        };
        (symbol.clone(), Some(cp), Some(kst_now()))
    } else {
        // 종목명 API 조회 (실패 시 존재하지 않는 종목으로 판단)
        let n = match api.get_stock_name(market, &symbol).await {
            Ok(n) if !n.is_empty() => n,
            Ok(_) => return format!("종목을 찾을 수 없습니다: {symbol}"),
            Err(e) => return format!("종목 조회 실패: {symbol} ({e:#})"),
        };
        (n, None, None)
    };

    let mut store = storage::load_portfolio(user_id);
    if store.holdings.iter().any(|h| h.symbol == symbol && h.account == account) {
        return format!("{symbol}{} 은(는) 이미 등록된 종목입니다. /port edit 으로 수정하세요.", account_tag(&account));
    }

    store.holdings.push(Holding {
        market,
        symbol: symbol.clone(),
        name: name.clone(),
        account: account.clone(),
        quantity,
        avg_price,
        added_at: kst_now(),
        cached_price,
        cached_at,
    });

    if let Err(e) = storage::save_portfolio(user_id, &store) {
        return format!("저장 실패: {e:#}");
    }

    let name_part = if name.is_empty() || name == symbol { String::new() } else { format!(" {name}") };
    format!("✅ {symbol}{name_part} ({market}){} 추가 완료", account_tag(&account))
}

fn cmd_remove(user_id: i64, args: &str) -> String {
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (parts, account) = extract_account(&raw);
    let symbol = parts.first().copied().unwrap_or("").trim();
    if symbol.is_empty() {
        return "사용법: /port remove [종목코드|*] [@계좌]".to_string();
    }

    // * 와일드카드: 전체 삭제 (계좌 지정 시 해당 계좌만)
    if symbol == "*" {
        let mut store = storage::load_portfolio(user_id);
        let before = store.holdings.len();
        if account.is_empty() {
            store.holdings.clear();
        } else {
            store.holdings.retain(|h| h.account != account);
        }
        let removed = before - store.holdings.len();
        if removed == 0 {
            return if account.is_empty() {
                "삭제할 종목이 없습니다.".to_string()
            } else {
                format!("{} 에 삭제할 종목이 없습니다.", account_tag(&account))
            };
        }
        if let Err(e) = storage::save_portfolio(user_id, &store) {
            return format!("저장 실패: {e:#}");
        }
        let mut signal_store = storage::load_signals(user_id);
        let sig_before = signal_store.signals.len();
        if account.is_empty() {
            signal_store.signals.clear();
        } else {
            signal_store.signals.retain(|s| s.account != account);
        }
        let sig_removed = sig_before - signal_store.signals.len();
        if sig_removed > 0 {
            if let Err(e) = storage::save_signals(user_id, &signal_store) {
                tracing::warn!("Failed to save signals after port remove * (user {}): {e:#}", user_id);
            }
        }
        let sig_note = if sig_removed > 0 { format!(" (시그널 {}개 함께 삭제)", sig_removed) } else { String::new() };
        let scope = if account.is_empty() { "전체".to_string() } else { format!("{} 계좌", account_tag(&account)) };
        return format!("✅ {scope} 종목 {}개 삭제 완료{sig_note}", removed);
    }

    let mut store = storage::load_portfolio(user_id);
    let before = store.holdings.len();

    if account.is_empty() {
        let count = store.holdings.iter().filter(|h| h.symbol == symbol).count();
        if count > 1 {
            let accts: Vec<String> = store.holdings.iter()
                .filter(|h| h.symbol == symbol)
                .map(|h| if h.account.is_empty() { "기본".to_string() } else { format!("@{}", h.account) })
                .collect();
            return format!("{symbol}이(가) 여러 계좌에 있습니다: {}\n계좌를 지정하세요. 예: /port remove {symbol} @계좌명", accts.join(", "));
        }
        store.holdings.retain(|h| h.symbol != symbol);
    } else {
        store.holdings.retain(|h| !(h.symbol == symbol && h.account == account));
    }

    if store.holdings.len() == before {
        return format!("{symbol}{} 을(를) 찾을 수 없습니다.", account_tag(&account));
    }

    if let Err(e) = storage::save_portfolio(user_id, &store) {
        return format!("저장 실패: {e:#}");
    }

    // 관련 시그널 삭제 (같은 symbol + account 매칭)
    let mut signal_store = storage::load_signals(user_id);
    let sig_before = signal_store.signals.len();
    if account.is_empty() {
        signal_store.signals.retain(|s| s.symbol != symbol);
    } else {
        signal_store.signals.retain(|s| !(s.symbol == symbol && s.account == account));
    }
    let sig_removed = sig_before - signal_store.signals.len();
    if sig_removed > 0 {
        if let Err(e) = storage::save_signals(user_id, &signal_store) {
            tracing::warn!("Failed to save signals after port remove (user {}): {e:#}", user_id);
        }
    }

    let sig_note = if sig_removed > 0 {
        format!(" (시그널 {}개 함께 삭제)", sig_removed)
    } else {
        String::new()
    };
    format!("✅ {symbol}{} 삭제 완료{sig_note}", account_tag(&account))
}

fn cmd_edit(user_id: i64, args: &str) -> String {
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (parts, account, cp_str) = extract_options(&raw);
    if parts.len() < 3 {
        return "사용법: /port edit [종목코드] [수량] [매입가] [@계좌] [=현재가]".to_string();
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

    // 계좌 미지정 시 동일 symbol이 여러 계좌에 있으면 지정 요청
    if account.is_empty() {
        let count = store.holdings.iter().filter(|h| h.symbol == symbol).count();
        if count > 1 {
            let accts: Vec<String> = store.holdings.iter()
                .filter(|h| h.symbol == symbol)
                .map(|h| if h.account.is_empty() { "기본".to_string() } else { format!("@{}", h.account) })
                .collect();
            return format!("{symbol}이(가) 여러 계좌에 있습니다: {}\n계좌를 지정하세요. 예: /port edit {symbol} {quantity} {avg_price} @계좌명", accts.join(", "));
        }
    }

    let holding = match store.holdings.iter_mut().find(|h| {
        h.symbol == symbol && (account.is_empty() || h.account == account)
    }) {
        Some(h) => h,
        None => return format!("{symbol}{} 을(를) 찾을 수 없습니다.", account_tag(&account)),
    };

    let market = holding.market;
    holding.quantity = quantity;
    holding.avg_price = avg_price;

    // =현재가 프리픽스로 현재가 갱신 (CART 마켓 전용)
    let cached_price_val = if let Some(s) = cp_str {
        if market != Market::CART {
            return "=현재가는 CART 마켓 종목만 사용할 수 있습니다.".to_string();
        }
        match s.parse::<f64>() {
            Ok(cp) => {
                holding.cached_price = Some(cp);
                holding.cached_at = Some(kst_now());
                Some(cp)
            }
            Err(_) => return "현재가는 숫자여야 합니다. (예: =55000000)".to_string(),
        }
    } else {
        None
    };

    if let Err(e) = storage::save_portfolio(user_id, &store) {
        return format!("저장 실패: {e:#}");
    }

    let price_note = match cached_price_val {
        Some(cp) => format!(", 현재가: {}", formatter::fmt_price(&market, cp)),
        None => String::new(),
    };
    format!(
        "✅ {symbol}{} 수정 완료 (수량: {}, 매입가: {}{})",
        account_tag(&account),
        formatter::fmt_quantity(quantity),
        formatter::fmt_price(&market, avg_price),
        price_note,
    )
}

async fn cmd_list(user_id: i64, args: &str, api: &ApiHandle) -> String {
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (_, account_filter) = extract_account(&raw);

    let mut store = storage::load_portfolio(user_id);
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다. /port add 로 종목을 추가하세요.".to_string();
    }

    // 계좌 필터: 표시 대상 인덱스만 추림 (store 자체는 변경 안 함 — 캐시 저장 시 전체 보존)
    let indices: Vec<usize> = if account_filter.is_empty() {
        (0..store.holdings.len()).collect()
    } else {
        store.holdings.iter().enumerate()
            .filter(|(_, h)| h.account == account_filter)
            .map(|(i, _)| i)
            .collect()
    };
    if indices.is_empty() {
        return format!("@{account_filter} 계좌에 종목이 없습니다.");
    }

    let now = kst_now().format("%Y-%m-%d %H:%M").to_string();
    let acct_label = if account_filter.is_empty() { String::new() } else { format!(" [@{}]", account_filter) };
    let mut msg = format!("📊 포트폴리오 현황{acct_label}\n{now} 기준\n");

    let usd_krw = api.get_exchange_rate().await.unwrap_or(1350.0);

    let mut domestic = Vec::new();
    let mut overseas = Vec::new();
    let mut bonds = Vec::new();
    let mut etc = Vec::new();
    let mut holdings_updated = false;
    let mut total_eval = 0.0f64;
    let mut total_cost = 0.0f64;
    let mut has_price = false;
    let mut has_cached = false;
    let mut failed_symbols: Vec<String> = Vec::new();

    for &idx in &indices {
        let h = &mut store.holdings[idx];
        // CART: API 호출 없이 cached_price 직접 사용
        let price_result = if h.market == Market::CART {
            match h.cached_price {
                Some(cp) => Ok(PriceData { name: h.name.clone(), current_price: cp, change_pct: 0.0 }),
                None => Err(anyhow::anyhow!("현재가 미설정")),
            }
        } else {
            api.get_price_for_market(h.market, &h.symbol).await
        };

        match price_result {
            Ok(price) => {
                if h.name.is_empty() && !price.name.is_empty() {
                    h.name = price.name.clone();
                }
                if h.market != Market::CART {
                    h.cached_price = Some(price.current_price);
                    h.cached_at = Some(kst_now());
                    holdings_updated = true;
                }
                let factor = h.market.value_factor();
                let eval = price.current_price * h.quantity * factor;
                let cost = h.avg_price * h.quantity * factor;
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
                    Market::CART => etc.push(line),
                }
            }
            Err(e) => {
                if h.market != Market::CART {
                    tracing::warn!("Failed to get price for {}: {e:#}", h.symbol);
                }
                if let (Some(cp), Some(_)) = (h.cached_price, h.cached_at) {
                    // 캐시 가격 사용
                    let cached_price_data = PriceData {
                        name: h.name.clone(),
                        current_price: cp,
                        change_pct: 0.0,
                    };
                    let factor = h.market.value_factor();
                    let eval = cp * h.quantity * factor;
                    let cost = h.avg_price * h.quantity * factor;
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
                        Market::CART => etc.push(line),
                    }
                } else {
                    failed_symbols.push(h.symbol.clone());
                    let line = formatter::format_holding_line_no_price(h);
                    match h.market {
                        Market::KRX => domestic.push(line),
                        Market::NAS | Market::NYS | Market::AMS => overseas.push(line),
                        Market::BOND => bonds.push(line),
                        Market::CART => etc.push(line),
                    }
                }
            }
        }
    }

    if holdings_updated {
        if let Err(e) = storage::save_portfolio(user_id, &store) {
            tracing::warn!("Failed to save portfolio: {e:#}");
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
    if !etc.is_empty() {
        msg.push_str("\n\n🏷 기타\n");
        msg.push_str(&etc.join("\n"));
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
        if !overseas.is_empty() {
            msg.push_str(&format!("\n💱 USD/KRW: {}", formatter::fmt_int(usd_krw)));
        }
    }

    if has_cached {
        msg.push_str("\n⏱ 직전 조회 가격");
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
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (parts, account) = extract_account(&raw);
    let symbol = parts.first().copied().unwrap_or("").trim();
    if symbol.is_empty() {
        return "사용법: /port info [종목코드] [@계좌]".to_string();
    }

    let mut store = storage::load_portfolio(user_id);
    let indices: Vec<usize> = store.holdings.iter().enumerate()
        .filter(|(_, h)| h.symbol == symbol && (account.is_empty() || h.account == account))
        .map(|(i, _)| i)
        .collect();

    if indices.is_empty() {
        return format!("{symbol}{} 을(를) 포트폴리오에서 찾을 수 없습니다.", account_tag(&account));
    }

    let signal_store = storage::load_signals(user_id);
    let market = store.holdings[indices[0]].market;

    // CART: API 호출 없이 cached_price 직접 사용
    let price_result = if market == Market::CART {
        match store.holdings[indices[0]].cached_price {
            Some(cp) => Ok(PriceData { name: store.holdings[indices[0]].name.clone(), current_price: cp, change_pct: 0.0 }),
            None => Err(anyhow::anyhow!("현재가 미설정")),
        }
    } else {
        api.get_price_for_market(market, &symbol).await
    };

    let usd_krw = if matches!(market, Market::NAS | Market::NYS | Market::AMS) {
        api.get_exchange_rate().await.unwrap_or(1350.0)
    } else {
        0.0
    };

    if let Ok(ref price) = price_result {
        if market != Market::CART {
            for &idx in &indices {
                store.holdings[idx].cached_price = Some(price.current_price);
                store.holdings[idx].cached_at = Some(kst_now());
            }
            if let Err(e) = storage::save_portfolio(user_id, &store) {
                tracing::warn!("Failed to save cache for {symbol}: {e:#}");
            }
        }
    }

    let mut parts_out: Vec<String> = Vec::new();
    for &idx in &indices {
        let h = &store.holdings[idx];
        let signals: Vec<&Signal> = signal_store.signals.iter()
            .filter(|s| s.symbol == symbol && s.active &&
                        (s.account.is_empty() || s.account == h.account))
            .collect();

        let part = match &price_result {
            Ok(price) => formatter::format_info(h, price, &signals, usd_krw),
            Err(e) => {
                tracing::warn!("Failed to get price for {symbol}: {e:#}");
                let display_name = if h.name.is_empty() { "-" } else { &h.name };
                let price_str = if let Some(cp) = h.cached_price {
                    format!("{}⏱", formatter::fmt_price(&h.market, cp))
                } else {
                    "- (조회 불가)".to_string()
                };
                let mut msg = format!(
                    "📈 {} {}{}\n매입가: {} × {}\n현재가: {}",
                    h.symbol, display_name, account_tag(&h.account),
                    formatter::fmt_price(&h.market, h.avg_price),
                    formatter::fmt_quantity(h.quantity),
                    price_str,
                );
                if !signals.is_empty() {
                    msg.push_str("\n\n⚡ 설정된 시그널:");
                    for s in &signals {
                        msg.push_str(&format!("\n• {} → 알림", formatter::format_condition(&s.condition, &h.market)));
                    }
                }
                if h.cached_price.is_some() {
                    msg.push_str("\n⏱ 직전 조회 가격");
                }
                msg
            }
        };
        parts_out.push(part);
    }

    parts_out.join("\n\n")
}

async fn cmd_summary(user_id: i64, api: &ApiHandle) -> String {
    let mut store = storage::load_portfolio(user_id);
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다.".to_string();
    }

    let usd_krw = api.get_exchange_rate().await.unwrap_or(1350.0);

    let mut domestic_val = 0.0f64;
    let mut overseas_val = 0.0f64;
    let mut bond_val = 0.0f64;
    let mut etc_val = 0.0f64;
    let mut total_cost = 0.0f64;
    let mut acct_vals: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let mut has_cached = false;
    let mut portfolio_updated = false;
    let mut failed_symbols: Vec<String> = Vec::new();

    for h in store.holdings.iter_mut() {
        let market = h.market;
        let symbol = h.symbol.clone();

        // CART: API 호출 없이 cached_price 직접 사용
        let current_price = if market == Market::CART {
            match h.cached_price {
                Some(cp) => cp,
                None => {
                    failed_symbols.push(symbol);
                    continue;
                }
            }
        } else {
            match api.get_price_for_market(market, &symbol).await {
                Ok(p) => {
                    h.cached_price = Some(p.current_price);
                    h.cached_at = Some(kst_now());
                    portfolio_updated = true;
                    p.current_price
                }
                Err(e) => {
                    tracing::warn!("Summary: failed to get price for {symbol}: {e:#}");
                    if let Some(cp) = h.cached_price {
                        has_cached = true;
                        cp
                    } else {
                        failed_symbols.push(symbol);
                        continue;
                    }
                }
            }
        };
        let factor = h.market.value_factor();
        let eval = current_price * h.quantity * factor;
        let cost = h.avg_price * h.quantity * factor;

        let eval_krw = match h.market {
            Market::NAS | Market::NYS | Market::AMS => eval * usd_krw,
            _ => eval,
        };
        let cost_krw = match h.market {
            Market::NAS | Market::NYS | Market::AMS => cost * usd_krw,
            _ => cost,
        };

        match h.market {
            Market::KRX => domestic_val += eval_krw,
            Market::NAS | Market::NYS | Market::AMS => overseas_val += eval_krw,
            Market::BOND => bond_val += eval_krw,
            Market::CART => etc_val += eval_krw,
        }
        total_cost += cost_krw;
        *acct_vals.entry(h.account.clone()).or_default() += eval_krw;
    }

    if portfolio_updated {
        if let Err(e) = storage::save_portfolio(user_id, &store) {
            tracing::warn!("Failed to save portfolio cache (summary): {e:#}");
        }
    }

    let total = domestic_val + overseas_val + bond_val + etc_val;
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
         🏷 기타: {}원 ({})\n\
         ──────────\n\
         💰 총 평가: {}원\n\
         💵 총 손익: {sign}{}원 ({sign}{:.1}%)",
        formatter::fmt_int(domestic_val), fmt_pct(domestic_val),
        formatter::fmt_int(overseas_val), fmt_pct(overseas_val),
        formatter::fmt_int(bond_val),     fmt_pct(bond_val),
        formatter::fmt_int(etc_val),      fmt_pct(etc_val),
        formatter::fmt_int(total),
        formatter::fmt_int(pnl),
        pnl_pct,
    );

    if overseas_val > 0.0 {
        msg.push_str(&format!("\n💱 USD/KRW: {}", formatter::fmt_int(usd_krw)));
    }
    // 계좌별 요약 (2종류 이상일 때만)
    if acct_vals.len() > 1 {
        msg.push_str("\n\n📂 계좌별");
        let mut accts: Vec<_> = acct_vals.into_iter().collect();
        accts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (acct, val) in &accts {
            let label = if acct.is_empty() { "기본" } else { acct };
            msg.push_str(&format!("\n@{}: {}원 ({})", label, formatter::fmt_int(*val), fmt_pct(*val)));
        }
    }
    if has_cached {
        msg.push_str("\n⏱ 직전 조회 가격");
    }
    if !failed_symbols.is_empty() {
        msg.push_str(&format!(
            "\n⚠️ 시세 없음 (제외됨): {}",
            failed_symbols.join(", ")
        ));
    }

    msg
}

/// CSV 파일 내용을 반환. 정상 시 BOM(\u{FEFF}) 접두, 에러 시 일반 문자열.
fn cmd_export(user_id: i64, args: &str) -> String {
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (_, account_filter) = extract_account(&raw);

    let store = storage::load_portfolio(user_id);
    if store.holdings.is_empty() {
        return "포트폴리오가 비어있습니다.".to_string();
    }

    let holdings: Vec<&Holding> = if account_filter.is_empty() {
        store.holdings.iter().collect()
    } else {
        store.holdings.iter().filter(|h| h.account == account_filter).collect()
    };
    if holdings.is_empty() {
        return format!("@{account_filter} 계좌에 종목이 없습니다.");
    }

    fn export_row(h: &Holding) -> String {
        let cost = h.avg_price * h.quantity * h.market.value_factor();
        let sym = if h.symbol.chars().all(|c| c.is_ascii_digit()) {
            format!("'{}", h.symbol)
        } else {
            h.symbol.clone()
        };
        let price = h.cached_price.map_or(String::new(), |p| format!("{p}"));
        format!("{},{},{},{},{},{}", h.name, price, sym, h.quantity, cost, h.account)
    }

    let mut domestic: Vec<String> = Vec::new();
    let mut overseas: Vec<String> = Vec::new();
    let mut bonds: Vec<String> = Vec::new();
    let mut etc: Vec<String> = Vec::new();

    let mut sorted = holdings.clone();
    sorted.sort_by(|a, b| a.symbol.cmp(&b.symbol));

    for h in &sorted {
        let row = export_row(h);
        match h.market {
            Market::KRX => domestic.push(row),
            Market::NAS | Market::NYS | Market::AMS => overseas.push(row),
            Market::BOND => bonds.push(row),
            Market::CART => etc.push(row),
        }
    }

    let mut lines: Vec<String> = vec![
        "종목명,현재가,코드,수량,매입금액,계좌".to_string(),
    ];
    let sections: &[(&str, &Vec<String>)] = &[
        ("국내", &domestic),
        ("미국", &overseas),
        ("채권", &bonds),
        ("기타", &etc),
    ];
    for (label, rows) in sections {
        if !rows.is_empty() {
            lines.push(String::new());
            lines.push(format!("{},,,,,", label));
            lines.extend(rows.iter().cloned());
        }
    }

    format!("\u{FEFF}{}\n", lines.join("\n"))
}

// --- 시그널 ---

fn cmd_signal(user_id: i64, args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    // 서브커맨드 이후 인자: 종목코드 등 대문자 정규화
    let rest = parts.get(1..).unwrap_or(&[]).join(" ").to_uppercase();
    match parts.first().copied() {
        Some("list") | Some("ls")    => cmd_signal_list(user_id),
        Some("remove") | Some("rm")  => cmd_signal_remove(user_id, &rest),
        Some("clear") | Some("cls")  => cmd_signal_clear(user_id, &rest),
        Some("purge") | Some("prn")  => cmd_signal_purge(user_id),
        Some("add") | Some("a") => {
            let rest_parts: Vec<&str> = rest.split_whitespace().collect();
            let (sig_parts, account) = extract_account(&rest_parts);
            if sig_parts.len() < 3 {
                return "사용법: /signal add [종목코드] [> 또는 <] [값 또는 수익률%] [@계좌]\n\
                        예: /signal add 005930 > 80000\n\
                        예: /signal add 005930 > 10% @IRP"
                    .to_string();
            }
            let symbol = sig_parts[0];
            let condition = match parse_condition(sig_parts[1], &sig_parts[2..]) {
                Ok(c) => c,
                Err(e) => return e,
            };
            // 포트폴리오에 해당 종목(+계좌)이 있는지 확인
            let portfolio = storage::load_portfolio(user_id);
            let holding = portfolio.holdings.iter().find(|h| {
                h.symbol == symbol && (account.is_empty() || h.account == account)
            });
            let market = match holding {
                None => return format!(
                    "{symbol}{} 이(가) 포트폴리오에 없습니다. /port add 로 먼저 추가하세요.",
                    account_tag(&account)
                ),
                Some(h) if h.market == Market::CART => {
                    return "CART 종목에는 시그널을 설정할 수 없습니다. (자동 시세 조회 불가)".to_string();
                }
                Some(h) => h.market,
            };
            let mut store = storage::load_signals(user_id);
            store.signals.push(Signal {
                id: Uuid::new_v4().to_string(),
                symbol: symbol.to_string(),
                account: account.clone(),
                condition: condition.clone(),
                active: true,
                created_at: kst_now(),
            });
            if let Err(e) = storage::save_signals(user_id, &store) {
                return format!("저장 실패: {e:#}");
            }
            format!("✅ 시그널 설정 완료\n{symbol}{}: {}", account_tag(&account), formatter::format_condition(&condition, &market))
        }
        _ => "사용법:\n\
              /s add (a) [종목코드] [> 또는 <] [값 또는 수익률%] [@계좌]\n\
              /s ls\n\
              /s rm [번호]  ← 여러 개: /s rm 1 2\n\
              /s cls [종목코드] [@계좌]\n\
              /s prn  ← 비활성 시그널 전체 삭제\n\
              ⚠️ 삭제 시 목록 확인 후, 여러 개는 한 번에 입력하세요."
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
    for (i, s) in store.signals.iter().enumerate() {
        let status = if s.active { "🟢" } else { "⚫" };
        let holding = portfolio.holdings.iter()
            .find(|h| h.symbol == s.symbol && (s.account.is_empty() || h.account == s.account));
        let name = holding.map(|h| h.name.as_str()).unwrap_or("");
        let market = holding.map(|h| h.market).unwrap_or(Market::KRX);
        let display = if name.is_empty() {
            s.symbol.clone()
        } else {
            format!("{} {}", s.symbol, name)
        };
        msg.push_str(&format!(
            "\n{}. {status} {}{} — {}",
            i + 1,
            display,
            account_tag(&s.account),
            formatter::format_condition(&s.condition, &market)
        ));
    }
    msg.push_str("\n\n⚠️ 여러 개 삭제 시 한 번에: /signal remove 1 2");
    msg
}

fn cmd_signal_remove(user_id: i64, args: &str) -> String {
    let arg = args.trim();
    if arg.is_empty() {
        return "사용법: /signal remove [번호] (여러 개: /signal remove 1 2)".to_string();
    }

    // 번호 파싱 (여러 개 허용)
    let mut nums: Vec<usize> = Vec::new();
    for token in arg.split_whitespace() {
        match token.parse::<usize>() {
            Ok(n) if n >= 1 => nums.push(n),
            _ => return format!("'{token}'은(는) 올바른 번호가 아닙니다. /signal list로 번호를 확인하세요."),
        }
    }

    let mut store = storage::load_signals(user_id);
    let total = store.signals.len();

    if let Some(&max) = nums.iter().max() {
        if max > total {
            return format!("번호 {max}이(가) 없습니다. /signal list로 번호를 확인하세요.");
        }
    }

    // 내림차순 정렬 후 제거 (인덱스 밀림 방지)
    nums.sort_unstable_by(|a, b| b.cmp(a));
    nums.dedup();

    let portfolio = storage::load_portfolio(user_id);
    let mut removed_descs: Vec<String> = Vec::new();
    for num in &nums {
        let s = store.signals.remove(num - 1);
        let market = portfolio.holdings.iter()
            .find(|h| h.symbol == s.symbol && (s.account.is_empty() || h.account == s.account))
            .map(|h| h.market)
            .unwrap_or(Market::KRX);
        removed_descs.push(format!("{}. {}: {}", num, s.symbol, formatter::format_condition(&s.condition, &market)));
    }

    if let Err(e) = storage::save_signals(user_id, &store) {
        return format!("저장 실패: {e:#}");
    }

    removed_descs.reverse(); // 번호 오름차순으로 출력
    format!("✅ 시그널 삭제 완료\n{}", removed_descs.join("\n"))
}

fn cmd_signal_clear(user_id: i64, args: &str) -> String {
    let raw: Vec<&str> = args.split_whitespace().collect();
    let (parts, account) = extract_account(&raw);
    let symbol = parts.first().copied().unwrap_or("").trim();
    if symbol.is_empty() {
        return "사용법: /signal clear [종목코드] [@계좌]".to_string();
    }

    let mut store = storage::load_signals(user_id);
    let before = store.signals.len();

    if account.is_empty() {
        store.signals.retain(|s| s.symbol != symbol);
    } else {
        store.signals.retain(|s| !(s.symbol == symbol && s.account == account));
    }

    let removed = before - store.signals.len();

    if removed == 0 {
        return format!("{symbol}{} 에 설정된 시그널이 없습니다.", account_tag(&account));
    }

    if let Err(e) = storage::save_signals(user_id, &store) {
        return format!("저장 실패: {e:#}");
    }

    format!("✅ {symbol}{} 시그널 {removed}개 삭제 완료", account_tag(&account))
}

fn cmd_signal_purge(user_id: i64) -> String {
    let mut store = storage::load_signals(user_id);
    let before = store.signals.len();
    store.signals.retain(|s| s.active);
    let removed = before - store.signals.len();

    if removed == 0 {
        return "비활성 시그널이 없습니다.".to_string();
    }

    if let Err(e) = storage::save_signals(user_id, &store) {
        return format!("저장 실패: {e:#}");
    }

    format!("✅ 비활성 시그널 {removed}개 삭제 완료")
}

fn cmd_user(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    match parts.first().copied() {
        Some("add") | Some("a") => {
            let id: i64 = match parts.get(1).and_then(|s| s.parse().ok()) {
                Some(id) => id,
                None => return "사용법: /user add [chat_id]".to_string(),
            };
            let mut users = storage::load_allowed_users();
            if users.contains(&id) {
                return format!("{id} 는 이미 허용된 사용자입니다.");
            }
            users.push(id);
            match storage::save_allowed_users(&users) {
                Ok(_) => format!("✅ {id} 추가 완료"),
                Err(e) => format!("저장 실패: {e:#}"),
            }
        }
        Some("remove") | Some("rm") => {
            let id: i64 = match parts.get(1).and_then(|s| s.parse().ok()) {
                Some(id) => id,
                None => return "사용법: /user remove [chat_id]".to_string(),
            };
            let mut users = storage::load_allowed_users();
            let before = users.len();
            users.retain(|&u| u != id);
            if users.len() == before {
                return format!("{id} 를 찾을 수 없습니다.");
            }
            match storage::save_allowed_users(&users) {
                Ok(_) => format!("✅ {id} 삭제 완료"),
                Err(e) => format!("저장 실패: {e:#}"),
            }
        }
        Some("list") | Some("ls") => {
            let users = storage::load_allowed_users();
            if users.is_empty() {
                "허용된 사용자가 없습니다.".to_string()
            } else {
                format!(
                    "허용된 사용자:\n{}",
                    users.iter().map(|u| u.to_string()).collect::<Vec<_>>().join("\n")
                )
            }
        }
        _ => "/user add [chat_id]\n/user remove [chat_id]\n/user list".to_string(),
    }
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

// --- 워치리스트 ---

async fn cmd_watchlist(
    args: &str,
    _api: &ApiHandle,
    discovery_enabled: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    discovery_trigger: &std::sync::Arc<tokio::sync::Notify>,
) -> String {
    use std::sync::atomic::Ordering;

    let parts: Vec<&str> = args.split_whitespace().collect();
    let sub = parts.first().copied().unwrap_or("");
    let rest = parts.get(1..).unwrap_or(&[]).join(" ");

    match sub {
        "run" => {
            // 프롬프트 설정 확인
            let hunt = wdb::get_prompt(PromptType::Hunt).ok().flatten();
            let judge = wdb::get_prompt(PromptType::Judge).ok().flatten();
            if hunt.is_none() || judge.is_none() {
                let mut missing = Vec::new();
                if hunt.is_none() { missing.push("hunt (사냥용)"); }
                if judge.is_none() { missing.push("judge (평가용)"); }
                return format!(
                    "⚠️ 프롬프트 미설정: {}\n\n\
                     /w prompt hunt set [내용]\n\
                     /w prompt judge set [내용]",
                    missing.join(", ")
                );
            }

            if discovery_enabled.load(Ordering::SeqCst) {
                return "이미 디스커버리가 실행 중입니다.".to_string();
            }

            discovery_enabled.store(true, Ordering::SeqCst);
            discovery_trigger.notify_one(); // 즉시 사냥 1회
            let hunt_min = storage::with_config(|c| c.watchlist.hunt_interval_minutes);
            format!(
                "🔍 사냥 시작! (즉시 1회 + {hunt_min}분 주기)\n\
                 /w stop 으로 중지"
            )
        }
        "stop" => {
            if !discovery_enabled.load(Ordering::SeqCst) {
                return "디스커버리가 실행 중이 아닙니다.".to_string();
            }
            discovery_enabled.store(false, Ordering::SeqCst);
            "⏹ 디스커버리 중지".to_string()
        }
        "list" | "ls" => cmd_watch_list(),
        "pending" => cmd_watch_pending(),
        "info" | "i" => cmd_watch_info(&rest.to_uppercase()),
        "export" | "ex" => cmd_watch_export(),
        "blacklist" | "bl" => cmd_watch_blacklist(&rest),
        "budget" => cmd_watch_budget(),
        "prompt" => cmd_watch_prompt(&rest),
        "history" | "hist" => cmd_watch_history(),
        "clear" => cmd_watch_clear(&rest),
        _ => "📋 워치리스트 명령어\n\n\
              /w run — 사냥 시작 (사냥/수집/평가 자동)\n\
              /w stop — 사냥 중지\n\
              /w ls — 평가 완료 종목 (점수순)\n\
              /w export — CSV 내보내기\n\
              /w pending — 대기 중 후보\n\
              /w info [TICKER] — 종목 상세\n\
              /w bl — 블랙리스트\n\
              /w bl add [TICKER] [사유]\n\
              /w bl rm [TICKER]\n\
              /w budget — 오늘 Gemini 사용량\n\
              /w prompt hunt show|set\n\
              /w prompt judge show|set\n\
              /w hist — 최근 호출 이력\n\
              /w clear pending|judged|bl — 일괄 삭제"
            .to_string(),
    }
}


fn cmd_watch_list() -> String {
    use crate::watchlist::models::CandidateStatus;
    let candidates = match wdb::list_candidates(Some(CandidateStatus::Judged)) {
        Ok(c) => c,
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    if candidates.is_empty() {
        return "평가된 종목이 없습니다. /w run 으로 디스커버리를 실행하세요.".to_string();
    }
    let total = candidates.len();
    let mut msg = if total > 30 {
        format!("⚖️ 평가 완료 ({total})개 — 점수순 상위 30개\n")
    } else {
        format!("⚖️ 평가 완료 ({total})개\n")
    };
    let bob_count = 7;
    for (i, c) in candidates.iter().enumerate().take(30) {
        let score = c.score.map_or("-".to_string(), |s| format!("{s:.0}"));
        let reason_short = truncate_chars(&c.reason, 40);
        let bob = if i < bob_count { "🏆" } else { "" };
        let hunt_n = if c.hunt_count > 1 { format!(" ×{}", c.hunt_count) } else { String::new() };
        msg.push_str(&format!(
            "\n{bob}{}. {} ({}{hunt_n}) [{score}점]\n   {reason_short}",
            i + 1, c.ticker, c.sector,
        ));
    }
    msg
}

fn cmd_watch_export() -> String {
    use crate::watchlist::models::CandidateStatus;
    let candidates = match wdb::list_candidates(Some(CandidateStatus::Judged)) {
        Ok(c) => c,
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    if candidates.is_empty() {
        return "평가된 종목이 없습니다.".to_string();
    }
    let mut lines = vec!["ticker,market,name,sector,hunt_score,score,reason,verdict".to_string()];
    for c in &candidates {
        let hunt_s = c.hunt_score.map_or(String::new(), |s| format!("{s:.0}"));
        let score = c.score.map_or(String::new(), |s| format!("{s:.0}"));
        let verdict = c.verdict.as_deref().unwrap_or("");
        lines.push(format!(
            "{},{},{},\"{}\",{},{},\"{}\",\"{}\"",
            c.ticker, c.market, c.name,
            c.sector.replace('"', "\"\""),
            hunt_s, score,
            c.reason.replace('"', "\"\""),
            verdict.replace('"', "\"\""),
        ));
    }
    format!("\u{FEFF}{}\n", lines.join("\n"))
}

fn cmd_watch_pending() -> String {
    use crate::watchlist::models::CandidateStatus;
    let candidates = match wdb::list_candidates(Some(CandidateStatus::Pending)) {
        Ok(c) => c,
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    if candidates.is_empty() {
        return "대기 중인 후보가 없습니다.".to_string();
    }
    let mut msg = format!("🎯 대기 중 후보 ({}개)\n", candidates.len());
    for (i, c) in candidates.iter().enumerate().take(50) {
        msg.push_str(&format!(
            "\n{}. {} — {} ({})",
            i + 1, c.ticker, c.name, c.sector,
        ));
    }
    msg
}

fn cmd_watch_info(ticker: &str) -> String {
    if ticker.is_empty() {
        return "사용법: /w info [TICKER]".to_string();
    }
    let candidate = match wdb::get_candidate_by_ticker(ticker) {
        Ok(Some(c)) => c,
        Ok(None) => return format!("{ticker} 을(를) 찾을 수 없습니다."),
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    let hunt_s = candidate.hunt_score.map_or("-".to_string(), |s| format!("{s:.0}"));
    let score = candidate.score.map_or("-".to_string(), |s| format!("{s:.0}"));
    let verdict = candidate.verdict.as_deref().unwrap_or("-");
    let status = candidate.status.as_str();
    format!(
        "🔍 {ticker}\n\
         이름: {}\n\
         섹터: {}\n\
         상태: {status}\n\
         사냥점수: {hunt_s}\n\
         최종점수: {score}\n\
         판결: {verdict}\n\
         사유: {}\n\
         등록: {}",
        candidate.name, candidate.sector, candidate.reason, candidate.created_at,
    )
}

fn cmd_watch_blacklist(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    match parts.first().copied() {
        Some("add") | Some("a") => {
            let ticker = match parts.get(1) {
                Some(t) => t.to_uppercase(),
                None => return "사용법: /w bl add [TICKER] [사유]".to_string(),
            };
            let reason = parts.get(2..).map(|p| p.join(" ")).unwrap_or_default();
            match wdb::add_blacklist(&ticker, &reason) {
                Ok(_) => format!("✅ {ticker} 블랙리스트 추가"),
                Err(e) => format!("실패: {e:#}"),
            }
        }
        Some("rm") | Some("remove") => {
            let ticker = match parts.get(1) {
                Some(t) => t.to_uppercase(),
                None => return "사용법: /w bl rm [TICKER]".to_string(),
            };
            match wdb::remove_blacklist(&ticker) {
                Ok(true) => format!("✅ {ticker} 블랙리스트 해제"),
                Ok(false) => format!("{ticker} 은(는) 블랙리스트에 없습니다."),
                Err(e) => format!("실패: {e:#}"),
            }
        }
        _ => {
            // 블랙리스트 개수 + 최근 몇개만
            let list = match wdb::list_blacklist() {
                Ok(l) => l,
                Err(e) => return format!("조회 실패: {e:#}"),
            };
            if list.is_empty() {
                return "블랙리스트가 비어있습니다.".to_string();
            }
            let total = list.len();
            let show = 10;
            let mut msg = format!("🚫 블랙리스트 ({total}개)\n\n최근 {show}개:");
            for b in list.iter().take(show) {
                let reason = truncate_chars(&b.reason, 30);
                msg.push_str(&format!("\n{} — {}", b.ticker, reason));
            }
            msg.push_str("\n\n/w bl add [TICKER] [사유]\n/w bl rm [TICKER]");
            msg
        }
    }
}

fn cmd_watch_budget() -> String {
    let hunt = match wdb::hunt_calls_today() {
        Ok(n) => n,
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    let judge = match wdb::judge_calls_today() {
        Ok(n) => n,
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    let (max_hunt, max_judge) = storage::with_config(|c| {
        (c.watchlist.max_hunt_calls_per_day, c.watchlist.max_judge_calls_per_day)
    });
    format!("💰 오늘 사냥: {hunt}/{max_hunt} | 평가: {judge}/{max_judge}")
}

fn cmd_watch_prompt(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let prompt_type = match parts.first().copied() {
        Some("hunt") => PromptType::Hunt,
        Some("judge") => PromptType::Judge,
        _ => return "사용법:\n/w prompt hunt show|set [내용]\n/w prompt judge show|set [내용]".to_string(),
    };

    let action = parts.get(1).copied().unwrap_or("show");

    match action {
        "show" => {
            match wdb::get_prompt(prompt_type) {
                Ok(Some(content)) => format!(
                    "📝 {} 프롬프트:\n\n{}",
                    prompt_type.as_str(), content
                ),
                Ok(None) => format!(
                    "{} 프롬프트가 설정되지 않았습니다.\n/w prompt {} set [내용]",
                    prompt_type.as_str(), prompt_type.as_str()
                ),
                Err(e) => format!("조회 실패: {e:#}"),
            }
        }
        "set" => {
            let content = parts.get(2..).map(|p| p.join(" ")).unwrap_or_default();
            if content.is_empty() {
                return format!("사용법: /w prompt {} set [프롬프트 내용]", prompt_type.as_str());
            }
            match wdb::set_prompt(prompt_type, &content) {
                Ok(_) => format!("✅ {} 프롬프트 설정 완료", prompt_type.as_str()),
                Err(e) => format!("저장 실패: {e:#}"),
            }
        }
        _ => format!("사용법: /w prompt {} show|set [내용]", prompt_type.as_str()),
    }
}

fn cmd_watch_clear(args: &str) -> String {
    use crate::watchlist::models::CandidateStatus;
    match args.trim() {
        "pending" => {
            match wdb::clear_candidates_by_status(CandidateStatus::Pending) {
                Ok(n) => format!("🗑 pending {n}건 삭제"),
                Err(e) => format!("삭제 실패: {e:#}"),
            }
        }
        "judged" => {
            match wdb::clear_candidates_by_status(CandidateStatus::Judged) {
                Ok(n) => format!("🗑 judged {n}건 삭제"),
                Err(e) => format!("삭제 실패: {e:#}"),
            }
        }
        "bl" => {
            match wdb::clear_all_blacklist() {
                Ok(n) => format!("🗑 블랙리스트 {n}건 삭제"),
                Err(e) => format!("삭제 실패: {e:#}"),
            }
        }
        _ => "사용법: /w clear pending|judged|bl".to_string(),
    }
}

fn cmd_watch_history() -> String {
    let records = match wdb::list_prompt_history(10) {
        Ok(r) => r,
        Err(e) => return format!("조회 실패: {e:#}"),
    };
    if records.is_empty() {
        return "호출 이력이 없습니다.".to_string();
    }
    let mut msg = format!("📜 최근 Gemini 호출 이력 ({}건)\n", records.len());
    for r in &records {
        let tickers_short = truncate_chars_owned(&r.tickers_extracted, 50);
        msg.push_str(&format!(
            "\n#{} [{}] {} — {}\n   종목: {}",
            r.id, r.prompt_type, r.status, r.created_at, tickers_short,
        ));
    }
    msg
}

// --- 잠금 모드 핸들러 ---

pub async fn handle_locked_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    unlock_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<crate::config::Config>>>>,
    boot: BootConfig,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
    let owner_id = boot.telegram.owner_chat_id;

    match cmd {
        Command::Unlock(passphrase) => {
            // 메시지 즉시 삭제 (패스프레이즈 노출 방지)
            let _ = bot.delete_message(chat_id, msg.id).await;

            if owner_id != 0 && user_id != owner_id {
                bot.send_message(chat_id, "오너만 잠금 해제할 수 있습니다.").await?;
                return Ok(());
            }

            let passphrase = passphrase.trim().to_string();
            if passphrase.is_empty() {
                bot.send_message(chat_id, "사용법: /unlock [패스프레이즈]").await?;
                return Ok(());
            }

            // 복호화 시도
            match boot.clone().decrypt_into_config(&passphrase) {
                Ok(config) => {
                    // 패스프레이즈 저장 (update_config에서 암호화 저장용)
                    storage::set_passphrase(&passphrase);
                    // unlock 채널로 config 전송
                    let mut tx_guard = unlock_tx.lock().await;
                    if let Some(tx) = tx_guard.take() {
                        let _ = tx.send(config);
                        bot.send_message(chat_id, "🔓 잠금 해제 완료!").await?;
                    } else {
                        bot.send_message(chat_id, "이미 잠금 해제되었습니다.").await?;
                    }
                }
                Err(e) => {
                    tracing::warn!("잠금 해제 실패: {e:#}");
                    bot.send_message(chat_id, "❌ 패스프레이즈가 틀렸습니다.").await?;
                }
            }
        }
        Command::Ping => {
            bot.send_message(chat_id, "pong (🔒 잠금 상태)").await?;
        }
        _ => {
            bot.send_message(chat_id, "🔒 잠금 상태입니다.\n/unlock [패스프레이즈] 로 잠금 해제하세요.").await?;
        }
    }

    Ok(())
}

// --- 암호화 마이그레이션 ---

async fn cmd_encrypt(bot: &Bot, chat_id: ChatId, msg_id: teloxide::types::MessageId, args: &str) -> String {
    // 패스프레이즈 노출 방지: 메시지 즉시 삭제
    let _ = bot.delete_message(chat_id, msg_id).await;

    let passphrase = args.trim();
    if passphrase.is_empty() {
        return "사용법: /encrypt [패스프레이즈]\n\n\
                현재 평문 config를 암호화합니다.\n\
                ⚠️ 패스프레이즈를 분실하면 설정을 복구할 수 없습니다!"
            .to_string();
    }

    let config_path = storage::CONFIG_PATH;

    // 현재 config를 암호화하여 저장
    let result = storage::with_config(|config| {
        tracing::info!(
            "encrypt: gemini_api_key={}",
            if config.secrets.gemini_api_key.is_empty() { "EMPTY" } else { "SET" },
        );
        config.save_encrypted(config_path, passphrase)
    });

    match result {
        Ok(_) => {
            // 패스프레이즈 저장 (이후 update_config에서 암호화 저장용)
            storage::set_passphrase(passphrase);
            tracing::info!("config 암호화 완료");
            "✅ 설정 암호화 완료!\n\n\
             다음 재시작부터 /unlock [패스프레이즈]로 잠금 해제해야 합니다.\n\
             ⚠️ 패스프레이즈를 안전한 곳에 보관하세요."
                .to_string()
        }
        Err(e) => format!("❌ 암호화 실패: {e:#}"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_options ---

    #[test]
    fn extract_options_account_only() {
        let parts = vec!["005930", "10", "70000", "@IRP"];
        let (rest, account, cp) = extract_options(&parts);
        assert_eq!(rest, vec!["005930", "10", "70000"]);
        assert_eq!(account, "IRP");
        assert!(cp.is_none());
    }

    #[test]
    fn extract_options_account_and_price() {
        let parts = vec!["비트코인", "2", "50000000", "@코인", "=55000000"];
        let (rest, account, cp) = extract_options(&parts);
        assert_eq!(rest, vec!["비트코인", "2", "50000000"]);
        assert_eq!(account, "코인");
        assert_eq!(cp, Some("55000000"));
    }

    #[test]
    fn extract_options_reverse_order() {
        let parts = vec!["=100", "@계좌", "AAPL"];
        let (rest, account, cp) = extract_options(&parts);
        assert_eq!(rest, vec!["AAPL"]);
        assert_eq!(account, "계좌");
        assert_eq!(cp, Some("100"));
    }

    #[test]
    fn extract_options_none() {
        let parts = vec!["AAPL", "10", "150"];
        let (rest, account, cp) = extract_options(&parts);
        assert_eq!(rest, vec!["AAPL", "10", "150"]);
        assert_eq!(account, "");
        assert!(cp.is_none());
    }

    #[test]
    fn extract_options_empty() {
        let parts: Vec<&str> = vec![];
        let (rest, account, cp) = extract_options(&parts);
        assert!(rest.is_empty());
        assert_eq!(account, "");
        assert!(cp.is_none());
    }

    // --- parse_condition ---

    #[test]
    fn parse_price_above() {
        let c = parse_condition(">", &["80000"]).unwrap();
        assert!(matches!(c, Condition::PriceAbove { target } if target == 80000.0));
    }

    #[test]
    fn parse_price_below() {
        let c = parse_condition("<", &["50000"]).unwrap();
        assert!(matches!(c, Condition::PriceBelow { target } if target == 50000.0));
    }

    #[test]
    fn parse_profit_above() {
        let c = parse_condition(">", &["10%"]).unwrap();
        assert!(matches!(c, Condition::ProfitAbove { percentage } if percentage == 10.0));
    }

    #[test]
    fn parse_profit_below() {
        let c = parse_condition("<", &["-5%"]).unwrap();
        assert!(matches!(c, Condition::ProfitBelow { percentage } if percentage == -5.0));
    }

    #[test]
    fn parse_condition_missing_value() {
        assert!(parse_condition(">", &[]).is_err());
    }

    #[test]
    fn parse_condition_invalid_number() {
        assert!(parse_condition(">", &["abc"]).is_err());
    }

    #[test]
    fn parse_condition_unknown_operator() {
        assert!(parse_condition("==", &["100"]).is_err());
    }

    // --- account_tag ---

    #[test]
    fn account_tag_empty() {
        assert_eq!(account_tag(""), "");
    }

    #[test]
    fn account_tag_with_value() {
        assert_eq!(account_tag("IRP"), " [@IRP]");
    }
}

/// UTF-8 안전하게 `max_chars`글자로 자르기 (borrowed)
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// UTF-8 안전하게 `max_chars`글자로 자르기 (owned, 잘렸으면 "..." 붙임)
fn truncate_chars_owned(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => format!("{}...", &s[..idx]),
        None => s.to_string(),
    }
}

