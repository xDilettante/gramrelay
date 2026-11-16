//! Bounded scripted execution without sockets. Uses production serialization and decoding.
//! Fixtures and explicit body inspection may contain test data; use synthetic inputs.
use crate::{Bot, BotBuilder, EffectOutcome, Error, ErrorKind, Result, api::Method};
use http_body_util::BodyExt;
use serde_json::Value;
use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// An ordered, bounded script shared by Bot clones. Never falls back to HTTP.
#[derive(Clone)]
pub struct MockTransport {
    inner: Arc<Inner>,
}
struct Inner {
    state: Mutex<State>,
    max_requests: usize,
    max_bytes: usize,
}
struct State {
    steps: VecDeque<Step>,
    requests: Vec<RecordedRequest>,
    recorded_bytes: usize,
    active: usize,
}

/// A synthetic HTTP response. No raw remote errors or secrets enter Debug.
pub struct MockResponse {
    status: u16,
    chunks: Vec<Vec<u8>>,
    delay: Duration,
    chunk_delay: Duration,
    fail_after: Option<usize>,
}
impl MockResponse {
    pub fn json(body: Value) -> Self {
        Self::bytes(body.to_string().into_bytes())
    }
    pub fn bytes(body: impl Into<Vec<u8>>) -> Self {
        Self::chunks([body.into()])
    }
    pub fn chunks(chunks: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            status: 200,
            chunks: chunks.into_iter().collect(),
            delay: Duration::ZERO,
            chunk_delay: Duration::ZERO,
            fail_after: None,
        }
    }
    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
    pub fn chunk_delay(mut self, delay: Duration) -> Self {
        self.chunk_delay = delay;
        self
    }
    pub fn fail_after_chunks(mut self, count: usize) -> Self {
        self.fail_after = Some(count);
        self
    }

    async fn response(self, active: Active) -> Result<reqwest::Response> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let delay = self.chunk_delay;
        let fail_after = self.fail_after;
        let stream = futures_util::stream::unfold(
            (self.chunks.into_iter(), 0, false, active),
            move |(mut chunks, count, failed, active)| async move {
                if failed {
                    return None;
                }
                let failed = fail_after == Some(count);
                let chunk = if failed {
                    Err(std::io::Error::other("mock_response_body"))
                } else {
                    Ok(chunks.next()?)
                };
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                Some((chunk, (chunks, count + 1, failed, active)))
            },
        );
        let body = reqwest::Body::wrap_stream(stream);
        let response = http::Response::builder()
            .status(self.status)
            .body(body)
            .map_err(|_| Error::local(ErrorKind::Configuration, "mock_response"))?;
        Ok(reqwest::Response::from(response))
    }
}

/// One expected method and its response or failure. Names are checked against the catalog.
pub struct Step {
    method: &'static str,
    action: Action,
}
enum Action {
    Reply(MockResponse),
    Transport,
    Timeout,
    PartialUpload(usize),
}
impl Step {
    pub fn reply(method: &'static str, response: MockResponse) -> Self {
        Self {
            method,
            action: Action::Reply(response),
        }
    }
    pub fn transport_error(method: &'static str) -> Self {
        Self {
            method,
            action: Action::Transport,
        }
    }
    /// Remain pending after consuming the upload; the Bot's real deadline/cancellation applies.
    pub fn timeout(method: &'static str) -> Self {
        Self {
            method,
            action: Action::Timeout,
        }
    }
    pub fn partial_upload(method: &'static str, bytes: usize) -> Self {
        Self {
            method,
            action: Action::PartialUpload(bytes),
        }
    }
}

/// Explicit inspection of synthetic test data. URL and credentials are never recorded.
#[derive(Clone)]
pub struct RecordedRequest {
    method: &'static str,
    http_method: String,
    content_type: Option<String>,
    body: Vec<u8>,
    upload_complete: bool,
}
impl RecordedRequest {
    pub fn method(&self) -> &'static str {
        self.method
    }
    pub fn http_method(&self) -> &str {
        &self.http_method
    }
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn upload_complete(&self) -> bool {
        self.upload_complete
    }
    /// Decode the actual JSON body using a strict generated method parameter type.
    pub fn decode<M: Method + serde::de::DeserializeOwned>(&self) -> Result<M> {
        if self.method != M::NAME
            || !self.upload_complete
            || self.content_type() != Some("application/json")
        {
            return Err(Error::local(ErrorKind::Validation, "mock_request"));
        }
        serde_json::from_slice(&self.body)
            .map_err(|_| Error::local(ErrorKind::Decode, "mock_request"))
    }
}

