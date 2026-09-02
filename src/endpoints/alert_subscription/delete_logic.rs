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
    path = "/user/alerts/subscriptions/{ticker}",
    params(
        ("ticker" = String, Path, description = "Ticker a desuscribir")
    ),
    responses(
        (status = 204, description = "Suscripcion eliminada"),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "No existe una suscripcion a ese ticker", example = json!({
            "code": 404,
            "message": "Subscription not found"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "Alerts"
)]
pub async fn unsubscribe_ticker(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();

    let result = sqlx::query(
        r#"
        DELETE FROM alert_subscriptions
        WHERE user_id = $1
          AND share_id = (SELECT id FROM shares WHERE ticker = $2)
        "#,
    )
    .bind(auth_user.user_id)
    .bind(ticker)
    .execute(&pool)
    .await;

    respond_delete(result)
}

#[utoipa::path(
    delete,
    path = "/user/alerts/subscriptions/portfolio",
    responses(
        (status = 204, description = "Suscripcion a la cartera eliminada"),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "No existe una suscripcion a la cartera completa", example = json!({
            "code": 404,
            "message": "Subscription not found"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "Alerts"
)]
pub async fn unsubscribe_portfolio(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    let result = sqlx::query(
        r#"
        DELETE FROM alert_subscriptions
        WHERE user_id = $1 AND share_id IS NULL
        "#,
    )
    .bind(auth_user.user_id)
    .execute(&pool)
    .await;

    respond_delete(result)
}

fn respond_delete(result: Result<sqlx::postgres::PgQueryResult, sqlx::Error>) -> impl IntoResponse {
    match result {
        Ok(res) if res.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 404,
                "message": "Subscription not found"
            })),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!("Failed to delete alert subscription: {}", err);
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
