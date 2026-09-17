use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

#[derive(Serialize, ToSchema)]
pub struct GetUserResponse {
    pub id: i32,
    pub email: String,
    pub full_name: String,
    pub risk_profile: Option<String>,
    pub has_to_redo_risk_profile: bool,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

#[utoipa::path(
    get,
    path = "/user",
    responses(
        (status = 200, description = "Authenticated user retrieved successfully", body = GetUserResponse, example = json!({
            "id": 1,
            "email": "financiar186@gmail.com",
            "full_name": "John Doe",
            "risk_profile": "moderate",
            "has_to_redo_risk_profile": false,
            "is_active": true,
            "created_at": "2026-05-13T12:00:00Z"
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
) -> impl IntoResponse {
    let row = sqlx::query!(
        r#"
        SELECT id, email, full_name, is_active, created_at
        FROM users
        WHERE users.id = $1
        "#,
        auth_user.user_id
    )
    .fetch_optional(&pool)
    .await;

    let (risk_profile, has_to_redo_risk_profile) = sqlx::query!(
        r#"
        SELECT 
        risk_profile, 
        (NOW() > created_at + INTERVAL '6 months') AS has_to_redo_risk_profile
        FROM user_investing_profiles
        WHERE user_id = $1 
        ORDER BY created_at DESC 
        LIMIT 1;
        "#,
        auth_user.user_id
    )
    .fetch_optional(&pool)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("Database query failed during /user lookup: {}", e);
        None
    })
    .map(|row| (row.risk_profile, row.has_to_redo_risk_profile))
    .unwrap_or_else(|| (None, false));

    match row {
        Ok(Some(user)) => (
            StatusCode::OK,
            Json(json!({
                "id": user.id,
                "email": user.email,
                "full_name": user.full_name,
                "risk_profile": user.risk_profile,
                "has_to_redo_risk_profile": has_to_redo_risk_profile,
                "is_active": user.is_active,
                "created_at": user.created_at,
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
            tracing::error!("Database query failed during /user lookup: {}", err);
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
