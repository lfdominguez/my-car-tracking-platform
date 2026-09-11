//! Conversation and transcript persistence.
//!
//! `chat_messages` is both the OpenRouter wire transcript and the display feed:
//! [`load_transcript`] rebuilds OpenAI messages from every row, [`load_display`] keeps
//! only what a human should read.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// Character budget for the replayed transcript. Roughly 30k tokens of history,
/// leaving room for the system prompt, this turn's tool results and the answer.
pub const HISTORY_CHAR_BUDGET: usize = 120_000;

#[derive(Debug, Clone, Serialize)]
pub struct ConversationDto {
    pub id: Uuid,
    pub title: String,
    pub car_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageDto {
    pub id: Uuid,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub status: String,
    /// Caller-safe failure text; never the raw internal error.
    pub error: Option<String>,
    pub tool_trace: Option<Value>,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn list_conversations(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<ConversationDto>> {
    let rows = sqlx::query(
        r#"
        SELECT id, title, car_id, created_at, updated_at
        FROM chat_conversations
        WHERE user_id = $1
        ORDER BY updated_at DESC
        LIMIT 200
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ConversationDto {
            id: r.get("id"),
            title: r.get("title"),
            car_id: r.try_get("car_id").ok().flatten(),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        })
        .collect())
}

pub async fn create_conversation(
    pool: &PgPool,
    user_id: Uuid,
    title: Option<String>,
    car_id: Option<Uuid>,
) -> AppResult<ConversationDto> {
    let title = normalize_title(title.as_deref()).unwrap_or_else(|| "New chat".to_string());
    let row = sqlx::query(
        r#"
        INSERT INTO chat_conversations (user_id, title, car_id)
        VALUES ($1, $2, $3)
        RETURNING id, title, car_id, created_at, updated_at
        "#,
    )
    .bind(user_id)
    .bind(&title)
    .bind(car_id)
    .fetch_one(pool)
    .await?;

    Ok(ConversationDto {
        id: row.get("id"),
        title: row.get("title"),
        car_id: row.try_get("car_id").ok().flatten(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// Fetch a conversation the user owns. A conversation belonging to someone else is
/// reported as missing, never as forbidden — 403 would confirm the id exists.
pub async fn owned_conversation(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> AppResult<ConversationDto> {
    let row = sqlx::query(
        r#"
        SELECT id, title, car_id, created_at, updated_at
        FROM chat_conversations
        WHERE id = $1 AND user_id = $2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(ConversationDto {
        id: row.get("id"),
        title: row.get("title"),
        car_id: row.try_get("car_id").ok().flatten(),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn delete_conversation(pool: &PgPool, user_id: Uuid, id: Uuid) -> AppResult<()> {
    let done = sqlx::query("DELETE FROM chat_conversations WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn touch_conversation(pool: &PgPool, id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE chat_conversations SET updated_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Name an untitled conversation after its opening question.
pub async fn title_from_first_message(pool: &PgPool, id: Uuid, content: &str) -> AppResult<()> {
    let Some(title) = normalize_title(Some(content)) else {
        return Ok(());
    };
    sqlx::query(
        r#"
        UPDATE chat_conversations
        SET title = $2
        WHERE id = $1 AND title = 'New chat'
        "#,
    )
    .bind(id)
    .bind(&title)
    .execute(pool)
    .await?;
    Ok(())
}

/// Messages a human should see: the conversation proper, without tool plumbing.
pub async fn load_display(pool: &PgPool, conversation_id: Uuid) -> AppResult<Vec<MessageDto>> {
    let rows = sqlx::query(
        r#"
        SELECT id, seq, role, content, status, error, tool_trace, model, created_at
        FROM chat_messages
        WHERE conversation_id = $1
          AND role IN ('user', 'assistant')
          -- An assistant row that only carried tool_calls has no text to show, but a
          -- still-generating row must appear so the client can attach its stream.
          AND (content <> '' OR status IN ('pending', 'running', 'failed'))
        ORDER BY seq
        "#,
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let status: String = r.get("status");
            let raw_error: Option<String> = r.try_get("error").ok().flatten();
            MessageDto {
                id: r.get("id"),
                seq: r.get("seq"),
                role: r.get("role"),
                content: r.get("content"),
                error: public_error(&status, raw_error.as_deref()),
                status,
                tool_trace: r.try_get("tool_trace").ok().flatten(),
                model: r.try_get("model").ok().flatten(),
                created_at: r.get("created_at"),
            }
        })
        .collect())
}

/// Internal job errors (SQL, provider stack traces) stay in the database for
/// operators; the SPA gets one generic line, exactly as trip analysis does.
pub fn public_error(status: &str, raw: Option<&str>) -> Option<String> {
    if status != "failed" {
        return None;
    }
    match raw {
        Some(e) if e.contains("api key") || e.contains("OpenRouter API key") => {
            Some("Add your OpenRouter API key in Settings to use chat.".into())
        }
        _ => Some("That answer could not be generated. Try again in a moment.".into()),
    }
}

/// Rebuild the OpenAI message array for replay, oldest first.
pub async fn load_transcript(pool: &PgPool, conversation_id: Uuid) -> AppResult<Vec<Value>> {
    let rows = sqlx::query(
        r#"
        SELECT role, content, tool_calls, tool_call_id, tool_name
        FROM chat_messages
        WHERE conversation_id = $1
          AND status = 'complete'
        ORDER BY seq
        "#,
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let role: String = r.get("role");
        let content: String = r.get("content");
        match role.as_str() {
            "user" => out.push(json!({ "role": "user", "content": content })),
            "assistant" => {
                let tool_calls: Option<Value> = r.try_get("tool_calls").ok().flatten();
                match tool_calls {
                    Some(calls) => out.push(json!({
                        "role": "assistant",
                        "content": if content.is_empty() { Value::Null } else { Value::String(content) },
                        "tool_calls": calls,
                    })),
                    None => out.push(json!({ "role": "assistant", "content": content })),
                }
            }
            "tool" => {
                let tool_call_id: Option<String> = r.try_get("tool_call_id").ok().flatten();
                let tool_name: Option<String> = r.try_get("tool_name").ok().flatten();
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id.unwrap_or_default(),
                    "name": tool_name.unwrap_or_default(),
                    "content": content,
                }));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Drop the oldest messages until the transcript fits `budget`.
///
/// The cut always lands on a `user` message. An assistant message carrying
/// `tool_calls` and the `tool` replies that answer it must never be separated —
/// OpenRouter rejects a `tool` message whose `tool_call_id` has no preceding call,
/// and that rejection fails the whole turn rather than degrading it. Starting at a
/// `user` boundary makes orphaning structurally impossible.
pub fn trim_history(messages: Vec<Value>, budget: usize) -> Vec<Value> {
    let sizes: Vec<usize> = messages.iter().map(approx_size).collect();
    let total: usize = sizes.iter().sum();
    if total <= budget {
        return messages;
    }

    // Walk back from the newest message while the budget allows.
    let mut start = messages.len();
    let mut used = 0usize;
    for i in (0..messages.len()).rev() {
        if used + sizes[i] > budget {
            break;
        }
        used += sizes[i];
        start = i;
    }

    // Advance to the next `user` message so the window cannot begin mid-tool-call.
    let mut cut = start;
    while cut < messages.len() && messages[cut]["role"] != "user" {
        cut += 1;
    }

    if cut >= messages.len() {
        // The newest turn alone exceeds the budget. Keep it whole anyway: an
        // over-budget request is recoverable, a malformed one is not.
        let last_user = messages
            .iter()
            .rposition(|m| m["role"] == "user")
            .unwrap_or(0);
        return messages[last_user..].to_vec();
    }

    messages[cut..].to_vec()
}

fn approx_size(message: &Value) -> usize {
    message.to_string().len()
}

/// Append one message, allocating the next `seq` in the conversation.
#[allow(clippy::too_many_arguments)]
pub async fn append_message(
    pool: &PgPool,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    status: &str,
    tool_calls: Option<&Value>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
) -> AppResult<(Uuid, i64)> {
    let row = sqlx::query(
        r#"
        INSERT INTO chat_messages
            (conversation_id, seq, role, content, status, tool_calls, tool_call_id, tool_name)
        SELECT $1,
               COALESCE(MAX(seq), 0) + 1,
               $2, $3, $4, $5, $6, $7
        FROM chat_messages
        WHERE conversation_id = $1
        RETURNING id, seq
        "#,
    )
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(status)
    .bind(tool_calls)
    .bind(tool_call_id)
    .bind(tool_name)
    .fetch_one(pool)
    .await?;
    Ok((row.get("id"), row.get("seq")))
}

/// Flush partial content mid-generation so a reconnect resumes near where it left off.
pub async fn update_partial(pool: &PgPool, message_id: Uuid, content: &str) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE chat_messages
        SET content = $2, status = 'running', updated_at = NOW()
        WHERE id = $1 AND status IN ('pending', 'running')
        "#,
    )
    .bind(message_id)
    .bind(content)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn complete_message(
    pool: &PgPool,
    message_id: Uuid,
    content: &str,
    tool_trace: &Value,
    model: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        r#"
        UPDATE chat_messages
        SET content = $2, tool_trace = $3, model = $4,
            status = 'complete', error = NULL, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(message_id)
    .bind(content)
    .bind(tool_trace)
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fail_message(pool: &PgPool, message_id: Uuid, error: &str) -> AppResult<()> {
    let error: String = error.chars().take(500).collect();
    sqlx::query(
        r#"
        UPDATE chat_messages
        SET status = 'failed', error = $2, updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(message_id)
    .bind(&error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Persist the assistant/tool messages a finished turn produced, so the next turn
/// can replay the tool results instead of re-running the tools.
pub async fn append_turn_messages(
    pool: &PgPool,
    conversation_id: Uuid,
    assistant_message_id: Uuid,
    new_messages: &[Value],
) -> AppResult<()> {
    for message in new_messages {
        let role = message["role"].as_str().unwrap_or("");
        match role {
            // The final assistant text is already stored on the placeholder row.
            "assistant" if message.get("tool_calls").is_none() => {}
            "assistant" => {
                let content = message["content"].as_str().unwrap_or("");
                append_message(
                    pool,
                    conversation_id,
                    "assistant",
                    content,
                    "complete",
                    message.get("tool_calls"),
                    None,
                    None,
                )
                .await?;
            }
            "tool" => {
                append_message(
                    pool,
                    conversation_id,
                    "tool",
                    message["content"].as_str().unwrap_or(""),
                    "complete",
                    None,
                    message["tool_call_id"].as_str(),
                    message["name"].as_str(),
                )
                .await?;
            }
            _ => {}
        }
    }

    // Re-stamp the answer last so it sorts after the tool rows it was derived from.
    sqlx::query(
        r#"
        UPDATE chat_messages
        SET seq = (SELECT COALESCE(MAX(seq), 0) + 1 FROM chat_messages WHERE conversation_id = $2)
        WHERE id = $1
        "#,
    )
    .bind(assistant_message_id)
    .bind(conversation_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// True when a turn is already generating in this conversation.
pub async fn has_active_message(pool: &PgPool, conversation_id: Uuid) -> AppResult<bool> {
    let count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM chat_messages
        WHERE conversation_id = $1 AND status IN ('pending', 'running')
        "#,
    )
    .bind(conversation_id)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// An assistant message the user owns, with its current content.
pub struct OwnedMessage {
    pub content: String,
    pub status: String,
    pub error: Option<String>,
    pub tool_trace: Option<Value>,
}

pub async fn owned_message(
    pool: &PgPool,
    user_id: Uuid,
    message_id: Uuid,
) -> AppResult<OwnedMessage> {
    let row = sqlx::query(
        r#"
        SELECT m.content, m.status, m.error, m.tool_trace
        FROM chat_messages m
        JOIN chat_conversations c ON c.id = m.conversation_id
        WHERE m.id = $1 AND c.user_id = $2
        "#,
    )
    .bind(message_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let status: String = row.get("status");
    let raw_error: Option<String> = row.try_get("error").ok().flatten();
    Ok(OwnedMessage {
        content: row.get("content"),
        error: public_error(&status, raw_error.as_deref()),
        status,
        tool_trace: row.try_get("tool_trace").ok().flatten(),
    })
}

/// Fail any generation left in flight by a previous process. A row stuck in
/// `running` would block its conversation's concurrency guard forever.
pub async fn fail_interrupted_messages(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        r#"
        UPDATE chat_messages
        SET status = 'failed', error = 'interrupted by server restart'
        WHERE status IN ('pending', 'running')
        "#,
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// First line of a message, clipped, for use as a conversation title.
fn normalize_title(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let first_line = raw.lines().next().unwrap_or(raw).trim();
    let mut title: String = first_line.chars().take(60).collect();
    if first_line.chars().count() > 60 {
        title.push('…');
    }
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Value {
        json!({ "role": "user", "content": text })
    }

    fn assistant_with_tools(id: &str) -> Value {
        json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": [{
                "id": id,
                "type": "function",
                "function": { "name": "list_trips", "arguments": "{}" }
            }]
        })
    }

    fn tool_reply(id: &str, body: &str) -> Value {
        json!({ "role": "tool", "tool_call_id": id, "name": "list_trips", "content": body })
    }

    fn assistant(text: &str) -> Value {
        json!({ "role": "assistant", "content": text })
    }

    /// Every `tool` message must be preceded by an assistant message announcing its id.
    fn assert_no_orphan_tool_messages(messages: &[Value]) {
        let mut announced: Vec<String> = Vec::new();
        for m in messages {
            match m["role"].as_str() {
                Some("assistant") => {
                    if let Some(calls) = m["tool_calls"].as_array() {
                        for c in calls {
                            announced.push(c["id"].as_str().unwrap_or_default().to_string());
                        }
                    }
                }
                Some("tool") => {
                    let id = m["tool_call_id"].as_str().unwrap_or_default().to_string();
                    assert!(
                        announced.contains(&id),
                        "orphaned tool message {id} in {messages:#?}"
                    );
                }
                _ => {}
            }
        }
    }

    fn sample_transcript() -> Vec<Value> {
        vec![
            user("first question"),
            assistant_with_tools("call_1"),
            tool_reply("call_1", &"a".repeat(4000)),
            assistant("first answer"),
            user("second question"),
            assistant_with_tools("call_2"),
            tool_reply("call_2", &"b".repeat(4000)),
            assistant("second answer"),
            user("third question"),
        ]
    }

    #[test]
    fn trim_history_keeps_everything_under_budget() {
        let messages = sample_transcript();
        let kept = trim_history(messages.clone(), 1_000_000);
        assert_eq!(kept.len(), messages.len());
    }

    #[test]
    fn trim_history_cuts_on_a_user_boundary() {
        let kept = trim_history(sample_transcript(), 6_000);
        assert_eq!(kept[0]["role"], "user");
        assert_no_orphan_tool_messages(&kept);
    }

    #[test]
    fn trim_history_never_orphans_a_tool_reply_at_any_budget() {
        // The interesting failures hide at budgets that land mid tool-call group.
        let messages = sample_transcript();
        for budget in (100..14_000).step_by(97) {
            let kept = trim_history(messages.clone(), budget);
            assert!(!kept.is_empty(), "budget {budget} produced nothing");
            assert_eq!(kept[0]["role"], "user", "budget {budget} cut mid-turn");
            assert_no_orphan_tool_messages(&kept);
        }
    }

    #[test]
    fn trim_history_keeps_the_newest_turn_even_when_it_alone_exceeds_budget() {
        let messages = sample_transcript();
        let kept = trim_history(messages, 10);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["content"], "third question");
    }

    #[test]
    fn trim_history_preserves_order() {
        let kept = trim_history(sample_transcript(), 8_000);
        let texts: Vec<&str> = kept
            .iter()
            .filter(|m| m["role"] == "user")
            .map(|m| m["content"].as_str().unwrap())
            .collect();
        let mut sorted = texts.clone();
        sorted.sort_unstable();
        // "second" < "third" alphabetically, matching chronological order here.
        assert_eq!(texts, sorted);
    }

    #[test]
    fn normalize_title_uses_the_first_line_and_clips() {
        assert_eq!(
            normalize_title(Some("How efficient was August?\nmore text")).unwrap(),
            "How efficient was August?"
        );
        assert_eq!(normalize_title(Some("   ")), None);
        assert_eq!(normalize_title(None), None);
        let long = normalize_title(Some(&"x".repeat(200))).unwrap();
        assert!(long.ends_with('…'));
        assert_eq!(long.chars().count(), 61);
    }

    #[test]
    fn public_error_hides_internals_but_flags_a_missing_key() {
        assert_eq!(public_error("complete", Some("boom")), None);
        assert_eq!(public_error("running", None), None);
        let generic = public_error("failed", Some("sqlx: connection refused at 10.0.0.2")).unwrap();
        assert!(!generic.contains("10.0.0.2"), "{generic}");
        let key = public_error("failed", Some("openrouter/agent error: api key is empty")).unwrap();
        assert!(key.contains("Settings"), "{key}");
    }
}
