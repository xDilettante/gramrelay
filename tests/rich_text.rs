use serde_json::json;
use tg::{
    Bot, ChatId, EffectOutcome, EntityKind, InputFile, ParseMode, RichText,
    api::types::MessageEntity,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method},
};

#[test]
fn utf8_ranges_become_utf16_and_composition_moves_entities() {
    let text = RichText::new("A🦀e\u{301}Z")
        .entity(1..5, EntityKind::Bold)
        .unwrap()
        .entity(5..8, EntityKind::Italic)
        .unwrap();
    assert_eq!(text.entities()[0].offset, 1);
    assert_eq!(text.entities()[0].length, 2);
    assert_eq!(text.entities()[1].offset, 3);
    assert_eq!(text.entities()[1].length, 2);
    let text = RichText::new("🦀 ").append(RichText::styled("Привет", EntityKind::Bold).unwrap());
    assert_eq!(text.text(), "🦀 Привет");
    assert_eq!(text.entities()[0].offset, 3);
    assert_eq!(text.entities()[0].length, 6);
    for range in [2..5, 1..4, 5..5, 9..10] {
        assert_eq!(
            RichText::new("A🦀")
                .entity(range, EntityKind::Bold)
                .unwrap_err()
                .outcome(),
            EffectOutcome::NotSent
        );
    }
    let halves = vec![MessageEntity::new("bold".into(), 1, 1)];
    assert!(RichText::try_from_entities("🦀", halves).is_err());
    let huge = vec![MessageEntity::new("bold".into(), i64::MAX, 1)];
    assert!(RichText::try_from_entities("x", huge).is_err());
}

#[test]
fn nesting_is_strict_and_preserves_compatible_styles() {
    let valid = RichText::new("abcdef")
        .entity(.., EntityKind::Bold)
        .unwrap()
        .entity(1..5, EntityKind::Italic)
        .unwrap()
        .entity(2..4, EntityKind::TextLink("https://example.test".into()))
        .unwrap();
    assert_eq!(valid.entities().len(), 3);
    assert!(
        RichText::new("abcdef")
            .entity(0..4, EntityKind::Bold)
            .unwrap()
            .entity(2..6, EntityKind::Italic)
            .is_err()
    );
    assert!(
        RichText::new("abcdef")
            .entity(.., EntityKind::Code)
            .unwrap()
            .entity(1..3, EntityKind::Bold)
            .is_err()
    );
    assert!(
        RichText::new("abcdef")
            .entity(.., EntityKind::Bold)
            .unwrap()
            .entity(1..3, EntityKind::Code)
            .is_err()
    );
    assert!(
        RichText::new("abcdef")
            .entity(.., EntityKind::Blockquote)
            .unwrap()
            .entity(1..3, EntityKind::ExpandableBlockquote)
            .is_err()
    );
    assert!(
        RichText::new("abcdef")
            .entity(.., EntityKind::TextLink("https://example.test".into()))
            .unwrap()
            .entity(1..3, EntityKind::CustomEmoji("1".into()))
            .is_err()
    );
    let quote = RichText::new("abcdef")
        .entity(.., EntityKind::Blockquote)
        .unwrap()
        .entity(1..3, EntityKind::Code)
        .unwrap();
    assert_eq!(quote.entities().len(), 2);
    assert!(
        RichText::new("abcdef")
            .entity(.., EntityKind::Bold)
            .unwrap()
            .entity(1..3, EntityKind::Bold)
            .is_err()
    );
}

