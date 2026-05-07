use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

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
}

#[utoipa::path(
    post,
    path = "/login",
    request_body = LoginUserRequest,
    responses(
        (status = 200, description = "User logged in successfully", body = LoginUserResponse, example = json!({
            "code": 200,
            "message": "Login successful"
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
    State(pool): State<PgPool>,
    axum::Json(payload): axum::Json<LoginUserRequest>,
) -> axum::response::Json<serde_json::Value> {
    if payload.email.trim().is_empty() || payload.password.is_empty() {
        return axum::response::Json(json!({
            "code": 400,
            "message": "Email and password are required"
        }));
    }

    let user_result = sqlx::query!(
        "SELECT password_hash FROM users WHERE email = $1",
        payload.email
    )
    .fetch_optional(&pool)
    .await;

    let stored_hash = match user_result {
        Ok(Some(row)) => row.password_hash,
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

    axum::response::Json(json!({
        "code": 200,
        "message": "Login successful"
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
