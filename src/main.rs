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
        Err(_) => {
            // 디렉토리가 없으면 생성
            if let Err(e) = std::fs::create_dir_all(storage::DATA_DIR) {
                eprintln!("디렉토리 생성 실패: {}\n  {e}", storage::DATA_DIR);
                eprintln!("  sudo mkdir -p {} 로 직접 생성하세요.", storage::DATA_DIR);
                return;
            }
            // config 파일이 없으면 템플릿 생성
            if !std::path::Path::new(config_path).exists() {
                let template = include_str!("../data/config.template.json");
                if let Err(e) = std::fs::write(config_path, template) {
                    eprintln!("설정 파일 생성 실패: {config_path}\n  {e}");
                    return;
                }
            }
            eprintln!("설정 파일이 생성되었습니다: {config_path}");
            eprintln!();
            eprintln!("아래 항목을 수정한 후 다시 실행하세요:");
            eprintln!("  - telegram.bot_token: 텔레그램 봇 토큰");
            eprintln!("  - telegram.chat_id: 텔레그램 채팅 ID");
            eprintln!("  - kis_api.app_key / app_secret: 한투 API 키 (선택)");
            return;
        }
    };

    // 2. API Actor 채널 생성 + spawn
    let (api_tx, api_rx) = mpsc::channel::<ApiRequest>(32);
    let api_handle = ApiHandle::new(api_tx);
    tokio::spawn(run_api_actor(api_rx, config.clone()));

    // 3. 텔레그램 봇 + 스케줄러 spawn
    let tg_bot = Bot::new(&config.telegram.bot_token);
    let chat_id = ChatId(config.telegram.chat_id);

    tokio::spawn(scheduler::run_scheduler(
        api_handle.clone(),
        config.scheduler.clone(),
        tg_bot.clone(),
        chat_id,
    ));

    tracing::info!("Bot and scheduler running");

    // 4. 텔레그램 봇 실행 (메인 태스크, block)
    bot::run_bot(config.telegram, api_handle).await;
}
