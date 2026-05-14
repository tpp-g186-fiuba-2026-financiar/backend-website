use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tower_sessions::Session;
use utoipa::ToSchema;

use crate::auth::jwt::JwtConfig;

#[derive(Deserialize, ToSchema)]
pub struct LoginUserRequest {
    #[schema(example = "financiar186@gmail.com")]
    pub email: String,
    #[schema(example = "StrongPassword123!")]
    pub password: String,
}

#[derive(Serialize, ToSchema)]
pub struct LoginUserResponse {
    pub code: u16,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
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
        (status = 401, description = "Invalid credentials", body = LoginUserResponse, example = json!({
            "code": 401,
            "message": "Invalid email or password"
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
) -> axum::response::Json<serde_json::Value> {
    if payload.email.trim().is_empty() || payload.password.is_empty() {
        return axum::response::Json(json!({
            "code": 400,
            "message": "Email and password are required"
        }));
    }

    let email = payload.email.trim();

    let user_result = sqlx::query("SELECT password_hash, id FROM users WHERE email = $1")
        .bind(email)
        .fetch_optional(&pool)
        .await;

    let (stored_hash, serial_id) = match user_result {
        Ok(Some(row)) => {
            let (password_hashed, id): (String, i32) = match sqlx::FromRow::from_row(&row) {
                Ok(data) => data,
                Err(err) => {
                    tracing::error!("Failed to parse user data during login: {}", err);
                    return axum::response::Json(json!({
                        "code": 500,
                        "message": "An unexpected error occurred. Please try again later."
                    }));
                }
            };
            (password_hashed, id)
        }
        Ok(None) => {
            return axum::response::Json(json!({
                "code": 401,
                "message": "Invalid email or password"
            }));
        }
        Err(err) => {
            tracing::error!("Database query failed during login lookup: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    };

    if !verify_password(&payload.password, &stored_hash) {
        return axum::response::Json(json!({
            "code": 401,
            "message": "Invalid email or password"
        }));
    }

    let token = match jwt_config.encode_token(serial_id, email) {
        Ok(t) => t,
        Err(err) => {
            tracing::error!("Failed to issue JWT for user {}: {}", serial_id, err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    };

    if let Err(err) = session.insert("user_id", serial_id).await {
        tracing::error!("Failed to create session during login: {}", err);
        return axum::response::Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }));
    }

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
            tracing::error!("Stored password hash is not parseable: {}", err);
            return false;
        }
    };

    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok()
}