//! Tolerant OpenRouter Chat Completions client.
//!
//! Rig 0.28's built-in OpenRouter provider deserializes responses through a strict
//! untagged `ApiResponse` enum. Real OpenRouter payloads often include nested
//! `error` objects, reasoning metadata, partial `usage`, or array `content`, which
//! yields:
//! `CompletionError: JsonError: data did not match any variant of untagged enum ApiResponse`
//! even on HTTP 200. This module parses `serde_json::Value` flexibly instead.
//!
//! Transport note: reqwest's Display for body failures is often just
//! `"error decoding response body"` while the real cause (timeout, reset, incomplete
//! chunk) lives in the source chain — we surface the full chain and retry transients.
//!
//! Timeouts are deliberately *not* one budget for the whole request. A slow
//! reasoning model can legitimately stream for many minutes, while a stalled
//! connection should fail fast; so there is a connect timeout, a bound on waiting
//! for response headers, and an idle timeout between body chunks. The overall cap
//! on a job lives with the job (see the server's job supervisor).

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::time::Duration;

use futures::StreamExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::error::AiError;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
/// Overrides the API base (e.g. `https://openrouter.ai/api/v1`) for an
/// OpenAI-compatible gateway, or for a local mock in tests.
const BASE_URL_ENV: &str = "OPENROUTER_BASE_URL";
const MAX_BODY_LOG: usize = 800;
/// Connect budget only (DNS/TLS/TCP).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Wait for response headers. Non-streaming requests only send headers once the
/// provider has started answering, so this is generous.
const RESPONSE_HEADERS_TIMEOUT: Duration = Duration::from_secs(180);
/// Longest silence tolerated between body chunks. OpenRouter sends keepalive
/// comments while a model thinks, so a quiet connection this long is dead.
const IDLE_CHUNK_TIMEOUT: Duration = Duration::from_secs(90);
/// Extra attempts after the first try for transient transport failures.
const MAX_TRANSIENT_RETRIES: u32 = 2;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);
/// Longest `Retry-After` worth waiting for inside a request. Beyond this the caller
/// is better served by a clear "rate limited" error than by a silent stall.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct AssistantTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub model: Option<String>,
    pub finish_reason: Option<String>,
}

#[derive(Clone)]
pub struct OpenRouterClient {
    http: reqwest::Client,
    api_key: String,
    endpoint: String,
}

/// The chat-completions endpoint, honouring [`BASE_URL_ENV`].
fn endpoint_from_env() -> String {
    match std::env::var(BASE_URL_ENV) {
        Ok(base) if !base.trim().is_empty() => {
            format!("{}/chat/completions", base.trim().trim_end_matches('/'))
        }
        _ => OPENROUTER_URL.to_string(),
    }
}

impl OpenRouterClient {
    pub fn new(api_key: impl Into<String>) -> Result<Self, AiError> {
        Self::with_endpoint(api_key, endpoint_from_env())
    }

    pub(crate) fn with_endpoint(
        api_key: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .user_agent("car-tracking-platform-ai/0.1")
            .connect_timeout(CONNECT_TIMEOUT)
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|e| AiError::Agent(format!("http client: {e}")))?;
        Ok(Self {
            http,
            api_key: api_key.into(),
            endpoint: endpoint.into(),
        })
    }

    fn request(&self, body: &Value, stream: bool) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header("Content-Type", "application/json")
            // OpenRouter optional ranking headers
            .header(
                "HTTP-Referer",
                "https://github.com/lfdominguez/my-car-tracking-platform",
            )
            .header("X-Title", "Car Tracking Platform");
        if stream {
            req = req.header("Accept", "text/event-stream");
        }
        req.json(body)
    }

    /// Send and wait for headers, bounded by [`RESPONSE_HEADERS_TIMEOUT`].
    async fn send(
        &self,
        body: &Value,
        stream: bool,
        what: &str,
    ) -> Result<reqwest::Response, TransportErr> {
        match tokio::time::timeout(RESPONSE_HEADERS_TIMEOUT, self.request(body, stream).send())
            .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(TransportErr::from_reqwest(what, e)),
            Err(_) => Err(TransportErr::timeout(format!(
                "{what}: no response headers within {}s",
                RESPONSE_HEADERS_TIMEOUT.as_secs()
            ))),
        }
    }

    /// OpenAI-compatible chat completion. `messages` and `tools` are raw JSON values.
    pub async fn chat_completion(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        max_tokens: u32,
    ) -> Result<AssistantTurn, AiError> {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            // Explicit non-stream: we always read a full JSON body.
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = json!("auto");
        }

        with_retries(
            "openrouter request",
            || self.chat_completion_once(&body),
            || true,
        )
        .await
    }

    async fn chat_completion_once(&self, body: &Value) -> Result<AssistantTurn, TransportErr> {
        let response = self.send(body, false, "openrouter request failed").await?;

        let status = response.status();
        let retry_after = retry_after(response.headers());
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let content_length = response.content_length();

        let text = read_body(response, "openrouter read body").await?;

        if text.trim().is_empty() {
            return Err(TransportErr {
                message: format!(
                    "openrouter empty body (HTTP {status}, content-type={content_type}, content-length={content_length:?})"
                ),
                // Empty body on success is unusual; treat as transient (proxy glitch).
                transient: status.is_success() || status.is_server_error(),
                kind: ErrKind::for_status(status, ""),
                retry_after,
            });
        }

        parse_chat_response(status, &text).map_err(|e| {
            let message = e.to_string().replacen("openrouter/agent error: ", "", 1);
            TransportErr::http(status, message, retry_after)
        })
    }
}

