use teloxide::prelude::*;

use crate::api::ApiHandle;
use crate::bot::commands::Command;
use crate::storage;

pub async fn run_bot(api: ApiHandle) {
    let bot_token = storage::with_config(|c| c.telegram.bot_token.clone());
    let bot = Bot::new(&bot_token);
    let handler = Update::filter_message()
        .filter_command::<Command>()
        .endpoint(move |bot: Bot, msg: Message, cmd: Command| {
            let api = api.clone();
            async move { super::commands::handle_command(bot, msg, cmd, api).await }
        });

    Dispatcher::builder(bot, handler)
        .default_handler(|_| async {})
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
