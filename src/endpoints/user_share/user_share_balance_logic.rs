use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

// ---------------------------------------------------------------------
// Data-collector client (IO)
// ---------------------------------------------------------------------

/// Fetches the full historical price series for `ticker` from the
/// data-collector service, retrying up to 3 attempts on server errors or
/// transport failures. Returns the raw list of points (not guaranteed to
/// be ordered) — interpretation (current price, entry price for a given
/// purchase date) lives in the pure functions below so it can be unit
/// tested without a network call.
async fn fetch_ticker_history(ticker: &str) -> Result<Vec<PricePoint>, PriceFetchError> {
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

    let response = response.map_err(PriceFetchError::Request)?;
    if !response.status().is_success() {
        return Err(PriceFetchError::Http(
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        ));
    }

    let envelope: HistoricalDataEnvelope =
        response.json().await.map_err(PriceFetchError::Request)?;
    Ok(envelope.data)
}

// The data-collector wraps the price history in an envelope; the fields we
// don't need (`cached`, `status`, `ticker_info`) are left off the struct
// and ignored by serde. `close_amount` comes back as a JSON string, not a
// number, so it needs an extra parse step (see `parse_close_amount`).
#[derive(Debug, Deserialize)]
struct HistoricalDataEnvelope {
    data: Vec<PricePoint>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PricePoint {
    pub close_amount: String,
    /// Unix timestamp in milliseconds.
    pub ts: i64,
}

#[derive(Debug)]
pub enum PriceFetchError {
    Request(reqwest::Error),
    Http(StatusCode),
    Empty,
    InvalidPrice(String),
}

// ---------------------------------------------------------------------
// Price resolution (pure, unit-testable without network or DB)
// ---------------------------------------------------------------------

const MS_PER_DAY: i64 = 86_400_000;

/// UTC calendar-day bucket for a unix-ms timestamp. Two timestamps in the
/// same UTC calendar day map to the same bucket.
fn day_bucket(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(MS_PER_DAY)
}

fn parse_close_amount(point: &PricePoint) -> Result<f64, PriceFetchError> {
    point
        .close_amount
        .parse::<f64>()
        .map_err(|_| PriceFetchError::InvalidPrice(point.close_amount.clone()))
}

/// Current price: the point with the highest `ts`. History is not
/// trusted to be ordered.
pub fn latest_price(history: &[PricePoint]) -> Result<f64, PriceFetchError> {
    let latest = history
        .iter()
        .max_by_key(|point| point.ts)
        .ok_or(PriceFetchError::Empty)?;
    parse_close_amount(latest)
}

/// Entry price for a purchase made at `purchase_ts_ms`:
///
/// - the close of that same UTC calendar day, if the market traded that
///   day;
/// - otherwise, the most recent close on a *prior* day (e.g. a purchase
///   recorded on a Saturday falls back to Friday's close).
///
/// Returns `Ok(None)` if no point exists on or before that day at all
/// (e.g. the ticker's history starts after the purchase date) — callers
/// should leave `entry_price` unset in that case rather than treating it
/// as an error.
pub fn entry_price_for_purchase(
    history: &[PricePoint],
    purchase_ts_ms: i64,
) -> Result<Option<f64>, PriceFetchError> {
    let target_day = day_bucket(purchase_ts_ms);

    let candidate = history
        .iter()
        .filter(|point| day_bucket(point.ts) <= target_day)
        .max_by_key(|point| point.ts);

    match candidate {
        Some(point) => parse_close_amount(point).map(Some),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------
// Balance calculation (pure, unit-testable) — unchanged from before
// ---------------------------------------------------------------------

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

// ---------------------------------------------------------------------
// DB row + persistence (IO)
// ---------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct UserShareRow {
    // user_shares.id is SERIAL -> i32.
    id: i32,
    ticker: String,
    quantity: i32,
    entry_price: Option<f64>,
    // Cast to unix ms directly in SQL so this file doesn't need to know
    // whether the project represents TIMESTAMPTZ as chrono or time.
    created_at_ms: i64,
}

/// Persists a backfilled entry price for a single lot. Only called when
/// the row was missing `entry_price` and we managed to resolve one from
/// history.
async fn persist_entry_price(
    pool: &PgPool,
    user_share_id: i32,
    entry_price: f64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE user_shares SET entry_price = $1 WHERE id = $2")
        .bind(entry_price)
        .bind(user_share_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------

fn price_error_detail(err: &PriceFetchError) -> String {
    match err {
        PriceFetchError::Request(source) => source.to_string(),
        PriceFetchError::Http(status) => format!("upstream returned {}", status),
        PriceFetchError::Empty => "no price data returned".to_string(),
        PriceFetchError::InvalidPrice(raw) => format!("invalid price value '{}'", raw),
    }
}

fn bad_gateway_response(ticker: &str, err: &PriceFetchError) -> axum::response::Response {
    let detail = price_error_detail(err);
    tracing::error!("Failed to fetch price data for {}: {}", ticker, detail);
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "code": 502,
            "message": format!(
                "Could not retrieve current price data for {} ({}). Please try again later.",
                ticker, detail
            )
        })),
    )
        .into_response()
}