/// Run `attempt` with the retry policy shared by both request shapes.
///
/// `may_retry` is consulted after a failure: the streaming path uses it to refuse a
/// replay once anything reached the user.
async fn with_retries<T, F, Fut>(
    what: &str,
    mut attempt: F,
    mut may_retry: impl FnMut() -> bool,
) -> Result<T, AiError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, TransportErr>>,
{
    let attempts = 1 + MAX_TRANSIENT_RETRIES;
    let mut n = 1;
    loop {
        let err = match attempt().await {
            Ok(value) => return Ok(value),
            Err(err) => err,
        };
        if !(err.transient && n < attempts && may_retry()) {
            return Err(err.into_ai_error());
        }
        let delay = match err.retry_after {
            // The server said when; waiting less just burns an attempt.
            Some(after) if after > MAX_RETRY_AFTER => return Err(err.into_ai_error()),
            Some(after) => after,
            None => RETRY_BASE_DELAY.saturating_mul(n),
        };
        warn!(
            attempt = n,
            attempts,
            delay_ms = delay.as_millis() as u64,
            error = %err.message,
            "{what}: transient failure; retrying"
        );
        tokio::time::sleep(delay).await;
        n += 1;
    }
}

/// Read a whole body, failing if the connection goes quiet for too long.
async fn read_body(response: reqwest::Response, what: &str) -> Result<String, TransportErr> {
    let mut stream = response.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = next_chunk(&mut stream, what).await? {
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The next body chunk, bounded by [`IDLE_CHUNK_TIMEOUT`].
async fn next_chunk<S, B>(stream: &mut S, what: &str) -> Result<Option<B>, TransportErr>
where
    S: futures::Stream<Item = Result<B, reqwest::Error>> + Unpin,
{
    match tokio::time::timeout(IDLE_CHUNK_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(chunk))) => Ok(Some(chunk)),
        Ok(Some(Err(e))) => Err(TransportErr::from_reqwest(what, e)),
        Ok(None) => Ok(None),
        Err(_) => Err(TransportErr::timeout(format!(
            "{what}: no data for {}s",
            IDLE_CHUNK_TIMEOUT.as_secs()
        ))),
    }
}

/// Seconds from a `Retry-After` header. HTTP-date values are ignored (OpenRouter
/// sends seconds); the caller then falls back to its own backoff.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    let secs: f64 = raw.trim().parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// A fragment emitted while an assistant turn streams in.
#[derive(Debug, Clone)]
pub enum StreamDelta {
    /// Assistant text fragment, in order.
    Text(String),
    /// A tool call's name became known (arguments may still be streaming).
    ToolCallNamed(String),
}

impl OpenRouterClient {
    /// Streaming counterpart of [`chat_completion`]. `on_delta` is called for each
    /// fragment as it arrives; the fully accumulated turn is returned at the end.
    ///
    /// Retry policy is deliberately narrower than the non-streaming path: once any
    /// fragment has been handed to `on_delta` the caller has already shown it to a
    /// user, so replaying the turn would duplicate visible text. Transient failures
    /// are therefore only retried while nothing has been emitted.
    pub async fn chat_completion_stream<F>(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        max_tokens: u32,
        mut on_delta: F,
    ) -> Result<AssistantTurn, AiError>
    where
        F: FnMut(StreamDelta) + Send,
    {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": max_tokens,
            "stream": true,
        });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
            body["tool_choice"] = json!("auto");
        }

        let emitted = std::sync::atomic::AtomicBool::new(false);
        let on_delta = std::sync::Mutex::new(&mut on_delta);
        with_retries(
            "openrouter stream",
            || async {
                let mut sink = |d: StreamDelta| {
                    emitted.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(mut f) = on_delta.lock() {
                        (*f)(d);
                    }
                };
                self.chat_completion_stream_once(&body, &mut sink).await
            },
            // Anything already on screen makes a replay worse than an error.
            || !emitted.load(std::sync::atomic::Ordering::Relaxed),
        )
        .await
    }

    async fn chat_completion_stream_once<F>(
        &self,
        body: &Value,
        on_delta: &mut F,
    ) -> Result<AssistantTurn, TransportErr>
    where
        F: FnMut(StreamDelta),
    {
        let response = self
            .send(body, true, "openrouter stream request failed")
            .await?;

        let status = response.status();
        if !status.is_success() {
            let retry_after = retry_after(response.headers());
            // Errors come back as a normal JSON body even when streaming was requested.
            let text = read_body(response, "openrouter read error body").await?;
            let message = parse_chat_response(status, &text)
                .err()
                .map(|e| e.to_string().replacen("openrouter/agent error: ", "", 1))
                .unwrap_or_else(|| format!("openrouter HTTP {status}"));
            return Err(TransportErr::http(status, message, retry_after));
        }

        let mut acc = StreamAccumulator::default();
        let mut lines = SseLineBuffer::default();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = next_chunk(&mut stream, "openrouter stream chunk").await? {
            for line in lines.push(&chunk) {
                match acc.push_line(&line) {
                    Ok(deltas) => deltas.into_iter().for_each(&mut *on_delta),
                    Err(msg) => {
                        return Err(TransportErr::fatal(msg));
                    }
                }
                if acc.done {
                    break;
                }
            }
            if acc.done {
                break;
            }
        }
        if !acc.done
            && let Some(line) = lines.take_rest()
            && let Ok(deltas) = acc.push_line(&line)
        {
            deltas.into_iter().for_each(&mut *on_delta);
        }

        acc.finish().map_err(|e| match e {
            // A mid-stream error payload carries the provider's own classification.
            AiError::Agent(msg) => {
                let kind = ErrKind::from_message(&msg);
                TransportErr {
                    message: msg.replacen("openrouter/agent error: ", "", 1),
                    transient: false,
                    kind,
                    retry_after: None,
                }
            }
            other => TransportErr::fatal(other.to_string()),
        })
    }
}

