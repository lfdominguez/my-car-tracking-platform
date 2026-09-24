//! Conversation and transcript persistence.
//!
//! `chat_messages` is both the OpenRouter wire transcript and the display feed:
//! [`load_transcript`] rebuilds OpenAI messages from every row, [`load_display`] keeps
//! only what a human should read.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool, Row};
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
        Some(e) if e == crate::analysis::jobs::CANCELLED_ERROR => {
            Some("Stopped before the answer was finished.".into())
        }
        Some("timed out") => {
            Some("That answer took too long and was stopped. Try a narrower question.".into())
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
    Ok(drop_unanswered_tool_calls(out))
}

/// Make a replayed transcript structurally valid for the provider.
///
/// OpenRouter rejects a request in which an assistant `tool_calls` entry has no
/// `tool` reply, or a `tool` reply names a call nobody made, and that rejection
/// fails every later turn of the conversation — not just one. Rows written before
/// turns were persisted atomically (or by a crash between inserts) can leave exactly
/// that shape behind, so replay repairs it instead of trusting storage:
///
/// * a tool call without a reply is removed from its assistant message, and an
///   assistant message left with neither calls nor text is dropped;
/// * a tool reply that does not answer the immediately preceding assistant message
///   is dropped.
pub fn drop_unanswered_tool_calls(messages: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        let message = &messages[i];
        let role = message["role"].as_str().unwrap_or("");

        if role == "tool" {
            // Reached only when no assistant message claimed it (see below).
            i += 1;
            continue;
        }

        let Some(calls) = message["tool_calls"]
            .as_array()
            .filter(|_| role == "assistant")
        else {
            out.push(message.clone());
            i += 1;
            continue;
        };

        // The replies to this message are the run of `tool` rows right after it.
        let mut j = i + 1;
        let mut replies: Vec<&Value> = Vec::new();
        while j < messages.len() && messages[j]["role"] == "tool" {
            replies.push(&messages[j]);
            j += 1;
        }

        let call_ids: Vec<&str> = calls.iter().filter_map(|c| c["id"].as_str()).collect();
        let answered: Vec<&str> = call_ids
            .iter()
            .copied()
            .filter(|id| replies.iter().any(|r| r["tool_call_id"] == *id))
            .collect();

        if !answered.is_empty() {
            let kept_calls: Vec<Value> = calls
                .iter()
                .filter(|c| c["id"].as_str().is_some_and(|id| answered.contains(&id)))
                .cloned()
                .collect();
            let mut assistant = message.clone();
            assistant["tool_calls"] = Value::Array(kept_calls);
            out.push(assistant);
            let mut seen: Vec<&str> = Vec::new();
            for reply in replies {
                let id = reply["tool_call_id"].as_str().unwrap_or("");
                // One reply per call: a duplicate is as invalid as an orphan.
                if answered.contains(&id) && !seen.contains(&id) {
                    seen.push(id);
                    out.push(reply.clone());
                }
            }
        } else if let Some(text) = message["content"].as_str().filter(|t| !t.trim().is_empty()) {
            out.push(json!({ "role": "assistant", "content": text }));
        }
        i = j;
    }
    out
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

/// Take the conversation's row lock for the rest of the transaction.
///
/// Every write that allocates a `seq` or checks the one-generation guard goes
/// through this lock, which is what makes `MAX(seq) + 1` and check-then-insert safe:
/// two requests for the same conversation queue here instead of racing.
async fn lock_conversation(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    user_id: Option<Uuid>,
) -> AppResult<()> {
    let found = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM chat_conversations
        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)
        FOR UPDATE
        "#,
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await?;
    found.map(|_| ()).ok_or(AppError::NotFound)
}

/// Append one message, allocating the next `seq`. The caller must hold
/// [`lock_conversation`] in the same transaction.
#[allow(clippy::too_many_arguments)]
async fn insert_message(
    conn: &mut PgConnection,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    status: &str,
    tool_calls: Option<&Value>,
    tool_call_id: Option<&str>,
    tool_name: Option<&str>,
) -> AppResult<Uuid> {
    let id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO chat_messages
            (conversation_id, seq, role, content, status, tool_calls, tool_call_id, tool_name)
        SELECT $1,
               COALESCE(MAX(seq), 0) + 1,
               $2, $3, $4, $5, $6, $7
        FROM chat_messages
        WHERE conversation_id = $1
        RETURNING id
        "#,
    )
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(status)
    .bind(tool_calls)
    .bind(tool_call_id)
    .bind(tool_name)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// The two rows a new user turn creates.
pub struct StartedTurn {
    pub user_message_id: Uuid,
    pub assistant_message_id: Uuid,
}

