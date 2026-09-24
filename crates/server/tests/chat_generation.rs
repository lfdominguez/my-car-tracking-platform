//! Chat generation end to end against a mock OpenRouter: persistence order,
//! concurrency, cancellation, deletion, provider errors, restart recovery and
//! replay of a damaged transcript. Requires DATABASE_URL (Postgres+PostGIS).
//!
//! One test function on purpose: the mock's address reaches the `ai` client through
//! the process environment, which may only be set while nothing else runs.

#[path = "mcp_support.rs"]
mod support;

use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

struct Chat {
    base: String,
    http: reqwest::Client,
}

impl Chat {
    async fn new_conversation(&self) -> Uuid {
        let r = self
            .http
            .post(format!("{}/api/chat/conversations", self.base))
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::CREATED);
        let body: Value = r.json().await.unwrap();
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap()
    }

    async fn post(&self, conversation: Uuid, text: &str) -> reqwest::Response {
        self.http
            .post(format!(
                "{}/api/chat/conversations/{conversation}/messages",
                self.base
            ))
            .json(&json!({ "content": text }))
            .send()
            .await
            .unwrap()
    }

    /// Post and return the assistant message id.
    async fn ask(&self, conversation: Uuid, text: &str) -> Uuid {
        let r = self.post(conversation, text).await;
        assert_eq!(r.status(), reqwest::StatusCode::ACCEPTED, "{text}");
        let body: Value = r.json().await.unwrap();
        Uuid::parse_str(body["assistant_message_id"].as_str().unwrap()).unwrap()
    }

    async fn detail(&self, conversation: Uuid) -> Value {
        self.http
            .get(format!(
                "{}/api/chat/conversations/{conversation}",
                self.base
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn message(&self, conversation: Uuid, id: Uuid) -> Value {
        self.detail(conversation).await["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == json!(id))
            .cloned()
            .unwrap_or(Value::Null)
    }

    async fn wait_status(&self, conversation: Uuid, id: Uuid, status: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let msg = self.message(conversation, id).await;
            if msg["status"] == status {
                return msg;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "never reached {status}: {msg}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn cancel(&self, id: Uuid) -> reqwest::StatusCode {
        self.http
            .post(format!("{}/api/chat/messages/{id}/cancel", self.base))
            .send()
            .await
            .unwrap()
            .status()
    }
}

#[tokio::test]
async fn chat_generation_lifecycle() {
    let mock = support::mock_openrouter().await;
    support::use_openrouter_base(&mock.base);
    let Some(state) = support::state().await else {
        eprintln!("skipping: DATABASE_URL not set or DB unavailable");
        return;
    };
    let pool = state.pool.clone();
    let base = support::serve(state.clone()).await;
    let (http, user) = support::login(&base).await;
    let chat = Chat {
        base: base.clone(),
        http,
    };

    // --- a plain answer is saved before `done` is announced -----------------
    support::set_openrouter(&state, user, "mock/ok").await;
    let convo = chat.new_conversation().await;
    let id = chat.ask(convo, "hi").await;
    let stream = chat
        .http
        .get(format!("{base}/api/chat/messages/{id}/stream"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(stream.contains("event: done"), "{stream}");
    // Immediately after `done`, a re-read must already see the finished answer.
    let msg = chat.message(convo, id).await;
    assert_eq!(msg["status"], "complete", "{msg}");
    assert_eq!(msg["content"], "Hello there.");

    // --- a tool round trip is stored call, reply, then answer ----------------
    support::set_openrouter(&state, user, "mock/tool").await;
    let id = chat.ask(convo, "which cars?").await;
    let msg = chat.wait_status(convo, id, "complete").await;
    assert_eq!(msg["content"], "Here is what I found.");
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT role, seq FROM chat_messages WHERE conversation_id = $1 ORDER BY seq",
    )
    .bind(convo)
    .fetch_all(&pool)
    .await
    .unwrap();
    let roles: Vec<&str> = rows.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        roles,
        [
            "user",
            "assistant",
            "user",
            "assistant",
            "tool",
            "assistant"
        ],
        "{rows:?}"
    );

    // --- one generation per conversation, even under concurrent posts --------
    support::set_openrouter(&state, user, "mock/slow").await;
    let texts: Vec<String> = (0..5).map(|i| format!("race {i}")).collect();
    let posts = futures::future::join_all(texts.iter().map(|t| chat.post(convo, t)));
    let statuses: Vec<u16> = posts.await.iter().map(|r| r.status().as_u16()).collect();
    assert_eq!(
        statuses.iter().filter(|s| **s == 202).count(),
        1,
        "{statuses:?}"
    );
    assert_eq!(
        statuses.iter().filter(|s| **s == 409).count(),
        4,
        "{statuses:?}"
    );

    // --- cancelling stops it and frees the conversation ----------------------
    let running: Uuid = sqlx::query_scalar(
        "SELECT id FROM chat_messages WHERE conversation_id = $1 AND status IN ('pending','running')",
    )
    .bind(convo)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(chat.cancel(running).await, reqwest::StatusCode::ACCEPTED);
    let msg = chat.wait_status(convo, running, "failed").await;
    assert!(msg["error"].as_str().unwrap().contains("Stopped"), "{msg}");
    support::set_openrouter(&state, user, "mock/ok").await;
    let id = chat.ask(convo, "after cancel").await;
    chat.wait_status(convo, id, "complete").await;

    // --- deleting a conversation stops its generation ------------------------
    support::set_openrouter(&state, user, "mock/slow").await;
    let doomed = chat.new_conversation().await;
    let doomed_msg = chat.ask(doomed, "slow one").await;
    let r = chat
        .http
        .delete(format!("{base}/api/chat/conversations/{doomed}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NO_CONTENT);
    let gone: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE id = $1")
        .bind(doomed_msg)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(gone, 0);

    // --- a provider failure is recorded with an actionable, safe message -----
    support::set_openrouter(&state, user, "mock/credits").await;
    let id = chat.ask(convo, "boom").await;
    let msg = chat.wait_status(convo, id, "failed").await;
    let error = msg["error"].as_str().unwrap();
    assert!(error.contains("credits"), "{error}");
    assert!(!error.contains("402"), "internal detail leaked: {error}");

    // --- restart recovery: a row left running blocks until swept -------------
    support::set_openrouter(&state, user, "mock/ok").await;
    sqlx::query(
        "INSERT INTO chat_messages (conversation_id, seq, role, content, status)
         SELECT $1, MAX(seq) + 1, 'assistant', '', 'running' FROM chat_messages
         WHERE conversation_id = $1",
    )
    .bind(convo)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        chat.post(convo, "blocked").await.status(),
        reqwest::StatusCode::CONFLICT
    );
    server::chat::fail_interrupted_messages(&pool)
        .await
        .unwrap();
    let id = chat.ask(convo, "unblocked").await;
    chat.wait_status(convo, id, "complete").await;

    // --- a damaged transcript is repaired before replay ----------------------
    // An assistant tool call with no reply, as a crash mid-append used to leave.
    sqlx::query(
        "INSERT INTO chat_messages (conversation_id, seq, role, content, status, tool_calls)
         SELECT $1, MAX(seq) + 1, 'assistant', '', 'complete',
                '[{\"id\":\"orphan\",\"type\":\"function\",\"function\":{\"name\":\"list_cars\",\"arguments\":\"{}\"}}]'::jsonb
         FROM chat_messages WHERE conversation_id = $1",
    )
    .bind(convo)
    .execute(&pool)
    .await
    .unwrap();
    let id = chat.ask(convo, "still works?").await;
    chat.wait_status(convo, id, "complete").await;
    let last = mock.requests.lock().await.last().cloned().unwrap();
    let sent = last["messages"].to_string();
    assert!(!sent.contains("orphan"), "orphaned call replayed: {sent}");

    // --- other users cannot see or cancel it ---------------------------------
    let (other, _) = support::login(&base).await;
    let r = other
        .post(format!("{base}/api/chat/messages/{id}/cancel"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
    let r = other
        .get(format!("{base}/api/chat/conversations/{convo}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
}
