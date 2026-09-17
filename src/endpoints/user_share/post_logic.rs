use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;
use crate::endpoints::user_share::user_share_operations::{
    fetch_current_price, record_operation, OperationType,
};

const MAX_TICKER_LEN: usize = 20;

#[derive(Deserialize, ToSchema)]
pub struct CreateShareRequest {
    #[schema(example = "GGAL")]
    pub ticker: String,
    #[schema(example = 10)]
    pub quantity: i32,
    /// Precio de entrada (precio pagado por accion). Opcional — si se
    /// omite, se resuelve con el precio de mercado actual en el momento
    /// de la creacion.
    #[serde(default)]
    #[schema(example = 1520.50)]
    pub entry_price: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct CreateShareResponse {
    pub id: i32,
    pub user_id: i32,
    pub ticker: String,
    pub quantity: i32,
    pub entry_price: Option<f64>,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    post,
    path = "/user/shares",
    request_body = CreateShareRequest,
    responses(
        (status = 201, description = "Share added to the authenticated user's portfolio", body = CreateShareResponse, example = json!({
            "id": 1,
            "user_id": 42,
            "ticker": "GGAL",
            "quantity": 10,
            "entry_price": 1520.50,
            "created_at": "2026-05-28T12:00:00Z"
        })),
        (status = 400, description = "Invalid input data", examples(
            ("Invalid Ticker" = (
                summary = "Triggered when the ticker is empty, too long, or has invalid characters",
                value = json!({
                    "code": 400,
                    "message": "Invalid ticker. Must be 1-20 alphanumeric characters, optionally with '.' separators."
                })
            )),
            ("Invalid Quantity" = (
                summary = "Triggered when quantity is zero or negative",
                value = json!({
                    "code": 400,
                    "message": "Quantity must be a positive integer."
                })
            )),
            ("Invalid Entry Price" = (
                summary = "Triggered when entry_price is present but zero or negative",
                value = json!({
                    "code": 400,
                    "message": "Entry price must be a positive number."
                })
            ))
        )),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 409, description = "The authenticated user already has this ticker in their portfolio", example = json!({
            "code": 409,
            "message": "Share already exists for that ticker. Use PUT to update the quantity."
        })),
        (status = 502, description = "Failed to resolve a current price to use as entry_price", example = json!({
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
    Json(payload): Json<CreateShareRequest>,
) -> impl IntoResponse {
    let ticker = payload.ticker.trim().to_uppercase();

    if !is_valid_ticker(&ticker) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Invalid ticker. Must be 1-20 alphanumeric characters, optionally with '.' separators."
            })),
        );
    }

    if payload.quantity <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Quantity must be a positive integer."
            })),
        );
    }

    if payload.entry_price.is_some_and(|price| price <= 0.0) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Entry price must be a positive number."
            })),
        );
    }

    match crate::endpoints::share::model_catalog::ready_tickers().await {
        Ok(ready) if ready.contains(&ticker) => {}
        Ok(_) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "code": 422,
                    "message": "Este ticker no tiene suficiente histórico para generar predicciones."
                })),
            );
        }
        Err(error) => {
            tracing::error!("Failed to validate model-ready ticker: {}", error);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "code": 503, "message": "No se pudo validar el ticker." })),
            );
        }
    }

    let rows = sqlx::query_as::<_, (i32, String)>(
        r#"
        SELECT id, ticker
        FROM shares
        WHERE ticker = $1
        "#,
    )
    .bind(&ticker)
    .fetch_one(&pool)
    .await;

    let (share_id, ticker) = match rows {
        Ok(row) => row,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": 404, "message": "Ticker not found." })),
            );
        }
    };

    // Every ledger entry needs a price. If the caller didn't provide one,
    // resolve it live so entry_price is never left NULL for new
    // purchases (only pre-existing rows from before this ledger existed
    // can still have a NULL entry_price).
    let resolved_entry_price = match payload.entry_price {
        Some(price) => price,
        None => match fetch_current_price(&ticker).await {
            Ok(price) => price,
            Err(err) => {
                tracing::error!(
                    "Failed to resolve current price for {} on create: {:?}",
                    ticker,
                    err
                );
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
        },
    };

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("Failed to start transaction for share creation: {}", err);
            return internal_error();
        }
    };

    let insert_result = sqlx::query_as::<_, (i32, i32, i32, i32, Option<f64>, DateTime<Utc>)>(
        r#"
        INSERT INTO user_shares (user_id, share_id, quantity, entry_price)
        VALUES ($1, $2, $3, $4)
        RETURNING id, user_id, share_id, quantity, entry_price, created_at
        "#,
    )
    .bind(auth_user.user_id)
    .bind(share_id)
    .bind(payload.quantity)
    .bind(resolved_entry_price)
    .fetch_one(&mut *tx)
    .await;

    let (id, user_id, _share_id, quantity, entry_price, created_at) = match insert_result {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "code": 409,
                    "message": "Share already exists for that ticker. Use PUT to update the quantity."
                })),
            );
        }
        Err(err) => {
            tracing::error!("Failed to insert share: {}", err);
            return internal_error();
        }
    };

    if let Err(err) = record_operation(
        &mut tx,
        user_id,
        share_id,
        OperationType::Buy,
        quantity,
        resolved_entry_price,
    )
    .await
    {
        tracing::error!("Failed to record buy operation: {}", err);
        return internal_error();
    }

    if let Err(err) = tx.commit().await {
        tracing::error!("Failed to commit share creation: {}", err);
        return internal_error();
    }

    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "user_id": user_id,
            "ticker": ticker,
            "quantity": quantity,
            "entry_price": entry_price,
            "created_at": created_at,
        })),
    )
}

fn internal_error() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
    )
}

fn is_valid_ticker(ticker: &str) -> bool {
    if ticker.is_empty() || ticker.len() > MAX_TICKER_LEN {
        return false;
    }
    ticker
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.')
        && ticker.chars().any(|c| c.is_ascii_alphanumeric())
}
