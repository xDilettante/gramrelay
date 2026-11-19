use super::{WebhookError, WebhookIngress};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use std::error::Error as _;
#[cfg(feature = "tracing")]
use tracing::Instrument;

const SECRET: &str = "x-telegram-bot-api-secret-token";
impl WebhookIngress {
    /// Mount in the host's existing Axum/TLS server. The host owns connection limits/shutdown.
    pub fn router(&self) -> axum::Router {
        axum::Router::new()
            .route(&self.options().path, post(receive))
            .with_state(self.clone())
    }
}
async fn receive(State(ingress): State<WebhookIngress>, request: Request) -> Response {
    let mut observation = super::telemetry::Observation::new();
    let work = receive_inner(ingress, request);
    #[cfg(feature = "tracing")]
    let result = work.instrument(observation.span()).await;
    #[cfg(not(feature = "tracing"))]
    let result = work.await;
    observation.finish(&result);
    response(result)
}
async fn receive_inner(ingress: WebhookIngress, request: Request) -> Result<(), WebhookError> {
    let supplied = one(request.headers(), SECRET);
    let admitted = ingress.begin(supplied)?;
    let declared = metadata(request.headers())?;
    if declared.is_some_and(|length| length > admitted.max_body_bytes()) {
        return Err(WebhookError::PayloadTooLarge);
    }
    let bytes = tokio::select! {
        _=admitted.cancelled()=>return Err(WebhookError::Unavailable),
        result=tokio::time::timeout_at(admitted.deadline,to_bytes(request.into_body(),admitted.max_body_bytes()))=>{
            result.map_err(|_|WebhookError::Timeout)?.map_err(|error| {
                if error.source().is_some_and(|source|source.is::<http_body_util::LengthLimitError>()) {WebhookError::PayloadTooLarge}else{WebhookError::BadRequest}
            })?
        }
    };
    if declared.is_some_and(|length| length != bytes.len()) {
        return Err(WebhookError::BadRequest);
    }
    admitted.deliver(&bytes).await
}
fn one<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    Some(value.as_bytes())
}
fn metadata(headers: &HeaderMap) -> Result<Option<usize>, WebhookError> {
    let content = one(headers, header::CONTENT_TYPE.as_str())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or(WebhookError::UnsupportedMediaType)?;
    let mut fields = content.split(';');
    if !fields
        .next()
        .is_some_and(|part| part.trim().eq_ignore_ascii_case("application/json"))
    {
        return Err(WebhookError::UnsupportedMediaType);
    }
    if let Some(charset) = fields.next()
        && (!["charset=utf-8", "charset=\"utf-8\""]
            .iter()
            .any(|valid| charset.trim().eq_ignore_ascii_case(valid))
            || fields.next().is_some())
    {
        return Err(WebhookError::UnsupportedMediaType);
    }
    if headers.contains_key(header::CONTENT_ENCODING)
        && !one(headers, header::CONTENT_ENCODING.as_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(b"identity"))
    {
        return Err(WebhookError::UnsupportedMediaType);
    }
    if headers.contains_key(header::CONTENT_LENGTH) {
        if headers.contains_key(header::TRANSFER_ENCODING) {
            return Err(WebhookError::BadRequest);
        }
        let bytes =
            one(headers, header::CONTENT_LENGTH.as_str()).ok_or(WebhookError::BadRequest)?;
        if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
            return Err(WebhookError::BadRequest);
        }
        return std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Some)
            .ok_or(WebhookError::BadRequest);
    }
    Ok(None)
}
fn response(result: Result<(), WebhookError>) -> Response {
    let (status, body) = match result {
        Ok(()) => (StatusCode::OK, "{\"ok\":true}".to_owned()),
        Err(error) => (
            StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            format!("{{\"ok\":false,\"code\":\"{}\"}}", error.code()),
        ),
    };
    let mut response = (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Body::from(body),
    )
        .into_response();
    if status.is_server_error() {
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("1"),
        );
    }
    response
}
