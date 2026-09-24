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
/// Character budget for one request: system prompt, replayed history, tool schemas
/// and everything this turn added, together. At ~4 characters per token that is
/// ~50k tokens, which leaves room for the answer on every model we offer. Without
/// a running check, twelve round trips of 24k-character tool results could build a
/// request several times that size and fail the turn outright.
pub const TURN_CHAR_BUDGET: usize = 200_000;
/// Replaces a tool result evicted to stay within [`TURN_CHAR_BUDGET`].
const EVICTED_TOOL_RESULT: &str = "[result omitted to stay within the context budget; \
                                   call the tool again if you still need it]";
/// Sent (never persisted) when the model must stop calling tools and answer.
const ANSWER_NOW: &str = "Answer the user now with the data you already have. Tools are no \
                          longer available for this question; say plainly if something \
                          could not be checked.";
/// Appended when the model hit its output limit mid-answer.
const TRUNCATED_NOTICE: &str = "\n\n_[This answer was cut off because it reached the length \
                                limit. Ask me to continue, or narrow the question.]_";

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
    ///
    /// [`run_chat`] never emits this itself: only the caller knows when the turn has
    /// been persisted, and a client that sees `Done` and immediately re-reads the
    /// conversation must find the answer there. The caller emits it after saving.
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
    /// See [`TURN_CHAR_BUDGET`].
    pub context_char_budget: usize,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            max_turns: MAX_CHAT_TURNS,
            max_tokens: DEFAULT_MAX_TOKENS,
            context_char_budget: TURN_CHAR_BUDGET,
        }
    }
}

/// The outcome of one user turn.
#[derive(Debug)]
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
    /// The model stopped at its output limit; `content` ends with a notice saying so.
    pub truncated: bool,
}

/// Run one user turn to completion, streaming fragments to `sink`.
///
/// Emits deltas and tool progress but not [`ChatEvent::Done`]; see its docs.
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
    run_chat_with(&client, model, system, history, toolbox, sink, opts).await
}

