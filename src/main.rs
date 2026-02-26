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
    let config = match config::Config::load("data/config.json") {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load config: {e}");
            tracing::error!("Create data/config.json first. See CLAUDE.md for schema.");
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