/// Splits a byte stream into SSE lines, decoding only **complete** lines.
///
/// Network chunks cut wherever they like, including through the middle of a
/// multi-byte UTF-8 character. Decoding each chunk on its own turned both halves of
/// such a character into U+FFFD, corrupting e.g. every `ñ` or `é` that happened to
/// straddle a chunk boundary. A newline byte never occurs inside a multi-byte
/// sequence, so splitting the raw bytes on `\n` first is always safe.
#[derive(Default)]
pub(crate) struct SseLineBuffer {
    pending: Vec<u8>,
}

impl SseLineBuffer {
    /// Append a chunk and return every line it completed, without terminators.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(pos) = self.pending[start..].iter().position(|b| *b == b'\n') {
            let end = start + pos;
            let mut line = &self.pending[start..end];
            if let Some(stripped) = line.strip_suffix(b"\r") {
                line = stripped;
            }
            out.push(String::from_utf8_lossy(line).into_owned());
            start = end + 1;
        }
        self.pending.drain(..start);
        out
    }

    /// The unterminated last line, if the stream ended without a final newline.
    pub(crate) fn take_rest(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        let rest = rest.strip_suffix(b"\r").unwrap_or(&rest);
        (!rest.is_empty()).then(|| String::from_utf8_lossy(rest).into_owned())
    }
}

/// Reassembles an OpenRouter SSE stream into a single [`AssistantTurn`].
///
/// Tool calls arrive fragmented: `id` and `function.name` usually land in the first
/// chunk for a given `index`, while `function.arguments` accumulates across many
/// later chunks. Keying by `index` (not by position) is what keeps parallel tool
/// calls from being spliced into each other.
#[derive(Default)]
pub(crate) struct StreamAccumulator {
    content: String,
    tool_calls: BTreeMap<u64, PartialToolCall>,
    model: Option<String>,
    finish_reason: Option<String>,
    error: Option<String>,
    pub(crate) done: bool,
}

#[derive(Default, Clone)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    announced: bool,
}

