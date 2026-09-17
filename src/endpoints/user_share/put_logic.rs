use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;
use crate::endpoints::user_share::user_share_operations::{
    fetch_current_price, record_operation, OperationType,
};

#[derive(Deserialize, ToSchema)]
pub struct UpdateShareRequest {
    #[schema(example = 25)]
    pub quantity: i32,
    /// Precio de entrada (precio pagado por accion). Solo se usa si esta
    /// actualizacion sube la cantidad (compra parcial); si se omite en
    /// ese caso, se resuelve con el precio de mercado actual. Si la
    /// actualizacion baja la cantidad (venta parcial), este campo se
    /// ignora: el precio de venta siempre es el de mercado actual.
    #[serde(default)]
    #[schema(example = 1520.50)]
    pub entry_price: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct UpdateShareResponse {
    pub id: i32,
    pub user_id: i32,
    pub ticker: String,
    pub quantity: i32,
    pub entry_price: Option<f64>,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    put,
    path = "/user/shares/{id}",
    params(
        ("id" = i32, Path, description = "ID of the share to update")
    ),
    request_body = UpdateShareRequest,
    responses(
        (status = 200, description = "Share updated successfully", body = UpdateShareResponse, example = json!({
            "id": 1,
            "user_id": 42,
            "ticker": "GGAL",
            "quantity": 25,
            "entry_price": 1520.50,
            "created_at": "2026-05-28T12:00:00Z"
        })),
        (status = 400, description = "Invalid input data", example = json!({
            "code": 400,
            "message": "Quantity must be a positive integer."
        })),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "Share not found for the authenticated user", example = json!({
            "code": 404,
            "message": "Share not found"
        })),
        (status = 502, description = "Failed to resolve a current price for the buy/sell delta", example = json!({
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
    Path(share_id): Path<i32>,
    Json(payload): Json<UpdateShareRequest>,
) -> impl IntoResponse {
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

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("Failed to start transaction for share update: {}", err);
            return internal_error();
        }
    };

    // Lock the row and read its current quantity/ticker before deciding
    // whether this update is a partial buy, a partial sell, or neither.
    let current = sqlx::query_as::<_, (i32, String, i32)>(
        r#"
        SELECT us.share_id, s.ticker, us.quantity
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.id = $1 AND us.user_id = $2
        FOR UPDATE
        "#,
    )
    .bind(share_id)
    .bind(auth_user.user_id)
    .fetch_optional(&mut *tx)
    .await;

    let (position_share_id, ticker, old_quantity) = match current {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "code": 404,
                    "message": "Share not found"
                })),
            );
        }
        Err(err) => {
            tracing::error!("Failed to load share for update: {}", err);
            return internal_error();
        }
    };

    let delta = payload.quantity - old_quantity;

    let operation = if delta > 0 {
        let price = match payload.entry_price {
            Some(price) => price,
            None => match fetch_current_price(&ticker).await {
                Ok(price) => price,
                Err(err) => {
                    tracing::error!(
                        "Failed to resolve current price for {} on partial buy: {:?}",
                        ticker,
                        err
                    );
                    return bad_gateway(&ticker);
                }
            },
        };
        Some((OperationType::Buy, delta, price))
    } else if delta < 0 {
        let price = match fetch_current_price(&ticker).await {
            Ok(price) => price,
            Err(err) => {
                tracing::error!(
                    "Failed to resolve current price for {} on partial sell: {:?}",
                    ticker,
                    err
                );
                return bad_gateway(&ticker);
            }
        };
        Some((OperationType::Sell, -delta, price))
    } else {
        None
    };

    let result = sqlx::query_as::<_, (i32, i32, i32, Option<f64>, DateTime<Utc>)>(
        r#"
        UPDATE user_shares
        SET quantity = $1, entry_price = COALESCE($2, entry_price)
        WHERE id = $3
        RETURNING id, user_id, quantity, entry_price, created_at
        "#,
    )
    .bind(payload.quantity)
    .bind(payload.entry_price)
    .bind(share_id)
    .fetch_one(&mut *tx)
    .await;

    let (id, user_id, quantity, entry_price, created_at) = match result {
        Ok(row) => row,
        Err(err) => {
            tracing::error!("Failed to update share: {}", err);
            return internal_error();
        }
    };

    if let Some((op_type, op_quantity, op_price)) = operation {
        if let Err(err) = record_operation(
            &mut tx,
            user_id,
            position_share_id,
            op_type,
            op_quantity,
            op_price,
        )
        .await
        {
            tracing::error!("Failed to record partial buy/sell operation: {}", err);
            return internal_error();
        }
    }

    if let Err(err) = tx.commit().await {
        tracing::error!("Failed to commit share update: {}", err);
        return internal_error();
    }

    (
        StatusCode::OK,
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

fn bad_gateway(ticker: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "code": 502,
            "message": format!(
                "Could not retrieve current price data for {}. Please try again later.",
                ticker
            )
        })),
    )
}
