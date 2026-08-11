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

use std::error::Error as StdError;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{json, Value};
use tracing::{debug, warn};

use crate::error::AiError;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const MAX_BODY_LOG: usize = 800;
/// Connect budget only (DNS/TLS/TCP).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Full request budget including model generation. Slow reasoning models + tools
/// regularly exceed 60s; that used to surface as a vague body-decode error.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(240);
/// Extra attempts after the first try for transient transport failures.
/// Keep modest: each attempt may run up to REQUEST_TIMEOUT (worst case ~3 × timeout + backoff).
const MAX_TRANSIENT_RETRIES: u32 = 2;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);

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
}

impl OpenRouterClient {
    pub fn new(api_key: impl Into<String>) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .user_agent("car-tracking-platform-ai/0.1")
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|e| AiError::Agent(format!("http client: {e}")))?;
        Ok(Self {
            http,
            api_key: api_key.into(),
        })
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

        let attempts = 1 + MAX_TRANSIENT_RETRIES;
        let mut last_err = None;

        for attempt in 1..=attempts {
            match self.chat_completion_once(&body).await {
                Ok(turn) => return Ok(turn),
                Err(err) => {
                    let retryable = err.transient;
                    let msg = err.message;
                    if retryable && attempt < attempts {
                        let delay = RETRY_BASE_DELAY.saturating_mul(attempt);
                        warn!(
                            attempt,
                            attempts,
                            delay_ms = delay.as_millis() as u64,
                            error = %msg,
                            "openrouter transient failure; retrying"
                        );
                        tokio::time::sleep(delay).await;
                        last_err = Some(msg);
                        continue;
                    }
                    return Err(AiError::Agent(msg));
                }
            }
        }

        Err(AiError::Agent(last_err.unwrap_or_else(|| {
            "openrouter request failed after retries".into()
        })))
    }

    async fn chat_completion_once(&self, body: &Value) -> Result<AssistantTurn, TransportErr> {
        let response = self
            .http
            .post(OPENROUTER_URL)
            .bearer_auth(&self.api_key)
            .header("Content-Type", "application/json")
            // OpenRouter optional ranking headers
            .header(
                "HTTP-Referer",
                "https://github.com/lfdominguez/my-car-tracking-platform",
            )
            .header("X-Title", "Car Tracking Platform")
            .json(body)
            .send()
            .await
            .map_err(|e| TransportErr::from_reqwest("openrouter request failed", e))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let content_length = response.content_length();

        let text = response
            .text()
            .await
            .map_err(|e| TransportErr::from_reqwest("openrouter read body", e))?;

        if text.trim().is_empty() {
            return Err(TransportErr {
                message: format!(
                    "openrouter empty body (HTTP {status}, content-type={content_type}, content-length={content_length:?})"
                ),
                // Empty body on success is unusual; treat as transient (proxy glitch).
                transient: status.is_success() || status.is_server_error(),
            });
        }

        parse_chat_response(status, &text).map_err(|e| TransportErr {
            message: e.to_string().replacen("openrouter/agent error: ", "", 1),
            // 429 / 5xx already folded into AiError strings; allow retry on rate limits.
            transient: status.as_u16() == 429
                || status.is_server_error()
                || status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::GATEWAY_TIMEOUT,
        })
    }
}

struct TransportErr {
    message: String,
    transient: bool,
}

impl TransportErr {
    fn from_reqwest(prefix: &str, e: reqwest::Error) -> Self {
        let transient = is_transient_reqwest(&e);
        Self {
            message: format!("{prefix}: {}", format_reqwest_error(&e)),
            transient,
        }
    }
}

fn is_transient_reqwest(e: &reqwest::Error) -> bool {
    e.is_timeout()
        || e.is_connect()
        || e.is_request()
        || e.is_body()
        || e.is_decode()
        || e.status().is_some_and(|s| s.as_u16() == 429 || s.is_server_error())
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

pub(crate) fn parse_chat_response(status: StatusCode, text: &str) -> Result<AssistantTurn, AiError> {
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
        if !status.is_success() || value.get("choices").and_then(|c| c.as_array()).map(|a| a.is_empty()).unwrap_or(true) {
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
        return Err(AiError::Agent(format!(
            "openrouter HTTP {status}: {msg}"
        )));
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
        let msg = extract_error_message(&value)
            .unwrap_or_else(|| "empty choices".into());
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
            if t.is_empty() {
                None
            } else {
                Some(s.clone())
            }
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
            if t.is_empty() {
                None
            } else {
                Some(out)
            }
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

    #[test]
    fn retry_and_timeout_constants_are_sane() {
        // Documented resilience knobs for slow OpenRouter body reads.
        assert!(REQUEST_TIMEOUT.as_secs() >= 180);
        assert!(MAX_TRANSIENT_RETRIES >= 2);
        assert_eq!(1 + MAX_TRANSIENT_RETRIES, 3);
    }

    #[test]
    fn non_json_body_mentions_snippet() {
        let err = parse_chat_response(StatusCode::OK, "<html>gateway timeout</html>").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("non-JSON"), "{msg}");
        assert!(msg.contains("gateway"), "{msg}");
    }
}
