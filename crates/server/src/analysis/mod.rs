//! Trip AI analysis HTTP API + background worker.

mod context;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::shares::access::{can_read_car, require_owner};
use crate::state::AppState;
use crate::units::UnitSystem;

use self::context::build_trip_analysis_context;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/trips/{id}/analysis", get(get_analysis))
        .route("/api/trips/{id}/analyze", post(start_analysis))
}

/// Fail any in-flight jobs left from a previous process.
pub async fn fail_interrupted_jobs(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        r#"
        UPDATE tracks
        SET analysis_status = 'failed',
            analysis_error = 'interrupted by server restart'
        WHERE analysis_status IN ('pending', 'running')
        "#,
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

#[derive(Debug, Serialize)]
pub struct AnalyzeAccepted {
    pub analysis_status: String,
}

#[derive(Debug, Serialize)]
pub struct AnalysisResponse {
    pub analyzed: bool,
    pub analysis_status: String,
    pub analyzed_at: Option<DateTime<Utc>>,
    pub analysis_model: Option<String>,
    pub analysis_error: Option<String>,
    pub can_analyze: bool,
    pub report: Option<Value>,
}

async fn get_analysis(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<AnalysisResponse>> {
    let row = sqlx::query(
        r#"
        SELECT
            t.analysis_status,
            t.analyzed_at,
            t.analysis_model,
            t.analysis_error,
            t.analysis_report,
            c.owner_user_id
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        WHERE t.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let car_id = sqlx::query_scalar::<_, Uuid>("SELECT car_id FROM tracks WHERE id = $1")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    can_read_car(&state.pool, user.id, car_id).await?;

    let status: String = row.get("analysis_status");
    let owner_id: Uuid = row.get("owner_user_id");
    let report: Option<Value> = row.try_get("analysis_report").ok().flatten();
    let analyzed = status == "completed" || report.is_some();
    // Never expose internal job diagnostics (SQL/LLM stack traces) to the SPA.
    let raw_err: Option<String> = row.try_get("analysis_error").ok().flatten();
    let analysis_error = if raw_err.as_ref().is_some_and(|e| !e.trim().is_empty())
        || status == "failed"
    {
        Some("System Error".into())
    } else {
        None
    };

    Ok(Json(AnalysisResponse {
        analyzed,
        analysis_status: status,
        analyzed_at: row.try_get("analyzed_at").ok().flatten(),
        analysis_model: row.try_get("analysis_model").ok().flatten(),
        analysis_error,
        can_analyze: owner_id == user.id,
        report,
    }))
}

async fn start_analysis(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<(StatusCode, Json<AnalyzeAccepted>)> {
    let meta = sqlx::query(
        r#"
        SELECT t.car_id, c.owner_user_id, t.analysis_status
        FROM tracks t
        JOIN cars c ON c.id = t.car_id
        WHERE t.id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let car_id: Uuid = meta.get("car_id");
    let owner_id: Uuid = meta.get("owner_user_id");
    require_owner(&state.pool, user.id, car_id).await?;

    if crate::vault::owner_vault_active(&state.pool, owner_id).await? {
        return Err(AppError::Conflict(
            "Vault car: use POST /api/vault/jobs with a client-prepared analysis bundle".into(),
        ));
    }

    let status: String = meta.get("analysis_status");
    if status == "pending" || status == "running" {
        return Err(AppError::Conflict(
            "Analysis already in progress for this trip".into(),
        ));
    }

    // Load owner's OpenRouter credentials
    let creds = sqlx::query(
        r#"
        SELECT openrouter_api_key_enc, openrouter_api_key_nonce, openrouter_key_version,
               openrouter_model, unit_system
        FROM users WHERE id = $1
        "#,
    )
    .bind(owner_id)
    .fetch_one(&state.pool)
    .await?;

    let enc: Option<Vec<u8>> = creds.try_get("openrouter_api_key_enc").ok().flatten();
    let nonce: Option<Vec<u8>> = creds.try_get("openrouter_api_key_nonce").ok().flatten();
    let version: i32 = creds.try_get("openrouter_key_version").unwrap_or(1);
    let model: String = creds
        .try_get::<String, _>("openrouter_model")
        .unwrap_or_else(|_| "anthropic/claude-3.7-sonnet".into());
    let unit_raw: String = creds
        .try_get::<String, _>("unit_system")
        .unwrap_or_else(|_| "metric".into());
    let unit_system = UnitSystem::parse(&unit_raw).unwrap_or_default();

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

    let updated = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE tracks
        SET analysis_status = 'pending',
            analysis_error = NULL,
            analysis_started_at = NOW()
        WHERE id = $1
          AND analysis_status NOT IN ('pending', 'running')
        RETURNING id
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    if updated.is_none() {
        return Err(AppError::Conflict(
            "Analysis already in progress for this trip".into(),
        ));
    }

    let pool = state.pool.clone();
    let secrets_key = state.config.secrets_key.clone();
    // Re-encrypt not needed; pass plaintext only into task (in-memory)
    let model_owned = model.clone();
    let key_owned = api_key;

    tokio::spawn(async move {
        if let Err(e) = run_analysis_job(
            &pool,
            id,
            &key_owned,
            &model_owned,
            unit_system,
            &secrets_key,
        )
        .await
        {
            tracing::error!(track_id = %id, error = %e, "trip analysis job failed");
            let msg: String = e.chars().take(500).collect();
            let _ = sqlx::query(
                r#"
                UPDATE tracks
                SET analysis_status = 'failed',
                    analysis_error = $2
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(&msg)
            .execute(&pool)
            .await;
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(AnalyzeAccepted {
            analysis_status: "pending".into(),
        }),
    ))
}

async fn run_analysis_job(
    pool: &sqlx::PgPool,
    track_id: Uuid,
    api_key: &str,
    model: &str,
    unit_system: UnitSystem,
    _secrets_key: &str,
) -> Result<(), String> {
    sqlx::query(
        r#"
        UPDATE tracks
        SET analysis_status = 'running',
            analysis_started_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(track_id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    let ctx = build_trip_analysis_context(pool, track_id, unit_system)
        .await
        .map_err(|e| e.to_string())?;

    let report = ai::analyze_trip(api_key, model, ctx)
        .await
        .map_err(|e| e.to_string())?;

    let report_json = serde_json::to_value(&report).map_err(|e| e.to_string())?;

    sqlx::query(
        r#"
        UPDATE tracks
        SET analysis_status = 'completed',
            analysis_report = $2,
            analysis_model = $3,
            analyzed_at = NOW(),
            analysis_error = NULL
        WHERE id = $1
        "#,
    )
    .bind(track_id)
    .bind(&report_json)
    .bind(model)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    tracing::info!(%track_id, %model, "trip analysis completed");
    Ok(())
}

