//! Provider-agnostic streaming chat loop over a caller-supplied toolbox.
//!
//! Where [`crate::analyze_trip`] is a one-shot agent bound to a pre-built
//! [`crate::TripAnalysisContext`] and terminated by a submit tool, this module runs an
//! open-ended conversation: the caller owns the tools, the transcript and the
//! persistence. The `ai` crate never touches a database and knows nothing about cars.

use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::error::AiError;
use crate::openrouter::{OpenRouterClient, StreamDelta, ToolCall};

/// Round trips to the model in a single user turn. Deliberately half the analysis
/// budget: a chat answer that needs 24 tool round trips is a runaway, not an answer.
pub const MAX_CHAT_TURNS: usize = 12;
const DEFAULT_MAX_TOKENS: u32 = 4096;
/// Cap on a single tool result fed back into the transcript. Long JSON payloads
/// crowd out conversation history and cost tokens on every later turn.
const MAX_TOOL_RESULT_CHARS: usize = 24_000;

/// The read-only data tools a chat turn may call.
///
/// Implemented by the caller so the tool surface — and its authorization — stays
/// outside this crate.
#[async_trait]
pub trait ChatToolbox: Send + Sync {
    /// OpenAI-shaped `tools` array advertised to the model.
    fn definitions(&self) -> Vec<Value>;

    /// Run one tool. `arguments` is the raw JSON string from the model, which may be
    /// malformed; returning `Err` feeds the message back so the model can correct it.
    async fn dispatch(&self, name: &str, arguments: &str) -> Result<String, String>;
}

/// Something that happened while a turn was generating.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Assistant text fragment. `offset` is the byte length of the accumulated
    /// content *before* this fragment, so a reconnecting client can discard
    /// fragments it already has.
    Delta { offset: usize, text: String },
    /// A tool call started.
    ToolStarted { name: String },
    /// A tool call finished.
    ToolFinished { name: String, ok: bool, ms: u64 },
    /// The turn completed; `content` is the full assistant text.
    Done { content: String },
    /// The turn failed. The message is already caller-safe.
    Failed { message: String },
}

/// Receives [`ChatEvent`]s as they happen. Implementations must not block.
pub trait ChatSink: Send + Sync {
    fn emit(&self, event: ChatEvent);
}

/// A sink that drops everything — useful for tests and non-streaming callers.
pub struct NullSink;

impl ChatSink for NullSink {
    fn emit(&self, _event: ChatEvent) {}
}

/// One tool invocation, for display alongside the finished message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub name: String,
    pub ok: bool,
    pub ms: u64,
}

#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub max_turns: usize,
    pub max_tokens: u32,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            max_turns: MAX_CHAT_TURNS,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

/// The outcome of one user turn.
pub struct ChatTurnResult {
    /// Final assistant text shown to the user.
    pub content: String,
    /// Assistant and tool messages produced this turn, in OpenAI wire shape, ready
    /// to append to the stored transcript so the next turn can replay them.
    pub new_messages: Vec<Value>,
    /// Display-only record of which tools ran.
    pub tool_trace: Vec<ToolInvocation>,
    /// Model id as reported by OpenRouter (providers may resolve aliases).
    pub model: Option<String>,
}

