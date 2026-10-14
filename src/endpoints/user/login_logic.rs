use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tower_sessions::Session;
use utoipa::ToSchema;

use crate::auth::jwt::JwtConfig;
use crate::endpoints::user::two_factor::totp::verify_code;
use crate::observability::json_with_body_code;

/// (password_hash, id, totp_secret, two_factor_enabled, email_verified)
type UserLoginRow = (String, i32, Option<String>, bool, bool);

#[derive(Deserialize, ToSchema)]
pub struct LoginUserRequest {
    #[schema(example = "financiar186@gmail.com")]
    pub email: String,
    #[schema(example = "StrongPassword123!")]
    pub password: String,
    /// Codigo de 6 digitos de la app autenticadora; solo se exige si el
    /// usuario tiene 2FA activado.
    #[schema(example = "123456")]
    #[serde(default)]
    pub totp_code: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LoginUserResponse {
    pub code: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub two_factor_required: Option<bool>,
    /// true cuando la contrasena es correcta pero el mail todavia no se
    /// verifico (ver /verify-email y /verify-email/resend).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_verification_required: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/login",
    request_body = LoginUserRequest,
    responses(
        (status = 200, description = "User logged in successfully", body = LoginUserResponse, example = json!({
            "code": 200,
            "message": "Login successful",
            "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
        })),
        (status = 400, description = "Missing required fields", body = LoginUserResponse, example = json!({
            "code": 400,
            "message": "Email and password are required"
        })),
        (status = 401, description = "Invalid credentials, or missing/invalid 2FA code (two_factor_required = true)", body = LoginUserResponse, example = json!({
            "code": 401,
            "message": "Two-factor code required",
            "two_factor_required": true
        })),
        (status = 403, description = "Valid credentials but the email is not verified yet (email_verification_required = true)", body = LoginUserResponse, example = json!({
            "code": 403,
            "message": "Email not verified",
            "email_verification_required": true
        })),
        (status = 500, description = "Internal server error", body = LoginUserResponse, example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Authentication"
)]
pub async fn handler(
    session: Session,
    State(pool): State<PgPool>,
    State(jwt_config): State<JwtConfig>,
    axum::Json(payload): axum::Json<LoginUserRequest>,
) -> axum::response::Response {
    // HTTP 200 con el codigo real en el body: se marca para el log de acceso
    json_with_body_code(login(session, pool, jwt_config, payload).await.0)
}

async fn login(
    session: Session,
    pool: PgPool,
    jwt_config: JwtConfig,
    payload: LoginUserRequest,
) -> axum::response::Json<serde_json::Value> {
    if payload.email.trim().is_empty() || payload.password.is_empty() {
        return axum::response::Json(json!({
            "code": 400,
            "message": "Email and password are required"
        }));
    }

    let email = payload.email.trim();

    let user_result = sqlx::query(
        "SELECT password_hash, id, totp_secret, two_factor_enabled, email_verified FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(&pool)
    .await;

    let (stored_hash, serial_id, totp_secret, two_factor_enabled, verified) = match user_result {
        Ok(Some(row)) => {
            let (password_hashed, id, secret, enabled, verified): UserLoginRow =
                match sqlx::FromRow::from_row(&row) {
                    Ok(data) => data,
                    Err(err) => {
                        tracing::error!(
                            "No se pudieron leer los datos del usuario en el login: {}",
                            err
                        );
                        return axum::response::Json(json!({
                            "code": 500,
                            "message": "An unexpected error occurred. Please try again later."
                        }));
                    }
                };
            (password_hashed, id, secret, enabled, verified)
        }
        Ok(None) => {
            tracing::warn!("Login fallido: no existe un usuario con ese email");
            return axum::response::Json(json!({
                "code": 401,
                "message": "Invalid email or password"
            }));
        }
        Err(err) => {
            tracing::error!("Fallo la consulta del usuario en el login: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    };

    if !verify_password(&payload.password, &stored_hash) {
        tracing::warn!(user_id = serial_id, "Login fallido: contrasena incorrecta");
        return axum::response::Json(json!({
            "code": 401,
            "message": "Invalid email or password"
        }));
    }

    if !verified {
        tracing::info!(user_id = serial_id, "Login rechazado: email sin verificar");
        return axum::response::Json(json!({
            "code": 403,
            "message": "Email not verified",
            "email_verification_required": true
        }));
    }

    if two_factor_enabled {
        let secret = totp_secret.unwrap_or_default();
        match payload.totp_code.as_deref().map(str::trim) {
            None | Some("") => {
                tracing::debug!(user_id = serial_id, "Login pide codigo 2FA");
                return axum::response::Json(json!({
                    "code": 401,
                    "message": "Two-factor code required",
                    "two_factor_required": true
                }));
            }
            Some(code) => {
                if !verify_code(&secret, email, code) {
                    tracing::warn!(user_id = serial_id, "Login fallido: codigo 2FA invalido");
                    return axum::response::Json(json!({
                        "code": 401,
                        "message": "Invalid two-factor code",
                        "two_factor_required": true
                    }));
                }
            }
        }
    }

    let token = match jwt_config.encode_token(serial_id, email) {
        Ok(t) => t,
        Err(err) => {
            tracing::error!(user_id = serial_id, "No se pudo emitir el JWT: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    };

    if let Err(err) = session.insert("user_id", serial_id).await {
        tracing::error!(
            user_id = serial_id,
            "No se pudo crear la sesion en el login: {}",
            err
        );
        return axum::response::Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }));
    }

    tracing::info!(user_id = serial_id, "Login exitoso");
    axum::response::Json(json!({
        "code": 200,
        "message": "Login successful",
        "token": token
    }))
}

fn verify_password(password: &str, stored_hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(stored_hash) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!(
                "El hash de contrasena guardado no se puede parsear: {}",
                err
            );
            return false;
        }
    };

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}
