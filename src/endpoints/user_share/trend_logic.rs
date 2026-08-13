use std::{
    collections::HashSet,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

const MAX_CONCURRENT_PREPARATIONS: usize = 2;
static PREPARING_TICKERS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

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
        (status = 200, description = "Trend prediction (Modal) for each stock declared by the authenticated user", body = ListTrendsResponse, example = json!({
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
        let modal_lstm_url = std::env::var("MODAL_LSTM_URL")
            .unwrap_or_else(|_| "https://matimorales01--lstm-trend-model-main.modal.run".into());
        // El proxy de Render puede cortar la request antes que un cold start de
        // Modal. Cortamos nosotros primero para responder 200 con cada ticker
        // en estado "preparando" y no perder CORS con un 502 del proxy.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("reqwest client");
        let mut requests = JoinSet::new();
        for ticker in tickers {
            let client = client.clone();
            let modal_lstm_url = modal_lstm_url.clone();
            requests.spawn(async move {
                let trend = fetch_trend(&client, &modal_lstm_url, &ticker).await;
                if needs_preparation(&trend) && prepare_models_in_background(&ticker) {
                    discover_ticker_for_training(&ticker);
                }
                trend
            });
        }
        while let Some(result) = requests.join_next().await {
            match result {
                Ok(trend) => trends.push(trend),
                Err(error) => tracing::error!("Fallo una consulta de tendencia: {}", error),
            }
        }
        trends.sort_by(|left, right| {
            left.get("ticker")
                .and_then(Value::as_str)
                .cmp(&right.get("ticker").and_then(Value::as_str))
        });
    }

    (StatusCode::OK, Json(json!({ "trends": trends })))
}

/// Dispara el bootstrap en Modal sin hacer esperar al request del usuario.
/// Los endpoints son idempotentes: si el artefacto ya existe no reentrenan.
fn prepare_models_in_background(ticker: &str) -> bool {
    let ticker = ticker.to_owned();
    {
        let mut preparing = PREPARING_TICKERS.lock().expect("preparing tickers lock");
        if preparing.contains(&ticker) || preparing.len() >= MAX_CONCURRENT_PREPARATIONS {
            return false;
        }
        preparing.insert(ticker.clone());
    }
    let lstm_url = std::env::var("MODAL_LSTM_PREPARE_URL")
        .unwrap_or_else(|_| "https://matimorales01--lstm-trend-model-prepare.modal.run".into());
    let xgboost_url = std::env::var("MODAL_XGBOOST_PREPARE_URL")
        .unwrap_or_else(|_| "https://matimorales01--xgboost-trend-model-prepare.modal.run".into());
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(7200))
            .build()
            .expect("reqwest client");
        let (lstm, xgboost) = tokio::join!(
            client.get(&lstm_url).query(&[("ticker", &ticker)]).send(),
            client
                .get(&xgboost_url)
                .query(&[("ticker", &ticker)])
                .send(),
        );
        for (model, result) in [("lstm", lstm), ("xgboost", xgboost)] {
            match result {
                Ok(response) if response.status().is_success() => {
                    tracing::info!("Bootstrap {} completado para {}", model, ticker);
                }
                Ok(response) => tracing::warn!(
                    "Bootstrap {} fallo para {}: HTTP {}",
                    model,
                    ticker,
                    response.status()
                ),
                Err(error) => {
                    tracing::warn!("Bootstrap {} fallo para {}: {}", model, ticker, error)
                }
            }
        }
        PREPARING_TICKERS
            .lock()
            .expect("preparing tickers lock")
            .remove(&ticker);
    });
    true
}

/// Si un ticker de la cartera aun no tiene artefacto, pide su historico en
/// background. data-colector lo incorpora al catalogo cuando tiene suficientes
/// ruedas y los cron diarios de Modal lo entrenan sin listas hardcodeadas.
fn discover_ticker_for_training(ticker: &str) {
    let ticker = ticker.to_owned();
    let collector = std::env::var("DATA_COLLECTOR_URL")
        .unwrap_or_else(|_| "https://data-colector.onrender.com".into());
    tokio::spawn(async move {
        let result = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .expect("reqwest client")
            .post(format!(
                "{}/historical-data/{}",
                collector.trim_end_matches('/'),
                ticker
            ))
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {
                tracing::info!("Historico solicitado para descubrir ticker {}", ticker);
            }
            Ok(response) => tracing::warn!(
                "No se pudo descubrir ticker {}: data-colector respondio {}",
                ticker,
                response.status()
            ),
            Err(error) => {
                tracing::warn!("No se pudo descubrir ticker {}: {}", ticker, error);
            }
        }
    });
}

/// Pide la tendencia de un ticker al LSTM productivo de Modal. Si falla (caido, timeout, ticker
/// sin modelo entrenado, etc.) no corta el resto: devuelve un item marcado
/// como no disponible en vez de tirar 500 para todos los demas tickers.
async fn fetch_trend(client: &reqwest::Client, modal_url: &str, ticker: &str) -> Value {
    let response = client
        .get(modal_url)
        .query(&[("ticker", ticker), ("horizon", "5")])
        .send()
        .await;

    match response {
        Ok(res) if res.status().is_success() => match res.json::<Value>().await {
            Ok(body) if body.get("error").is_none() => json!({
                "ticker": ticker,
                "available": true,
                "signal": body.get("signal"),
                "condition": body.get("condition"),
                "rsi": body.get("rsi"),
                "horizon_days": body.get("horizon_days"),
                "last_close": body.get("last_close"),
                "predicted_close": body.get("predicted_close"),
                "as_of": body.get("as_of"),
                "model": "lstm-modal",
                "model_version": body.get("model_version"),
                "reason": Value::Null,
            }),
            Ok(body) => unavailable(
                ticker,
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Modal devolvio una respuesta invalida"),
            ),
            Err(err) => {
                tracing::error!("Respuesta invalida de Modal para {}: {}", ticker, err);
                unavailable(ticker, "Respuesta invalida de Modal")
            }
        },
        Ok(res) => {
            tracing::warn!("Modal respondio {} para {}", res.status(), ticker);
            unavailable(ticker, "Modal no pudo predecir para este ticker")
        }
        Err(err) => {
            tracing::error!("No se pudo contactar a Modal para {}: {}", ticker, err);
            unavailable(ticker, "No se pudo contactar a Modal")
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

/// Un timeout o una caida de Modal no significa que falte entrenar. Solo se
/// inicia el bootstrap cuando el propio modelo confirma que no hay artefacto.
fn needs_preparation(trend: &Value) -> bool {
    trend
        .get("reason")
        .and_then(Value::as_str)
        .is_some_and(|reason| reason.contains("todavia no hay un modelo entrenado"))
}
