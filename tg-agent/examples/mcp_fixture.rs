//! Synthetic read-only MCP archive for reproducible tool evaluation. No Telegram.
use serde_json::{Value, json};
use tg_agent::{
    executor::{
        Action, Budget, ChatId, ChatScope, Executor, ExecutorOptions, Grant, PolicyStore, Principal,
    },
    journal::{Journal, JournalOptions},
    mcp::McpServer,
    tools::{ExecutionMode, OutputLimit, ToolSession},
    updates::{SourceMode, UpdateFeed, UpdateOptions},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: Value = serde_json::from_str(include_str!("../evals/v1/archive.json"))?;
    let directory = tempfile::tempdir()?;
    let remote = MockServer::start().await;
    let token = "0:OFFLINE_EVALUATION_TOKEN";
    Mock::given(method("POST")).and(path(format!("/bot{token}/getMe")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":{"id":0,"is_bot":true,"first_name":"Archive","username":"offline_archive_bot"}}))).mount(&remote).await;
    let mut chats = Vec::new();
    for chat in fixture["chats"].as_array().unwrap() {
        let id = chat["id"].as_str().unwrap().parse::<i64>()?;
        chats.push(ChatId::new(id)?);
        let mut info = serde_json::to_value(tg::api::types::ChatFullInfo::new(
            tg::ChatId::from(id),
            "supergroup".into(),
            1,
            1,
            tg::api::types::AcceptedGiftTypes::new(false, false, false, false, false),
        ))?;
        info["title"] = chat["title"].clone();
        Mock::given(method("POST"))
            .and(path(format!("/bot{token}/getChat")))
            .and(body_partial_json(json!({"chat_id":id})))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"ok":true,"result":info})),
            )
            .mount(&remote)
            .await;
    }
    let journal = Journal::open(
        directory.path().join("archive.sqlite"),
        JournalOptions::default(),
    )
    .await?;
    let feed = UpdateFeed::open(journal.clone(), "archive", 0, UpdateOptions::default()).await?;
    let source = feed.claim_source(SourceMode::Webhook).await?;
    for (index, event) in fixture["events"].as_array().unwrap().iter().enumerate() {
        let id = index as i64 + 1;
        let mut message = json!({"message_id":id,"date":1715702400+id,"chat":{"id":event["chat"].as_str().unwrap().parse::<i64>()?,"type":"supergroup"},"from":{"id":100,"is_bot":false,"first_name":event["author"]},"text":event["text"]});
        if let Some(document) = event.get("document") {
            message["document"] = document.clone();
        }
        feed.append_webhook(
            &source,
            serde_json::from_value(json!({"update_id":id,"message":message}))?,
        )
        .await?;
    }
    feed.release_source(source).await?;
    let principal = Principal::new("evaluation")?;
    let policy = PolicyStore::default();
    policy.replace(
        &principal,
        "archive",
        Grant::new(0, Budget::new("archive-budget")?)
            .allow(Action::GetMe)
            .allow(Action::GetChat)
            .allow(Action::ReadUpdates)
            .allow(Action::GetOperation)
            .chats(ChatScope::only(chats)),
    )?;
    let executor = Executor::new(policy, ExecutorOptions::default())?;
    executor.register_profile(
        feed,
        tg::Bot::builder(token)
            .api_url(remote.uri())
            .allow_http_loopback(true)
            .build()?,
    )?;
    let result = McpServer::new(
        ToolSession::new(executor, principal),
        ExecutionMode::Execute,
        OutputLimit::default(),
    )
    .serve_parts(tokio::io::stdin(), tokio::io::stdout())
    .await;
    journal.close().await?;
    result?;
    Ok(())
}