pub(crate) async fn run_chat_with(
    client: &OpenRouterClient,
    model: &str,
    system: &str,
    history: Vec<Value>,
    toolbox: &dyn ChatToolbox,
    sink: &dyn ChatSink,
    opts: ChatOptions,
) -> Result<ChatTurnResult, AiError> {
    let all_tools = toolbox.definitions();
    let schema_chars: usize = all_tools.iter().map(|t| t.to_string().len()).sum();
    // Room left for messages once the tool schemas are paid for.
    let message_budget = opts.context_char_budget.saturating_sub(schema_chars);

    let mut messages: Vec<Value> = Vec::with_capacity(history.len() + 1);
    messages.push(json!({ "role": "system", "content": system }));
    messages.extend(history);
    // Everything from here on was added by this turn and may be evicted.
    let turn_start = messages.len();
    let mut answer_now = false;

    let mut new_messages: Vec<Value> = Vec::new();
    let mut tool_trace: Vec<ToolInvocation> = Vec::new();
    let mut content = String::new();
    let mut resolved_model: Option<String> = None;

    info!(%model, tools = all_tools.len(), "starting chat turn");

    for turn_idx in 0..opts.max_turns {
        // Each round trip streams into the same accumulated `content`, so text from a
        // model that narrates between tool calls stays in order for the client.
        let base_offset = content.len();
        let mut streamed = String::new();

        // The last round trip, or one that no longer fits, must produce the answer:
        // offer no tools and say so, rather than ending on "ran out of steps".
        let last_round = turn_idx + 1 == opts.max_turns;
        let mut request: Vec<Value>;
        let (request_messages, tool_defs): (&[Value], &[Value]) = if answer_now || last_round {
            request = messages.clone();
            request.push(json!({ "role": "user", "content": ANSWER_NOW }));
            (&request, &[])
        } else {
            (&messages, &all_tools)
        };

        let turn = client
            .chat_completion_stream(
                model,
                request_messages,
                tool_defs,
                opts.max_tokens,
                |delta| match delta {
                    StreamDelta::Text(text) => {
                        let offset = base_offset + streamed.len();
                        streamed.push_str(&text);
                        sink.emit(ChatEvent::Delta { offset, text });
                    }
                    StreamDelta::ToolCallNamed(name) => {
                        sink.emit(ChatEvent::ToolStarted { name });
                    }
                },
            )
            .await?;

        if resolved_model.is_none() {
            resolved_model = turn.model.clone();
        }
        content.push_str(&streamed);

        if turn.tool_calls.is_empty() {
            // Plain answer: this is the end of the turn.
            let mut text = turn.content.unwrap_or(streamed);
            let truncated = turn.finish_reason.as_deref() == Some("length");
            if truncated {
                // Say so in the answer itself: a sentence that simply stops reads
                // like a finished (and wrong) answer.
                warn!("chat answer hit the output token limit");
                sink.emit(ChatEvent::Delta {
                    offset: content.len(),
                    text: TRUNCATED_NOTICE.to_string(),
                });
                content.push_str(TRUNCATED_NOTICE);
                text.push_str(TRUNCATED_NOTICE);
            }
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

            return Ok(ChatTurnResult {
                content: final_content,
                new_messages,
                tool_trace,
                model: resolved_model,
                truncated,
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

        if !fit_turn_budget(&mut messages, turn_start, message_budget) {
            warn!(
                budget = message_budget,
                "chat turn exceeds its context budget; asking for an answer now"
            );
            answer_now = true;
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
    Ok(ChatTurnResult {
        content: message,
        new_messages,
        tool_trace,
        model: resolved_model,
        truncated: false,
    })
}

/// Approximate request size in characters, as serialized.
pub fn approx_chars(messages: &[Value]) -> usize {
    messages.iter().map(|m| m.to_string().len()).sum()
}

/// Keep the request under `budget` by evicting this turn's tool results, oldest
/// first, down to a short stub. Returns `false` when even that is not enough —
/// the caller must then stop offering tools.
///
/// Only messages from `turn_start` on are touched: the replayed history was already
/// trimmed to its own budget, and the stub keeps each reply's `tool_call_id`, so
/// the call/reply pairing the provider validates stays intact. The stored transcript
/// is unaffected; it keeps the full results.
fn fit_turn_budget(messages: &mut [Value], turn_start: usize, budget: usize) -> bool {
    let mut total = approx_chars(messages);
    if total <= budget {
        return true;
    }
    for m in messages.iter_mut().skip(turn_start) {
        if total <= budget {
            break;
        }
        if m["role"] != "tool" || m["content"] == EVICTED_TOOL_RESULT {
            continue;
        }
        let before = m.to_string().len();
        m["content"] = Value::String(EVICTED_TOOL_RESULT.to_string());
        total = total - before + m.to_string().len();
    }
    total <= budget
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

    use crate::test_http::{Scripted, serve};

    /// Records every call and answers with a fixed-size payload.
    struct FakeTools {
        calls: Mutex<Vec<(String, String)>>,
        result_chars: usize,
    }

    impl FakeTools {
        fn new(result_chars: usize) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result_chars,
            }
        }
    }

    #[async_trait]
    impl ChatToolbox for FakeTools {
        fn definitions(&self) -> Vec<Value> {
            vec![json!({
                "type": "function",
                "function": {
                    "name": "get_trip",
                    "description": "trip",
                    "parameters": { "type": "object", "properties": {} }
                }
            })]
        }

        async fn dispatch(&self, name: &str, arguments: &str) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_string(), arguments.to_string()));
            Ok(format!(
                "{{\"data\":\"{}\"}}",
                "x".repeat(self.result_chars)
            ))
        }
    }

    fn text_frame(text: &str, finish: Option<&str>) -> String {
        let finish = finish.map_or("null".to_string(), |f| format!("\"{f}\""));
        format!(
            "data: {{\"model\":\"m\",\"choices\":[{{\"delta\":{{\"content\":{}}},\"finish_reason\":{finish}}}]}}\n\n",
            serde_json::to_string(text).unwrap()
        )
    }

    fn tool_frame(index: u32, id: &str, args: &str) -> String {
        format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":{index},\"id\":\"{id}\",\"function\":{{\"name\":\"get_trip\",\"arguments\":{}}}}}]}}}}]}}\n\n",
            serde_json::to_string(args).unwrap()
        )
    }

    fn sse(frames: &[String]) -> Scripted {
        let refs: Vec<&str> = frames.iter().map(String::as_str).collect();
        Scripted::sse(&refs)
    }

    async fn run(
        endpoint: &str,
        tools: &FakeTools,
        sink: &RecordingSink,
        opts: ChatOptions,
    ) -> Result<ChatTurnResult, AiError> {
        let client = OpenRouterClient::with_endpoint("sk", endpoint).unwrap();
        run_chat_with(
            &client,
            "m",
            "system prompt",
            vec![json!({ "role": "user", "content": "q" })],
            tools,
            sink,
            opts,
        )
        .await
    }

    #[tokio::test]
    async fn a_length_cut_answer_is_marked_truncated() {
        let mock = serve(vec![sse(&[
            text_frame("Your trips in August were", None),
            text_frame(" mostly", Some("length")),
            "data: [DONE]\n\n".into(),
        ])])
        .await;
        let sink = RecordingSink(Mutex::new(Vec::new()));
        let out = run(
            &mock.endpoint,
            &FakeTools::new(1),
            &sink,
            ChatOptions::default(),
        )
        .await
        .unwrap();
        assert!(out.truncated);
        assert!(out.content.ends_with(TRUNCATED_NOTICE), "{}", out.content);
        // The live client is told too, at the right offset.
        let events = sink.0.lock().unwrap();
        let last_delta = events
            .iter()
            .rev()
            .find_map(|e| match e {
                ChatEvent::Delta { offset, text } => Some((*offset, text.clone())),
                _ => None,
            })
            .unwrap();
        assert_eq!(last_delta.1, TRUNCATED_NOTICE);
        assert_eq!(last_delta.0, "Your trips in August were mostly".len());
    }

    #[tokio::test]
    async fn parallel_tool_calls_are_all_answered_in_order() {
        let mock = serve(vec![
            sse(&[
                tool_frame(0, "call_a", "{\"trip_id\":\"a\"}"),
                tool_frame(1, "call_b", "{\"trip_id\":\"b\"}"),
                "data: [DONE]\n\n".into(),
            ]),
            sse(&[text_frame("done", Some("stop"))]),
        ])
        .await;
        let tools = FakeTools::new(10);
        let sink = RecordingSink(Mutex::new(Vec::new()));
        let out = run(&mock.endpoint, &tools, &sink, ChatOptions::default())
            .await
            .unwrap();
        assert_eq!(out.content, "done");
        assert_eq!(tools.calls.lock().unwrap().len(), 2);

        let second = &mock.bodies().await[1];
        let msgs = second["messages"].as_array().unwrap();
        let n = msgs.len();
        assert_eq!(msgs[n - 3]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(msgs[n - 2]["tool_call_id"], "call_a");
        assert_eq!(msgs[n - 1]["tool_call_id"], "call_b");
        // Persisted shape: one assistant call row, two replies, then the answer.
        assert_eq!(out.new_messages.len(), 4);
    }

    #[tokio::test]
    async fn tool_results_are_evicted_to_fit_the_turn_budget() {
        let mock = serve(vec![
            sse(&[tool_frame(0, "c1", "{}"), "data: [DONE]\n\n".into()]),
            sse(&[tool_frame(0, "c2", "{}"), "data: [DONE]\n\n".into()]),
            sse(&[text_frame("answer", Some("stop"))]),
        ])
        .await;
        // Each result is ~20k chars; the budget holds one, not two.
        let tools = FakeTools::new(20_000);
        let sink = RecordingSink(Mutex::new(Vec::new()));
        let opts = ChatOptions {
            context_char_budget: 30_000,
            ..ChatOptions::default()
        };
        let out = run(&mock.endpoint, &tools, &sink, opts).await.unwrap();
        assert_eq!(out.content, "answer");

        let third = &mock.bodies().await[2];
        let msgs = third["messages"].as_array().unwrap();
        let tool_contents: Vec<&str> = msgs
            .iter()
            .filter(|m| m["role"] == "tool")
            .map(|m| m["content"].as_str().unwrap())
            .collect();
        assert_eq!(tool_contents.len(), 2);
        assert_eq!(tool_contents[0], EVICTED_TOOL_RESULT);
        assert!(tool_contents[1].len() > 20_000);
        assert!(approx_chars(msgs) <= 30_000);
        // The stored transcript keeps the full results.
        assert!(
            out.new_messages
                .iter()
                .filter(|m| m["role"] == "tool")
                .all(|m| m["content"].as_str().unwrap().len() > 20_000)
        );
    }

    #[tokio::test]
    async fn a_turn_that_cannot_fit_stops_offering_tools() {
        let mock = serve(vec![
            sse(&[tool_frame(0, "c1", "{}"), "data: [DONE]\n\n".into()]),
            sse(&[text_frame("best effort", Some("stop"))]),
        ])
        .await;
        let tools = FakeTools::new(20_000);
        let sink = RecordingSink(Mutex::new(Vec::new()));
        // Too small for even the stubbed request plus the schema: answer now.
        let opts = ChatOptions {
            context_char_budget: 200,
            ..ChatOptions::default()
        };
        let out = run(&mock.endpoint, &tools, &sink, opts).await.unwrap();
        assert_eq!(out.content, "best effort");
        let second = &mock.bodies().await[1];
        assert!(second.get("tools").is_none(), "{second}");
        let last = second["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(last["content"], ANSWER_NOW);
    }

    #[tokio::test]
    async fn the_last_round_trip_must_answer() {
        let mock = serve(vec![
            sse(&[tool_frame(0, "c1", "{}"), "data: [DONE]\n\n".into()]),
            sse(&[text_frame("final", Some("stop"))]),
        ])
        .await;
        let tools = FakeTools::new(10);
        let sink = RecordingSink(Mutex::new(Vec::new()));
        let opts = ChatOptions {
            max_turns: 2,
            ..ChatOptions::default()
        };
        let out = run(&mock.endpoint, &tools, &sink, opts).await.unwrap();
        assert_eq!(out.content, "final");
        let bodies = mock.bodies().await;
        assert!(bodies[0].get("tools").is_some());
        assert!(bodies[1].get("tools").is_none());
    }

    #[tokio::test]
    async fn a_mid_stream_error_fails_the_turn_without_a_done_event() {
        let mock = serve(vec![Scripted::sse(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"Provider crashed\",\"code\":502}}\n\n",
        ])])
        .await;
        let sink = RecordingSink(Mutex::new(Vec::new()));
        let err = run(
            &mock.endpoint,
            &FakeTools::new(1),
            &sink,
            ChatOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Provider crashed"), "{err}");
        // Only the caller may announce the end of a turn.
        assert!(
            !sink
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, ChatEvent::Done { .. }))
        );
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
