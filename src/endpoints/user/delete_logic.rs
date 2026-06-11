use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::middleware::AuthUser;

#[utoipa::path(
    delete,
    path = "/user",
    responses(
        (status = 204, description = "Authenticated user (and all related data) deleted successfully"),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "Authenticated user no longer exists", example = json!({
            "code": 404,
            "message": "User not found"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "User"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    // shares se borran solas por el ON DELETE CASCADE de la FK shares.user_id
    let result = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(auth_user.user_id)
        .execute(&pool)
        .await;

    match result {
        Ok(res) if res.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 404,
                "message": "User not found"
            })),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!("Failed to delete user {}: {}", auth_user.user_id, err);
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
