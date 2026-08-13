use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct HistoricalPricePoint {
    pub ts: i64,
    pub close: f64,
}

#[derive(Serialize, ToSchema)]
pub struct ShareHistoryResponse {
    pub ticker: String,
    pub prices: Vec<HistoricalPricePoint>,
}

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/history",
    params(("ticker" = String, Path, description = "Ticker (ej: GGAL)")),
    responses(
        (status = 200, description = "Historico de cierres disponible en data-colector", body = ShareHistoryResponse),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 502, description = "data-colector no pudo responder")
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();
    let base = std::env::var("DATA_COLLECTOR_URL")
        .unwrap_or_else(|_| "https://data-colector.onrender.com".into());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("reqwest client");
    let url = format!("{}/historical-data/{}", base.trim_end_matches('/'), ticker);
    let mut response = client.post(&url).send().await;
    for attempt in 2..=3 {
        let retry = match &response {
            Err(_) => true,
            Ok(value) => value.status().is_server_error(),
        };
        if !retry {
            break;
        }
        tracing::warn!("Reintentando histórico de {} (intento {})", ticker, attempt);
        response = client.post(&url).send().await;
    }

    match response {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) => {
                let prices = body
                    .get("data")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|row| {
                        let ts = row.get("ts")?.as_i64()?;
                        let raw_close = row.get("close_amount")?;
                        let close = raw_close
                            .as_f64()
                            .or_else(|| raw_close.as_str()?.parse::<f64>().ok())?;
                        close
                            .is_finite()
                            .then_some(HistoricalPricePoint { ts, close })
                    })
                    .collect::<Vec<_>>();
                (
                    StatusCode::OK,
                    Json(json!({"ticker": ticker, "prices": prices})),
                )
            }
            Err(error) => {
                tracing::error!("Historico invalido para {}: {}", ticker, error);
                bad_gateway("data-colector devolvio una respuesta invalida")
            }
        },
        Ok(response) => {
            tracing::warn!(
                "data-colector respondio {} para {}",
                response.status(),
                ticker
            );
            bad_gateway("data-colector no pudo obtener el historico")
        }
        Err(error) => {
            tracing::error!("No se pudo obtener historico de {}: {}", ticker, error);
            bad_gateway("No se pudo contactar a data-colector")
        }
    }
}

fn bad_gateway(message: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"code": 502, "message": message})),
    )
}
