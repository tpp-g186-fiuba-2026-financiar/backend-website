use argon2::{
    password_hash::{self, rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::endpoints::user::registration::validators::{
    email_validator::EmailValidator, password_validator::PasswordValidator, Validator,
};

#[derive(Deserialize, ToSchema)]
pub struct RegisterUserRequest {
    #[schema(example = "financiar186@gmail.com")]
    pub email: String,
    #[schema(example = "StrongPassword123!")]
    pub password: String,
    #[schema(example = "John Doe")]
    pub full_name: String,

    /// Optional. Must be 'conservative', 'moderate', or 'aggressive'
    #[schema(example = "moderate")]
    pub risk_profile: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct RegisterUserResponse {
    pub code: u16,
    pub message: String,
}

#[utoipa::path(
    post,
    path = "/register",
    request_body = RegisterUserRequest,
    responses(
        (status = 200, description = "User registered successfully", body = RegisterUserResponse, example = json!({
            "code": 200,
            "message": "User registered successfully"
        })),
        (status = 400, description = "Invalid input data", body = RegisterUserResponse, examples(
            ("Invalid Email" = (
                summary = "Triggered when the email is invalid (either wrong format or not provided)",
                value = json!({
                    "code": 400,
                    "message": "Invalid e-mail"
                })
            )),
            ("Weak Password" = (
                summary = "Triggered when password requirements are not met",
                value = json!({
                    "code": 400,
                    "message": "Password must be at least 8 characters long and contain a mix of letters, numbers, and special characters"
                })
            )),
            ("Invalid Risk Profile" = (
                summary = "Triggered when risk profile is not a permitted value",
                value = json!({
                    "code": 400,
                    "message": "Invalid risk profile. Must be 'conservative', 'moderate', or 'aggressive'."
                })
            ))
        )),
        (status = 500, description = "Internal server error", body = RegisterUserResponse, example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
        (status = 409, description = "User already exists for that email", body = RegisterUserResponse, example = json!({
            "code": 409,
            "message": "User already exists for that email!"
        }))
    ),
    tag = "Authentication"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    axum::Json(payload): axum::Json<RegisterUserRequest>,
) -> axum::response::Json<serde_json::Value> {
    // --- 1. Validation ---
    let validators: Vec<Box<dyn Validator>> =
        vec![Box::new(EmailValidator::new()), Box::new(PasswordValidator)];

    for validator in validators {
        if let Err(err) = validator.validate(&payload) {
            return axum::response::Json(json!({
                "code": 400,
                "message": err
            }));
        }
    }

    // Lucas comments: this can be easily be moved to a validators, not sure still how will be implemented yet.
    if let Some(ref profile) = payload.risk_profile {
        let valid_profiles = ["conservative", "moderate", "aggressive"];
        if !valid_profiles.contains(&profile.as_str()) {
            return axum::response::Json(json!({
                "code": 400,
                "message": "Invalid risk profile. Must be 'conservative', 'moderate', or 'aggressive'."
            }));
        }
    }

    // --- 2. Check Existing User ---
    let existing_user_result = sqlx::query("SELECT id FROM users WHERE email = $1")
        .bind(payload.email.trim())
        .fetch_optional(&pool)
        .await;

    match existing_user_result {
        Ok(Some(_)) => {
            return axum::response::Json(json!({
                "code": 409,
                "message": "User already exists for that email!"
            }));
        }
        Ok(None) => {} // User does not exist, proceed
        Err(err) => {
            tracing::error!("Database query failed during existence check: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    }

    let hashed_password = match hash_password(&payload.password) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!("Failed to hash password: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "Failed to process user credentials."
            }));
        }
    };

    let insert_result = sqlx::query(
        r#"
        INSERT INTO users (email, password_hash, full_name, risk_profile) 
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(payload.email.trim())
    .bind(hashed_password)
    .bind(payload.full_name)
    .bind(payload.risk_profile)
    .execute(&pool)
    .await;

    if let Err(err) = insert_result {
        tracing::error!("Failed to insert new user: {}", err);
        return axum::response::Json(json!({
            "code": 500,
            "message": "An unexpected error occurred while saving the user."
        }));
    }

    // --- 5. Success ---
    axum::response::Json(json!({
        "code": 200,
        "message": "User registered successfully"
    }))
}

pub fn hash_password(password: &str) -> Result<String, password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();

    // Hash the password along with the generated salt
    match argon2.hash_password(password.as_bytes(), &salt) {
        Ok(hash) => Ok(hash.to_string()),
        Err(err) => Err(err),
    }
}