/// Record a user message and its pending assistant placeholder, atomically and only
/// if no other turn is generating in the conversation.
///
/// The guard and the inserts share one transaction under the conversation lock, so
/// two concurrent posts cannot both pass the check.
pub async fn start_turn(
    pool: &PgPool,
    user_id: Uuid,
    conversation_id: Uuid,
    content: &str,
) -> AppResult<StartedTurn> {
    let mut tx = pool.begin().await?;
    lock_conversation(&mut tx, conversation_id, Some(user_id)).await?;

    let active = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM chat_messages
        WHERE conversation_id = $1 AND status IN ('pending', 'running')
        "#,
    )
    .bind(conversation_id)
    .fetch_one(&mut *tx)
    .await?;
    if active > 0 {
        return Err(AppError::Conflict(
            "This conversation is still answering. Wait for it to finish.".into(),
        ));
    }

    let user_message_id = insert_message(
        &mut tx,
        conversation_id,
        "user",
        content,
        "complete",
        None,
        None,
        None,
    )
    .await?;
    let assistant_message_id = insert_message(
        &mut tx,
        conversation_id,
        "assistant",
        "",
        "pending",
        None,
        None,
        None,
    )
    .await?;

    if let Some(title) = normalize_title(Some(content)) {
        sqlx::query(
            r#"
            UPDATE chat_conversations
            SET title = $2
            WHERE id = $1 AND title = 'New chat'
            "#,
        )
        .bind(conversation_id)
        .bind(&title)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE chat_conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(StartedTurn {
        user_message_id,
        assistant_message_id,
    })
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

/// Mark a generating message failed. `false` when it was no longer generating.
pub async fn fail_message(pool: &PgPool, message_id: Uuid, error: &str) -> AppResult<bool> {
    let error: String = error.chars().take(500).collect();
    let res = sqlx::query(
        r#"
        UPDATE chat_messages
        SET status = 'failed', error = $2, updated_at = NOW()
        WHERE id = $1 AND status IN ('pending', 'running')
        "#,
    )
    .bind(message_id)
    .bind(&error)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Assistant messages still generating in a conversation.
pub async fn active_message_ids(pool: &PgPool, conversation_id: Uuid) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM chat_messages
        WHERE conversation_id = $1 AND status IN ('pending', 'running')
        "#,
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?)
}

/// Everything a finished turn stores.
pub struct FinishedTurn<'a> {
    pub content: &'a str,
    pub tool_trace: &'a Value,
    pub model: Option<&'a str>,
    /// Assistant and tool messages in OpenAI wire shape, as `ai::run_chat` returns.
    pub new_messages: &'a [Value],
}

/// Persist a finished turn in one transaction: the tool plumbing, the answer on its
/// placeholder row (re-stamped to sort after that plumbing) and the conversation's
/// `updated_at`.
///
/// All-or-nothing matters because replay trusts this shape. A crash between the
/// assistant `tool_calls` row and its `tool` replies used to leave an orphaned call
/// that made the provider reject every later turn.
///
/// Returns `false` without writing when the placeholder is no longer generating —
/// the turn was cancelled or the conversation deleted while it ran.
pub async fn finish_turn(
    pool: &PgPool,
    conversation_id: Uuid,
    assistant_message_id: Uuid,
    turn: FinishedTurn<'_>,
) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    match lock_conversation(&mut tx, conversation_id, None).await {
        Ok(()) => {}
        Err(AppError::NotFound) => return Ok(false),
        Err(e) => return Err(e),
    }

    let still_generating = sqlx::query_scalar::<_, String>(
        "SELECT status FROM chat_messages WHERE id = $1 AND conversation_id = $2",
    )
    .bind(assistant_message_id)
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some_and(|status| matches!(status.as_str(), "pending" | "running"));
    if !still_generating {
        return Ok(false);
    }

    for message in turn.new_messages {
        let role = message["role"].as_str().unwrap_or("");
        match role {
            // The final assistant text is stored on the placeholder row below.
            "assistant" if message.get("tool_calls").is_none() => {}
            "assistant" => {
                let content = message["content"].as_str().unwrap_or("");
                insert_message(
                    &mut tx,
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
                insert_message(
                    &mut tx,
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
        SET content = $3, tool_trace = $4, model = $5,
            status = 'complete', error = NULL, updated_at = NOW(),
            seq = (SELECT COALESCE(MAX(seq), 0) + 1 FROM chat_messages WHERE conversation_id = $2)
        WHERE id = $1
        "#,
    )
    .bind(assistant_message_id)
    .bind(conversation_id)
    .bind(turn.content)
    .bind(turn.tool_trace)
    .bind(turn.model)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE chat_conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
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
    fn replay_drops_a_tool_call_that_never_got_a_reply() {
        // A crash between the assistant row and its tool rows used to leave this.
        let messages = vec![user("q1"), assistant_with_tools("call_1"), user("q2")];
        let kept = drop_unanswered_tool_calls(messages);
        assert_no_orphan_tool_messages(&kept);
        assert!(
            kept.iter().all(|m| m.get("tool_calls").is_none()),
            "{kept:#?}"
        );
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn replay_keeps_only_the_answered_half_of_parallel_calls() {
        let mut both = assistant_with_tools("call_a");
        both["tool_calls"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "call_b", "type": "function",
                          "function": { "name": "get_car", "arguments": "{}" } }));
        let kept = drop_unanswered_tool_calls(vec![
            user("q"),
            both,
            tool_reply("call_a", "{}"),
            assistant("answer"),
        ]);
        let calls = kept[1]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], "call_a");
        assert_eq!(kept.len(), 4);
    }

    #[test]
    fn replay_drops_stray_and_duplicate_tool_replies() {
        let kept = drop_unanswered_tool_calls(vec![
            user("q"),
            tool_reply("ghost", "{}"),
            assistant_with_tools("call_1"),
            tool_reply("call_1", "{}"),
            tool_reply("call_1", "{}"),
            assistant("answer"),
        ]);
        assert_no_orphan_tool_messages(&kept);
        assert_eq!(kept.iter().filter(|m| m["role"] == "tool").count(), 1);
    }

    #[test]
    fn replay_keeps_narration_of_an_unanswered_call_as_plain_text() {
        let mut narrated = assistant_with_tools("call_1");
        narrated["content"] = json!("Let me look that up.");
        let kept = drop_unanswered_tool_calls(vec![user("q"), narrated]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1]["content"], "Let me look that up.");
        assert!(kept[1].get("tool_calls").is_none());
    }

    #[test]
    fn replay_leaves_a_well_formed_transcript_alone() {
        let messages = sample_transcript();
        assert_eq!(drop_unanswered_tool_calls(messages.clone()), messages);
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
