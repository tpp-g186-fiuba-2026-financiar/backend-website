use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

/// Fetches historical price data for `ticker` from the data-collector service,
/// retrying up to 3 attempts on server errors or transport failures.
async fn get_ticker_history(ticker: &str) -> Result<reqwest::Response, reqwest::Error> {
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
    response
}

// The data-collector wraps the price history in an envelope; the fields we
// don't need (`cached`, `status`, `ticker_info`) are left off the struct
// and ignored by serde. `close_amount` comes back as a JSON string, not a
// number, so it needs an extra parse step. We don't trust `data` to be
// ordered, so the current price is the point with the highest `ts`
// (unix milliseconds), not simply the last element.
#[derive(Debug, Deserialize)]
struct HistoricalDataEnvelope {
    data: Vec<PricePoint>,
}

#[derive(Debug, Deserialize)]
struct PricePoint {
    close_amount: String,
    ts: i64,
}

#[derive(Debug)]
enum PriceFetchError {
    Request(reqwest::Error),
    Http(StatusCode),
    Empty,
    InvalidPrice(String),
}

async fn get_current_price(ticker: &str) -> Result<f64, PriceFetchError> {
    let response = get_ticker_history(ticker)
        .await
        .map_err(PriceFetchError::Request)?;

    if !response.status().is_success() {
        return Err(PriceFetchError::Http(
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        ));
    }

    let envelope: HistoricalDataEnvelope =
        response.json().await.map_err(PriceFetchError::Request)?;

    let latest = envelope
        .data
        .iter()
        .max_by_key(|point| point.ts)
        .ok_or(PriceFetchError::Empty)?;

    latest
        .close_amount
        .parse::<f64>()
        .map_err(|_| PriceFetchError::InvalidPrice(latest.close_amount.clone()))
}

/// Pure per-share balance calculation, kept free of controller/IO concerns
/// so it can be unit tested directly without touching the DB or the network.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct ShareBalance {
    pub ticker: String,
    pub quantity: i32,
    pub entry_price: Option<f64>,
    pub current_price: f64,
    pub cost_basis: Option<f64>,
    pub current_value: f64,
    pub profit_loss: Option<f64>,
    pub profit_loss_percent: Option<f64>,
}

pub fn calculate_share_balance(
    ticker: &str,
    quantity: i32,
    entry_price: Option<f64>,
    current_price: f64,
) -> ShareBalance {
    let current_value = quantity as f64 * current_price;

    let cost_basis = entry_price.map(|ep| quantity as f64 * ep);
    let profit_loss = cost_basis.map(|cb| current_value - cb);
    let profit_loss_percent = match (cost_basis, profit_loss) {
        (Some(cb), Some(pl)) if cb != 0.0 => Some((pl / cb) * 100.0),
        _ => None,
    };

    ShareBalance {
        ticker: ticker.to_uppercase(),
        quantity,
        entry_price,
        current_price,
        cost_basis,
        current_value,
        profit_loss,
        profit_loss_percent,
    }
}

/// Pure portfolio-level aggregation, also independently testable. If any
/// individual position is missing an entry price, the portfolio-level cost
/// basis / P&L fields become `None` rather than silently ignoring that
/// position. An empty portfolio reports a cost basis of 0.0.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct PortfolioBalance {
    pub shares: Vec<ShareBalance>,
    pub total_current_value: f64,
    pub total_cost_basis: Option<f64>,
    pub total_profit_loss: Option<f64>,
    pub total_profit_loss_percent: Option<f64>,
}

pub fn calculate_portfolio_balance(shares: Vec<ShareBalance>) -> PortfolioBalance {
    let total_current_value: f64 = shares.iter().map(|s| s.current_value).sum();

    let total_cost_basis: Option<f64> = if shares.is_empty() {
        Some(0.0)
    } else if shares.iter().all(|s| s.cost_basis.is_some()) {
        Some(shares.iter().filter_map(|s| s.cost_basis).sum())
    } else {
        None
    };

    let total_profit_loss = total_cost_basis.map(|cb| total_current_value - cb);
    let total_profit_loss_percent = match (total_cost_basis, total_profit_loss) {
        (Some(cb), Some(pl)) if cb != 0.0 => Some((pl / cb) * 100.0),
        _ => None,
    };

    PortfolioBalance {
        shares,
        total_current_value,
        total_cost_basis,
        total_profit_loss,
        total_profit_loss_percent,
    }
}

#[derive(Debug, sqlx::FromRow)]
struct UserShareRow {
    ticker: String,
    quantity: i32,
    entry_price: Option<f64>,
}

#[utoipa::path(
    get,
    path = "/user/shares/balance",
    responses(
        (status = 200, description = "Current balance of the authenticated user's whole portfolio", body = PortfolioBalance),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 502, description = "Failed to fetch current price data for one or more tickers", example = json!({
            "code": 502,
            "message": "Could not retrieve current price data for GGAL. Please try again later."
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
    let rows = sqlx::query_as::<_, UserShareRow>(
        r#"
        SELECT s.ticker, us.quantity, us.entry_price
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.user_id = $1
        ORDER BY us.created_at ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!("Failed to load user shares for portfolio balance: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
    };

    if rows.is_empty() {
        let portfolio = calculate_portfolio_balance(Vec::new());
        return (StatusCode::OK, Json(json!(portfolio)));
    }

    // Fetch each distinct ticker's current price only once, even if the
    // user holds multiple lots of the same ticker.
    let unique_tickers: Vec<String> = rows
        .iter()
        .map(|r| r.ticker.to_uppercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut fetches: JoinSet<(String, Result<f64, PriceFetchError>)> = JoinSet::new();
    for ticker in unique_tickers {
        fetches.spawn(async move {
            let result = get_current_price(&ticker).await;
            (ticker, result)
        });
    }

    let mut prices: HashMap<String, f64> = HashMap::new();
    while let Some(joined) = fetches.join_next().await {
        match joined {
            Ok((ticker, Ok(price))) => {
                prices.insert(ticker, price);
            }
            Ok((ticker, Err(err))) => {
                tracing::error!("Failed to fetch current price for {}: {:?}", ticker, err);
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "code": 502,
                        "message": format!(
                            "Could not retrieve current price data for {}. Please try again later.",
                            ticker
                        )
                    })),
                );
            }
            Err(join_err) => {
                tracing::error!("Price-fetch task panicked: {}", join_err);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "code": 500,
                        "message": "An unexpected error occurred. Please try again later."
                    })),
                );
            }
        }
    }

    let shares: Vec<ShareBalance> = rows
        .into_iter()
        .map(|row| {
            let ticker = row.ticker.to_uppercase();
            // Safe: every ticker in `rows` was included in `unique_tickers`,
            // and we already returned 502 above if any lookup failed.
            let current_price = prices[&ticker];
            calculate_share_balance(&ticker, row.quantity, row.entry_price, current_price)
        })
        .collect();

    let portfolio = calculate_portfolio_balance(shares);

    (StatusCode::OK, Json(json!(portfolio)))
}