#[test]
fn attributes_are_validated_without_leaking_values() {
    let missing = MessageEntity::new("text_link".into(), 0, 1);
    assert!(RichText::try_from_entities("x", vec![missing]).is_err());
    let wrong = MessageEntity::new("bold".into(), 0, 1).url("https://private.test/secret".into());
    let error = RichText::try_from_entities("x", vec![wrong]).unwrap_err();
    assert!(!format!("{error:?}").contains("private.test"));
    for format in ["", "r", "wDt", "T", "dt"] {
        assert!(
            RichText::styled(
                "date",
                EntityKind::DateTime {
                    unix_time: 1,
                    format: format.into()
                }
            )
            .is_ok()
        );
    }
    for format in ["rr", "rw", "tw", "dD", "anything"] {
        assert!(
            RichText::styled(
                "date",
                EntityKind::DateTime {
                    unix_time: 1,
                    format: format.into()
                }
            )
            .is_err()
        );
    }
    assert!(RichText::styled("x", EntityKind::CustomEmoji(String::new())).is_err());
    assert!(
        RichText::try_from_entities("x", vec![MessageEntity::new("future_kind".into(), 0, 1)])
            .is_err()
    );
    assert!(!format!("{:?}", RichText::new("private message")).contains("private message"));
}

fn bot(server: &MockServer) -> Bot {
    Bot::builder("123456:OFFLINE_TEST_TOKEN")
        .api_url(server.uri())
        .allow_http_loopback(true)
        .build()
        .unwrap()
}
fn response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(
        json!({"ok": true,"result":{"message_id":1,"date":1,"chat":{"id":1,"type":"private"}}}),
    )
}

#[tokio::test]
async fn convenience_message_sends_entities_and_plain_remains_plain() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(body_json(json!({"chat_id":1,"text":"🦀 bold","entities":[{"type":"bold","offset":3,"length":4}]})))
        .respond_with(response()).expect(1).mount(&server).await;
    let text = RichText::new("🦀 ").append(RichText::styled("bold", EntityKind::Bold).unwrap());
    bot(&server)
        .send_message(ChatId::from(1), text)
        .await
        .unwrap();
}

#[tokio::test]
async fn explicit_parse_modes_and_caption_entities_work() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response())
        .expect(2)
        .mount(&server)
        .await;
    let bot = bot(&server);
    bot.send_message(ChatId::from(1), "<b>literal</b>")
        .parse_mode(ParseMode::Html)
        .await
        .unwrap();
    bot.send_document(ChatId::from(1), InputFile::bytes("x", vec![1]))
        .caption(RichText::styled("🦀", EntityKind::Bold).unwrap())
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].body_json::<serde_json::Value>().unwrap()["parse_mode"],
        "HTML"
    );
    let body = String::from_utf8_lossy(&requests[1].body);
    assert!(body.contains("name=\"caption_entities\""));
    assert!(body.contains("\"length\":2"));
}

#[tokio::test]
async fn parse_mode_conflicts_and_text_limits_are_local() {
    let server = MockServer::start().await;
    let bot = bot(&server);
    let rich = RichText::styled("x", EntityKind::Bold).unwrap();
    let conflict = bot
        .send_message(ChatId::from(1), rich.clone())
        .parse_mode(ParseMode::Html)
        .await
        .unwrap_err();
    assert_eq!(conflict.field(), Some("parse_mode"));
    let conflict = bot
        .send_document(ChatId::from(1), InputFile::bytes("x", vec![1]))
        .caption(rich)
        .parse_mode(ParseMode::MarkdownV2)
        .await
        .unwrap_err();
    assert_eq!(conflict.outcome(), EffectOutcome::NotSent);
    let too_long = bot
        .send_message(ChatId::from(1), RichText::new("🦀".repeat(4097)))
        .await
        .unwrap_err();
    assert_eq!(too_long.field(), Some("text"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn markup_source_length_does_not_replace_the_rendered_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(response())
        .expect(2)
        .mount(&server)
        .await;
    let bot = bot(&server);
    bot.send_message(ChatId::from(1), format!("<b>{}</b>", "x".repeat(4096)))
        .parse_mode(ParseMode::Html)
        .await
        .unwrap();
    bot.send_message(ChatId::from(1), "🦀".repeat(4096))
        .await
        .unwrap();
}
