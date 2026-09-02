use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct AlertSubscriptionResponse {
    pub id: i32,
    pub ticker: Option<String>,
}

/// Suscribe al usuario autenticado a alertas por mail de un ticker puntual.
/// El ticker tiene que estar en la cartera declarada del usuario (misma
/// fuente que `/user/shares/trends`): no tiene sentido alertar sobre una
/// accion que el usuario no sigue.
#[utoipa::path(
    post,
    path = "/user/alerts/subscriptions/{ticker}",
    params(
        ("ticker" = String, Path, description = "Ticker de la cartera del usuario a suscribir")
    ),
    responses(
        (status = 201, description = "Suscripcion creada", body = AlertSubscriptionResponse),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "El ticker no esta en la cartera declarada del usuario", example = json!({
            "code": 404,
            "message": "Share not found in your portfolio"
        })),
        (status = 409, description = "Ya existe una suscripcion a este ticker", example = json!({
            "code": 409,
            "message": "Already subscribed to this ticker"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "Alerts"
)]
pub async fn subscribe_ticker(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();

    let share = sqlx::query_as::<_, (i32,)>(
        r#"
        SELECT s.id
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.user_id = $1 AND s.ticker = $2
        "#,
    )
    .bind(auth_user.user_id)
    .bind(&ticker)
    .fetch_optional(&pool)
    .await;

    let share_id = match share {
        Ok(Some((id,))) => id,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "code": 404,
                    "message": "Share not found in your portfolio"
                })),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!("Failed to look up share for alert subscription: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            )
                .into_response();
        }
    };

    let inserted = sqlx::query_as::<_, (i32,)>(
        r#"
        INSERT INTO alert_subscriptions (user_id, share_id)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(auth_user.user_id)
    .bind(share_id)
    .fetch_one(&pool)
    .await;

    match inserted {
        Ok((id,)) => (
            StatusCode::CREATED,
            Json(json!({ "id": id, "ticker": ticker })),
        )
            .into_response(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "code": 409,
                "message": "Already subscribed to this ticker"
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("Failed to insert alert subscription: {}", err);
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

/// Suscribe al usuario autenticado a alertas por mail de toda su cartera
/// (cualquier ticker que declare, presente o futuro).
#[utoipa::path(
    post,
    path = "/user/alerts/subscriptions/portfolio",
    responses(
        (status = 201, description = "Suscripcion a toda la cartera creada", body = AlertSubscriptionResponse),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 409, description = "Ya existe una suscripcion a la cartera completa", example = json!({
            "code": 409,
            "message": "Already subscribed to your whole portfolio"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "Alerts"
)]
pub async fn subscribe_portfolio(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    let inserted = sqlx::query_as::<_, (i32,)>(
        r#"
        INSERT INTO alert_subscriptions (user_id, share_id)
        VALUES ($1, NULL)
        RETURNING id
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_one(&pool)
    .await;

    match inserted {
        Ok((id,)) => (
            StatusCode::CREATED,
            Json(json!({ "id": id, "ticker": Option::<String>::None })),
        )
            .into_response(),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "code": 409,
                "message": "Already subscribed to your whole portfolio"
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("Failed to insert portfolio alert subscription: {}", err);
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
