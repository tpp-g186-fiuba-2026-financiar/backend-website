use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use super::reset_logic::PasswordResetResponse;
use super::token::{dispatch_password_reset_email, store_new_token};

#[derive(Deserialize, ToSchema)]
pub struct ForgotPasswordRequest {
    #[schema(example = "cuenta_ejemplo@gmail.com")]
    pub email: String,
}

/// Mensaje unico para no revelar si el mail esta registrado.
const GENERIC_MESSAGE: &str =
    "If an account exists for that email, a password reset email was sent";

#[utoipa::path(
    post,
    path = "/forgot-password",
    request_body = ForgotPasswordRequest,
    responses(
        (status = 200, description = "Respuesta generica: el mail se manda solo si la cuenta existe y no se mando otro en el ultimo minuto. El link vence en 1 hora (ver /reset-password)", body = PasswordResetResponse, example = json!({
            "code": 200,
            "message": GENERIC_MESSAGE
        })),
        (status = 400, description = "Email vacio", body = PasswordResetResponse, example = json!({
            "code": 400,
            "message": "Email is required"
        })),
        (status = 500, description = "Internal server error", body = PasswordResetResponse, example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Authentication"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Json(payload): Json<ForgotPasswordRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let email = payload.email.trim();
    if email.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Email is required"
            })),
        );
    }

    let user =
        sqlx::query_as::<_, (i32, String)>("SELECT id, full_name FROM users WHERE email = $1")
            .bind(email)
            .fetch_optional(&pool)
            .await;

    let reset = match user {
        Ok(Some((user_id, full_name))) => store_new_token(&pool, user_id)
            .await
            .map(|token| token.map(|token| (full_name, token))),
        Ok(None) => Ok(None),
        Err(err) => Err(err),
    };

    match reset {
        Ok(Some((full_name, token))) => {
            dispatch_password_reset_email(email.to_string(), full_name, token);
        }
        Err(err) => {
            tracing::error!("Failed to start password reset: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
        _ => {} // En otro caso (en particular Ok(None) significa que el mail no fue encontrado, entonces se continua.)
    }

    (
        StatusCode::OK,
        Json(json!({
            "code": 200,
            "message": GENERIC_MESSAGE
        })),
    )
}
