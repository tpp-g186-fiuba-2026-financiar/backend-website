use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::middleware::AuthUser;
use crate::endpoints::user_share::user_share_operations::{
    fetch_current_price, record_operation, OperationType,
};

#[utoipa::path(
    delete,
    path = "/user/shares/{id}",
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
        (status = 502, description = "Failed to resolve the current price needed to record the sale", example = json!({
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
) -> impl IntoResponse {
    // Look up what's being deleted first, so we know the ticker/quantity
    // to record as a sale before anything is removed.
    let existing = sqlx::query_as::<_, (i32, String, i32)>(
        r#"
        SELECT us.share_id, s.ticker, us.quantity
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.id = $1 AND us.user_id = $2
        "#,
    )
    .bind(share_id)
    .bind(auth_user.user_id)
    .fetch_optional(&pool)
    .await;

    let (position_share_id, ticker, quantity) = match existing {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "code": 404,
                    "message": "Share not found"
                })),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!("Failed to look up share for deletion: {}", err);
            return internal_error();
        }
    };

    // Selling needs a price. Resolve it live *before* deleting anything,
    // so a data-collector failure refuses the delete instead of silently
    // dropping the position with no record of what it sold for.
    let sell_price = match fetch_current_price(&ticker).await {
        Ok(price) => price,
        Err(err) => {
            tracing::error!(
                "Failed to resolve current price for {} on delete: {:?}",
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
            )
                .into_response();
        }
    };

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("Failed to start transaction for share deletion: {}", err);
            return internal_error();
        }
    };

    let result = sqlx::query("DELETE FROM user_shares WHERE id = $1 AND user_id = $2")
        .bind(share_id)
        .bind(auth_user.user_id)
        .execute(&mut *tx)
        .await;

    match result {
        Ok(res) if res.rows_affected() == 0 => {
            // Deleted by a concurrent request between the lookup above
            // and here — treat it as already gone.
            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "code": 404,
                    "message": "Share not found"
                })),
            )
                .into_response()
        }
        Ok(_) => {
            if let Err(err) = record_operation(
                &mut tx,
                auth_user.user_id,
                position_share_id,
                OperationType::Sell,
                quantity,
                sell_price,
            )
            .await
            {
                tracing::error!("Failed to record sell operation: {}", err);
                return internal_error();
            }
            if let Err(err) = tx.commit().await {
                tracing::error!("Failed to commit share deletion: {}", err);
                return internal_error();
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => {
            tracing::error!("Failed to delete share: {}", err);
            internal_error()
        }
    }
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
