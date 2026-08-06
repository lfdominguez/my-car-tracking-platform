//! Zero-knowledge vault HTTP API (ciphertext storage + wraps; no server decrypt).

use axum::extract::{Path, Query, State};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditEvent};
use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_edit_car, can_read_car, require_owner};
use crate::state::AppState;

const MAX_OBJECT_TYPE_LEN: usize = 64;
const MAX_PUBKEY_LEN: usize = 32;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/vault/status", get(vault_status))
        .route("/api/vault/enable", post(vault_enable))
        .route("/api/vault/activate", post(vault_activate))
        .route("/api/vault/objects", put(put_object).get(get_objects))
        .route(
            "/api/vault/cars/{id}/deks",
            get(list_deks).put(upsert_dek),
        )
        .route(
            "/api/vault/cars/{id}/deks/{recipient_user_id}",
            axum::routing::delete(delete_dek),
        )
        .route(
            "/api/vault/migration/clear-car/{id}",
            post(migration_clear_car),
        )
        .route("/api/vault/jobs", post(create_job))
        .route("/api/vault/jobs/{id}", get(get_job))
}

// --- helpers ----------------------------------------------------------------

pub async fn user_vault_status(pool: &sqlx::PgPool, user_id: Uuid) -> AppResult<String> {
    let status = sqlx::query_scalar::<_, String>(
        "SELECT vault_status FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| "disabled".into());
    Ok(status)
}

pub async fn owner_vault_active(pool: &sqlx::PgPool, owner_user_id: Uuid) -> AppResult<bool> {
    let active = sqlx::query_scalar::<_, bool>(
        "SELECT vault_status = 'active' FROM users WHERE id = $1",
    )
    .bind(owner_user_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(false);
    Ok(active)
}

fn decode_b64(label: &str, s: &str) -> AppResult<Vec<u8>> {
    B64.decode(s.trim())
        .map_err(|_| AppError::BadRequest(format!("invalid base64 for {label}")))
}

fn encode_b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

fn validate_object_type(t: &str) -> AppResult<()> {
    if t.is_empty() || t.len() > MAX_OBJECT_TYPE_LEN {
        return Err(AppError::BadRequest("invalid object_type".into()));
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(AppError::BadRequest("invalid object_type".into()));
    }
    Ok(())
}

// --- status / enable / activate ---------------------------------------------

#[derive(Debug, Serialize)]
pub struct VaultStatusResponse {
    pub vault_enabled: bool,
    pub vault_status: String,
    pub vault_identity_version: i32,
    pub vault_identity_pubkey_b64: Option<String>,
    pub vault_ui_enabled: bool,
    pub owned_cars: i64,
    pub cars_with_owner_dek: i64,
    pub vault_object_count: i64,
}

async fn vault_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<VaultStatusResponse>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        vault_enabled: bool,
        vault_status: String,
        vault_identity_version: i32,
        vault_identity_pubkey: Option<Vec<u8>>,
    }

    let row = sqlx::query_as::<_, Row>(
        r#"
        SELECT vault_enabled, vault_status, vault_identity_version, vault_identity_pubkey
        FROM users WHERE id = $1
        "#,
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    let owned_cars = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM cars WHERE owner_user_id = $1")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?;

    let cars_with_owner_dek = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*) FROM vault_car_deks d
        JOIN cars c ON c.id = d.car_id
        WHERE c.owner_user_id = $1 AND d.recipient_user_id = $1
        "#,
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    let vault_object_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*) FROM vault_objects o
        JOIN cars c ON c.id = o.car_id
        WHERE c.owner_user_id = $1
        "#,
    )
    .bind(user.id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(VaultStatusResponse {
        vault_enabled: row.vault_enabled,
        vault_status: row.vault_status,
        vault_identity_version: row.vault_identity_version,
        vault_identity_pubkey_b64: row.vault_identity_pubkey.as_deref().map(encode_b64),
        vault_ui_enabled: state.config.vault_ui_enabled,
        owned_cars,
        cars_with_owner_dek,
        vault_object_count,
    }))
}

