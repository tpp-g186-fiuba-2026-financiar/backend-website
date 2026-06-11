use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::middleware::AuthUser;

#[utoipa::path(
    delete,
    path = "/shares/{id}",
    params(
        ("id" = i32, Path, description = "ID of the share to delete")
    ),
    responses(
        (status = 204, description = "Share deleted successfully"),
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
) -> impl IntoResponse {
    let result = sqlx::query("DELETE FROM shares WHERE id = $1 AND user_id = $2")
        .bind(share_id)
        .bind(auth_user.user_id)
        .execute(&pool)
        .await;

    match result {
        Ok(res) if res.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 404,
                "message": "Share not found"
            })),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!("Failed to delete share: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            )
                .into_response()
        }
    }
}
