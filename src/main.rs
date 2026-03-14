mod api;
mod bot;
mod config;
mod crypto;
mod models;
mod scheduler;
mod signal;
mod storage;
mod watchlist;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use teloxide::prelude::*;
use tokio::sync::mpsc;

use crate::api::{run_api_actor, ApiHandle};
use crate::models::messages::ApiRequest;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime 생성 실패");

    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async_main());
}

async fn async_main() {
    let config_path = storage::CONFIG_PATH;

    // 1. BootConfig 로드
    let boot = match config::BootConfig::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            if let Err(dir_err) = std::fs::create_dir_all(storage::DATA_DIR) {
                eprintln!("디렉토리 생성 실패: {}\n  {dir_err}", storage::DATA_DIR);
                return;
            }
            if std::path::Path::new(config_path).exists() {
                eprintln!("설정 파일 파싱 실패: {config_path}");
                eprintln!("  오류: {e:#}");
                return;
            }
            let template = include_str!("../docs/config.template.json");
            if let Err(write_err) = std::fs::write(config_path, template) {
                eprintln!("설정 파일 생성 실패: {config_path}\n  {write_err}");
                return;
            }
            eprintln!("설정 파일이 생성되었습니다: {config_path}");
            eprintln!("  - telegram.bot_token: 텔레그램 봇 토큰");
            eprintln!("  - kis_api.app_key / app_secret: 한투 API 키");
            return;
        }
    };

    // 2. 봇 토큰 검증
    if boot.telegram.bot_token.is_empty() || boot.telegram.bot_token.starts_with("YOUR_") {
        eprintln!("telegram.bot_token이 설정되지 않았습니다. {config_path} 를 수정하세요.");
        return;
    }

    // 3. 로깅 초기화
    init_logging(&boot.log);
    tracing::info!("gidbcn starting... (log dir: {}, retain: {}d)", boot.log.dir, boot.log.retain_days);

    // 4. API 채널 생성 (actor는 unlock 후 spawn)
    let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
    let api_handle = ApiHandle::new(api_tx);

    let is_encrypted = boot.is_encrypted();
    let bot_token = boot.telegram.bot_token.clone();

    if is_encrypted {
        // === 암호화 모드: 잠금 봇 시작 ===
        tracing::info!("🔒 암호화 모드 — /unlock 대기 중");

        let locked = Arc::new(AtomicBool::new(true));
        let (unlock_tx, unlock_rx) = tokio::sync::oneshot::channel::<config::Config>();
        let unlock_tx = Arc::new(tokio::sync::Mutex::new(Some(unlock_tx)));

        let tg_bot = Bot::new(&bot_token);

        // unlock 수신 → 정상 모드 전환 (spawn_local 컨텍스트)
        let api_handle2 = api_handle.clone();
        let tg_bot2 = tg_bot.clone();
        let locked2 = locked.clone();
        tokio::task::spawn_local(async move {
            if let Ok(config) = unlock_rx.await {
                storage::init_config(config);
                if let Err(e) = watchlist::db::init_db() {
                    tracing::error!("watchlist DB 초기화 실패: {e:#}");
                }
                tokio::task::spawn_local(run_api_actor(api_rx));
                tokio::task::spawn_local(scheduler::run_scheduler(api_handle2, tg_bot2));
                locked2.store(false, Ordering::SeqCst);
                tracing::info!("🔓 잠금 해제 완료 — 정상 모드");
            }
        });

        // 봇 실행 (잠금 상태 포함)
        bot::run_bot_with_lock(api_handle, locked, unlock_tx, boot).await;
    } else {
        // === 평문 모드: 기존 흐름 ===
        let config = match boot.into_plaintext_config() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("설정 구성 실패: {e:#}");
                return;
            }
        };

        // 평문 모드 필수 설정 검증
        {
            let mut missing: Vec<&str> = Vec::new();
            if config.kis_api.app_key.is_empty() || config.kis_api.app_key.starts_with("YOUR_") {
                missing.push("kis_api.app_key");
            }
            if config.kis_api.app_secret.is_empty() || config.kis_api.app_secret.starts_with("YOUR_") {
                missing.push("kis_api.app_secret");
            }
            if config.kis_api.hts_id.is_empty() || config.kis_api.hts_id.starts_with("YOUR_") {
                missing.push("kis_api.hts_id");
            }
            if !missing.is_empty() {
                eprintln!("필수 설정이 누락되었습니다. {config_path} 를 수정하세요:");
                for field in missing {
                    eprintln!("  - {field}");
                }
                return;
            }
        }

        // log 섹션 마이그레이션
        {
            let raw = std::fs::read_to_string(config_path).unwrap_or_default();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if v.get("log").is_none() {
                    if let Err(e) = config.save(config_path) {
                        tracing::warn!("config.json 마이그레이션 저장 실패: {e:#}");
                    }
                }
            }
        }

        storage::init_config(config);

        if let Err(e) = watchlist::db::init_db() {
            tracing::error!("watchlist DB 초기화 실패: {e:#}");
        }

        tokio::task::spawn_local(run_api_actor(api_rx));

        let tg_bot = Bot::new(&bot_token);
        tokio::task::spawn_local(scheduler::run_scheduler(
            api_handle.clone(),
            tg_bot.clone(),
        ));

        tracing::info!("Bot and scheduler running (plaintext mode)");
        bot::run_bot(api_handle).await;
    }
}

fn init_logging(log_config: &config::LogConfig) {
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(log_config.retain_days as usize)
        .filename_prefix("gidbcn")
        .filename_suffix("log")
        .build(&log_config.dir)
        .expect("로그 디렉토리 초기화 실패");
    let (non_blocking, _log_guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::{filter::LevelFilter, fmt, prelude::*, EnvFilter};
    let stdout_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(stdout_filter))
        .with(
            fmt::layer()
                .with_ansi(false)
                .with_writer(non_blocking)
                .with_filter(LevelFilter::WARN),
        )
        .init();
}
