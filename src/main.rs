mod api;
mod bot;
mod config;
mod models;
mod scheduler;
mod signal;
mod storage;

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
    // 1. config 로드 (로깅 초기화 전 — 오류는 eprintln 사용)
    let config_path = storage::CONFIG_PATH;
    let config = match config::Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            // 디렉토리가 없으면 생성
            if let Err(dir_err) = std::fs::create_dir_all(storage::DATA_DIR) {
                eprintln!("디렉토리 생성 실패: {}\n  {dir_err}", storage::DATA_DIR);
                eprintln!("  sudo mkdir -p {} 로 직접 생성하세요.", storage::DATA_DIR);
                return;
            }
            if std::path::Path::new(config_path).exists() {
                // 파일은 있지만 파싱 실패 → 실제 오류 출력
                eprintln!("설정 파일 파싱 실패: {config_path}");
                eprintln!("  오류: {e:#}");
                return;
            }
            // 파일이 없으면 템플릿 생성
            let template = include_str!("../docs/config.template.json");
            if let Err(write_err) = std::fs::write(config_path, template) {
                eprintln!("설정 파일 생성 실패: {config_path}\n  {write_err}");
                return;
            }
            eprintln!("설정 파일이 생성되었습니다: {config_path}");
            eprintln!();
            eprintln!("아래 항목을 수정한 후 다시 실행하세요:");
            eprintln!("  - telegram.bot_token: 텔레그램 봇 토큰");
            eprintln!("  - kis_api.app_key / app_secret: 한투 API 키 (선택)");
            return;
        }
    };

    // 1-1. 새 섹션(log 등) 누락 시 defaults 포함해서 파일 업데이트
    {
        let raw = std::fs::read_to_string(config_path).unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            if v.get("log").is_none() {
                if let Err(e) = config.save(config_path) {
                    eprintln!("config.json 마이그레이션 저장 실패: {e:#}");
                }
            }
        }
    }

    // 2. 필수 설정 검증 (로깅 초기화 전)
    {
        let mut missing: Vec<&str> = Vec::new();
        if config.telegram.bot_token.is_empty() || config.telegram.bot_token.starts_with("YOUR_") {
            missing.push("telegram.bot_token");
        }
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

    // 3. 로깅 초기화 (config.log 기준)
    // WARN/ERROR만 파일에 기록, stdout은 RUST_LOG 환경변수 기준
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(config.log.retain_days as usize)
        .filename_prefix("gidbcn")
        .filename_suffix("log")
        .build(&config.log.dir)
        .expect("로그 디렉토리 초기화 실패");
    let (non_blocking, _log_guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::{filter::LevelFilter, fmt, prelude::*, EnvFilter};
    // RUST_LOG 없으면 stdout은 INFO 기본값
    let stdout_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(stdout_filter))
        .with(fmt::layer().with_ansi(false).with_writer(non_blocking).with_filter(LevelFilter::WARN))
        .init();
    tracing::info!("gidbcn starting... (log dir: {}, retain: {}d)", config.log.dir, config.log.retain_days);

    // 4. Config 인메모리 적재
    storage::init_config(config);

    // 5. API Actor 채널 생성 + spawn
    let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
    let api_handle = ApiHandle::new(api_tx);
    tokio::task::spawn_local(run_api_actor(api_rx));

    // 6. 텔레그램 봇 + 스케줄러 spawn
    let bot_token = storage::with_config(|c| c.telegram.bot_token.clone());
    let tg_bot = Bot::new(&bot_token);

    tokio::task::spawn_local(scheduler::run_scheduler(
        api_handle.clone(),
        tg_bot.clone(),
    ));

    tracing::info!("Bot and scheduler running");

    // 7. 텔레그램 봇 실행 (메인 태스크, block)
    bot::run_bot(api_handle).await;
}
