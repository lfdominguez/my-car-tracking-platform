use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub async fn connect(database_url: &str) -> Result<PgPool, DbError> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> Result<(), DbError> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}

/// Ensure PostGIS is available (extension may require superuser on first boot).
pub async fn ensure_postgis(pool: &PgPool) -> Result<(), DbError> {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS postgis")
        .execute(pool)
        .await?;
    Ok(())
}

/// Ensure TimescaleDB is available (extension may require superuser on first boot).
pub async fn ensure_timescaledb(pool: &PgPool) -> Result<(), DbError> {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS timescaledb")
        .execute(pool)
        .await?;
    Ok(())
}
