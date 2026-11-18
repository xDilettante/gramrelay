use std::{
    error::Error,
    path::Path as FilePath,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::to_bytes,
    extract::{DefaultBodyLimit, FromRequest, Multipart, Path, Request, State},
    http::{StatusCode, Version, header::CONTENT_TYPE},
    routing::post,
    serve::ListenerExt,
};
use frankenstein::{
    AsyncTelegramApi,
    methods::{AnswerCallbackQueryParams, SendDocumentParams, SendMessageParams},
};
use serde_json::{Value, json};
use teloxide::requests::Requester;

const TOKEN: &str = "0:BENCHMARK";
const CHAT: i64 = -17001;
const TEXT: &str = "Telegram Rust: Привет 🌍";
const CALLBACK: &str = "opaque-benchmark-id";
const FILENAME: &str = "benchmark.bin";
static DOCUMENT: [u8; 65536] = [0x5a; 65536];
const TIMEOUT: Duration = Duration::from_secs(5);
const POOL_CAP: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    GetMe,
    SendMessage,
    AnswerCallback,
    SendDocument,
}

impl Scenario {
    const ALL: [Self; 4] = [
        Self::GetMe,
        Self::SendMessage,
        Self::AnswerCallback,
        Self::SendDocument,
    ];

    fn method(self) -> &'static str {
        match self {
            Self::GetMe => "getMe",
            Self::SendMessage => "sendMessage",
            Self::AnswerCallback => "answerCallbackQuery",
            Self::SendDocument => "sendDocument",
        }
    }

    fn receipt(self) -> i64 {
        match self {
            Self::GetMe => 7,
            Self::SendMessage => 41,
            Self::AnswerCallback => 1,
            Self::SendDocument => 42,
        }
    }

    fn response(self) -> Value {
        match self {
            Self::GetMe => json!({
                "id": 7, "is_bot": true, "first_name": "Benchmark", "username": "benchmark_bot",
                "can_join_groups": true, "can_read_all_group_messages": false,
                "supports_inline_queries": false, "can_connect_to_business": false,
                "has_main_web_app": false
            }),
            Self::AnswerCallback => json!(true),
            Self::SendMessage | Self::SendDocument => {
                let mut message = json!({
                    "message_id": self.receipt(), "date": 1715702400,
                    "chat": {"id": CHAT, "type": "supergroup", "title": "Benchmark"}
                });
                if self == Self::SendMessage {
                    message["text"] = json!(TEXT);
                } else {
                    message["document"] = json!({
                        "file_id": "bench-file", "file_unique_id": "bench-unique",
                        "file_name": FILENAME, "file_size": DOCUMENT.len()
                    });
                }
                message
            }
        }
    }
}

struct Mock {
    count: AtomicUsize,
    connections: AtomicUsize,
    replies: [String; 4],
}