impl StreamAccumulator {
    /// Feed one raw SSE line. Returns any fragments that should reach the caller.
    pub(crate) fn push_line(&mut self, line: &str) -> Result<Vec<StreamDelta>, String> {
        let line = line.trim();
        // `: OPENROUTER PROCESSING` keepalives and blank frame separators.
        if line.is_empty() || line.starts_with(':') {
            return Ok(Vec::new());
        }
        let Some(data) = line.strip_prefix("data:") else {
            // Unknown SSE field (event:, id:, retry:) — nothing to accumulate.
            return Ok(Vec::new());
        };
        let data = data.trim();
        if data == "[DONE]" {
            self.done = true;
            return Ok(Vec::new());
        }

        let value: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, chunk = %truncate(data, 200), "skipping unparseable stream chunk");
                return Ok(Vec::new());
            }
        };

        if let Some(msg) = extract_error_message(&value) {
            self.error = Some(msg);
            self.done = true;
            return Ok(Vec::new());
        }

        if self.model.is_none() {
            self.model = value
                .get("model")
                .and_then(|m| m.as_str())
                .map(String::from);
        }

        let Some(choice) = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        else {
            return Ok(Vec::new());
        };

        if let Some(reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            self.finish_reason = Some(reason.to_string());
        }

        // Non-streaming providers occasionally answer a stream request with a full
        // `message` instead of `delta`; treat both alike.
        let delta = choice.get("delta").or_else(|| choice.get("message"));
        let Some(delta) = delta else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();

        if let Some(text) = extract_text_content(delta.get("content")) {
            self.content.push_str(&text);
            out.push(StreamDelta::Text(text));
        }

        if let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
            for (pos, call) in calls.iter().enumerate() {
                let index = call
                    .get("index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(pos as u64);
                let slot = self.tool_calls.entry(index).or_default();

                if let Some(id) = call.get("id").and_then(|v| v.as_str())
                    && !id.is_empty()
                {
                    slot.id = Some(id.to_string());
                }
                let func = call.get("function").unwrap_or(call);
                if let Some(name) = func.get("name").and_then(|v| v.as_str())
                    && !name.is_empty()
                {
                    // Providers may stream a name in fragments too.
                    match slot.name.as_mut() {
                        Some(existing) => existing.push_str(name),
                        None => slot.name = Some(name.to_string()),
                    }
                }
                match func.get("arguments") {
                    Some(Value::String(s)) => slot.arguments.push_str(s),
                    Some(Value::Null) | None => {}
                    Some(other) => slot.arguments.push_str(&other.to_string()),
                }

                if !slot.announced
                    && let Some(name) = slot.name.clone()
                {
                    slot.announced = true;
                    out.push(StreamDelta::ToolCallNamed(name));
                }
            }
        }

        Ok(out)
    }

    pub(crate) fn finish(self) -> Result<AssistantTurn, AiError> {
        if let Some(msg) = self.error {
            return Err(AiError::Agent(format!("openrouter stream error: {msg}")));
        }

        let tool_calls: Vec<ToolCall> = self
            .tool_calls
            .into_iter()
            .filter_map(|(index, partial)| {
                let name = partial.name?;
                let arguments = if partial.arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    partial.arguments
                };
                Some(ToolCall {
                    id: partial.id.unwrap_or_else(|| format!("call_{index}")),
                    name,
                    arguments,
                })
            })
            .collect();

        let content = {
            let trimmed = self.content.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(self.content.clone())
            }
        };

        if content.is_none() && tool_calls.is_empty() {
            return Err(AiError::Agent(
                "openrouter stream ended with no content or tool calls".into(),
            ));
        }

        debug!(
            model = self.model.as_deref().unwrap_or("?"),
            finish = self.finish_reason.as_deref().unwrap_or("?"),
            tools = tool_calls.len(),
            has_content = content.is_some(),
            "openrouter streamed assistant turn"
        );

        Ok(AssistantTurn {
            content,
            tool_calls,
            model: self.model,
            finish_reason: self.finish_reason,
        })
    }
}

/// What a failed request means for the user, beyond "it failed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrKind {
    Other,
    InvalidApiKey,
    InsufficientCredits,
    ModelNotFound,
    RateLimited,
    Timeout,
}

impl ErrKind {
    fn for_status(status: StatusCode, message: &str) -> Self {
        match status.as_u16() {
            401 => Self::InvalidApiKey,
            402 => Self::InsufficientCredits,
            404 => Self::ModelNotFound,
            429 => Self::RateLimited,
            408 | 504 => Self::Timeout,
            _ => Self::from_message(message),
        }
    }

    /// Classify from the provider's text when the HTTP status says nothing (a 400
    /// for a bad model id, or an error payload in the middle of a 200 stream).
    fn from_message(message: &str) -> Self {
        let m = message.to_ascii_lowercase();
        if m.contains("(code 401)")
            || m.contains("no auth credentials")
            || m.contains("invalid api key")
        {
            Self::InvalidApiKey
        } else if m.contains("(code 402)") || m.contains("insufficient credits") {
            Self::InsufficientCredits
        } else if m.contains("not a valid model")
            || m.contains("model not found")
            || m.contains("no endpoints found")
        {
            Self::ModelNotFound
        } else if m.contains("(code 429)") || m.contains("rate limit") {
            Self::RateLimited
        } else {
            Self::Other
        }
    }
}

struct TransportErr {
    message: String,
    transient: bool,
    kind: ErrKind,
    /// Server-requested wait before retrying (HTTP `Retry-After`).
    retry_after: Option<Duration>,
}

impl TransportErr {
    fn from_reqwest(prefix: &str, e: reqwest::Error) -> Self {
        let transient = is_transient_reqwest(&e);
        let kind = if e.is_timeout() {
            ErrKind::Timeout
        } else {
            ErrKind::Other
        };
        Self {
            message: format!("{prefix}: {}", format_reqwest_error(&e)),
            transient,
            kind,
            retry_after: None,
        }
    }

    fn timeout(message: String) -> Self {
        Self {
            message,
            transient: true,
            kind: ErrKind::Timeout,
            retry_after: None,
        }
    }

    fn fatal(message: String) -> Self {
        Self {
            message,
            transient: false,
            kind: ErrKind::Other,
            retry_after: None,
        }
    }

    /// An HTTP error response. Rate limits, timeouts and 5xx are worth another try;
    /// a bad key, missing credits or an unknown model will fail the same way again.
    fn http(status: StatusCode, message: String, retry_after: Option<Duration>) -> Self {
        Self {
            transient: status.as_u16() == 429
                || status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::GATEWAY_TIMEOUT,
            kind: ErrKind::for_status(status, &message),
            message,
            retry_after,
        }
    }

    fn into_ai_error(self) -> AiError {
        match self.kind {
            ErrKind::InvalidApiKey => AiError::InvalidApiKey(self.message),
            ErrKind::InsufficientCredits => AiError::InsufficientCredits(self.message),
            ErrKind::ModelNotFound => AiError::ModelNotFound(self.message),
            ErrKind::RateLimited => AiError::RateLimited(self.message),
            ErrKind::Timeout => AiError::Timeout(self.message),
            ErrKind::Other => AiError::Agent(self.message),
        }
    }
}