/// Run one user turn to completion, streaming fragments to `sink`.
///
/// `history` is the replayed transcript **including** the new user message; the
/// system prompt is passed separately and is never persisted by the caller.
pub async fn run_chat(
    api_key: &str,
    model: &str,
    system: &str,
    history: Vec<Value>,
    toolbox: &dyn ChatToolbox,
    sink: &dyn ChatSink,
    opts: ChatOptions,
) -> Result<ChatTurnResult, AiError> {
    let model = model.trim();
    if model.is_empty() {
        return Err(AiError::Agent("model id is empty".into()));
    }
    if api_key.trim().is_empty() {
        return Err(AiError::Agent("api key is empty".into()));
    }

    let client = OpenRouterClient::new(api_key)?;
    let tool_defs = toolbox.definitions();

    let mut messages: Vec<Value> = Vec::with_capacity(history.len() + 1);
    messages.push(json!({ "role": "system", "content": system }));
    messages.extend(history);

    let mut new_messages: Vec<Value> = Vec::new();
    let mut tool_trace: Vec<ToolInvocation> = Vec::new();
    let mut content = String::new();
    let mut resolved_model: Option<String> = None;

    info!(%model, tools = tool_defs.len(), "starting chat turn");

    for turn_idx in 0..opts.max_turns {
        // Each round trip streams into the same accumulated `content`, so text from a
        // model that narrates between tool calls stays in order for the client.
        let base_offset = content.len();
        let mut streamed = String::new();

        let turn = client
            .chat_completion_stream(model, &messages, &tool_defs, opts.max_tokens, |delta| {
                match delta {
                    StreamDelta::Text(text) => {
                        let offset = base_offset + streamed.len();
                        streamed.push_str(&text);
                        sink.emit(ChatEvent::Delta { offset, text });
                    }
                    StreamDelta::ToolCallNamed(name) => {
                        sink.emit(ChatEvent::ToolStarted { name });
                    }
                }
            })
            .await?;

        if resolved_model.is_none() {
            resolved_model = turn.model.clone();
        }
        content.push_str(&streamed);

        if turn.tool_calls.is_empty() {
            // Plain answer: this is the end of the turn.
            let text = turn.content.unwrap_or(streamed);
            let message = json!({ "role": "assistant", "content": text });
            new_messages.push(message);

            let final_content = if content.trim().is_empty() {
                // A provider that returned neither text nor tools would already have
                // errored upstream; this only guards an all-whitespace answer.
                warn!("chat turn produced empty content");
                String::new()
            } else {
                content
            };

            sink.emit(ChatEvent::Done {
                content: final_content.clone(),
            });
            return Ok(ChatTurnResult {
                content: final_content,
                new_messages,
                tool_trace,
                model: resolved_model,
            });
        }

        let assistant_msg = assistant_tool_call_message(&turn.tool_calls, turn.content.as_deref());
        messages.push(assistant_msg.clone());
        new_messages.push(assistant_msg);

        for call in &turn.tool_calls {
            let started = Instant::now();
            let result = toolbox.dispatch(&call.name, &call.arguments).await;
            let ms = started.elapsed().as_millis() as u64;
            let ok = result.is_ok();

            let payload = match result {
                Ok(text) => truncate_tool_result(&text),
                Err(e) => {
                    warn!(tool = %call.name, error = %e, "chat tool failed");
                    json!({
                        "error": e,
                        "hint": "Fix the arguments and retry, or answer with what you already have. \
                                 Arguments must be a single JSON object matching the tool schema.",
                    })
                    .to_string()
                }
            };

            info!(turn = turn_idx, tool = %call.name, ok, ms, "chat tool call");
            sink.emit(ChatEvent::ToolFinished {
                name: call.name.clone(),
                ok,
                ms,
            });
            tool_trace.push(ToolInvocation {
                name: call.name.clone(),
                ok,
                ms,
            });

            let tool_msg = json!({
                "role": "tool",
                "tool_call_id": call.id,
                "name": call.name,
                "content": payload,
            });
            messages.push(tool_msg.clone());
            new_messages.push(tool_msg);
        }
    }

    // Ran out of round trips. Anything already streamed is still worth keeping.
    warn!(
        max_turns = opts.max_turns,
        "chat turn hit the round-trip cap"
    );
    let message = if content.trim().is_empty() {
        "I ran out of steps before I could finish that. Try narrowing the question — \
         for example to one car or one month."
            .to_string()
    } else {
        content
    };
    new_messages.push(json!({ "role": "assistant", "content": message }));
    sink.emit(ChatEvent::Done {
        content: message.clone(),
    });
    Ok(ChatTurnResult {
        content: message,
        new_messages,
        tool_trace,
        model: resolved_model,
    })
}

fn assistant_tool_call_message(tool_calls: &[ToolCall], content: Option<&str>) -> Value {
    let calls: Vec<Value> = tool_calls
        .iter()
        .map(|tc| {
            json!({
                "id": tc.id,
                "type": "function",
                "function": { "name": tc.name, "arguments": tc.arguments }
            })
        })
        .collect();

    json!({
        "role": "assistant",
        "content": content.map(Value::from).unwrap_or(Value::Null),
        "tool_calls": calls,
    })
}

fn truncate_tool_result(text: &str) -> String {
    if text.len() <= MAX_TOOL_RESULT_CHARS {
        return text.to_string();
    }
    // Cut on a char boundary, then say so — a silently clipped JSON payload reads to
    // the model as corrupt data rather than as an omission.
    let mut end = MAX_TOOL_RESULT_CHARS;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[truncated: result exceeded {MAX_TOOL_RESULT_CHARS} characters; \
         narrow the query with filters such as car_id, from/to or limit]",
        &text[..end]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct RecordingSink(Mutex<Vec<ChatEvent>>);

    impl ChatSink for RecordingSink {
        fn emit(&self, event: ChatEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn truncate_tool_result_keeps_short_payloads() {
        assert_eq!(truncate_tool_result("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn truncate_tool_result_flags_the_cut() {
        let long = "x".repeat(MAX_TOOL_RESULT_CHARS + 500);
        let out = truncate_tool_result(&long);
        assert!(out.contains("truncated"), "{out}");
        assert!(out.len() < long.len() + 200);
    }

    #[test]
    fn truncate_tool_result_respects_char_boundaries() {
        // Multi-byte chars straddling the cut must not panic.
        let long = "é".repeat(MAX_TOOL_RESULT_CHARS);
        let out = truncate_tool_result(&long);
        assert!(out.contains("truncated"), "{out}");
    }

    #[test]
    fn assistant_tool_call_message_keeps_null_content() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "list_trips".into(),
            arguments: "{}".into(),
        }];
        let msg = assistant_tool_call_message(&calls, None);
        assert!(msg["content"].is_null());
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "list_trips");
        assert_eq!(msg["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn null_sink_and_recording_sink_are_object_safe() {
        // `run_chat` takes `&dyn ChatSink`; guard the trait staying dyn-compatible.
        let sinks: Vec<Box<dyn ChatSink>> = vec![
            Box::new(NullSink),
            Box::new(RecordingSink(Mutex::new(Vec::new()))),
        ];
        for s in &sinks {
            s.emit(ChatEvent::ToolStarted {
                name: "get_trip".into(),
            });
        }
    }
}
