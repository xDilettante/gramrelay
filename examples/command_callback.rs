use serde_json::json;
use tg::{
    Bot, CallbackContext, InlineKeyboardButton, InlineKeyboardMarkup, MessageContext, Router,
    Update,
    test_kit::{MockResponse, MockTransport, Step},
};

fn router() -> Router {
    Router::new()
        .command("start", |ctx: MessageContext| async move {
            let keyboard = InlineKeyboardMarkup::new()
                .row([InlineKeyboardButton::callback("Confirm", "confirm")]);
            ctx.reply("Press the button").reply_markup(keyboard).await?;
            Ok(())
        })
        .callback("confirm", |ctx: CallbackContext| async move {
            ctx.answer().text("Button received").await?;
            Ok(())
        })
}

fn live_requested(arguments: &[String]) -> Result<bool, &'static str> {
    match arguments {
        [] => Ok(false),
        [argument] if argument == "--live" => Ok(true),
        _ => Err("usage: command_callback [--live]; default is offline"),
    }
}

async fn offline_demo(router: Router) -> Result<(), Box<dyn std::error::Error>> {
    let mock = MockTransport::new([
        Step::reply(
            "sendMessage",
            MockResponse::json(json!({
                "ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}
            })),
        ),
        Step::reply(
            "answerCallbackQuery",
            MockResponse::json(json!({"ok":true,"result":true})),
        ),
    ])?;
    let bot = mock.bot()?;
    let message: Update = serde_json::from_value(json!({"update_id":1,"message":{
        "message_id":7,"date":1,"chat":{"id":1,"type":"private"},"text":"/start"
    }}))?;
    router.dispatch(&bot, message).await?;
    let callback: Update = serde_json::from_value(json!({"update_id":2,"callback_query":{
        "id":"callback-1","from":{"id":1,"is_bot":false,"first_name":"Offline"},
        "chat_instance":"fixture","data":"confirm"
    }}))?;
    router.dispatch(&bot, callback.clone()).await?;
    // Only the exact registered callback value is handled.
    let mut other = callback;
    other.callback_query.as_mut().unwrap().data = Some("confirm-other".into());
    router.dispatch(&bot, other).await?;
    let requests = mock.requests()?;
    assert_eq!(requests.len(), 2);
    let sent = serde_json::to_value(requests[0].decode::<tg::api::methods::SendMessage>()?)?;
    assert_eq!(sent["chat_id"], 1);
    assert_eq!(sent["text"], "Press the button");
    assert_eq!(
        sent["reply_markup"],
        json!({"inline_keyboard":[[{"text":"Confirm","callback_data":"confirm"}]]})
    );
    assert_eq!(sent["reply_parameters"]["message_id"], 7);
    let answered =
        serde_json::to_value(requests[1].decode::<tg::api::methods::AnswerCallbackQuery>()?)?;
    assert_eq!(answered["callback_query_id"], "callback-1");
    assert_eq!(answered["text"], "Button received");
    mock.assert_finished()?;
    println!("Offline command and callback passed: two requests, no sockets or real credentials.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if live_requested(&arguments)? {
        Bot::from_env("TELEGRAM_BOT_TOKEN")?
            .run_polling(router())
            .await?;
    } else {
        offline_demo(router()).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn documented_handlers_send_button_and_ack_exact_callback() {
        offline_demo(router()).await.unwrap();
    }

    #[test]
    fn live_mode_requires_one_exact_flag() {
        assert!(!live_requested(&[]).unwrap());
        assert!(live_requested(&["--live".into()]).unwrap());
        assert!(live_requested(&["--other".into()]).is_err());
        assert!(live_requested(&["--live".into(), "extra".into()]).is_err());
    }
}
