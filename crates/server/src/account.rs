//! Account-level data rights: download everything, or delete everything.
//!
//! `GET /api/me/export` streams one JSON document with the user's profile, owned
//! cars and their trips (points included), devices, shares, chats and audit log.
//! Secrets are never included: API keys, token hashes and session ids are left
//! out. Vault objects are exported as the ciphertext the server holds.
//!
//! `DELETE /api/me` erases the account. Owned cars (and through them trips,
//! points, devices, vault objects and derived tables) cascade from the user row;
//! photo files on disk are removed explicitly.

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{self, AuditEvent, ClientMeta};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me/export", get(export_account))
        .route("/api/me", axum::routing::delete(delete_account))
}

/// Sections of the export, each a single JSON value computed by Postgres.
/// `$1` is the user id. Secret columns are subtracted from each row.
const SECTIONS: &[(&str, &str)] = &[
    (
        "profile",
        "SELECT to_jsonb(u) - 'openrouter_api_key_enc' - 'openrouter_key_hint'
                - 'ors_api_key_enc' - 'ors_key_hint' - 'mcp_token_hash' - 'mcp_token_hint'
         FROM users u WHERE u.id = $1",
    ),
    (
        "cars",
        "SELECT COALESCE(jsonb_agg(to_jsonb(c) ORDER BY c.created_at), '[]')
         FROM cars c WHERE c.owner_user_id = $1",
    ),
    (
        "devices",
        "SELECT COALESCE(jsonb_agg(to_jsonb(d) - 'token_hash' ORDER BY d.created_at), '[]')
         FROM devices d JOIN cars c ON c.id = d.car_id WHERE c.owner_user_id = $1",
    ),
    (
        "shares_given",
        "SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'car_id', s.car_id, 'email', u.email, 'role', s.role,
                    'created_at', s.created_at)), '[]')
         FROM car_shares s JOIN cars c ON c.id = s.car_id JOIN users u ON u.id = s.user_id
         WHERE c.owner_user_id = $1",
    ),
    (
        "shares_received",
        "SELECT COALESCE(jsonb_agg(jsonb_build_object(
                    'car_id', s.car_id, 'role', s.role, 'created_at', s.created_at)), '[]')
         FROM car_shares s WHERE s.user_id = $1",
    ),
    (
        "trips",
        "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY t.started_at), '[]')
         FROM tracks t JOIN cars c ON c.id = t.car_id WHERE c.owner_user_id = $1",
    ),
    (
        "chat_conversations",
        "SELECT COALESCE(jsonb_agg(to_jsonb(cc) || jsonb_build_object('messages',
                    (SELECT COALESCE(jsonb_agg(to_jsonb(m) ORDER BY m.seq), '[]')
                     FROM chat_messages m WHERE m.conversation_id = cc.id))
                  ORDER BY cc.created_at), '[]')
         FROM chat_conversations cc WHERE cc.user_id = $1",
    ),
    (
        "vault_objects",
        "SELECT COALESCE(jsonb_agg(to_jsonb(v) ORDER BY v.created_at), '[]')
         FROM vault_objects v JOIN cars c ON c.id = v.car_id WHERE c.owner_user_id = $1",
    ),
    (
        "audit_log",
        "SELECT COALESCE(jsonb_agg(to_jsonb(a) - 'actor_session_id' ORDER BY a.created_at), '[]')
         FROM audit_events a WHERE a.user_id = $1",
    ),
];

type Chunk = Result<Bytes, std::io::Error>;

async fn export_account(State(state): State<AppState>, user: AuthUser) -> AppResult<Response> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Chunk>(8);
    let pool = state.pool.clone();
    let user_id = user.id;
    tokio::spawn(async move {
        if let Err(e) = write_export(&pool, user_id, &tx).await {
            tracing::warn!(%user_id, error = %e, "account export failed mid-stream");
            let _ = tx.send(Err(std::io::Error::other("export failed"))).await;
        }
    });
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    });

    let mut res = Response::new(Body::from_stream(stream));
    *res.status_mut() = StatusCode::OK;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"car-tracking-export.json\""),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(res)
}

