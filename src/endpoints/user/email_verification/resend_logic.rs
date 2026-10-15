use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use super::token::{dispatch_verification_email, store_new_token_for_resend};
use super::verify_logic::EmailVerificationResponse;

#[derive(Deserialize, ToSchema)]
pub struct ResendVerificationRequest {
    #[schema(example = "financiar186@gmail.com")]
    pub email: String,
}

/// Mensaje unico para no revelar si el mail esta registrado o ya verificado.
const GENERIC_MESSAGE: &str =
    "If the account exists and is not verified yet, a new verification email was sent";

#[utoipa::path(
    post,
    path = "/verify-email/resend",
    request_body = ResendVerificationRequest,
    responses(
        (status = 200, description = "Respuesta generica: se reenvia el mail solo si la cuenta existe, no esta verificada y no se envio otro en el ultimo minuto", body = EmailVerificationResponse, example = json!({
            "code": 200,
            "message": GENERIC_MESSAGE
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
    Json(payload): Json<ResendVerificationRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let email = payload.email.trim();

    let user = sqlx::query_as::<_, (i32, String)>(
        "SELECT id, full_name FROM users WHERE email = $1 AND email_verified = FALSE",
    )
    .bind(email)
    .fetch_optional(&pool)
    .await;

    let resend = match user {
        Ok(Some((user_id, full_name))) => store_new_token_for_resend(&pool, user_id)
            .await
            .map(|token| token.map(|token| (full_name, token))),
        Ok(None) => Ok(None),
        Err(err) => Err(err),
    };

    match resend {
        Ok(Some((full_name, token))) => {
            dispatch_verification_email(email.to_string(), full_name, token);
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!("Failed to resend verification email: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "code": 200,
            "message": GENERIC_MESSAGE
        })),
    )
}
