use std::{collections::HashMap, time::Duration};

use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

const DEFAULT_LSTM_URL: &str = "https://matimorales01--lstm-trend-model-main.modal.run";
const DEFAULT_XGBOOST_URL: &str = "https://matimorales01--xgboost-trend-model-main.modal.run";
const DEFAULT_ARIMA_URL: &str = "https://matimorales01--arima-model-main.modal.run";

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
    pub backtest: Option<Value>,
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
    let arima_url = std::env::var("MODAL_ARIMA_URL").unwrap_or_else(|_| DEFAULT_ARIMA_URL.into());

    let (lstm, xgboost, arima) = tokio::join!(
        fetch_modal(&client, "lstm-modal", &lstm_url, &ticker),
        fetch_modal(&client, "xgboost-modal", &xgboost_url, &ticker),
        fetch_arima(&client, &arima_url, &ticker),
    );

    if !is_available(&lstm) && !is_available(&xgboost) && !is_available(&arima) {
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
            "predictions": {
                "lstm-modal": lstm,
                "xgboost-modal": xgboost,
                "arima-modal": arima
            }
        })),
    )
}

async fn fetch_arima(client: &reqwest::Client, url: &str, ticker: &str) -> Value {
    match client
        .get(url)
        .query(&[
            ("ticker", ticker),
            ("predictions", "5"),
            ("media_movil", "20"),
        ])
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) if body.get("error").is_none() => {
                let last_close = body.get("valor_actual").and_then(Value::as_f64);
                let predicted_close = body
                    .get("prediction")
                    .and_then(Value::as_array)
                    .and_then(|values| values.last())
                    .and_then(Value::as_f64);
                match (last_close, predicted_close) {
                    (Some(last), Some(predicted)) => {
                        let change = predicted / last - 1.0;
                        let signal = if change > 0.01 {
                            "alza"
                        } else if change < -0.01 {
                            "baja"
                        } else {
                            "neutral"
                        };
                        json!({
                            "available": true,
                            "signal": signal,
                            "condition": null,
                            "rsi": null,
                            "horizon_days": 5,
                            "last_close": last,
                            "predicted_close": predicted,
                            "as_of": null,
                            "model": "arima-modal",
                            "model_version": body.get("model_version"),
                            "backtest": body.get("backtest"),
                            "reason": null
                        })
                    }
                    _ => unavailable("ARIMA no devolvio precios validos"),
                }
            }
            Ok(body) => unavailable(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("ARIMA devolvio una respuesta invalida"),
            ),
            Err(error) => unavailable(&format!("Respuesta invalida de ARIMA: {error}")),
        },
        Ok(response) => unavailable(&format!("ARIMA respondio HTTP {}", response.status())),
        Err(error) => unavailable(&format!("No se pudo contactar a ARIMA: {error}")),
    }
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
        "backtest": null,
        "reason": reason
    })
}

fn is_available(value: &Value) -> bool {
    value.get("available").and_then(Value::as_bool) == Some(true)
}
