use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct ShareTrendItem {
    pub ticker: String,
    pub available: bool,
    pub signal: Option<String>,
    pub condition: Option<String>,
    pub rsi: Option<f64>,
    pub horizon_days: Option<i64>,
    pub last_close: Option<f64>,
    pub predicted_close: Option<f64>,
    pub as_of: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ListTrendsResponse {
    pub trends: Vec<ShareTrendItem>,
}

#[utoipa::path(
    get,
    path = "/user/shares/trends",
    responses(
        (status = 200, description = "Trend prediction (api-ml) for each stock declared by the authenticated user", body = ListTrendsResponse, example = json!({
            "trends": [
                {
                    "ticker": "GGAL",
                    "available": true,
                    "signal": "alza",
                    "condition": "neutral",
                    "rsi": 58.3,
                    "horizon_days": 5,
                    "last_close": 8365.0,
                    "predicted_close": 8511.82,
                    "as_of": "2026-06-17",
                    "model": "lstm",
                    "model_version": "lstm-20260618T003942Z",
                    "reason": null
                },
                {
                    "ticker": "YPFD",
                    "available": false,
                    "signal": null,
                    "condition": null,
                    "rsi": null,
                    "horizon_days": null,
                    "last_close": null,
                    "predicted_close": null,
                    "as_of": null,
                    "model": null,
                    "model_version": null,
                    "reason": "No se pudo contactar a api-ml"
                }
            ]
        })),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    let tickers = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT DISTINCT s.ticker
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.user_id = $1
        ORDER BY s.ticker ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    let tickers = match tickers {
        Ok(rows) => rows.into_iter().map(|(ticker,)| ticker).collect::<Vec<_>>(),
        Err(err) => {
            tracing::error!("Failed to list tickers for trends: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
    };

    let mut trends = Vec::with_capacity(tickers.len());
    if !tickers.is_empty() {
        let api_ml_url = match std::env::var("API_ML_URL") {
            Ok(url) => url,
            Err(_) => {
                tracing::error!("API_ML_URL no esta configurada");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "code": 500,
                        "message": "An unexpected error occurred. Please try again later."
                    })),
                );
            }
        };
        let client = reqwest::Client::new();
        for ticker in tickers {
            trends.push(fetch_trend(&client, &api_ml_url, &ticker).await);
        }
    }

    (StatusCode::OK, Json(json!({ "trends": trends })))
}

/// Pide la tendencia de un ticker a api-ml. Si falla (caido, timeout, ticker
/// sin modelo entrenado, etc.) no corta el resto: devuelve un item marcado
/// como no disponible en vez de tirar 500 para todos los demas tickers.
async fn fetch_trend(client: &reqwest::Client, api_ml_url: &str, ticker: &str) -> Value {
    let response = client
        .get(format!(
            "{}/predict/trend/{}",
            api_ml_url.trim_end_matches('/'),
            ticker
        ))
        .send()
        .await;

    match response {
        Ok(res) if res.status().is_success() => match res.json::<Value>().await {
            Ok(body) => json!({
                "ticker": ticker,
                "available": true,
                "signal": body.get("signal"),
                "condition": body.get("condition"),
                "rsi": body.get("rsi"),
                "horizon_days": body.get("horizon_days"),
                "last_close": body.get("last_close"),
                "predicted_close": body.get("predicted_close"),
                "as_of": body.get("as_of"),
                "model": body.get("model"),
                "model_version": body.get("model_version"),
                "reason": Value::Null,
            }),
            Err(err) => {
                tracing::error!("Respuesta invalida de api-ml para {}: {}", ticker, err);
                unavailable(ticker, "Respuesta invalida de api-ml")
            }
        },
        Ok(res) => {
            tracing::warn!("api-ml respondio {} para {}", res.status(), ticker);
            unavailable(ticker, "api-ml no pudo predecir para este ticker")
        }
        Err(err) => {
            tracing::error!("No se pudo contactar a api-ml para {}: {}", ticker, err);
            unavailable(ticker, "No se pudo contactar a api-ml")
        }
    }
}

fn unavailable(ticker: &str, reason: &str) -> Value {
    json!({
        "ticker": ticker,
        "available": false,
        "signal": Value::Null,
        "condition": Value::Null,
        "rsi": Value::Null,
        "horizon_days": Value::Null,
        "last_close": Value::Null,
        "predicted_close": Value::Null,
        "as_of": Value::Null,
        "model": Value::Null,
        "model_version": Value::Null,
        "reason": reason,
    })
}