fn internal_error_response() -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
    )
        .into_response()
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
        SELECT us.id, s.ticker, us.quantity, us.entry_price,
               (EXTRACT(EPOCH FROM us.created_at) * 1000)::BIGINT AS created_at_ms
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
            return internal_error_response();
        }
    };

    if rows.is_empty() {
        let portfolio = calculate_portfolio_balance(Vec::new());
        return (StatusCode::OK, Json(json!(portfolio))).into_response();
    }

    // Fetch each distinct ticker's full history only once, even if the
    // user holds multiple lots of the same ticker — it's reused both for
    // the current price and for backfilling any missing per-lot entry
    // prices below.
    let unique_tickers: Vec<String> = rows
        .iter()
        .map(|r| r.ticker.to_uppercase())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut fetches: JoinSet<(String, Result<Vec<PricePoint>, PriceFetchError>)> = JoinSet::new();
    for ticker in unique_tickers {
        fetches.spawn(async move {
            let result = fetch_ticker_history(&ticker).await;
            (ticker, result)
        });
    }

    let mut histories: HashMap<String, Vec<PricePoint>> = HashMap::new();
    while let Some(joined) = fetches.join_next().await {
        match joined {
            Ok((ticker, Ok(history))) => {
                histories.insert(ticker, history);
            }
            Ok((ticker, Err(err))) => return bad_gateway_response(&ticker, &err),
            Err(join_err) => {
                tracing::error!("Price-fetch task panicked: {}", join_err);
                return internal_error_response();
            }
        }
    }

    let mut current_prices: HashMap<String, f64> = HashMap::new();
    for (ticker, history) in &histories {
        match latest_price(history) {
            Ok(price) => {
                current_prices.insert(ticker.clone(), price);
            }
            Err(err) => return bad_gateway_response(ticker, &err),
        }
    }

    let mut shares = Vec::with_capacity(rows.len());
    for row in rows {
        let ticker = row.ticker.to_uppercase();
        // Safe: every ticker here has a history fetched above, and we
        // already returned 502 if the current price couldn't be derived
        // from it.
        let current_price = current_prices[&ticker];
        let history = &histories[&ticker];

        let entry_price = match row.entry_price {
            Some(existing) => Some(existing),
            None => match entry_price_for_purchase(history, row.created_at_ms) {
                Ok(resolved) => {
                    if let Some(price) = resolved {
                        // Best-effort: a failed write doesn't fail the
                        // whole request, it just gets recomputed and
                        // retried on the next call.
                        if let Err(err) = persist_entry_price(&pool, row.id, price).await {
                            tracing::error!(
                                "Failed to persist backfilled entry_price for user_shares.id={}: {}",
                                row.id,
                                err
                            );
                        }
                    }
                    resolved
                }
                Err(err) => return bad_gateway_response(&ticker, &err),
            },
        };

        shares.push(calculate_share_balance(
            &ticker,
            row.quantity,
            entry_price,
            current_price,
        ));
    }

    let portfolio = calculate_portfolio_balance(shares);
    (StatusCode::OK, Json(json!(portfolio))).into_response()
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use axum::{
        extract::{Path, State},
        response::Response,
        routing::post,
        Router,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    async fn history_stub(
        State(attempts): State<Arc<AtomicUsize>>,
        Path(ticker): Path<String>,
    ) -> Response {
        match ticker.as_str() {
            "BAD" => StatusCode::BAD_REQUEST.into_response(),
            "INVALID" => "not-json".into_response(),
            "RETRY" if attempts.fetch_add(1, Ordering::Relaxed) < 2 => {
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
            _ => Json(json!({
                "data": [
                    {"close_amount": "100.0", "ts": 1},
                    {"close_amount": "125.5", "ts": 2}
                ]
            }))
            .into_response(),
        }
    }

    #[tokio::test]
    async fn history_client_retries_and_maps_http_parse_and_transport_errors() {
        let _env_guard = crate::ENV_TEST_LOCK.lock().await;
        let attempts = Arc::new(AtomicUsize::new(0));
        let stub = Router::new()
            .route("/historical-data/{ticker}", post(history_stub))
            .with_state(attempts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, stub).await.unwrap() });
        std::env::set_var("DATA_COLLECTOR_URL", format!("http://{address}"));

        let history = fetch_ticker_history(" ok ").await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(latest_price(&history).unwrap(), 125.5);

        let retry = fetch_ticker_history("retry").await.unwrap();
        assert_eq!(retry.len(), 2);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);

        let http = fetch_ticker_history("bad").await.unwrap_err();
        assert!(price_error_detail(&http).contains("upstream returned"));
        assert_eq!(
            bad_gateway_response("BAD", &http).status(),
            StatusCode::BAD_GATEWAY
        );

        let parse = fetch_ticker_history("invalid").await.unwrap_err();
        assert!(!price_error_detail(&parse).is_empty());

        std::env::set_var("DATA_COLLECTOR_URL", "http://127.0.0.1:1");
        let transport = fetch_ticker_history("offline").await.unwrap_err();
        assert!(!price_error_detail(&transport).is_empty());
        assert_eq!(
            internal_error_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