async fn send(tx: &tokio::sync::mpsc::Sender<Chunk>, s: String) -> AppResult<()> {
    tx.send(Ok(Bytes::from(s)))
        .await
        .map_err(|_| AppError::internal("export reader went away"))
}

/// Emit the export as one JSON object. Points are written trip by trip so a long
/// history never has to fit in memory at once.
async fn write_export(
    pool: &PgPool,
    user_id: Uuid,
    tx: &tokio::sync::mpsc::Sender<Chunk>,
) -> AppResult<()> {
    send(
        tx,
        format!(
            "{{\"format\":\"car-tracking-export/v1\",\"exported_at\":{}",
            serde_json::to_string(&chrono::Utc::now())
                .map_err(|e| AppError::internal(e.to_string()))?
        ),
    )
    .await?;
    for (name, sql) in SECTIONS {
        let value: Option<serde_json::Value> = sqlx::query_scalar(sqlx::AssertSqlSafe(*sql))
            .bind(user_id)
            .fetch_one(pool)
            .await?;
        send(
            tx,
            format!(",\"{name}\":{}", value.unwrap_or(serde_json::Value::Null)),
        )
        .await?;
    }

    let track_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT t.id FROM tracks t JOIN cars c ON c.id = t.car_id
         WHERE c.owner_user_id = $1 ORDER BY t.started_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    send(tx, ",\"track_points\":{".into()).await?;
    for (i, id) in track_ids.iter().enumerate() {
        let points: serde_json::Value = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(
                        (to_jsonb(tp) - 'gps' - 'track_id')
                        || jsonb_build_object('lat', ST_Y(tp.gps::geometry),
                                              'lon', ST_X(tp.gps::geometry))
                        ORDER BY tp.recorded_at), '[]')
             FROM track_points tp WHERE tp.track_id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await?;
        let sep = if i == 0 { "" } else { "," };
        send(tx, format!("{sep}\"{id}\":{points}")).await?;
    }
    send(tx, "}}".into()).await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct DeleteAccountRequest {
    /// Must equal the account email; guards against a stray or forged request.
    confirm_email: String,
}

async fn delete_account(
    State(state): State<AppState>,
    user: AuthUser,
    client: ClientMeta,
    jar: CookieJar,
    Json(body): Json<DeleteAccountRequest>,
) -> AppResult<(CookieJar, Json<serde_json::Value>)> {
    if !body.confirm_email.trim().eq_ignore_ascii_case(&user.email) {
        return Err(AppError::BadRequest(
            "confirm_email does not match this account".into(),
        ));
    }

    let photos: Vec<String> = sqlx::query_scalar(
        "SELECT photo_path FROM cars WHERE owner_user_id = $1 AND photo_path IS NOT NULL",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;

    let mut tx = state.pool.begin().await?;
    // Erasure covers the security log too; the row below records only that an
    // account was deleted, not whose.
    sqlx::query("DELETE FROM audit_events WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    for rel in photos {
        if let Ok(abs) = crate::cars::resolve_photo_path(&state.config.upload_dir, &rel)
            && let Err(e) = tokio::fs::remove_file(&abs).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, "removing photo of deleted account failed");
        }
    }

    audit::record(
        &state.pool,
        AuditEvent {
            user_id: None,
            actor_session_id: None,
            action: audit::actions::ACCOUNT_DELETED,
            resource_type: None,
            resource_id: None,
            ip: Some(&client.ip),
            user_agent: client.user_agent.as_deref(),
            meta: serde_json::json!({}),
        },
    )
    .await;

    Ok((
        crate::auth::clear_session_cookie(jar),
        Json(serde_json::json!({ "ok": true })),
    ))
}
