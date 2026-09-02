use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct ListAlertSubscriptionsResponse {
    /// True si el usuario esta suscripto a toda su cartera (cualquier ticker
    /// que declare recibe alertas), no solo a los de `tickers`.
    pub portfolio: bool,
    pub tickers: Vec<String>,
}

#[utoipa::path(
    get,
    path = "/user/alerts/subscriptions",
    responses(
        (status = 200, description = "Suscripciones a alertas del usuario autenticado", body = ListAlertSubscriptionsResponse, example = json!({
            "portfolio": false,
            "tickers": ["GGAL", "YPFD"]
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
    tag = "Alerts"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    let rows = sqlx::query_as::<_, (Option<String>,)>(
        r#"
        SELECT s.ticker
        FROM alert_subscriptions a
        LEFT JOIN shares s ON s.id = a.share_id
        WHERE a.user_id = $1
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => {
            let mut portfolio = false;
            let mut tickers = Vec::with_capacity(rows.len());
            for (ticker,) in rows {
                match ticker {
                    Some(ticker) => tickers.push(ticker),
                    None => portfolio = true,
                }
            }
            tickers.sort();
            (
                StatusCode::OK,
                Json(json!({ "portfolio": portfolio, "tickers": tickers })),
            )
        }
        Err(err) => {
            tracing::error!("Failed to list alert subscriptions: {}", err);
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