async fn receive(
    State(mock): State<Arc<Mock>>,
    Path((credential, method)): Path<(String, String)>,
    request: Request,
) -> Result<([(axum::http::HeaderName, &'static str); 1], String), StatusCode> {
    let (index, scenario) = Scenario::ALL
        .into_iter()
        .enumerate()
        .find(|(_, scenario)| scenario.method().eq_ignore_ascii_case(&method))
        .ok_or(StatusCode::NOT_FOUND)?;
    if credential.strip_prefix("bot") != Some(TOKEN) || request.version() != Version::HTTP_11 {
        return Err(StatusCode::BAD_REQUEST);
    }
    if scenario == Scenario::SendDocument {
        validate_document(request).await?;
    } else {
        let json_content = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next() == Some("application/json"));
        let body = to_bytes(request.into_body(), 2048)
            .await
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if !json_content {
            return Err(StatusCode::BAD_REQUEST);
        }
        let valid = if scenario == Scenario::GetMe && body.is_empty() {
            true
        } else {
            let body: Value = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
            match scenario {
                Scenario::GetMe => body == json!({}),
                Scenario::SendMessage => body == json!({"chat_id": CHAT, "text": TEXT}),
                Scenario::AnswerCallback => body == json!({"callback_query_id": CALLBACK}),
                Scenario::SendDocument => unreachable!(),
            }
        };
        if !valid {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    mock.count.fetch_add(1, Ordering::Relaxed);
    Ok((
        [(CONTENT_TYPE, "application/json")],
        mock.replies[index].clone(),
    ))
}

async fn validate_document(request: Request) -> Result<(), StatusCode> {
    let mut parts = Multipart::from_request(request, &())
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut chat = false;
    let mut document_part = None;
    let mut document_reference = None;
    while let Some(part) = parts
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        match (part.name(), part.file_name()) {
            (Some("chat_id"), None) if !chat => {
                chat = part.text().await.map_err(|_| StatusCode::BAD_REQUEST)? == CHAT.to_string();
                if !chat {
                    return Err(StatusCode::BAD_REQUEST);
                }
            }
            (Some("document"), None) if document_reference.is_none() => {
                let reference = part.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                let name = reference
                    .strip_prefix("attach://")
                    .filter(|name| !name.is_empty() && *name != "document" && *name != "chat_id")
                    .ok_or(StatusCode::BAD_REQUEST)?;
                document_reference = Some(name.to_owned());
            }
            (Some(name), Some(FILENAME)) if document_part.is_none() => {
                let name = name.to_owned();
                let bytes = part.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if bytes.as_ref() != DOCUMENT.as_slice() {
                    return Err(StatusCode::BAD_REQUEST);
                }
                document_part = Some(name);
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    }
    if !chat
        || document_part.as_deref() != Some(document_reference.as_deref().unwrap_or("document"))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

enum Client {
    Tg(tg::Bot),
    Teloxide(teloxide::Bot),
    Frankenstein(frankenstein::client_reqwest::Bot),
}

impl Client {
    fn name(&self) -> &'static str {
        match self {
            Self::Tg(_) => "tg",
            Self::Teloxide(_) => "teloxide",
            Self::Frankenstein(_) => "frankenstein",
        }
    }

    async fn request(&self, scenario: Scenario, document: &FilePath) -> Result<(), Box<dyn Error>> {
        let receipt = match (self, scenario) {
            (Self::Tg(bot), Scenario::GetMe) => i64::from(bot.api().get_me().await?.id),
            (Self::Tg(bot), Scenario::SendMessage) => {
                i64::from(bot.send_message(CHAT.into(), TEXT).await?.message_id)
            }
            (Self::Tg(bot), Scenario::AnswerCallback) => {
                i64::from(bot.answer_callback_query(CALLBACK.into()).await?)
            }
            (Self::Tg(bot), Scenario::SendDocument) => {
                let file = tg::InputFile::path(document);
                i64::from(bot.send_document(CHAT.into(), file).await?.message_id)
            }
            (Self::Teloxide(bot), Scenario::GetMe) => i64::try_from(bot.get_me().await?.id.0)?,
            (Self::Teloxide(bot), Scenario::SendMessage) => i64::from(
                bot.send_message(teloxide::types::ChatId(CHAT), TEXT)
                    .await?
                    .id
                    .0,
            ),
            (Self::Teloxide(bot), Scenario::AnswerCallback) => {
                let query = teloxide::types::CallbackQueryId(CALLBACK.to_owned());
                let teloxide::types::True = bot.answer_callback_query(query).await?;
                1
            }
            (Self::Teloxide(bot), Scenario::SendDocument) => {
                let file = teloxide::types::InputFile::file(document);
                i64::from(
                    bot.send_document(teloxide::types::ChatId(CHAT), file)
                        .await?
                        .id
                        .0,
                )
            }
            (Self::Frankenstein(bot), Scenario::GetMe) => {
                i64::try_from(bot.get_me().await?.result.id)?
            }
            (Self::Frankenstein(bot), Scenario::SendMessage) => {
                let params = SendMessageParams::builder()
                    .chat_id(CHAT)
                    .text(TEXT)
                    .build();
                i64::from(bot.send_message(&params).await?.result.message_id)
            }
            (Self::Frankenstein(bot), Scenario::AnswerCallback) => {
                let params = AnswerCallbackQueryParams::builder()
                    .callback_query_id(CALLBACK)
                    .build();
                i64::from(bot.answer_callback_query(&params).await?.result)
            }
            (Self::Frankenstein(bot), Scenario::SendDocument) => {
                let params = SendDocumentParams::builder()
                    .chat_id(CHAT)
                    .document(document.to_path_buf())
                    .build();
                i64::from(bot.send_document(&params).await?.result.message_id)
            }
        };
        if receipt != scenario.receipt() {
            return Err("unexpected typed receipt".into());
        }
        Ok(())
    }
}

fn clients(base: &str) -> Result<[Client; 3], Box<dyn Error>> {
    let tg = tg::Bot::builder(TOKEN)
        .api_url(base)
        .allow_http_loopback(true)
        .timeout(TIMEOUT)
        .max_in_flight(POOL_CAP)
        .build()?;
    let teloxide_http = reqwest_012::Client::builder()
        .no_proxy()
        .redirect(reqwest_012::redirect::Policy::none())
        .retry(reqwest_012::retry::never())
        .connect_timeout(TIMEOUT)
        .timeout(TIMEOUT)
        .pool_max_idle_per_host(POOL_CAP)
        .build()?;
    let frankenstein_http = frankenstein::reqwest::Client::builder()
        .no_proxy()
        .redirect(frankenstein::reqwest::redirect::Policy::none())
        .retry(frankenstein::reqwest::retry::never())
        .connect_timeout(TIMEOUT)
        .timeout(TIMEOUT)
        .pool_max_idle_per_host(POOL_CAP)
        .build()?;
    Ok([
        Client::Tg(tg),
        Client::Teloxide(
            teloxide::Bot::with_client(TOKEN, teloxide_http).set_api_url(base.parse()?),
        ),
        Client::Frankenstein(
            frankenstein::client_reqwest::Bot::builder()
                .api_url(format!("{base}/bot{TOKEN}"))
                .client(frankenstein_http)
                .build(),
        ),
    ])
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let (rounds, warmup, samples) = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => (3, 100, 1000),
        [mode] if mode == "--smoke" => (1, 2, 6),
        _ => return Err("usage: telegram-http-benchmark [--smoke]".into()),
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join(FILENAME);
    std::fs::write(&path, DOCUMENT.as_slice())?;
    let mock = Arc::new(Mock {
        count: AtomicUsize::new(0),
        connections: AtomicUsize::new(0),
        replies: Scenario::ALL
            .map(|scenario| json!({"ok": true, "result": scenario.response()}).to_string()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let peer_counts = Arc::clone(&mock);
    let listener = listener.tap_io(move |stream| {
        stream.set_nodelay(true).expect("configure loopback socket");
        peer_counts.connections.fetch_add(1, Ordering::Relaxed);
    });
    let router = Router::new()
        .route("/{credential}/{method}", post(receive))
        .layer(DefaultBodyLimit::max(128 * 1024))
        .with_state(Arc::clone(&mock));
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    let clients = clients(&base)?;
    let mut batches = Vec::new();
    for round in 0..rounds {
        for scenario in Scenario::ALL {
            for client in clients.iter().cycle().skip(round).take(clients.len()) {
                let before = mock.count.load(Ordering::Relaxed);
                for _ in 0..warmup {
                    client.request(scenario, &path).await?;
                }
                let open_connections = mock.connections.load(Ordering::Relaxed);
                let mut durations = Vec::with_capacity(samples);
                for _ in 0..samples {
                    let start = Instant::now();
                    client.request(scenario, &path).await?;
                    durations.push(u64::try_from(start.elapsed().as_nanos())?);
                }
                if mock.count.load(Ordering::Relaxed) - before != warmup + samples {
                    return Err("mock request count differs from completed calls".into());
                }
                if mock.connections.load(Ordering::Relaxed) != open_connections {
                    return Err("a measured request opened a new TCP connection".into());
                }
                let mut sorted = durations.clone();
                sorted.sort_unstable();
                batches.push(json!({
                    "round": round + 1, "scenario": scenario.method(), "client": client.name(),
                    "samples_ns": durations, "p50_ns": sorted[(samples * 50).div_ceil(100) - 1],
                    "p95_ns": sorted[(samples * 95).div_ceil(100) - 1]
                }));
            }
        }
        eprintln!("completed rotation {}/{}", round + 1, rounds);
    }
    server.abort();
    let _ = server.await;
    println!(
        "{}",
        json!({
            "warmup_per_batch": warmup, "samples_per_batch": samples,
            "document_bytes": DOCUMENT.len(), "batches": batches
        })
    );
    Ok(())
}
