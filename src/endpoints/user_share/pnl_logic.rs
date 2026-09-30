use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct SharePnlItem {
    pub id: i32,
    pub ticker: String,
    pub quantity: i32,
    pub entry_price: Option<f64>,
    pub current_price: Option<f64>,
    pub pnl_amount: Option<f64>,
    pub pnl_percentage: Option<f64>,
    /// Peso de la tenencia sobre el valor de mercado de la cartera (0-100).
    /// Null si no hay precio actual para el ticker.
    pub weight_percentage: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct PortfolioPnlSummary {
    pub total_invested: f64,
    pub total_current_value: f64,
    pub total_pnl_amount: f64,
    pub total_pnl_percentage: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct PnlResponse {
    pub shares: Vec<SharePnlItem>,
    pub portfolio: PortfolioPnlSummary,
}

#[utoipa::path(
    get,
    path = "/user/shares/pnl",
    responses(
        (status = 200, description = "Ganancia/perdida (P&L) por ticker y agregada de la cartera del usuario autenticado", body = PnlResponse, example = json!({
            "shares": [
                {
                    "id": 1,
                    "ticker": "GGAL",
                    "quantity": 10,
                    "entry_price": 1500.0,
                    "current_price": 1800.0,
                    "pnl_amount": 3000.0,
                    "pnl_percentage": 20.0,
                    "weight_percentage": 66.67
                },
                {
                    "id": 2,
                    "ticker": "YPFD",
                    "quantity": 5,
                    "entry_price": null,
                    "current_price": 900.0,
                    "pnl_amount": null,
                    "pnl_percentage": null,
                    "weight_percentage": 33.33
                }
            ],
            "portfolio": {
                "total_invested": 15000.0,
                "total_current_value": 18000.0,
                "total_pnl_amount": 3000.0,
                "total_pnl_percentage": 20.0
            }
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
    let rows = sqlx::query_as::<_, (i32, String, i32, Option<f64>)>(
        r#"
        SELECT us.id, s.ticker, us.quantity, us.entry_price
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.user_id = $1
        ORDER BY s.ticker ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!("Failed to list shares for pnl: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
    };

    let collector_url = std::env::var("DATA_COLLECTOR_URL")
        .unwrap_or_else(|_| "https://data-colector.onrender.com".into());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("reqwest client");

    let mut requests = JoinSet::new();
    for (id, ticker, quantity, entry_price) in rows {
        let client = client.clone();
        let collector_url = collector_url.clone();
        requests.spawn(async move {
            let current_price = fetch_current_price(&client, &collector_url, &ticker).await;
            (id, ticker, quantity, entry_price, current_price)
        });
    }

    let mut shares = Vec::new();
    while let Some(result) = requests.join_next().await {
        match result {
            Ok(item) => shares.push(item),
            Err(error) => tracing::error!("Fallo una consulta de precio actual: {}", error),
        }
    }
    shares.sort_by(|left, right| left.1.cmp(&right.1));

    let mut total_invested = 0.0;
    let mut total_current_value = 0.0;
    let mut total_pnl_amount = 0.0;
    let mut items = Vec::with_capacity(shares.len());

    let positions: Vec<(i32, Option<f64>)> = shares
        .iter()
        .map(|(_, _, quantity, _, current)| (*quantity, *current))
        .collect();
    let weights = compute_weights(&positions);

    for ((id, ticker, quantity, entry_price, current_price), weight_percentage) in
        shares.into_iter().zip(weights)
    {
        // El P&L agregado solo suma tenencias con precio de entrada y precio
        // actual resueltos, para no mezclar valores parciales en el total.
        let pnl_amount = match (entry_price, current_price) {
            (Some(entry), Some(current)) => {
                let amount = (current - entry) * f64::from(quantity);
                total_invested += entry * f64::from(quantity);
                total_current_value += current * f64::from(quantity);
                total_pnl_amount += amount;
                Some(amount)
            }
            _ => None,
        };
        let pnl_percentage = match (entry_price, pnl_amount) {
            (Some(entry), Some(amount)) if entry > 0.0 => {
                Some(amount / (entry * f64::from(quantity)) * 100.0)
            }
            _ => None,
        };

        items.push(SharePnlItem {
            id,
            ticker,
            quantity,
            entry_price,
            current_price,
            pnl_amount,
            pnl_percentage,
            weight_percentage,
        });
    }

    let total_pnl_percentage = if total_invested > 0.0 {
        Some(total_pnl_amount / total_invested * 100.0)
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(json!({
            "shares": items,
            "portfolio": PortfolioPnlSummary {
                total_invested,
                total_current_value,
                total_pnl_amount,
                total_pnl_percentage,
            },
        })),
    )
}

/// Peso (0-100) de cada tenencia sobre el valor de mercado total. Usa toda
/// tenencia con precio actual, aunque no tenga precio de entrada (a diferencia
/// del P&L). Sin precio, o con valor total 0, el peso es `None`.
fn compute_weights(positions: &[(i32, Option<f64>)]) -> Vec<Option<f64>> {
    let total: f64 = positions
        .iter()
        .filter_map(|(quantity, price)| price.map(|p| p * f64::from(*quantity)))
        .sum();
    positions
        .iter()
        .map(|(quantity, price)| match price {
            Some(p) if total > 0.0 => Some(p * f64::from(*quantity) / total * 100.0),
            _ => None,
        })
        .collect()
}

/// Ultimo cierre disponible en data-colector para el ticker, usado como proxy
/// de precio actual (no hay feed de precios en tiempo real en el proyecto).
async fn fetch_current_price(
    client: &reqwest::Client,
    base_url: &str,
    ticker: &str,
) -> Option<f64> {
    let url = format!(
        "{}/historical-data/{}",
        base_url.trim_end_matches('/'),
        ticker
    );
    let response = match client.post(&url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::warn!(
                "data-colector respondio {} al pedir precio actual de {}",
                response.status(),
                ticker
            );
            return None;
        }
        Err(error) => {
            tracing::error!(
                "No se pudo contactar a data-colector para {}: {}",
                ticker,
                error
            );
            return None;
        }
    };

    let body = match response.json::<Value>().await {
        Ok(body) => body,
        Err(error) => {
            tracing::error!(
                "Respuesta invalida de data-colector para {}: {}",
                ticker,
                error
            );
            return None;
        }
    };

    body.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let ts = row.get("ts")?.as_i64()?;
            let raw_close = row.get("close_amount")?;
            let close = raw_close
                .as_f64()
                .or_else(|| raw_close.as_str()?.parse::<f64>().ok())?;
            close.is_finite().then_some((ts, close))
        })
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, close)| close)
}

#[cfg(test)]
mod tests {
    use super::compute_weights;

    #[test]
    fn weights_are_proportional_to_market_value() {
        // 10 x 30 = 300 y 5 x 40 = 200 -> 60% / 40%.
        let w = compute_weights(&[(10, Some(30.0)), (5, Some(40.0))]);
        assert!((w[0].unwrap() - 60.0).abs() < 1e-9);
        assert!((w[1].unwrap() - 40.0).abs() < 1e-9);
    }

    #[test]
    fn weights_sum_to_100() {
        let w = compute_weights(&[(3, Some(7.3)), (11, Some(1.9)), (2, Some(250.0))]);
        let sum: f64 = w.iter().map(|x| x.unwrap()).sum();
        assert!((sum - 100.0).abs() < 1e-9);
    }

    #[test]
    fn single_position_weighs_100() {
        assert_eq!(compute_weights(&[(4, Some(12.0))]), vec![Some(100.0)]);
    }

    #[test]
    fn unpriced_position_has_no_weight_and_is_excluded_from_total() {
        let w = compute_weights(&[(10, Some(10.0)), (5, None)]);
        assert_eq!(w, vec![Some(100.0), None]);
    }

    #[test]
    fn empty_or_zero_value_portfolio_has_no_weights() {
        assert!(compute_weights(&[]).is_empty());
        assert_eq!(compute_weights(&[(5, None)]), vec![None]);
        assert_eq!(compute_weights(&[(5, Some(0.0))]), vec![None]);
    }
}
