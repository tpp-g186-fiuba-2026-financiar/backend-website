use std::{collections::HashMap, time::Duration};

use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

const DEFAULT_LSTM_URL: &str = "https://matimorales01--lstm-trend-model-main.modal.run";
const DEFAULT_XGBOOST_URL: &str = "https://matimorales01--xgboost-trend-model-main.modal.run";

#[derive(Serialize, ToSchema)]
pub struct ModelPredictionItem {
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
pub struct CompareTrendsResponse {
    pub symbol: String,
    pub as_of: Option<String>,
    pub default_model: Option<String>,
    pub predictions: HashMap<String, ModelPredictionItem>,
}

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/trends/compare",
    params(("ticker" = String, Path, description = "Ticker a comparar (ej: GGAL)")),
    responses(
        (status = 200, description = "Compara los modelos productivos de tendencia desplegados en Modal", body = CompareTrendsResponse),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 502, description = "Ningun modelo de Modal pudo responder")
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("reqwest client");
    let lstm_url = std::env::var("MODAL_LSTM_URL").unwrap_or_else(|_| DEFAULT_LSTM_URL.into());
    let xgboost_url =
        std::env::var("MODAL_XGBOOST_URL").unwrap_or_else(|_| DEFAULT_XGBOOST_URL.into());

    let (lstm, xgboost) = tokio::join!(
        fetch_modal(&client, "lstm-modal", &lstm_url, &ticker),
        fetch_modal(&client, "xgboost-modal", &xgboost_url, &ticker),
    );

    if !is_available(&lstm) && !is_available(&xgboost) {
        tracing::error!("Ningun modelo Modal pudo comparar {}", ticker);
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"code": 502, "message": "Los modelos de Modal no pudieron responder"})),
        );
    }

    let as_of = lstm
        .get("as_of")
        .or_else(|| xgboost.get("as_of"))
        .cloned()
        .unwrap_or(Value::Null);
    (
        StatusCode::OK,
        Json(json!({
            "symbol": ticker,
            "as_of": as_of,
            "default_model": "lstm-modal",
            "predictions": {"lstm-modal": lstm, "xgboost-modal": xgboost}
        })),
    )
}

async fn fetch_modal(client: &reqwest::Client, name: &str, url: &str, ticker: &str) -> Value {
    match client
        .get(url)
        .query(&[("ticker", ticker), ("horizon", "5")])
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(mut body) if body.get("error").is_none() => {
                if let Some(object) = body.as_object_mut() {
                    object.insert("available".into(), Value::Bool(true));
                    object.insert("model".into(), Value::String(name.into()));
                    object.insert("reason".into(), Value::Null);
                }
                body
            }
            Ok(body) => unavailable(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Modal devolvio una respuesta invalida"),
            ),
            Err(error) => unavailable(&format!("Respuesta invalida de Modal: {error}")),
        },
        Ok(response) => unavailable(&format!("Modal respondio HTTP {}", response.status())),
        Err(error) => unavailable(&format!("No se pudo contactar a Modal: {error}")),
    }
}

fn unavailable(reason: &str) -> Value {
    json!({
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
        "reason": reason
    })
}

fn is_available(value: &Value) -> bool {
    value.get("available").and_then(Value::as_bool) == Some(true)
}
