use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use teloxide::prelude::*;
use tokio::sync::Notify;

use crate::api::ApiHandle;
use crate::bot::commands::Command;
use crate::config::BootConfig;
use crate::storage;

/// 평문 모드 봇 실행
pub async fn run_bot(
    api: ApiHandle,
    discovery_enabled: Arc<AtomicBool>,
    discovery_trigger: Arc<Notify>,
) {
    let bot_token = storage::with_config(|c| c.telegram.bot_token.clone());
    let bot = Bot::new(&bot_token);
    let handler = Update::filter_message()
        .filter_command::<Command>()
        .endpoint(move |bot: Bot, msg: Message, cmd: Command| {
            let api = api.clone();
            let disc = discovery_enabled.clone();
            let trig = discovery_trigger.clone();
            async move { super::commands::handle_command(bot, msg, cmd, api, disc, trig).await }
        });

    Dispatcher::builder(bot, handler)
        .default_handler(|_| async {})
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

/// 암호화 모드 봇 실행 (잠금 상태 포함)
pub async fn run_bot_with_lock(
    api: ApiHandle,
    locked: Arc<AtomicBool>,
    unlock_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<crate::config::Config>>>>,
    boot: BootConfig,
    discovery_enabled: Arc<AtomicBool>,
    discovery_trigger: Arc<Notify>,
) {
    let bot = Bot::new(&boot.telegram.bot_token);

    let handler = Update::filter_message()
        .filter_command::<Command>()
        .endpoint(move |bot: Bot, msg: Message, cmd: Command| {
            let api = api.clone();
            let locked = locked.clone();
            let unlock_tx = unlock_tx.clone();
            let boot = boot.clone();
            let disc = discovery_enabled.clone();
            let trig = discovery_trigger.clone();
            async move {
                if locked.load(Ordering::SeqCst) {
                    super::commands::handle_locked_command(bot, msg, cmd, unlock_tx, boot).await
                } else {
                    super::commands::handle_command(bot, msg, cmd, api, disc, trig).await
                }
            }
        });

    Dispatcher::builder(bot, handler)
        .default_handler(|_| async {})
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
