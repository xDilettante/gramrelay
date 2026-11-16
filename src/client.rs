use crate::{
    AnswerCallbackQuery, CallbackQueryId, ChatId, EffectOutcome, Error, ErrorKind, InputFile,
    Result, SendDocument, SendMessage,
};
use reqwest::{Client, RequestBuilder, Url};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    fmt,
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct Bot {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    token: String,
    api_url: Url,
    timeout: Duration,
    max_response_bytes: usize,
    max_upload_bytes: u64,
    in_flight: Semaphore,
}

/// Configure trusted transport settings. Do not populate these from untrusted tool arguments.
pub struct BotBuilder {
    token: String,
    api_url: String,
    allow_http_loopback: bool,
    timeout: Duration,
    max_response_bytes: usize,
    max_upload_bytes: u64,
    max_in_flight: usize,
}

impl fmt::Debug for Bot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bot")
            .field("token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Bot {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        Self::builder(token).build()
    }

    pub fn from_env(variable: &str) -> Result<Self> {
        let token = std::env::var(variable)
            .map_err(|_| Error::local(ErrorKind::Configuration, "token_env"))?;
        Self::new(token)
    }

    pub fn builder(token: impl Into<String>) -> BotBuilder {
        BotBuilder {
            token: token.into(),
            api_url: "https://api.telegram.org/".into(),
            allow_http_loopback: false,
            timeout: Duration::from_secs(30),
            max_response_bytes: 1024 * 1024,
            max_upload_bytes: 50_000_000,
            max_in_flight: 32,
        }
    }

    /// Create a lazy plain-text request. Only awaiting it dispatches the request.
    pub fn send_message(&self, chat: ChatId, text: impl Into<String>) -> SendMessage<'_> {
        SendMessage::new(self, chat, text.into())
    }

    /// Answer a callback from an update already received by the host.
    pub fn answer_callback_query(&self, id: CallbackQueryId) -> AnswerCallbackQuery<'_> {
        AnswerCallbackQuery::new(self, id)
    }

    pub fn send_document(&self, chat: ChatId, file: InputFile) -> SendDocument<'_> {
        SendDocument::new(self, chat, file)
    }

    pub(crate) fn upload_limit(&self) -> u64 {
        self.inner.max_upload_bytes
    }

    #[cfg(feature = "bot")]
    pub(crate) fn request_deadline(&self) -> Duration {
        self.inner.timeout
    }

    #[cfg(feature = "bot")]
    pub(crate) fn identity_key(&self) -> &str {
        let id = self
            .inner
            .token
            .split(':')
            .next()
            .unwrap_or_default()
            .trim_start_matches('0');
        if id.is_empty() { "0" } else { id }
    }

    pub(crate) fn post(&self, method: &str) -> Result<RequestBuilder> {
        // A leading ./ prevents the token's colon from being parsed as a URL scheme.
        let url = self
            .inner
            .api_url
            .join(&format!("./bot{}/{method}", self.inner.token))
            .map_err(|_| Error::local(ErrorKind::Configuration, "api_url"))?;
        Ok(self.inner.http.post(url))
    }

    pub(crate) async fn perform<T: DeserializeOwned>(
        &self,
        prepare: impl Future<Output = Result<RequestBuilder>>,
    ) -> Result<T> {
        let _permit = self
            .inner
            .in_flight
            .try_acquire()
            .map_err(|_| Error::local(ErrorKind::Capacity, "max_in_flight"))?;
        let mut dispatched = false;
        let work = async {
            let request = prepare
                .await?
                .build()
                .map_err(|_| Error::local(ErrorKind::Validation, "request"))?;
            // From this point onward, transport errors cannot prove the absence of an effect.
            dispatched = true;
            let mut response = self
                .inner
                .http
                .execute(request)
                .await
                .map_err(transport_error)?;
            let status = response.status();
            let limit = self.inner.max_response_bytes;
            if response.content_length().is_some_and(|n| n > limit as u64) {
                return Err(Error::new(
                    ErrorKind::ResponseTooLarge,
                    EffectOutcome::Unknown,
                ));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
                if chunk.len() > limit - body.len() {
                    return Err(Error::new(
                        ErrorKind::ResponseTooLarge,
                        EffectOutcome::Unknown,
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            let envelope: Envelope<T> = serde_json::from_slice(&body)
                .map_err(|_| Error::new(ErrorKind::Decode, EffectOutcome::Unknown))?;
            if status.is_success() && envelope.ok {
                return envelope
                    .result
                    .ok_or_else(|| Error::new(ErrorKind::Decode, EffectOutcome::Unknown));
            }
            let explicit_refusal = !envelope.ok
                && !status.is_server_error()
                && envelope
                    .error_code
                    .is_some_and(|code| (400..500).contains(&code))
                && (status.is_success() || status.is_client_error());
            let mut error = Error::new(
                if explicit_refusal {
                    ErrorKind::Telegram
                } else {
                    ErrorKind::Http
                },
                if explicit_refusal {
                    EffectOutcome::Rejected
                } else {
                    EffectOutcome::Unknown
                },
            );
            error.http_status = Some(status.as_u16());
            error.api_error_code = envelope.error_code;
            if let Some(parameters) = envelope.parameters {
                error.retry_after = parameters.retry_after.map(Duration::from_secs);
                error.migrate_to_chat_id = parameters.migrate_to_chat_id;
            }
            Err(error)
        };
        match tokio::time::timeout(self.inner.timeout, work).await {
            Ok(result) => result,
            Err(_) => Err(Error::new(
                ErrorKind::Timeout,
                if dispatched {
                    EffectOutcome::Unknown
                } else {
                    EffectOutcome::NotSent
                },
            )),
        }
    }
}

impl BotBuilder {
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }
    /// Permit plaintext HTTP only to a literal loopback IP, for local mocks.
    pub fn allow_http_loopback(mut self, allow: bool) -> Self {
        self.allow_http_loopback = allow;
        self
    }
    /// Deadline includes preparation, upload and reading the complete response.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub fn max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }
    pub fn max_upload_bytes(mut self, limit: u64) -> Self {
        self.max_upload_bytes = limit;
        self
    }
    /// Requests exceeding the limit fail immediately; no unbounded wait queue is created.
    pub fn max_in_flight(mut self, limit: usize) -> Self {
        self.max_in_flight = limit;
        self
    }

    pub fn build(self) -> Result<Bot> {
        let valid_token = self.token.split_once(':').is_some_and(|(id, secret)| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && !secret.is_empty()
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
        if !valid_token {
            return Err(Error::local(ErrorKind::Configuration, "token"));
        }
        if self.timeout.is_zero() || Instant::now().checked_add(self.timeout).is_none() {
            return Err(Error::local(ErrorKind::Configuration, "timeout"));
        }
        if self.max_response_bytes == 0
            || self.max_upload_bytes == 0
            || self.max_in_flight == 0
            || self.max_in_flight > Semaphore::MAX_PERMITS
        {
            return Err(Error::local(ErrorKind::Configuration, "limits"));
        }
        let mut api_url = Url::parse(&self.api_url)
            .map_err(|_| Error::local(ErrorKind::Configuration, "api_url"))?;
        let loopback = api_url
            .host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|ip| ip.is_loopback());
        if !(api_url.scheme() == "https"
            || (api_url.scheme() == "http" && self.allow_http_loopback && loopback))
            || api_url.host().is_none()
            || !api_url.username().is_empty()
            || api_url.password().is_some()
            || api_url.query().is_some()
            || api_url.fragment().is_some()
        {
            return Err(Error::local(ErrorKind::Configuration, "api_url"));
        }
        if !api_url.path().ends_with('/') {
            api_url.set_path(&format!("{}/", api_url.path()));
        }
        // https://docs.rs/reqwest/0.13.5/reqwest/retry/fn.never.html
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(self.timeout.min(Duration::from_secs(10)))
            .pool_max_idle_per_host(self.max_in_flight)
            .build()
            .map_err(|_| Error::local(ErrorKind::Configuration, "http_client"))?;
        Ok(Bot {
            inner: Arc::new(Inner {
                http,
                token: self.token,
                api_url,
                timeout: self.timeout,
                max_response_bytes: self.max_response_bytes,
                max_upload_bytes: self.max_upload_bytes,
                in_flight: Semaphore::new(self.max_in_flight),
            }),
        })
    }
}

fn transport_error(error: reqwest::Error) -> Error {
    Error::new(
        if error.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::Transport
        },
        EffectOutcome::Unknown,
    )
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    error_code: Option<i64>,
    parameters: Option<Parameters>,
}

#[derive(Deserialize)]
struct Parameters {
    retry_after: Option<u64>,
    migrate_to_chat_id: Option<ChatId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deadline_before_dispatch_is_not_sent() {
        let bot = Bot::builder("123456:OFFLINE_TEST_TOKEN")
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();
        let error = bot
            .perform::<bool>(std::future::pending())
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Timeout);
        assert_eq!(error.outcome(), EffectOutcome::NotSent);
    }
}
