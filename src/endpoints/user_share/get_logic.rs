use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct ShareItem {
    pub id: i32,
    pub user_id: i32,
    pub ticker: String,
    pub quantity: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct ListSharesResponse {
    pub shares: Vec<ShareItem>,
}

#[utoipa::path(
    get,
    path = "/shares",
    responses(
        (status = 200, description = "List of shares declared by the authenticated user", body = ListSharesResponse, example = json!({
            "shares": [
                {
                    "id": 1,
                    "user_id": 42,
                    "ticker": "GGAL",
                    "quantity": 10,
                    "created_at": "2026-05-28T12:00:00Z"
                }
            ]
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
    let rows = sqlx::query_as::<_, (i32, i32, String, i32, DateTime<Utc>)>(
        r#"
        SELECT id, user_id, ticker, quantity, created_at
        FROM shares
        WHERE user_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => {
            let shares: Vec<_> = rows
                .into_iter()
                .map(|(id, user_id, ticker, quantity, created_at)| {
                    json!({
                        "id": id,
                        "user_id": user_id,
                        "ticker": ticker,
                        "quantity": quantity,
                        "created_at": created_at,
                    })
                })
                .collect();

            (StatusCode::OK, Json(json!({ "shares": shares })))
        }
        Err(err) => {
            tracing::error!("Failed to list shares: {}", err);
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
