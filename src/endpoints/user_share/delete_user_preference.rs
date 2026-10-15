use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::middleware::AuthUser;
use crate::endpoints::user_share::post_user_preferences::is_valid_ticker;

#[utoipa::path(
    delete,
    path = "/user/preferences/{stock}",
    params(
        ("stock" = String, Path, description = "Stock ticker whose preference should be deleted")
    ),
    responses(
        (status = 204, description = "Preference deleted successfully"),
        (status = 400, description = "Invalid stock ticker", example = json!({
            "code": 400,
            "message": "Invalid stock ticker."
        })),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "Preference not found for the authenticated user and stock", example = json!({
            "code": 404,
            "message": "Preference not found"
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
    Path(stock): Path<String>,
) -> impl IntoResponse {
    let stock = stock.trim().to_uppercase();
    if !is_valid_ticker(&stock) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Invalid stock ticker."
            })),
        )
            .into_response();
    }

    let result = sqlx::query("DELETE FROM user_preferences WHERE user_id = $1 AND stock = $2")
        .bind(auth_user.user_id)
        .bind(stock)
        .execute(&pool)
        .await;

    match result {
        Ok(result) if result.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 404,
                "message": "Preference not found"
            })),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!("Failed to delete user stock model preference: {}", err);
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