impl MockTransport {
    pub fn new(steps: impl IntoIterator<Item = Step>) -> Result<Self> {
        Self::with_limits(steps, 256, 1024 * 1024)
    }
    /// Bound step/history count and total recorded bytes; queued response bytes have an equal cap.
    pub fn with_limits(
        steps: impl IntoIterator<Item = Step>,
        max_requests: usize,
        max_bytes: usize,
    ) -> Result<Self> {
        if max_requests == 0 || max_bytes == 0 {
            return Err(Error::local(ErrorKind::Configuration, "mock_limits"));
        }
        let mut queued = VecDeque::new();
        let mut response_bytes = 0usize;
        let mut response_chunks = 0usize;
        for step in steps {
            if queued.len() == max_requests {
                return Err(Error::local(ErrorKind::Capacity, "mock_steps"));
            }
            if step.method != "download" && !crate::api::catalog::METHODS.contains(&step.method) {
                return Err(Error::local(ErrorKind::Configuration, "mock_method"));
            }
            if let Action::Reply(response) = &step.action {
                response_chunks = response_chunks
                    .checked_add(response.chunks.len())
                    .filter(|count| *count <= max_bytes)
                    .ok_or_else(|| Error::local(ErrorKind::Capacity, "mock_response_chunks"))?;
                if !(100..600).contains(&response.status)
                    || [response.delay, response.chunk_delay]
                        .into_iter()
                        .any(|delay| Instant::now().checked_add(delay).is_none())
                {
                    return Err(Error::local(ErrorKind::Configuration, "mock_response"));
                }
                for chunk in &response.chunks {
                    response_bytes = response_bytes
                        .checked_add(chunk.len())
                        .filter(|sum| *sum <= max_bytes)
                        .ok_or_else(|| Error::local(ErrorKind::Capacity, "mock_response_bytes"))?;
                }
            }
            queued.push_back(step);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    steps: queued,
                    requests: Vec::new(),
                    recorded_bytes: 0,
                    active: 0,
                }),
                max_requests,
                max_bytes,
            }),
        })
    }
    pub fn bot_builder(&self) -> BotBuilder {
        Bot::builder("0:TEST_KIT").mock_transport(self.clone())
    }
    pub fn bot(&self) -> Result<Bot> {
        self.bot_builder().build()
    }
    pub fn requests(&self) -> Result<Vec<RecordedRequest>> {
        Ok(self.state()?.requests.clone())
    }
    /// All steps must have been claimed, and their execution futures finished or dropped.
    pub fn assert_finished(&self) -> Result<()> {
        let state = self.state()?;
        if state.steps.is_empty() && state.active == 0 {
            Ok(())
        } else {
            Err(Error::local(ErrorKind::Validation, "mock_unfinished"))
        }
    }
    fn state(&self) -> Result<MutexGuard<'_, State>> {
        self.inner
            .state
            .lock()
            .map_err(|_| Error::local(ErrorKind::Configuration, "mock_state"))
    }
    fn capture(&self, index: usize, bytes: &[u8]) -> Result<()> {
        let mut state = self.state()?;
        let length = bytes.len().min(self.inner.max_bytes - state.recorded_bytes);
        state.requests[index]
            .body
            .extend_from_slice(&bytes[..length]);
        state.recorded_bytes += length;
        if length != bytes.len() {
            Err(Error::new(ErrorKind::Capacity, EffectOutcome::Unknown))
        } else {
            Ok(())
        }
    }
    pub(crate) async fn execute(
        &self,
        method: &'static str,
        mut request: reqwest::Request,
    ) -> Result<reqwest::Response> {
        let (step, index) = {
            let mut state = self.state()?;
            if state.requests.len() == self.inner.max_requests {
                return Err(Error::local(ErrorKind::Capacity, "mock_requests"));
            }
            if !state
                .steps
                .front()
                .is_some_and(|step| step.method == method)
            {
                return Err(Error::local(ErrorKind::Validation, "mock_script"));
            }
            let step = state.steps.pop_front().expect("front checked above");
            let index = state.requests.len();
            state.requests.push(RecordedRequest {
                method,
                http_method: request.method().to_string(),
                content_type: request
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
                body: Vec::new(),
                upload_complete: false,
            });
            state.active += 1;
            (step, index)
        };
        let active = Active(self.clone());
        let partial = if let Action::PartialUpload(bytes) = step.action {
            Some(bytes)
        } else {
            None
        };
        if partial == Some(0) {
            return Err(failure());
        }
        let mut uploaded = 0usize;
        if let Some(mut body) = request.body_mut().take() {
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(crate::client::transport_error)?;
                if let Some(bytes) = frame.data_ref() {
                    let length =
                        partial.map_or(bytes.len(), |limit| bytes.len().min(limit - uploaded));
                    self.capture(index, &bytes[..length])?;
                    uploaded += length;
                    if partial == Some(uploaded) {
                        return Err(failure());
                    }
                }
            }
        }
        self.state()?.requests[index].upload_complete = true;
        match step.action {
            Action::Reply(response) => response.response(active).await,
            Action::Timeout => std::future::pending().await,
            Action::Transport => Err(failure()),
            Action::PartialUpload(_) => {
                Err(Error::local(ErrorKind::Validation, "mock_upload_prefix"))
            }
        }
    }
}
struct Active(MockTransport);
impl Drop for Active {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state() {
            state.active -= 1;
        }
    }
}
fn failure() -> Error {
    Error::new(ErrorKind::Transport, EffectOutcome::Unknown)
}

impl fmt::Debug for MockTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockTransport").finish_non_exhaustive()
    }
}
impl fmt::Debug for MockResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Step")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for RecordedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordedRequest")
            .field("method", &self.method)
            .field("body_bytes", &self.body.len())
            .field("upload_complete", &self.upload_complete)
            .finish_non_exhaustive()
    }
}
