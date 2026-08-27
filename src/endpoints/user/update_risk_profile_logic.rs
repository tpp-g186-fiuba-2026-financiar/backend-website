use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Deserialize, ToSchema)]
pub struct UpdateRiskProfileRequest {
    pub risk_profile: String,
}

#[utoipa::path(
    patch,
    path = "/user/risk-profile",
    request_body = UpdateRiskProfileRequest,
    responses(
        (status = 200, description = "Risk profile actualizado correctamente", example = json!({
            "id": 1,
            "risk_profile": "moderate"
        })),
        (status = 400, description = "Valor de risk_profile inválido", example = json!({
            "code": 400,
            "message": "Invalid risk_profile value"
        })),
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
    Json(payload): Json<UpdateRiskProfileRequest>,
) -> impl IntoResponse {
    // Whitelist de valores válidos — ajustalo a los que uses en tu dominio
    const VALID_PROFILES: [&str; 3] = ["conservative", "moderate", "aggressive"];

    if !VALID_PROFILES.contains(&payload.risk_profile.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Invalid risk_profile value"
            })),
        );
    }

    let row = sqlx::query(
        r#"
        UPDATE users
        SET risk_profile = $1
        WHERE id = $2
        RETURNING id, risk_profile
        "#,
    )
    .bind(payload.risk_profile)
    .bind(auth_user.user_id)
    .fetch_optional(&pool)
    .await;

    match row {
        Ok(Some(_)) => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "code": 404,
                "message": "User not found"
            })),
        ),
        Err(err) => {
            tracing::error!(
                "Database query failed during /user/risk-profile update: {}",
                err
            );
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
