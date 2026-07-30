use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

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
    pub predictions: std::collections::HashMap<String, ModelPredictionItem>,
}

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/trends/compare",
    params(
        ("ticker" = String, Path, description = "Ticker a comparar (ej: GGAL)")
    ),
    responses(
        (status = 200, description = "Prediccion de tendencia de todos los modelos que api-ml corre en paralelo (lstm, xgboost, transformer, arima, y las variantes -modal si estan configuradas), lado a lado sobre el mismo historico", body = CompareTrendsResponse, example = json!({
            "symbol": "GGAL",
            "as_of": "2026-07-28",
            "default_model": "lstm",
            "predictions": {
                "lstm": {
                    "available": true,
                    "signal": "alza",
                    "condition": "neutral",
                    "rsi": 58.3,
                    "horizon_days": 5,
                    "last_close": 8365.0,
                    "predicted_close": 8511.82,
                    "as_of": "2026-07-28",
                    "model": "lstm",
                    "model_version": "lstm-20260618T003942Z",
                    "reason": null
                },
                "transformer": {
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
                    "reason": "modelo no entrenado"
                }
            }
        })),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 502, description = "api-ml no pudo responder", example = json!({
            "code": 502,
            "message": "No se pudo contactar a api-ml"
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
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
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
    let response = client
        .get(format!(
            "{}/predict/trend/compare/{}",
            api_ml_url.trim_end_matches('/'),
            ticker
        ))
        .send()
        .await;

    match response {
        // Se reenvia casi tal cual (api-ml ya devuelve la forma que necesita
        // el frontend: symbol/as_of/default_model/predictions), salvo por
        // "available": api-ml solo manda ese campo (en false) para modelos
        // no disponibles; para una prediccion exitosa el campo directamente
        // no viene. Se normaliza aca para que el frontend pueda confiar en
        // que `available` siempre esta presente.
        Ok(res) if res.status().is_success() => match res.json::<Value>().await {
            Ok(body) => (StatusCode::OK, Json(normalize_availability(body))),
            Err(err) => {
                tracing::error!(
                    "Respuesta invalida de api-ml al comparar {}: {}",
                    ticker,
                    err
                );
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "code": 502,
                        "message": "Respuesta invalida de api-ml"
                    })),
                )
            }
        },
        Ok(res) => {
            tracing::warn!("api-ml respondio {} al comparar {}", res.status(), ticker);
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "code": 502,
                    "message": "api-ml no pudo comparar los modelos para este ticker"
                })),
            )
        }
        Err(err) => {
            tracing::error!(
                "No se pudo contactar a api-ml para comparar {}: {}",
                ticker,
                err
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "code": 502,
                    "message": "No se pudo contactar a api-ml"
                })),
            )
        }
    }
}

fn normalize_availability(mut body: Value) -> Value {
    if let Some(predictions) = body.get_mut("predictions").and_then(Value::as_object_mut) {
        for prediction in predictions.values_mut() {
            if let Some(obj) = prediction.as_object_mut() {
                obj.entry("available").or_insert(Value::Bool(true));
            }
        }
    }
    body
}