#[derive(Debug, Deserialize)]
pub struct VaultEnableRequest {
    /// Base64-encoded 32-byte X25519 public key.
    pub identity_pubkey: String,
    pub identity_version: Option<i32>,
}

async fn vault_enable(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<VaultEnableRequest>,
) -> AppResult<Json<VaultStatusResponse>> {
    if !state.config.vault_ui_enabled {
        return Err(AppError::Forbidden);
    }

    let pk = decode_b64("identity_pubkey", &body.identity_pubkey)?;
    if pk.len() != MAX_PUBKEY_LEN {
        return Err(AppError::BadRequest(
            "identity_pubkey must be 32 bytes".into(),
        ));
    }
    let version = body.identity_version.unwrap_or(1).max(1);

    let current = user_vault_status(&state.pool, user.id).await?;
    if current != "disabled" {
        return Err(AppError::Conflict(format!(
            "vault already {current}; cannot enable again"
        )));
    }

    let res = sqlx::query(
        r#"
        UPDATE users
        SET vault_status = 'migrating',
            vault_identity_pubkey = $2,
            vault_identity_version = $3,
            vault_created_at = NOW(),
            vault_enabled = FALSE
        WHERE id = $1 AND vault_status = 'disabled'
        "#,
    )
    .bind(user.id)
    .bind(&pk)
    .bind(version)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::Conflict("vault enable race".into()));
    }

    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: audit::actions::VAULT_ENABLED,
            resource_type: Some("user"),
            resource_id: Some(&user.id.to_string()),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "identity_version": version }),
        },
    )
    .await;

    vault_status(State(state), user).await
}

async fn vault_activate(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<VaultStatusResponse>> {
    let current = user_vault_status(&state.pool, user.id).await?;
    if current != "migrating" {
        return Err(AppError::Conflict(format!(
            "vault must be migrating to activate (is {current})"
        )));
    }

    sqlx::query(
        r#"
        UPDATE users
        SET vault_status = 'active', vault_enabled = TRUE
        WHERE id = $1 AND vault_status = 'migrating'
        "#,
    )
    .bind(user.id)
    .execute(&state.pool)
    .await?;

    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: audit::actions::VAULT_ACTIVATED,
            resource_type: Some("user"),
            resource_id: Some(&user.id.to_string()),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({}),
        },
    )
    .await;

    vault_status(State(state), user).await
}