fn is_transient_reqwest(e: &reqwest::Error) -> bool {
    e.is_timeout()
        || e.is_connect()
        || e.is_request()
        || e.is_body()
        || e.is_decode()
        || e.status()
            .is_some_and(|s| s.as_u16() == 429 || s.is_server_error())
}

/// reqwest Display often stops at "error decoding response body"; chain sources.
pub(crate) fn format_reqwest_error(e: &reqwest::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(e.to_string());
    if e.is_timeout() {
        parts.push("timed out".into());
    }
    if e.is_connect() {
        parts.push("connect".into());
    }
    if e.is_decode() {
        parts.push("decode/body-read".into());
    }
    let mut src = e.source();
    while let Some(cause) = src {
        let s = cause.to_string();
        if parts.last().map(|p| p != &s).unwrap_or(true) {
            parts.push(s);
        }
        src = cause.source();
    }
    parts.join(" | ")
}

pub(crate) fn parse_chat_response(
    status: StatusCode,
    text: &str,
) -> Result<AssistantTurn, AiError> {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return Err(AiError::Agent(format!(
                "openrouter returned non-JSON (HTTP {status}): {e}; body={}",
                truncate(text, MAX_BODY_LOG)
            )));
        }
    };

    if let Some(msg) = extract_error_message(&value) {
        // Prefer explicit error payloads even on 200
        if !status.is_success()
            || value
                .get("choices")
                .and_then(|c| c.as_array())
                .map(|a| a.is_empty())
                .unwrap_or(true)
        {
            return Err(AiError::Agent(format!(
                "openrouter error (HTTP {status}): {msg}"
            )));
        }
        // Some providers embed error alongside choices; still try choices first below.
        debug!(%msg, %status, "openrouter response contains error field");
    }

    if !status.is_success() {
        let msg = extract_error_message(&value)
            .unwrap_or_else(|| truncate(text, MAX_BODY_LOG).to_string());
        return Err(AiError::Agent(format!("openrouter HTTP {status}: {msg}")));
    }

    let choices = value
        .get("choices")
        .and_then(|c| c.as_array())
        .ok_or_else(|| {
            AiError::Agent(format!(
                "openrouter response missing choices; body={}",
                truncate(text, MAX_BODY_LOG)
            ))
        })?;

    if choices.is_empty() {
        let msg = extract_error_message(&value).unwrap_or_else(|| "empty choices".into());
        return Err(AiError::Agent(format!(
            "openrouter returned no choices: {msg}"
        )));
    }

    let choice0 = &choices[0];
    let message = choice0
        .get("message")
        .ok_or_else(|| AiError::Agent("openrouter choice missing message".into()))?;

    let turn = AssistantTurn {
        content: extract_text_content(message.get("content")),
        tool_calls: extract_tool_calls(message),
        model: value
            .get("model")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string()),
        finish_reason: choice0
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(|s| s.to_string()),
    };

    debug!(
        model = turn.model.as_deref().unwrap_or("?"),
        finish = turn.finish_reason.as_deref().unwrap_or("?"),
        tools = turn.tool_calls.len(),
        has_content = turn.content.is_some(),
        "openrouter assistant turn"
    );

    if turn.content.is_none() && turn.tool_calls.is_empty() {
        // Check choice-level error (OpenRouter provider failure shape)
        if let Some(msg) = choice0
            .get("error")
            .and_then(extract_error_message_from_value)
            .or_else(|| extract_error_message(&value))
        {
            return Err(AiError::Agent(format!("openrouter provider error: {msg}")));
        }
        warn!(
            body = %truncate(text, MAX_BODY_LOG),
            "openrouter assistant turn empty"
        );
        return Err(AiError::Agent(
            "openrouter returned empty assistant message (no content or tool calls)".into(),
        ));
    }

    Ok(turn)
}

fn extract_error_message(value: &Value) -> Option<String> {
    if let Some(msg) = extract_error_message_from_value(value.get("error")?) {
        return Some(msg);
    }
    // Flat { "message": "..." } (Rig's expected error shape)
    value
        .get("message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

fn extract_error_message_from_value(err: &Value) -> Option<String> {
    match err {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => {
            let message = map
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string();
            let code = map
                .get("code")
                .map(|c| match c {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    _ => String::new(),
                })
                .filter(|s| !s.is_empty());
            Some(match code {
                Some(c) => format!("{message} (code {c})"),
                None => message,
            })
        }
        _ => None,
    }
}

fn extract_text_content(content: Option<&Value>) -> Option<String> {
    let content = content?;
    match content {
        Value::Null => None,
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() { None } else { Some(s.clone()) }
        }
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    out.push_str(text);
                } else if let Some(s) = part.as_str() {
                    out.push_str(s);
                }
            }
            let t = out.trim();
            if t.is_empty() { None } else { Some(out) }
        }
        Value::Object(map) => {
            // Rare: single content object
            map.get("text")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    }
}

