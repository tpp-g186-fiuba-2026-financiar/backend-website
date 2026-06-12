use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

const MAX_TICKER_LEN: usize = 20;

#[derive(Deserialize, ToSchema)]
pub struct CreateShareRequest {
    #[schema(example = "GGAL")]
    pub ticker: String,
    #[schema(example = 10)]
    pub quantity: i32,
}

#[derive(Serialize, ToSchema)]
pub struct CreateShareResponse {
    pub id: i32,
    pub user_id: i32,
    pub ticker: String,
    pub quantity: i32,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    post,
    path = "/shares",
    request_body = CreateShareRequest,
    responses(
        (status = 201, description = "Share added to the authenticated user's portfolio", body = CreateShareResponse, example = json!({
            "id": 1,
            "user_id": 42,
            "ticker": "GGAL",
            "quantity": 10,
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

    let rows = sqlx::query_as::<_, (i32, String)>(
        r#"
        SELECT id, ticker
        FROM shares
        WHERE ticker = $1
        "#,
    )
    .bind(ticker)
    .fetch_one(&pool)
    .await;

    let row = match rows {
        Ok(row) => row,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": 404, "message": "Ticker not found." })),
            );
        }
    };
    let share_id = row.0;

    let insert_result = sqlx::query_as::<_, (i32, i32, i32, i32, DateTime<Utc>)>(
        r#"
        INSERT INTO user_shares (user_id, share_id, quantity)
        VALUES ($1, $2, $3)
        RETURNING id, user_id, share_id, quantity, created_at
        "#,
    )
    .bind(auth_user.user_id)
    .bind(&share_id)
    .bind(payload.quantity)
    .fetch_one(&pool)
    .await;
    match insert_result {
        Ok((id, user_id, share_id, quantity, created_at)) => (
            StatusCode::CREATED,
            Json(json!({
                "id": id,
                "user_id": user_id,
                "share_id": share_id,
                "quantity": quantity,
                "created_at": created_at,
            })),
        ),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "code": 409,
                "message": "Share already exists for that ticker. Use PUT to update the quantity."
            })),
        ),
        Err(err) => {
            tracing::error!("Failed to insert share: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            )
        }
    }
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
