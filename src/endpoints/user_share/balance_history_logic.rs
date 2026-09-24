use std::collections::{HashMap, HashSet};

use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::{DateTime, TimeZone, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use tokio::task::JoinSet;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;
use crate::endpoints::user_share::user_share_balance_logic::{
    entry_price_for_purchase, fetch_ticker_history, PriceFetchError, PricePoint,
};

const MS_PER_DAY: i64 = 86_400_000;

fn day_bucket(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(MS_PER_DAY)
}

fn day_bucket_to_date_string(day: i64) -> String {
    match Utc.timestamp_opt(day * (MS_PER_DAY / 1000), 0) {
        chrono::LocalResult::Single(dt) => dt.date_naive().to_string(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------
// Pure timeline computation — no IO, unit-testable on its own
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct LedgerOp {
    pub is_buy: bool,
    pub quantity: i32,
    pub price: f64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    pub day: i64,
    pub quantity: i32,
    pub avg_cost: f64,
}

/// Replays a ticker's operations in chronological order into a series of
/// checkpoints (holdings + running average cost after each operation).
/// Average-cost accounting: a sell doesn't change the average cost of the
/// remaining shares, only a buy does. Consistent with how `PUT
/// /user/shares/{id}` already treats `entry_price` as one blended value
/// per position rather than tracking individual lots.
pub fn build_checkpoints(mut ops: Vec<LedgerOp>) -> Vec<Checkpoint> {
    ops.sort_by_key(|o| o.created_at_ms);
    let mut checkpoints = Vec::with_capacity(ops.len());
    let mut quantity: i32 = 0;
    let mut avg_cost: f64 = 0.0;

    for op in ops {
        if op.is_buy {
            let total_cost = avg_cost * quantity as f64 + op.price * op.quantity as f64;
            quantity += op.quantity;
            avg_cost = if quantity > 0 {
                total_cost / quantity as f64
            } else {
                0.0
            };
        } else {
            quantity -= op.quantity;
            if quantity <= 0 {
                quantity = 0;
                avg_cost = 0.0;
            }
        }
        checkpoints.push(Checkpoint {
            day: day_bucket(op.created_at_ms),
            quantity,
            avg_cost,
        });
    }

    checkpoints
}

/// Holdings and average cost as of the end of `target_day`. Checkpoints
/// must be in chronological order (as `build_checkpoints` produces them);
/// relies on `Iterator::max_by_key` returning the *last* element among
/// ties to correctly pick the latest checkpoint on a day with several
/// operations.
pub fn holdings_on_day(checkpoints: &[Checkpoint], target_day: i64) -> (i32, f64) {
    checkpoints
        .iter()
        .filter(|c| c.day <= target_day)
        .max_by_key(|c| c.day)
        .map(|c| (c.quantity, c.avg_cost))
        .unwrap_or((0, 0.0))
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct DailyBalancePoint {
    pub date: String,
    pub total_current_value: f64,
    pub total_cost_basis: f64,
    pub total_profit_loss: f64,
}

/// Builds the full daily timeline from `start_day` to `end_day`
/// (inclusive, both as UTC day buckets) for every ticker the user has
/// ever held. For a ticker/day where no price data exists at all that far
/// back, that ticker's contribution to that day is skipped (logged by the
/// caller) rather than failing the whole timeline — an actual parse error
/// (`PriceFetchError::InvalidPrice`) still propagates, since that's bad
/// data rather than a legitimate gap.
pub fn build_daily_timeline(
    tickers: &HashMap<String, (Vec<Checkpoint>, Vec<PricePoint>)>,
    start_day: i64,
    end_day: i64,
) -> Result<Vec<DailyBalancePoint>, PriceFetchError> {
    let mut timeline = Vec::with_capacity((end_day - start_day + 1).max(0) as usize);

    for day in start_day..=end_day {
        let mut total_value = 0.0;
        let mut total_cost_basis = 0.0;

        for (checkpoints, price_history) in tickers.values() {
            let (qty, avg_cost) = holdings_on_day(checkpoints, day);
            if qty <= 0 {
                continue;
            }
            // Same "close of that day, or last trading day before it"
            // rule used to backfill entry_price in user_share_balance_logic.
            if let Some(price) = entry_price_for_purchase(price_history, day * MS_PER_DAY)? {
                total_value += qty as f64 * price;
                total_cost_basis += qty as f64 * avg_cost;
            }
        }

        timeline.push(DailyBalancePoint {
            date: day_bucket_to_date_string(day),
            total_current_value: total_value,
            total_cost_basis,
            total_profit_loss: total_value - total_cost_basis,
        });
    }

    Ok(timeline)
}

// ---------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct OperationRow {
    ticker: String,
    operation_type: String,
    quantity: i32,
    price: f64,
    created_at: DateTime<Utc>,
}

fn internal_error() -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
    )
        .into_response()
}

fn bad_gateway(ticker: &str) -> axum::response::Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "code": 502,
            "message": format!(
                "Could not retrieve price history for {}. Please try again later.",
                ticker
            )
        })),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/user/shares/balance/history",
    responses(
        (status = 200, description = "Daily balance/profit timeline for the authenticated user's whole account, from their first purchase to today", body = [DailyBalancePoint]),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 502, description = "Failed to fetch price history for one or more tickers", example = json!({
            "code": 502,
            "message": "Could not retrieve price history for GGAL. Please try again later."
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
    let rows = sqlx::query_as::<_, OperationRow>(
        r#"
        SELECT s.ticker, uso.operation_type, uso.quantity, uso.price, uso.created_at
        FROM user_share_operations uso
        JOIN shares s ON s.id = uso.share_id
        WHERE uso.user_id = $1
        ORDER BY uso.created_at ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!(
                "Failed to load share operations for balance history: {}",
                err
            );
            return internal_error();
        }
    };

    if rows.is_empty() {
        return (StatusCode::OK, Json(json!(Vec::<DailyBalancePoint>::new()))).into_response();
    }

    let mut ops_by_ticker: HashMap<String, Vec<LedgerOp>> = HashMap::new();
    let mut earliest_day = i64::MAX;
    for row in &rows {
        let ticker = row.ticker.to_uppercase();
        let created_at_ms = row.created_at.timestamp_millis();
        earliest_day = earliest_day.min(day_bucket(created_at_ms));
        ops_by_ticker.entry(ticker).or_default().push(LedgerOp {
            is_buy: row.operation_type == "buy",
            quantity: row.quantity,
            price: row.price,
            created_at_ms,
        });
    }

    let unique_tickers: HashSet<String> = ops_by_ticker.keys().cloned().collect();

    let mut fetches: JoinSet<(String, Result<Vec<PricePoint>, PriceFetchError>)> = JoinSet::new();
    for ticker in unique_tickers {
        fetches.spawn(async move {
            let result = fetch_ticker_history(&ticker).await;
            (ticker, result)
        });
    }

    let mut tickers: HashMap<String, (Vec<Checkpoint>, Vec<PricePoint>)> = HashMap::new();
    while let Some(joined) = fetches.join_next().await {
        match joined {
            Ok((ticker, Ok(history))) => {
                let ops = ops_by_ticker.remove(&ticker).unwrap_or_default();
                tickers.insert(ticker, (build_checkpoints(ops), history));
            }
            Ok((ticker, Err(err))) => {
                tracing::error!("Failed to fetch price history for {}: {:?}", ticker, err);
                return bad_gateway(&ticker);
            }
            Err(join_err) => {
                tracing::error!("Price-history task panicked: {}", join_err);
                return internal_error();
            }
        }
    }

    let today_day = day_bucket(Utc::now().timestamp_millis());

    let timeline = match build_daily_timeline(&tickers, earliest_day, today_day) {
        Ok(timeline) => timeline,
        Err(err) => {
            tracing::error!("Failed to build balance history timeline: {:?}", err);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "code": 502,
                    "message": "Could not compute historical balance. Please try again later."
                })),
            )
                .into_response();
        }
    };

    (StatusCode::OK, Json(json!(timeline))).into_response()
}
