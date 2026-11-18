use std::{
    convert::Infallible,
    error::Error,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use tg::{Bot, EffectOutcome, ErrorKind, InputFile, api::types::File};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt, DuplexStream, duplex},
    sync::Semaphore,
};

use super::{CAP, Duration, Mock, Relaxed, TOKEN, resident_bytes, timeout};

pub(super) const TRANSFER_LIMIT: usize = 16 * 1024;
pub(super) const RESPONSE_LIMIT: usize = 4096;
const UPLOAD_PREFIX: usize = TRANSFER_LIMIT / 2;
static PAYLOAD: [u8; TRANSFER_LIMIT] = patterned();
static OVERSIZED: [u8; TRANSFER_LIMIT + 1] = patterned();
static UPLOAD_REPLY: [u8; RESPONSE_LIMIT] =
    padded(br#"{"ok":true,"result":{"message_id":42,"date":1,"chat":{"id":1,"type":"private"}}}"#);
static OVERSIZED_JSON: [u8; RESPONSE_LIMIT + 1] = padded(
    br#"{"ok":true,"result":{"url":"","has_custom_certificate":false,"pending_update_count":0}}"#,
);

const fn patterned<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    let mut index = 0;
    while index < N {
        bytes[index] = (index % 251) as u8;
        index += 1;
    }
    bytes
}

const fn padded<const N: usize>(json: &[u8]) -> [u8; N] {
    let mut bytes = [b' '; N];
    let mut index = 0;
    while index < json.len() {
        bytes[index] = json[index];
        index += 1;
    }
    bytes
}

pub(super) fn routes() -> Router<Arc<Mock>> {
    Router::new()
        .route(&format!("/bot{TOKEN}/getWebhookInfo"), post(large_response))
        .route(&format!("/bot{TOKEN}/sendDocument"), post(upload))
        .route(&format!("/file/bot{TOKEN}/{{file}}"), get(download))
        .layer(DefaultBodyLimit::max(TRANSFER_LIMIT * 2))
}

fn chunked(mock: Arc<Mock>, content_type: &'static str, data: &'static [u8]) -> Response {
    let (prefix, rest) = data.split_at(2048);
    let chunks = stream::iter([Ok::<_, Infallible>(Bytes::from_static(prefix))]).chain(
        stream::once(async move {
            mock.hold().await;
            Ok(Bytes::from_static(rest))
        }),
    );
    ([(CONTENT_TYPE, content_type)], Body::from_stream(chunks)).into_response()
}

async fn large_response(State(mock): State<Arc<Mock>>) -> Response {
    chunked(mock, "application/json", OVERSIZED_JSON.as_slice())
}

