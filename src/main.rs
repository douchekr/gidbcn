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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("gidbcn starting...");

    // 1. config 로드
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

    // 2. 필수 설정 검증
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

    // 3. API Actor 채널 생성 + spawn
    let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
    let api_handle = ApiHandle::new(api_tx);
    tokio::spawn(run_api_actor(api_rx, config.clone()));

    // 4. 텔레그램 봇 + 스케줄러 spawn
    let tg_bot = Bot::new(&config.telegram.bot_token);

    tokio::spawn(scheduler::run_scheduler(
        api_handle.clone(),
        config.scheduler.clone(),
        tg_bot.clone(),
    ));

    tracing::info!("Bot and scheduler running");

    // 5. 텔레그램 봇 실행 (메인 태스크, block)
    bot::run_bot(config.telegram, api_handle).await;
}
