use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use sqlx::PgPool;

const DEFAULT_SYNC_INTERVAL_HOURS: u64 = 24;
const SYNC_RETRY_SECONDS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncResult {
    pub added: usize,
    pub total: usize,
}

/// Trae los tickers disponibles de data-colector y agrega a `shares` los que
/// falten. Es idempotente: los que ya existen no se tocan.
pub async fn sync_tickers(pool: &PgPool) -> Result<SyncResult, String> {
    let data_collector_url = std::env::var("DATA_COLLECTOR_URL")
        .map_err(|_| "DATA_COLLECTOR_URL no esta configurado.".to_string())?;

    let response = reqwest::Client::new()
        .post(format!("{}/available-tickers", data_collector_url))
        .send()
        .await
        .map_err(|err| {
            tracing::error!("Failed to reach data collector: {}", err);
            "Could not reach data collector.".to_string()
        })?;

    let body: serde_json::Value = response.json().await.map_err(|err| {
        tracing::error!("Failed to parse data collector response: {}", err);
        "Failed to parse data collector response.".to_string()
    })?;

    let tickers: Vec<String> = body["message"]["tickers"]
        .as_array()
        .ok_or_else(|| "Unexpected response format from data collector.".to_string())?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let result = sqlx::query(
        r#"INSERT INTO shares (ticker) SELECT UNNEST($1::text[]) ON CONFLICT (ticker) DO NOTHING"#,
    )
    .bind(&tickers)
    .execute(pool)
    .await
    .map_err(|err| {
        tracing::error!("Failed to insert tickers: {}", err);
        "Failed to store tickers.".to_string()
    })?;

    Ok(SyncResult {
        added: result.rows_affected() as usize,
        total: tickers.len(),
    })
}

/// Arranca el job en background que sincroniza el catalogo de tickers apenas
/// levanta el back (asi un entorno nuevo no necesita pegarle a mano a
/// `/shares/update`) y despues cada `SHARES_SYNC_INTERVAL_HOURS` (24 por
/// default), para que aparezcan los tickers nuevos de data-colector. Si
/// data-colector no responde (p. ej. dormido en Render), reintenta a los
/// pocos minutos sin bloquear el arranque.
pub fn spawn_ticker_sync_job(pool: PgPool) {
    let interval_hours: u64 = std::env::var("SHARES_SYNC_INTERVAL_HOURS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SYNC_INTERVAL_HOURS);

    tokio::spawn(async move {
        loop {
            let wait = match sync_tickers(&pool).await {
                Ok(result) => {
                    tracing::info!(
                        "[Shares Sync] Catalogo sincronizado: {} nuevos de {} tickers",
                        result.added,
                        result.total
                    );
                    Duration::from_secs(interval_hours * 3600)
                }
                Err(err) => {
                    tracing::warn!(
                        "[Shares Sync] Fallo la sincronizacion ({}), reintentando en {} s",
                        err,
                        SYNC_RETRY_SECONDS
                    );
                    Duration::from_secs(SYNC_RETRY_SECONDS)
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

#[utoipa::path(
    get,
    path = "/shares/update",
    responses(
        (status = 200, description = "Tickers synced successfully", example = json!({
            "added": 3,
            "total": 10
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Share"
)]
pub async fn handler(State(pool): State<PgPool>) -> impl IntoResponse {
    match sync_tickers(&pool).await {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({ "added": result.added, "total": result.total })),
        ),
        Err(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "code": 500, "message": message })),
        ),
    }
}