async fn download(
    State(mock): State<Arc<Mock>>,
    Path(file): Path<String>,
) -> Result<Response, StatusCode> {
    let data = match file.as_str() {
        "bounded.bin" => PAYLOAD.as_slice(),
        "oversized.bin" => OVERSIZED.as_slice(),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    Ok(chunked(mock, "application/octet-stream", data))
}

async fn upload(
    State(mock): State<Arc<Mock>>,
    mut parts: Multipart,
) -> Result<Response, StatusCode> {
    let mut chat = false;
    let mut document = false;
    while let Some(mut part) = parts
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
    {
        match (part.name(), part.file_name()) {
            (Some("chat_id"), None) if !chat => {
                if part.text().await.map_err(|_| StatusCode::BAD_REQUEST)? != "1" {
                    return Err(StatusCode::BAD_REQUEST);
                }
                chat = true;
            }
            (Some("document"), Some("pressure.bin")) if !document => {
                let first = part
                    .chunk()
                    .await
                    .map_err(|_| StatusCode::BAD_REQUEST)?
                    .ok_or(StatusCode::BAD_REQUEST)?;
                if first.is_empty() || first.len() >= PAYLOAD.len() || !PAYLOAD.starts_with(&first)
                {
                    return Err(StatusCode::BAD_REQUEST);
                }
                mock.hold().await;
                let rest = part.bytes().await.map_err(|_| StatusCode::BAD_REQUEST)?;
                if rest != PAYLOAD[first.len()..] {
                    return Err(StatusCode::BAD_REQUEST);
                }
                document = true;
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        }
    }
    if !chat || !document {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok((
        [(CONTENT_TYPE, "application/json")],
        UPLOAD_REPLY.as_slice(),
    )
        .into_response())
}

fn file(path: &str) -> File {
    File::new("fixture".into(), "fixture-unique".into()).file_path(path.to_owned())
}

fn fixture_reader(data: &'static [u8]) -> InputFile {
    InputFile::reader("pressure.bin", data, data.len() as u64)
}

fn expect_error<T>(
    received: tg::Result<T>,
    kind: ErrorKind,
    outcome: EffectOutcome,
) -> Result<(), String> {
    match received {
        Err(error) if error.kind() == kind && error.outcome() == outcome => Ok(()),
        Err(error) => Err(format!("expected {kind:?}/{outcome:?}, got {error}")),
        Ok(_) => Err(format!("expected {kind:?}/{outcome:?}, got success")),
    }
}

struct Witness {
    output: Vec<u8>,
    progress: Arc<Semaphore>,
}

impl AsyncWrite for Witness {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !data.is_empty() && self.output.is_empty() {
            self.progress.add_permits(1);
        }
        self.output.extend_from_slice(data);
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

enum Scenario {
    OversizedResponse,
    Upload(DuplexStream),
    Download,
    OversizedDownload,
}

async fn checked_transfer(
    bot: Bot,
    scenario: Scenario,
    progress: Arc<Semaphore>,
) -> Result<Option<usize>, String> {
    match scenario {
        Scenario::OversizedResponse => expect_error(
            bot.api().get_webhook_info().await,
            ErrorKind::ResponseTooLarge,
            EffectOutcome::Unknown,
        )?,
        Scenario::Upload(input) => {
            let input = InputFile::reader("pressure.bin", input, TRANSFER_LIMIT as u64);
            let reply = bot
                .send_document(1.into(), input)
                .await
                .map_err(|error| error.to_string())?;
            if i64::from(reply.message_id) != 42 {
                return Err("unexpected streamed upload result".into());
            }
        }
        Scenario::Download => {
            let mut writer = Witness {
                output: Vec::new(),
                progress,
            };
            let size = bot
                .download_file(&file("bounded.bin"))
                .to_writer(&mut writer)
                .await
                .map_err(|error| error.to_string())?;
            if size != TRANSFER_LIMIT as u64 || writer.output != PAYLOAD.as_slice() {
                return Err("streamed download did not preserve exact file bytes".into());
            }
        }
        Scenario::OversizedDownload => {
            let mut writer = Witness {
                output: Vec::new(),
                progress,
            };
            expect_error(
                bot.download_file(&file("oversized.bin"))
                    .to_writer(&mut writer)
                    .await,
                ErrorKind::ResponseTooLarge,
                EffectOutcome::Unknown,
            )?;
            if writer.output.len() > TRANSFER_LIMIT || !PAYLOAD.starts_with(&writer.output) {
                return Err("oversized download crossed the writer's byte limit".into());
            }
            return Ok(Some(writer.output.len()));
        }
    }
    Ok(None)
}

pub(super) async fn run(bot: &Bot, mock: &Mock) -> Result<Value, Box<dyn Error>> {
    let deadline = Duration::from_secs(10);
    let prior = mock.received.load(Relaxed);
    let ingress = mock.ingress.load(Relaxed);
    let mut tasks = tokio::task::JoinSet::new();
    let progress = Arc::new(Semaphore::new(0));
    let mut upload_writers = Vec::new();
    mock.permits.acquire_many(CAP as u32).await?.forget();
    for _ in 0..CAP / 4 {
        let (input, mut writer) = duplex(TRANSFER_LIMIT);
        writer.write_all(&PAYLOAD[..UPLOAD_PREFIX]).await?;
        upload_writers.push(writer);
        for scenario in [
            Scenario::OversizedResponse,
            Scenario::Upload(input),
            Scenario::Download,
            Scenario::OversizedDownload,
        ] {
            tasks.spawn(checked_transfer(bot.clone(), scenario, progress.clone()));
        }
    }
    mock.wait_for_full(prior).await?;
    timeout(deadline, progress.acquire_many((CAP / 2) as u32))
        .await??
        .forget();
    expect_error(
        timeout(deadline, bot.get_me()).await?,
        ErrorKind::Capacity,
        EffectOutcome::NotSent,
    )?;
    expect_error(
        timeout(deadline, bot.download_file(&file("bounded.bin")).bytes()).await?,
        ErrorKind::Capacity,
        EffectOutcome::NotSent,
    )?;
    expect_error(
        timeout(
            deadline,
            bot.send_document(1.into(), fixture_reader(PAYLOAD.as_slice())),
        )
        .await?,
        ErrorKind::Capacity,
        EffectOutcome::NotSent,
    )?;
    if mock.ingress.load(Relaxed) != ingress + CAP {
        return Err("overload entered the transfer fixture".into());
    }
    let saturated = resident_bytes()?;
    for mut writer in upload_writers {
        writer.write_all(&PAYLOAD[UPLOAD_PREFIX..]).await?;
        writer.shutdown().await?;
    }
    mock.permits.add_permits(CAP);
    let mut partial_download_bytes = Vec::new();
    while let Some(result) = timeout(deadline, tasks.join_next()).await? {
        partial_download_bytes.extend(result??);
    }
    expect_error(
        bot.send_document(1.into(), fixture_reader(OVERSIZED.as_slice()))
            .await,
        ErrorKind::Validation,
        EffectOutcome::NotSent,
    )?;
    bot.api().get_me().await?;
    if mock.active.load(Relaxed) != 0
        || mock.received.load(Relaxed) != prior + CAP + 1
        || mock.ingress.load(Relaxed) != ingress + CAP + 1
    {
        return Err("transfer completion or post-load admission failed".into());
    }
    Ok(json!({
        "held": CAP, "rejected_before_dispatch": 3, "recovered": true,
        "oversized_response_limit_bytes": RESPONSE_LIMIT, "transfer_limit_bytes": TRANSFER_LIMIT,
        "confirmed_each_scenario": CAP / 4, "downloads_with_observed_prefix_before_release": CAP / 2,
        "oversized_upload_rejected_before_dispatch": true,
        "oversized_download_prefix_bytes": partial_download_bytes,
        "saturated_resident_bytes": saturated, "settled_resident_bytes": resident_bytes()?,
    }))
}
