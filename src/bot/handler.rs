use teloxide::prelude::*;

use crate::api::ApiHandle;
use crate::bot::commands::Command;
use crate::config::TelegramConfig;

pub async fn run_bot(config: TelegramConfig, api: ApiHandle) {
    let bot = Bot::new(&config.bot_token);
    let handler = Update::filter_message().filter_command::<Command>().endpoint(
        move |bot: Bot, msg: Message, cmd: Command| {
            let api = api.clone();
            async move { super::commands::handle_command(bot, msg, cmd, api).await }
        },
    );

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
