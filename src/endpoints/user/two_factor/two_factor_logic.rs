use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use super::totp;
use crate::auth::middleware::AuthUser;

#[derive(Deserialize, ToSchema)]
pub struct TwoFactorCodeRequest {
    #[schema(example = "123456")]
    pub code: String,
}

fn internal_error() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
    )
}

fn invalid_code() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "code": 400,
            "message": "Invalid verification code"
        })),
    )
}

/// Devuelve `(totp_secret, two_factor_enabled)` del usuario autenticado.
async fn load_state(
    pool: &PgPool,
    user_id: i32,
) -> Result<Option<(Option<String>, bool)>, sqlx::Error> {
    sqlx::query_as::<_, (Option<String>, bool)>(
        "SELECT totp_secret, two_factor_enabled FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

#[utoipa::path(
    post,
    path = "/user/2fa/setup",
    responses(
        (status = 200, description = "Secreto generado; escanear el QR y confirmar con /user/2fa/enable", example = json!({
            "code": 200,
            "secret": "JBSWY3DPEHPK3PXP...",
            "otpauth_url": "otpauth://totp/FinanciAr:user@mail.com?secret=...",
            "qr_base64": "iVBORw0KGgo..."
        })),
        (status = 409, description = "2FA ya esta activado", example = json!({
            "code": 409,
            "message": "Two-factor authentication is already enabled"
        })),
        (status = 401, description = "Missing or invalid authentication token")
    ),
    security(("bearer_auth" = [])),
    tag = "Two-Factor"
)]
pub async fn setup(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    match load_state(&pool, auth_user.user_id).await {
        Ok(Some((_, true))) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "code": 409,
                    "message": "Two-factor authentication is already enabled"
                })),
            )
        }
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": 404, "message": "User not found" })),
            )
        }
        Err(err) => {
            tracing::error!("Failed to load 2FA state: {}", err);
            return internal_error();
        }
    }

    let secret = totp::generate_secret();
    let (url, qr) = match (
        totp::otpauth_url(&secret, &auth_user.email),
        totp::qr_base64(&secret, &auth_user.email),
    ) {
        (Ok(url), Ok(qr)) => (url, qr),
        (Err(err), _) | (_, Err(err)) => {
            tracing::error!("Failed to build TOTP enrollment: {}", err);
            return internal_error();
        }
    };

    // El secreto queda pendiente (two_factor_enabled = false) hasta que el
    // usuario demuestre con /enable que su app lo genera bien.
    if let Err(err) = sqlx::query("UPDATE users SET totp_secret = $1 WHERE id = $2")
        .bind(&secret)
        .bind(auth_user.user_id)
        .execute(&pool)
        .await
    {
        tracing::error!("Failed to store TOTP secret: {}", err);
        return internal_error();
    }

    (
        StatusCode::OK,
        Json(json!({
            "code": 200,
            "secret": secret,
            "otpauth_url": url,
            "qr_base64": qr
        })),
    )
}

#[utoipa::path(
    post,
    path = "/user/2fa/enable",
    request_body = TwoFactorCodeRequest,
    responses(
        (status = 200, description = "2FA activado", example = json!({ "code": 200, "message": "Two-factor authentication enabled" })),
        (status = 400, description = "Codigo invalido o setup no iniciado"),
        (status = 401, description = "Missing or invalid authentication token")
    ),
    security(("bearer_auth" = [])),
    tag = "Two-Factor"
)]
pub async fn enable(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
    Json(payload): Json<TwoFactorCodeRequest>,
) -> impl IntoResponse {
    let secret = match load_state(&pool, auth_user.user_id).await {
        Ok(Some((Some(secret), _))) => secret,
        Ok(Some((None, _))) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": 400,
                    "message": "Two-factor setup has not been started"
                })),
            )
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": 404, "message": "User not found" })),
            )
        }
        Err(err) => {
            tracing::error!("Failed to load 2FA state: {}", err);
            return internal_error();
        }
    };

    if !totp::verify_code(&secret, &auth_user.email, &payload.code) {
        return invalid_code();
    }

    match sqlx::query("UPDATE users SET two_factor_enabled = TRUE WHERE id = $1")
        .bind(auth_user.user_id)
        .execute(&pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "Two-factor authentication enabled"
            })),
        ),
        Err(err) => {
            tracing::error!("Failed to enable 2FA: {}", err);
            internal_error()
        }
    }
}

#[utoipa::path(
    post,
    path = "/user/2fa/disable",
    request_body = TwoFactorCodeRequest,
    responses(
        (status = 200, description = "2FA desactivado", example = json!({ "code": 200, "message": "Two-factor authentication disabled" })),
        (status = 400, description = "Codigo invalido o 2FA no activado"),
        (status = 401, description = "Missing or invalid authentication token")
    ),
    security(("bearer_auth" = [])),
    tag = "Two-Factor"
)]
pub async fn disable(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
    Json(payload): Json<TwoFactorCodeRequest>,
) -> impl IntoResponse {
    let secret = match load_state(&pool, auth_user.user_id).await {
        Ok(Some((Some(secret), true))) => secret,
        Ok(Some(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": 400,
                    "message": "Two-factor authentication is not enabled"
                })),
            )
        }
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": 404, "message": "User not found" })),
            )
        }
        Err(err) => {
            tracing::error!("Failed to load 2FA state: {}", err);
            return internal_error();
        }
    };

    if !totp::verify_code(&secret, &auth_user.email, &payload.code) {
        return invalid_code();
    }

    match sqlx::query(
        "UPDATE users SET two_factor_enabled = FALSE, totp_secret = NULL WHERE id = $1",
    )
    .bind(auth_user.user_id)
    .execute(&pool)
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "code": 200,
                "message": "Two-factor authentication disabled"
            })),
        ),
        Err(err) => {
            tracing::error!("Failed to disable 2FA: {}", err);
            internal_error()
        }
    }
}