// --- objects ----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PutObjectRequest {
    pub car_id: Uuid,
    pub object_type: String,
    pub logical_id: Uuid,
    pub chunk_index: Option<i32>,
    pub schema_version: Option<i32>,
    pub nonce: String,
    pub ciphertext: String,
    pub content_version: Option<i32>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ObjectRow {
    id: Uuid,
    car_id: Uuid,
    object_type: String,
    logical_id: Uuid,
    chunk_index: Option<i32>,
    schema_version: i32,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    byte_size: i32,
    content_version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct VaultObjectResponse {
    pub id: Uuid,
    pub car_id: Uuid,
    pub object_type: String,
    pub logical_id: Uuid,
    pub chunk_index: Option<i32>,
    pub schema_version: i32,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
    pub byte_size: i32,
    pub content_version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ObjectRow> for VaultObjectResponse {
    fn from(r: ObjectRow) -> Self {
        Self {
            id: r.id,
            car_id: r.car_id,
            object_type: r.object_type,
            logical_id: r.logical_id,
            chunk_index: r.chunk_index,
            schema_version: r.schema_version,
            nonce_b64: encode_b64(&r.nonce),
            ciphertext_b64: encode_b64(&r.ciphertext),
            byte_size: r.byte_size,
            content_version: r.content_version,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

async fn put_object(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<PutObjectRequest>,
) -> AppResult<Json<VaultObjectResponse>> {
    can_edit_car(&state.pool, user.id, body.car_id).await?;
    validate_object_type(&body.object_type)?;

    let nonce = decode_b64("nonce", &body.nonce)?;
    let ciphertext = decode_b64("ciphertext", &body.ciphertext)?;
    if nonce.len() != 12 {
        return Err(AppError::BadRequest("nonce must be 12 bytes".into()));
    }
    if ciphertext.is_empty() {
        return Err(AppError::BadRequest("ciphertext empty".into()));
    }
    if ciphertext.len() > state.config.vault_max_object_bytes {
        return Err(AppError::BadRequest(format!(
            "ciphertext exceeds max {} bytes",
            state.config.vault_max_object_bytes
        )));
    }

    let schema_version = body.schema_version.unwrap_or(1);
    let content_version = body.content_version.unwrap_or(1);
    let byte_size = ciphertext.len() as i32;
    let id = Uuid::new_v4();

    // Upsert: try update existing identity, else insert.
    let existing = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id FROM vault_objects
        WHERE car_id = $1 AND object_type = $2 AND logical_id = $3
          AND chunk_index IS NOT DISTINCT FROM $4
        "#,
    )
    .bind(body.car_id)
    .bind(&body.object_type)
    .bind(body.logical_id)
    .bind(body.chunk_index)
    .fetch_optional(&state.pool)
    .await?;

    let row = if let Some(existing_id) = existing {
        sqlx::query_as::<_, ObjectRow>(
            r#"
            UPDATE vault_objects
            SET schema_version = $2,
                nonce = $3,
                ciphertext = $4,
                byte_size = $5,
                content_version = $6,
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, car_id, object_type, logical_id, chunk_index, schema_version,
                      nonce, ciphertext, byte_size, content_version, created_at, updated_at
            "#,
        )
        .bind(existing_id)
        .bind(schema_version)
        .bind(&nonce)
        .bind(&ciphertext)
        .bind(byte_size)
        .bind(content_version)
        .fetch_one(&state.pool)
        .await?
    } else {
        sqlx::query_as::<_, ObjectRow>(
            r#"
            INSERT INTO vault_objects (
                id, car_id, object_type, logical_id, chunk_index, schema_version,
                nonce, ciphertext, byte_size, content_version
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            RETURNING id, car_id, object_type, logical_id, chunk_index, schema_version,
                      nonce, ciphertext, byte_size, content_version, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(body.car_id)
        .bind(&body.object_type)
        .bind(body.logical_id)
        .bind(body.chunk_index)
        .bind(schema_version)
        .bind(&nonce)
        .bind(&ciphertext)
        .bind(byte_size)
        .bind(content_version)
        .fetch_one(&state.pool)
        .await?
    };

    Ok(Json(row.into()))
}

#[derive(Debug, Deserialize)]
pub struct GetObjectsQuery {
    pub car_id: Uuid,
    pub object_type: Option<String>,
    pub logical_id: Option<Uuid>,
    pub chunk_index: Option<i32>,
}

async fn get_objects(
    State(state): State<AppState>,
    user: AuthUser,
    Query(q): Query<GetObjectsQuery>,
) -> AppResult<Json<Vec<VaultObjectResponse>>> {
    can_read_car(&state.pool, user.id, q.car_id).await?;

    let rows = sqlx::query_as::<_, ObjectRow>(
        r#"
        SELECT id, car_id, object_type, logical_id, chunk_index, schema_version,
               nonce, ciphertext, byte_size, content_version, created_at, updated_at
        FROM vault_objects
        WHERE car_id = $1
          AND ($2::text IS NULL OR object_type = $2)
          AND ($3::uuid IS NULL OR logical_id = $3)
          AND ($4::int IS NULL OR chunk_index IS NOT DISTINCT FROM $4)
        ORDER BY object_type, logical_id, chunk_index NULLS FIRST
        LIMIT 500
        "#,
    )
    .bind(q.car_id)
    .bind(q.object_type.as_deref())
    .bind(q.logical_id)
    .bind(q.chunk_index)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(rows.into_iter().map(VaultObjectResponse::from).collect()))
}

// --- DEK wraps --------------------------------------------------------------

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DekRow {
    car_id: Uuid,
    recipient_user_id: Uuid,
    wrapped_dek: Vec<u8>,
    wrap_alg: String,
    identity_version: i32,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct DekWrapResponse {
    pub car_id: Uuid,
    pub recipient_user_id: Uuid,
    pub wrapped_dek_b64: String,
    pub wrap_alg: String,
    pub identity_version: i32,
    pub created_at: DateTime<Utc>,
}

impl From<DekRow> for DekWrapResponse {
    fn from(r: DekRow) -> Self {
        Self {
            car_id: r.car_id,
            recipient_user_id: r.recipient_user_id,
            wrapped_dek_b64: encode_b64(&r.wrapped_dek),
            wrap_alg: r.wrap_alg,
            identity_version: r.identity_version,
            created_at: r.created_at,
        }
    }
}

async fn list_deks(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<Vec<DekWrapResponse>>> {
    let access = can_read_car(&state.pool, user.id, car_id).await?;
    let rows = if matches!(access, crate::shares::access::CarAccess::Owner) {
        sqlx::query_as::<_, DekRow>(
            r#"
            SELECT car_id, recipient_user_id, wrapped_dek, wrap_alg, identity_version, created_at
            FROM vault_car_deks WHERE car_id = $1
            "#,
        )
        .bind(car_id)
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as::<_, DekRow>(
            r#"
            SELECT car_id, recipient_user_id, wrapped_dek, wrap_alg, identity_version, created_at
            FROM vault_car_deks WHERE car_id = $1 AND recipient_user_id = $2
            "#,
        )
        .bind(car_id)
        .bind(user.id)
        .fetch_all(&state.pool)
        .await?
    };
    Ok(Json(rows.into_iter().map(DekWrapResponse::from).collect()))
}

#[derive(Debug, Deserialize)]
pub struct UpsertDekRequest {
    pub recipient_user_id: Uuid,
    pub wrapped_dek: String,
    pub wrap_alg: String,
    pub identity_version: Option<i32>,
}

async fn upsert_dek(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
    Json(body): Json<UpsertDekRequest>,
) -> AppResult<Json<DekWrapResponse>> {
    require_owner(&state.pool, user.id, car_id).await?;
    let wrapped = decode_b64("wrapped_dek", &body.wrapped_dek)?;
    if wrapped.len() < 32 + 12 + 16 {
        return Err(AppError::BadRequest("wrapped_dek too short".into()));
    }
    if body.wrap_alg.is_empty() || body.wrap_alg.len() > 128 {
        return Err(AppError::BadRequest("invalid wrap_alg".into()));
    }
    let identity_version = body.identity_version.unwrap_or(1);

    let row = sqlx::query_as::<_, DekRow>(
        r#"
        INSERT INTO vault_car_deks (car_id, recipient_user_id, wrapped_dek, wrap_alg, identity_version)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (car_id, recipient_user_id) DO UPDATE SET
            wrapped_dek = EXCLUDED.wrapped_dek,
            wrap_alg = EXCLUDED.wrap_alg,
            identity_version = EXCLUDED.identity_version
        RETURNING car_id, recipient_user_id, wrapped_dek, wrap_alg, identity_version, created_at
        "#,
    )
    .bind(car_id)
    .bind(body.recipient_user_id)
    .bind(&wrapped)
    .bind(&body.wrap_alg)
    .bind(identity_version)
    .fetch_one(&state.pool)
    .await?;

    let rid = body.recipient_user_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: audit::actions::VAULT_WRAP_ADDED,
            resource_type: Some("car"),
            resource_id: Some(&car_id.to_string()),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "recipient_user_id": rid }),
        },
    )
    .await;

    Ok(Json(row.into()))
}

async fn delete_dek(
    State(state): State<AppState>,
    user: AuthUser,
    Path((car_id, recipient_user_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    require_owner(&state.pool, user.id, car_id).await?;
    let res = sqlx::query(
        "DELETE FROM vault_car_deks WHERE car_id = $1 AND recipient_user_id = $2",
    )
    .bind(car_id)
    .bind(recipient_user_id)
    .execute(&state.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    let rid = recipient_user_id.to_string();
    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: audit::actions::VAULT_WRAP_REMOVED,
            resource_type: Some("car"),
            resource_id: Some(&car_id.to_string()),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "recipient_user_id": rid }),
        },
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

// --- migration clear --------------------------------------------------------

async fn migration_clear_car(
    State(state): State<AppState>,
    user: AuthUser,
    Path(car_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    require_owner(&state.pool, user.id, car_id).await?;
    let status = user_vault_status(&state.pool, user.id).await?;
    if status != "migrating" {
        return Err(AppError::Conflict(
            "clear-car only allowed while vault_status=migrating".into(),
        ));
    }

    // Clear sensitive car columns (keep id / ownership / timestamps).
    sqlx::query(
        r#"
        UPDATE cars SET
            name = '',
            make_model = '',
            notes = NULL,
            photo_path = NULL,
            updated_at = NOW()
        WHERE id = $1 AND owner_user_id = $2
        "#,
    )
    .bind(car_id)
    .bind(user.id)
    .execute(&state.pool)
    .await?;

    // Drop plaintext points for this car's tracks.
    sqlx::query(
        r#"
        DELETE FROM track_points
        WHERE track_id IN (SELECT id FROM tracks WHERE car_id = $1)
        "#,
    )
    .bind(car_id)
    .execute(&state.pool)
    .await?;

    // Clear plaintext analysis reports if column exists (analysis migration).
    let _ = sqlx::query(
        r#"
        UPDATE tracks SET analysis_report = NULL
        WHERE car_id = $1 AND analysis_report IS NOT NULL
        "#,
    )
    .bind(car_id)
    .execute(&state.pool)
    .await;

    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: audit::actions::VAULT_MIGRATION_CLEAR_CAR,
            resource_type: Some("car"),
            resource_id: Some(&car_id.to_string()),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({}),
        },
    )
    .await;

    Ok(Json(serde_json::json!({ "ok": true, "car_id": car_id })))
}

// --- ephemeral job bundles (AI / route-opt) ----------------------------------

#[derive(Debug, Deserialize)]
pub struct VaultJobRequest {
    /// `ai_analysis` | `route_opt`
    pub kind: String,
    /// Client-prepared plaintext JSON bundle (ephemeral; not stored in DB).
    pub bundle: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct VaultJobResponse {
    pub id: Uuid,
    pub kind: String,
    pub status: String,
    pub error: Option<String>,
    /// Present when status=done; client must seal into vault_objects.
    pub result: Option<serde_json::Value>,
}

async fn create_job(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<VaultJobRequest>,
) -> AppResult<Json<VaultJobResponse>> {
    let kind = body.kind.trim().to_ascii_lowercase();
    if kind != "ai_analysis" && kind != "route_opt" {
        return Err(AppError::BadRequest(
            "kind must be ai_analysis or route_opt".into(),
        ));
    }
    let status = user_vault_status(&state.pool, user.id).await?;
    if status != "active" {
        return Err(AppError::Conflict(
            "vault jobs require active vault; use legacy endpoints when not vaulted".into(),
        ));
    }

    // Soft size guard on ephemeral bundle (not durable).
    let bundle_bytes = serde_json::to_vec(&body.bundle).unwrap_or_default();
    if bundle_bytes.len() > state.config.vault_max_object_bytes {
        return Err(AppError::BadRequest("job bundle too large".into()));
    }

    let id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO vault_jobs (id, user_id, kind, status)
        VALUES ($1, $2, $3, 'running')
        "#,
    )
    .bind(id)
    .bind(user.id)
    .bind(&kind)
    .execute(&state.pool)
    .await?;

    audit::record(
        &state.pool,
        AuditEvent {
            user_id: Some(user.id),
            actor_session_id: Some(user.session_id.as_str()),
            action: audit::actions::VAULT_JOB_SUBMITTED,
            resource_type: Some("vault_job"),
            resource_id: Some(&id.to_string()),
            ip: None,
            user_agent: None,
            meta: serde_json::json!({ "kind": kind }),
        },
    )
    .await;

    let job_result = run_vault_job(&state, user.id, &kind, body.bundle).await;

    let (status_s, error, result) = match job_result {
        Ok(value) => ("done".to_string(), None, Some(value)),
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!(job_id = %id, error = %msg, "vault job failed");
            ("failed".to_string(), Some(msg.chars().take(500).collect()), None)
        }
    };

    sqlx::query(
        r#"
        UPDATE vault_jobs
        SET status = $2, error = $3, finished_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(&status_s)
    .bind(&error)
    .execute(&state.pool)
    .await?;

    // Result returned only on this response (ephemeral). Poll has status/error only.
    Ok(Json(VaultJobResponse {
        id,
        kind,
        status: status_s,
        error,
        result,
    }))
}

/// Execute AI / route-opt on client-prepared ephemeral plaintext. Never persists payload.
async fn run_vault_job(
    state: &AppState,
    user_id: Uuid,
    kind: &str,
    bundle: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    match kind {
        "ai_analysis" => run_vault_ai_job(state, user_id, bundle).await,
        "route_opt" => {
            // Route-opt still needs ORS + historical geometries; accept acknowledgment
            // when client sends a sealed plan body for round-trip, else explain.
            if let Some(plan) = bundle.get("client_plan").cloned() {
                return Ok(serde_json::json!({
                    "kind": "route_opt",
                    "plan": plan,
                    "note": "Echo client_plan for seal; full ORS recompute from vault points is client-assisted.",
                }));
            }
            Err(AppError::BadRequest(
                "route_opt bundle requires client_plan (or use non-vault path)".into(),
            ))
        }
        _ => Err(AppError::BadRequest("unknown job kind".into())),
    }
}

async fn run_vault_ai_job(
    state: &AppState,
    user_id: Uuid,
    bundle: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let ctx_val = bundle
        .get("context")
        .cloned()
        .ok_or_else(|| AppError::BadRequest("ai_analysis bundle requires context".into()))?;
    let ctx: ai::TripAnalysisContext = serde_json::from_value(ctx_val).map_err(|e| {
        AppError::BadRequest(format!("invalid analysis context: {e}"))
    })?;

    let creds = sqlx::query(
        r#"
        SELECT openrouter_api_key_enc, openrouter_api_key_nonce, openrouter_key_version,
               openrouter_model
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;

    use sqlx::Row;
    let enc: Option<Vec<u8>> = creds.try_get("openrouter_api_key_enc").ok().flatten();
    let nonce: Option<Vec<u8>> = creds.try_get("openrouter_api_key_nonce").ok().flatten();
    let version: i32 = creds.try_get("openrouter_key_version").unwrap_or(1);
    let model: String = creds
        .try_get::<String, _>("openrouter_model")
        .unwrap_or_else(|_| "anthropic/claude-3.7-sonnet".into());

    let (Some(enc), Some(nonce)) = (enc, nonce) else {
        return Err(AppError::BadRequest(
            "Configure your OpenRouter API key in Settings before analyzing trips".into(),
        ));
    };
    let api_key = crate::crypto::decrypt_secret_versioned(&nonce, &enc, version, &state.keyring)
        .map_err(|_| AppError::BadRequest("Could not decrypt OpenRouter API key".into()))?;
    if api_key.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Configure your OpenRouter API key in Settings before analyzing trips".into(),
        ));
    }

    let report = ai::analyze_trip(&api_key, &model, ctx)
        .await
        .map_err(|e| AppError::BadRequest(format!("analysis failed: {e}")))?;

    serde_json::to_value(&report).map_err(|e| AppError::Internal(e.to_string()))
}

async fn get_job(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<VaultJobResponse>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        kind: String,
        status: String,
        error: Option<String>,
    }
    let row = sqlx::query_as::<_, Row>(
        r#"
        SELECT id, kind, status, error FROM vault_jobs
        WHERE id = $1 AND user_id = $2
        "#,
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(Json(VaultJobResponse {
        id: row.id,
        kind: row.kind,
        status: row.status,
        error: row.error,
        result: None, // result only returned on create in v1 sync path
    }))
}