fn extract_tool_calls(message: &Value) -> Vec<ToolCall> {
    let Some(arr) = message.get("tool_calls").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for (i, tc) in arr.iter().enumerate() {
        let id = tc
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("call_{i}"));
        let func = tc.get("function").unwrap_or(tc);
        let Some(name) = func
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        else {
            warn!(?tc, "skipping tool_call without name");
            continue;
        };
        let arguments = match func.get("arguments") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => "{}".into(),
        };
        out.push(ToolCall {
            id,
            name,
            arguments,
        });
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    let mut t: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        t.push('…');
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn parses_nested_error_on_http_error() {
        let body = r#"{"error":{"message":"Insufficient credits","code":402}}"#;
        let err = parse_chat_response(StatusCode::PAYMENT_REQUIRED, body).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Insufficient credits"), "{msg}");
        assert!(msg.contains("402"), "{msg}");
    }

    #[test]
    fn parses_tool_call_turn() {
        let body = r#"{
          "id": "gen-1",
          "provider": "Anthropic",
          "model": "anthropic/claude-3.7-sonnet",
          "object": "chat.completion",
          "created": 1710000000,
          "choices": [{
            "index": 0,
            "finish_reason": "tool_calls",
            "native_finish_reason": "tool_use",
            "message": {
              "role": "assistant",
              "content": null,
              "reasoning": "thinking…",
              "reasoning_details": [{"type":"reasoning.text","text":"plan"}],
              "tool_calls": [{
                "id": "call_abc",
                "type": "function",
                "function": {
                  "name": "get_trip_overview",
                  "arguments": "{}"
                }
              }]
            }
          }],
          "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15,
            "cost": 0.0001,
            "completion_tokens_details": { "reasoning_tokens": 3 }
          }
        }"#;
        let turn = parse_chat_response(StatusCode::OK, body).unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "get_trip_overview");
        assert!(turn.content.is_none());
    }

    #[test]
    fn parses_array_content() {
        let body = r#"{
          "id": "gen-2",
          "model": "google/gemini-2.5-flash",
          "object": "chat.completion",
          "created": 1,
          "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "message": {
              "role": "assistant",
              "content": [
                {"type":"text","text":"{\"summary\":\"ok\"}"}
              ]
            }
          }],
          "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        }"#;
        let turn = parse_chat_response(StatusCode::OK, body).unwrap();
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.content.as_deref(), Some("{\"summary\":\"ok\"}"));
    }

    #[test]
    fn parses_partial_usage_and_extra_fields() {
        // Missing total_tokens would break Rig's strict Usage struct.
        let body = r#"{
          "id": "gen-3",
          "model": "x",
          "object": "chat.completion",
          "created": 1,
          "choices": [{
            "finish_reason": "stop",
            "message": { "role": "assistant", "content": "hello" }
          }],
          "usage": { "prompt_tokens": 1, "completion_tokens": 2 }
        }"#;
        let turn = parse_chat_response(StatusCode::OK, body).unwrap();
        assert_eq!(turn.content.as_deref(), Some("hello"));
    }

    #[test]
    fn empty_choices_with_error() {
        let body = r#"{"id":"x","choices":[],"error":{"message":"Provider down","code":502}}"#;
        let err = parse_chat_response(StatusCode::OK, body).unwrap_err();
        assert!(err.to_string().contains("Provider down"), "{err}");
    }

    // The point of this test is to pin constants, so constant assertions are
    // exactly what is wanted here.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn retry_and_timeout_constants_are_sane() {
        // Slow reasoning models stream for minutes; only silence should time out.
        assert!(IDLE_CHUNK_TIMEOUT.as_secs() >= 60);
        assert!(RESPONSE_HEADERS_TIMEOUT >= IDLE_CHUNK_TIMEOUT);
        assert!(MAX_TRANSIENT_RETRIES >= 2);
        assert_eq!(1 + MAX_TRANSIENT_RETRIES, 3);
    }

    fn feed(acc: &mut StreamAccumulator, lines: &[&str]) -> Vec<StreamDelta> {
        let mut out = Vec::new();
        for line in lines {
            out.extend(acc.push_line(line).expect("line accepted"));
        }
        out
    }

    #[test]
    fn stream_accumulates_text_fragments_in_order() {
        let mut acc = StreamAccumulator::default();
        let deltas = feed(
            &mut acc,
            &[
                r#"data: {"model":"x","choices":[{"delta":{"content":"Your least "}}]}"#,
                r#"data: {"choices":[{"delta":{"content":"efficient trip"}}]}"#,
                r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
                "data: [DONE]",
            ],
        );
        assert_eq!(deltas.len(), 2);
        assert!(acc.done);
        let turn = acc.finish().unwrap();
        assert_eq!(turn.content.as_deref(), Some("Your least efficient trip"));
        assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
        assert_eq!(turn.model.as_deref(), Some("x"));
    }

    #[test]
    fn stream_skips_keepalives_and_blank_frames() {
        let mut acc = StreamAccumulator::default();
        let deltas = feed(
            &mut acc,
            &[
                ": OPENROUTER PROCESSING",
                "",
                "event: message",
                r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#,
            ],
        );
        assert_eq!(deltas.len(), 1);
        assert_eq!(acc.finish().unwrap().content.as_deref(), Some("hi"));
    }

    #[test]
    fn stream_reassembles_fragmented_tool_call_arguments() {
        let mut acc = StreamAccumulator::default();
        let deltas = feed(
            &mut acc,
            &[
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"list_trips","arguments":""}}]}}]}"#,
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"car_id\""}}]}}]}"#,
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"abc\"}"}}]}}]}"#,
                r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            ],
        );
        // The name is announced exactly once, as soon as it is known.
        assert_eq!(
            deltas
                .iter()
                .filter(|d| matches!(d, StreamDelta::ToolCallNamed(_)))
                .count(),
            1
        );
        let turn = acc.finish().unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "list_trips");
        assert_eq!(turn.tool_calls[0].id, "call_a");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"car_id":"abc"}"#);
        assert_eq!(turn.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn stream_keys_parallel_tool_calls_by_index() {
        // Fragments for two calls interleave; keying by position would splice them.
        let mut acc = StreamAccumulator::default();
        feed(
            &mut acc,
            &[
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"get_trip","arguments":"{\"trip_id\":"}}]}}]}"#,
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"get_car","arguments":"{\"car_id\":"}}]}}]}"#,
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"\"c2\"}"}}]}}]}"#,
                r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"t1\"}"}}]}}]}"#,
            ],
        );
        let turn = acc.finish().unwrap();
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].name, "get_trip");
        assert_eq!(turn.tool_calls[0].arguments, r#"{"trip_id":"t1"}"#);
        assert_eq!(turn.tool_calls[1].name, "get_car");
        assert_eq!(turn.tool_calls[1].arguments, r#"{"car_id":"c2"}"#);
    }

    #[test]
    fn stream_missing_index_falls_back_to_position() {
        let mut acc = StreamAccumulator::default();
        feed(
            &mut acc,
            &[
                r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"a","function":{"name":"list_cars","arguments":"{}"}}]}}]}"#,
            ],
        );
        let turn = acc.finish().unwrap();
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].name, "list_cars");
    }

    #[test]
    fn stream_surfaces_mid_stream_error_payload() {
        let mut acc = StreamAccumulator::default();
        feed(
            &mut acc,
            &[r#"data: {"error":{"message":"Rate limited","code":429}}"#],
        );
        assert!(acc.done);
        let err = acc.finish().unwrap_err().to_string();
        assert!(err.contains("Rate limited"), "{err}");
        assert!(err.contains("429"), "{err}");
    }

    #[test]
    fn stream_ignores_unparseable_chunks() {
        let mut acc = StreamAccumulator::default();
        let deltas = feed(
            &mut acc,
            &[
                "data: {not json",
                r#"data: {"choices":[{"delta":{"content":"ok"}}]}"#,
            ],
        );
        assert_eq!(deltas.len(), 1);
        assert_eq!(acc.finish().unwrap().content.as_deref(), Some("ok"));
    }

    #[test]
    fn stream_with_nothing_at_all_is_an_error() {
        let acc = StreamAccumulator::default();
        assert!(acc.finish().is_err());
    }

    #[test]
    fn stream_accepts_message_shaped_chunks() {
        // Some providers answer a stream request with a full `message` object.
        let mut acc = StreamAccumulator::default();
        feed(
            &mut acc,
            &[
                r#"data: {"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#,
            ],
        );
        assert_eq!(acc.finish().unwrap().content.as_deref(), Some("done"));
    }

    #[test]
    fn sse_lines_survive_a_multibyte_char_split_across_chunks() {
        let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"año\"}}]}\n";
        let bytes = frame.as_bytes();
        // Cut inside the two-byte `ñ` (0xC3 0xB1).
        let cut = frame.find('ñ').unwrap() + 1;
        let mut buf = SseLineBuffer::default();
        assert!(buf.push(&bytes[..cut]).is_empty());
        let lines = buf.push(&bytes[cut..]);
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].contains('\u{FFFD}'), "{}", lines[0]);

        let mut acc = StreamAccumulator::default();
        acc.push_line(&lines[0]).unwrap();
        assert_eq!(acc.finish().unwrap().content.as_deref(), Some("año"));
    }

    #[test]
    fn sse_lines_split_byte_by_byte_and_strip_crlf() {
        let text = "data: 1\r\n: keepalive\n\ndata: é\n";
        let mut buf = SseLineBuffer::default();
        let mut lines = Vec::new();
        for b in text.as_bytes() {
            lines.extend(buf.push(std::slice::from_ref(b)));
        }
        assert_eq!(lines, vec!["data: 1", ": keepalive", "", "data: é"]);
    }

    #[test]
    fn sse_keeps_an_unterminated_tail_for_the_next_chunk() {
        let mut buf = SseLineBuffer::default();
        assert_eq!(buf.push(b"data: a\ndata: b"), vec!["data: a"]);
        assert_eq!(buf.push(b"c\n"), vec!["data: bc"]);
        assert_eq!(buf.push(b"data: [DONE]"), Vec::<String>::new());
        assert_eq!(buf.take_rest().as_deref(), Some("data: [DONE]"));
        assert_eq!(buf.take_rest(), None);
    }

    use crate::test_http::{Scripted, serve};

    const OK_BODY: &str = r#"{"model":"m","choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"hi"}}]}"#;

    async fn complete(endpoint: &str) -> Result<AssistantTurn, AiError> {
        let client = OpenRouterClient::with_endpoint("sk-test", endpoint).unwrap();
        client
            .chat_completion("m", &[json!({"role":"user","content":"q"})], &[], 64)
            .await
    }

    #[tokio::test]
    async fn a_429_waits_for_retry_after_then_succeeds() {
        let mock = serve(vec![
            Scripted::json(429, r#"{"error":{"message":"slow down","code":429}}"#)
                .header("Retry-After", "1"),
            Scripted::json(200, OK_BODY),
        ])
        .await;
        let started = std::time::Instant::now();
        let turn = complete(&mock.endpoint).await.expect("retried");
        assert_eq!(turn.content.as_deref(), Some("hi"));
        assert_eq!(mock.hits(), 2);
        assert!(
            started.elapsed() >= Duration::from_millis(950),
            "did not honour Retry-After"
        );
    }

    #[tokio::test]
    async fn a_long_retry_after_fails_fast_as_rate_limited() {
        let mock = serve(vec![
            Scripted::json(429, r#"{"error":{"message":"slow down","code":429}}"#)
                .header("Retry-After", "600"),
        ])
        .await;
        let err = complete(&mock.endpoint).await.unwrap_err();
        assert!(matches!(err, AiError::RateLimited(_)), "{err:?}");
        assert_eq!(mock.hits(), 1);
    }

    #[tokio::test]
    async fn a_bad_key_is_typed_and_never_retried() {
        let mock = serve(vec![Scripted::json(
            401,
            r#"{"error":{"message":"No auth credentials found","code":401}}"#,
        )])
        .await;
        let err = complete(&mock.endpoint).await.unwrap_err();
        assert!(matches!(err, AiError::InvalidApiKey(_)), "{err:?}");
        let sent = mock.bodies().await;
        assert_eq!(sent[0]["model"], "m");
        assert_eq!(sent[0]["stream"], false);
        assert!(
            crate::user_facing_error(&err.to_string())
                .unwrap()
                .contains("API key")
        );
        assert_eq!(mock.hits(), 1);
    }

    #[tokio::test]
    async fn missing_credits_and_unknown_models_are_typed() {
        let mock = serve(vec![Scripted::json(
            402,
            r#"{"error":{"message":"Insufficient credits","code":402}}"#,
        )])
        .await;
        let err = complete(&mock.endpoint).await.unwrap_err();
        assert!(matches!(err, AiError::InsufficientCredits(_)), "{err:?}");

        let mock = serve(vec![Scripted::json(
            400,
            r#"{"error":{"message":"foo/bar is not a valid model ID","code":400}}"#,
        )])
        .await;
        let err = complete(&mock.endpoint).await.unwrap_err();
        assert!(matches!(err, AiError::ModelNotFound(_)), "{err:?}");
        assert_eq!(mock.hits(), 1);
    }

    #[tokio::test]
    async fn a_5xx_is_retried() {
        let mock = serve(vec![
            Scripted::json(502, r#"{"error":{"message":"bad gateway","code":502}}"#)
                .header("Retry-After", "0"),
            Scripted::json(200, OK_BODY),
        ])
        .await;
        complete(&mock.endpoint).await.expect("retried");
        assert_eq!(mock.hits(), 2);
    }

    #[tokio::test]
    async fn a_mid_stream_error_payload_is_typed_and_not_replayed() {
        let mock = serve(vec![Scripted::sse(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"par\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"Insufficient credits\",\"code\":402}}\n\n",
        ])])
        .await;
        let client = OpenRouterClient::with_endpoint("sk-test", mock.endpoint.clone()).unwrap();
        let mut seen = String::new();
        let err = client
            .chat_completion_stream("m", &[json!({"role":"user","content":"q"})], &[], 64, |d| {
                if let StreamDelta::Text(t) = d {
                    seen.push_str(&t);
                }
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AiError::InsufficientCredits(_)), "{err:?}");
        assert_eq!(seen, "par");
        assert_eq!(mock.hits(), 1);
    }

    #[tokio::test]
    async fn a_stream_failure_before_any_output_is_retried() {
        let mock = serve(vec![
            Scripted::json(503, r#"{"error":{"message":"overloaded","code":503}}"#)
                .header("Retry-After", "0"),
            Scripted::sse(&[
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            ]),
        ])
        .await;
        let client = OpenRouterClient::with_endpoint("sk-test", mock.endpoint.clone()).unwrap();
        let turn = client
            .chat_completion_stream(
                "m",
                &[json!({"role":"user","content":"q"})],
                &[],
                64,
                |_| {},
            )
            .await
            .expect("retried");
        assert_eq!(turn.content.as_deref(), Some("ok"));
        assert_eq!(mock.hits(), 2);
    }

    #[test]
    fn retry_after_parses_seconds_only() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs(3)));
        h.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn non_json_body_mentions_snippet() {
        let err = parse_chat_response(StatusCode::OK, "<html>gateway timeout</html>").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("non-JSON"), "{msg}");
        assert!(msg.contains("gateway"), "{msg}");
    }
}
