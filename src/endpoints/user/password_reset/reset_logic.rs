use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::endpoints::user::email_verification::token::hash_token;
use crate::endpoints::user::registration::registration_logic::hash_password;
use crate::endpoints::user::registration::validators::password_validator::validate_password;

#[derive(Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    /// Token recibido en el link del mail de reseteo.
    #[schema(example = "3f9a1c...e07b")]
    pub token: String,
    /// Nueva contrasena. Mismas reglas que en el registro.
    #[schema(example = "NewStrongPassword123!")]
    pub new_password: String,
}

#[derive(Serialize, ToSchema)]
pub struct PasswordResetResponse {
    pub code: u16,
    pub message: String,
}

#[utoipa::path(
    post,
    path = "/reset-password",
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, description = "Contrasena actualizada; el token queda invalidado", body = PasswordResetResponse, example = json!({
            "code": 200,
            "message": "Password updated successfully"
        })),
        (status = 400, description = "Token invalido, vencido o ya usado; o contrasena que no cumple los requisitos", body = PasswordResetResponse, examples(
            ("Invalid Token" = (
                summary = "Triggered when the token is unknown, expired or already used",
                value = json!({
                    "code": 400,
                    "message": "Invalid or expired reset token"
                })
            )),
            ("Weak Password" = (
                summary = "Triggered when password requirements are not met",
                value = json!({
                    "code": 400,
                    "message": "Password must be at least 8 characters long"
                })
            ))
        )),
        (status = 500, description = "Internal server error", body = PasswordResetResponse, example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Authentication"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Json(payload): Json<ResetPasswordRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let token = payload.token.trim();
    if token.is_empty() {
        return invalid_token();
    }

    // Se valida antes de tocar la base: una contrasena debil no gasta el token.
    if let Err(message) = validate_password(&payload.new_password) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": message
            })),
        );
    }

    let hashed_password = match hash_password(&payload.new_password) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!("Failed to hash password during reset: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "Failed to process user credentials."
                })),
            );
        }
    };

    // Un solo UPDATE atomico: cambia la contrasena y quema el token a la vez,
    // asi dos requests con el mismo token no pueden ganar las dos.
    let result = sqlx::query(
        r#"
        UPDATE users
        SET password_hash = $1,
            password_reset_token_hash = NULL,
            password_reset_expires_at = NULL
        WHERE password_reset_token_hash = $2
          AND password_reset_expires_at > NOW()
        "#,
    )
    .bind(hashed_password)
    .bind(hash_token(token))
    .execute(&pool)
    .await;

    match result {
        Ok(done) if done.rows_affected() > 0 => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "Password updated successfully"
            })),
        ),
        Ok(_) => invalid_token(),
        Err(err) => {
            tracing::error!("Failed to reset password: {}", err);
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
            "message": "Invalid or expired reset token"
        })),
    )
}
