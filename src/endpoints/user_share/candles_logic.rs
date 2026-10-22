use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use utoipa::{IntoParams, ToSchema};

use crate::auth::middleware::AuthUser;
use crate::endpoints::upstream;

#[derive(Serialize, ToSchema)]
pub struct Candle {
    /// Fecha de la rueda (UTC), formato YYYY-MM-DD
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
}

#[derive(Serialize, ToSchema)]
pub struct CandlesResponse {
    pub ticker: String,
    pub candles: Vec<Candle>,
}

#[derive(Deserialize, IntoParams)]
pub struct CandlesQuery {
    /// Cantidad de ruedas mas recientes a devolver (default: todas las cacheadas)
    limit: Option<usize>,
}

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/candles",
    params(
        ("ticker" = String, Path, description = "Ticker (ej: GGAL)"),
        CandlesQuery
    ),
    responses(
        (status = 200, description = "Velas diarias OHLCV ordenadas por fecha ascendente", body = CandlesResponse, example = json!({
            "ticker": "GGAL",
            "candles": [
                {"date": "2026-06-25", "open": 7650.0, "high": 7720.0, "low": 7580.0, "close": 7605.0, "volume": 1200000}
            ]
        })),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 502, description = "data-colector no pudo responder")
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
    Query(query): Query<CandlesQuery>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();
    let client = upstream::client(Duration::from_secs(90));
    // POST /historical-data/{t} trae la serie completa con OHLCV (cacheada
    // en data-colector). `/movement` necesita `from` + `days` exactos y falla
    // si no pasaron esas ruedas, asi que no sirve para un grafico de velas.
    let url = format!(
        "{}/historical-data/{}",
        upstream::data_collector_url(),
        ticker
    );
    let response =
        upstream::send_with_retry(|| client.post(&url), &format!("velas de {ticker}")).await;

    let body = match response {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) => body,
            Err(error) => {
                tracing::error!("Velas invalidas para {}: {}", ticker, error);
                return upstream::bad_gateway("data-colector devolvio una respuesta invalida");
            }
        },
        Ok(response) => {
            tracing::warn!(
                "data-colector respondio {} para velas de {}",
                response.status(),
                ticker
            );
            return upstream::bad_gateway("data-colector no pudo obtener las velas");
        }
        Err(error) => {
            tracing::error!("No se pudo obtener velas de {}: {}", ticker, error);
            return upstream::bad_gateway("No se pudo contactar a data-colector");
        }
    };
    if let Some(message) = upstream::data_collector_body_error(&body) {
        tracing::warn!("data-colector fallo velas de {}: {}", ticker, message);
        return upstream::bad_gateway("data-colector no pudo obtener las velas");
    }

    let mut candles = parse_candles(&body);
    if let Some(limit) = query.limit {
        let skip = candles.len().saturating_sub(limit);
        candles.drain(..skip);
    }
    (
        StatusCode::OK,
        Json(json!({"ticker": ticker, "candles": candles})),
    )
}

fn parse_candles(body: &Value) -> Vec<Candle> {
    let mut candles = body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let ts = row.get("ts")?.as_i64()?;
            let date = Utc.timestamp_millis_opt(ts).single()?.date_naive();
            Some((
                ts,
                Candle {
                    date: date.to_string(),
                    open: upstream::as_f64(row.get("open_amount")?)?,
                    high: upstream::as_f64(row.get("high_amount")?)?,
                    low: upstream::as_f64(row.get("low_amount")?)?,
                    close: upstream::as_f64(row.get("close_amount")?)?,
                    volume: row.get("volume").and_then(Value::as_i64).unwrap_or(0),
                },
            ))
        })
        .collect::<Vec<_>>();
    candles.sort_by_key(|(ts, _)| *ts);
    candles.into_iter().map(|(_, candle)| candle).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_candles_sorts_and_skips_invalid_rows() {
        let body = json!({"data": [
            {"ts": 1_710_000_000_000_i64, "open_amount": "2", "high_amount": "3", "low_amount": "1", "close_amount": "2.5", "volume": 10},
            {"ts": 1_700_000_000_000_i64, "open_amount": "1", "high_amount": "2", "low_amount": "0.5", "close_amount": "1.5", "volume": 5},
            {"ts": 1_705_000_000_000_i64, "open_amount": "x", "high_amount": "2", "low_amount": "0.5", "close_amount": "1.5"}
        ]});
        let candles = parse_candles(&body);
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].date, "2023-11-14");
        assert_eq!(candles[0].close, 1.5);
        assert_eq!(candles[1].volume, 10);
    }
}
