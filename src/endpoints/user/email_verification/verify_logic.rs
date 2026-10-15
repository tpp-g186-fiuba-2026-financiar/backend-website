use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use super::token::hash_token;

#[derive(Deserialize, ToSchema)]
pub struct VerifyEmailRequest {
    /// Token recibido en el link del mail de verificacion.
    #[schema(example = "3f9a1c...e07b")]
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct EmailVerificationResponse {
    pub code: u16,
    pub message: String,
}

#[utoipa::path(
    post,
    path = "/verify-email",
    request_body = VerifyEmailRequest,
    responses(
        (status = 200, description = "Cuenta verificada", body = EmailVerificationResponse, example = json!({
            "code": 200,
            "message": "Email verified successfully"
        })),
        (status = 400, description = "Token invalido, vencido o ya usado", body = EmailVerificationResponse, example = json!({
            "code": 400,
            "message": "Invalid or expired verification token"
        })),
        (status = 500, description = "Internal server error", body = EmailVerificationResponse, example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Authentication"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Json(payload): Json<VerifyEmailRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let token = payload.token.trim();
    if token.is_empty() {
        return invalid_token();
    }

    let result = sqlx::query(
        r#"
        UPDATE users
        SET email_verified = TRUE,
            email_verification_token_hash = NULL,
            email_verification_expires_at = NULL
        WHERE email_verification_token_hash = $1
          AND email_verification_expires_at > NOW()
          AND email_verified = FALSE
        "#,
    )
    .bind(hash_token(token))
    .execute(&pool)
    .await;

    match result {
        Ok(done) if done.rows_affected() > 0 => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "Email verified successfully"
            })),
        ),
        Ok(_) => invalid_token(),
        Err(err) => {
            tracing::error!("Failed to verify email token: {}", err);
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

fn invalid_token() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "code": 400,
            "message": "Invalid or expired verification token"
        })),
    )
}
