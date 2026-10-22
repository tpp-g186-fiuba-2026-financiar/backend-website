use axum::{http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;
use crate::endpoints::upstream;

/// Series de contexto de mercado: (key que ve el frontend, path en data-colector,
/// campo del valor en cada fila).
const SERIES: &[(&str, &str, &str)] = &[
    ("CCL", "/macro/argdatos/CCL", "value"),
    ("MEP", "/macro/argdatos/MEP", "value"),
    ("RIESGO_PAIS", "/macro/argdatos/RIESGO_PAIS", "value"),
    ("TPM", "/interest-rate/ar/TPM", "value"),
    ("BADLAR", "/interest-rate/ar/BADLAR", "value"),
    ("RESERVAS", "/interest-rate/ar/RESERVAS", "value"),
    ("TNX", "/interest-rate/us/TNX", "value"),
    ("GOLD", "/historical-data/GOLD", "close_amount"),
    ("OIL", "/historical-data/OIL", "close_amount"),
];

#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct SeriesPoint {
    /// Epoch en milisegundos
    pub ts: i64,
    pub value: f64,
}

#[derive(Serialize, ToSchema)]
pub struct MarketSeries {
    /// CCL | MEP | RIESGO_PAIS | TPM | BADLAR | RESERVAS | TNX | GOLD | OIL
    pub key: String,
    pub points: Vec<SeriesPoint>,
}

#[derive(Serialize, ToSchema)]
pub struct UnavailableSeries {
    pub key: String,
    pub reason: String,
}

#[derive(Serialize, ToSchema)]
pub struct MarketSeriesResponse {
    /// Series que se pudieron obtener, en el orden fijo de arriba
    pub series: Vec<MarketSeries>,
    /// Series que fallaron (se omiten de `series`), con el motivo
    pub unavailable: Vec<UnavailableSeries>,
}

#[utoipa::path(
    get,
    path = "/market/series",
    responses(
        (status = 200, description = "Series macro/mercado (dolar, riesgo pais, tasas, reservas, commodities). Una serie caida no rompe las demas: se informa en `unavailable`", body = MarketSeriesResponse, example = json!({
            "series": [
                {"key": "CCL", "points": [{"ts": 1747008000000_i64, "value": 1610.8}]},
                {"key": "TNX", "points": [{"ts": 1747008000000_i64, "value": 4.25}]}
            ],
            "unavailable": [
                {"key": "RESERVAS", "reason": "Failed to fetch interest rate series"}
            ]
        })),
        (status = 401, description = "Missing or invalid authentication token")
    ),
    security(("bearer_auth" = [])),
    tag = "Market"
)]
pub async fn handler(Extension(_auth_user): Extension<AuthUser>) -> impl IntoResponse {
    let base = upstream::data_collector_url();
    let client = upstream::client(Duration::from_secs(60));

    let mut tasks = JoinSet::new();
    for (index, (key, path, field)) in SERIES.iter().enumerate() {
        let client = client.clone();
        let url = format!("{base}{path}");
        tasks.spawn(async move { (index, *key, fetch_series(&client, &url, key, field).await) });
    }

    let mut results = Vec::with_capacity(SERIES.len());
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(result) => results.push(result),
            Err(error) => tracing::error!("Tarea de serie de mercado fallo: {}", error),
        }
    }
    results.sort_by_key(|(index, _, _)| *index);

    let mut series = Vec::new();
    let mut unavailable = Vec::new();
    for (_, key, result) in results {
        match result {
            Ok(points) => series.push(MarketSeries {
                key: key.to_string(),
                points,
            }),
            Err(reason) => unavailable.push(UnavailableSeries {
                key: key.to_string(),
                reason,
            }),
        }
    }

    (
        StatusCode::OK,
        Json(MarketSeriesResponse {
            series,
            unavailable,
        }),
    )
}

async fn fetch_series(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    field: &str,
) -> Result<Vec<SeriesPoint>, String> {
    let response = upstream::send_with_retry(|| client.post(url), &format!("serie {key}")).await;
    let body = match response {
        Ok(response) if response.status().is_success() => response
            .json::<Value>()
            .await
            .map_err(|_| "data-colector devolvio una respuesta invalida".to_string())?,
        Ok(response) => {
            tracing::warn!("data-colector respondio {} para {}", response.status(), key);
            return Err(format!("data-colector respondio {}", response.status()));
        }
        Err(error) => {
            tracing::warn!("No se pudo obtener la serie {}: {}", key, error);
            return Err("No se pudo contactar a data-colector".to_string());
        }
    };
    if let Some(message) = upstream::data_collector_body_error(&body) {
        tracing::warn!("data-colector fallo la serie {}: {}", key, message);
        return Err(message);
    }
    let points = parse_points(&body, field);
    if points.is_empty() {
        return Err("la serie no tiene datos".to_string());
    }
    Ok(points)
}

fn parse_points(body: &Value, field: &str) -> Vec<SeriesPoint> {
    let mut points = body
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(SeriesPoint {
                ts: row.get("ts")?.as_i64()?,
                value: upstream::as_f64(row.get(field)?)?,
            })
        })
        .collect::<Vec<_>>();
    points.sort_by_key(|point| point.ts);
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_points_reads_the_requested_field_sorted() {
        let body = json!({"data": [
            {"ts": 2, "value": "4.5"},
            {"ts": 1, "value": 3.0},
            {"ts": 3, "value": "bad"}
        ]});
        let points = parse_points(&body, "value");
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].ts, 1);
        assert_eq!(points[1].value, 4.5);
        assert!(parse_points(&body, "close_amount").is_empty());
    }
}
