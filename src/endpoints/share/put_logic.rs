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

#[derive(Deserialize, ToSchema)]
pub struct UpdateShareRequest {
    #[schema(example = 25)]
    pub quantity: i32,
}

#[derive(Serialize, ToSchema)]
pub struct UpdateShareResponse {
    pub id: i32,
    pub user_id: i32,
    pub ticker: String,
    pub quantity: i32,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    put,
    path = "/shares/{id}",
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

    let result = sqlx::query_as::<_, (i32, i32, String, i32, DateTime<Utc>)>(
        r#"
        UPDATE shares
        SET quantity = $1
        WHERE id = $2 AND user_id = $3
        RETURNING id, user_id, ticker, quantity, created_at
        "#,
    )
    .bind(payload.quantity)
    .bind(share_id)
    .bind(auth_user.user_id)
    .fetch_optional(&pool)
    .await;

    match result {
        Ok(Some((id, user_id, ticker, quantity, created_at))) => (
            StatusCode::OK,
            Json(json!({
                "id": id,
                "user_id": user_id,
                "ticker": ticker,
                "quantity": quantity,
                "created_at": created_at,
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 404,
                "message": "Share not found"
            })),
        ),
        Err(err) => {
            tracing::error!("Failed to update share: {}", err);
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
