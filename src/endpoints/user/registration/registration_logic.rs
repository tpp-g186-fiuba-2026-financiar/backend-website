use argon2::{
    password_hash::{self, rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::endpoints::user::email_verification::token::{
    dispatch_verification_email, store_new_token,
};
use crate::endpoints::user::registration::validators::{
    email_validator::EmailValidator, password_validator::PasswordValidator, Validator,
};
use crate::observability::json_with_body_code;

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
        (status = 200, description = "User registered successfully. A verification email is sent and the account can't log in until it is verified (see /verify-email)", body = RegisterUserResponse, example = json!({
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
) -> axum::response::Response {
    // HTTP 200 con el codigo real en el body: se marca para el log de acceso
    json_with_body_code(register(pool, payload).await.0)
}

async fn register(
    pool: PgPool,
    payload: RegisterUserRequest,
) -> axum::response::Json<serde_json::Value> {
    // --- 1. Validation ---
    let validators: Vec<Box<dyn Validator>> =
        vec![Box::new(EmailValidator::new()), Box::new(PasswordValidator)];

    for validator in validators {
        if let Err(err) = validator.validate(&payload) {
            tracing::info!("Registro rechazado por validacion: {}", err);
            return axum::response::Json(json!({
                "code": 400,
                "message": err
            }));
        }
    }

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
            tracing::info!("Registro rechazado: ya existe un usuario con ese email");
            return axum::response::Json(json!({
                "code": 409,
                "message": "User already exists for that email!"
            }));
        }
        Ok(None) => {}
        Err(err) => {
            tracing::error!(
                "Fallo la consulta de usuario existente en el registro: {}",
                err
            );
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    }

    let hashed_password = match hash_password(&payload.password) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::error!("No se pudo hashear la contrasena: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "Failed to process user credentials."
            }));
        }
    };

    // --- 3. User + risk profile in a single transaction ---
    let mut transaction = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("No se pudo iniciar la transaccion del registro: {}", err);
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred. Please try again later."
            }));
        }
    };

    let user_id_result = sqlx::query_scalar!(
        r#"
        INSERT INTO users (email, password_hash, full_name)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
        payload.email.trim(),
        hashed_password,
        payload.full_name
    )
    .fetch_one(&mut *transaction)
    .await;

    let user_id = match user_id_result {
        Ok(id) => id,
        Err(err) => {
            tracing::error!("No se pudo insertar el usuario: {}", err);
            // transaction drops here -> rolled back automatically
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred while saving the user."
            }));
        }
    };

    if let Some(ref profile) = payload.risk_profile {
        let insert_risk_profile_result = sqlx::query!(
            r#"
            INSERT INTO user_investing_profiles (user_id, risk_profile)
            VALUES ($1, $2)
            "#,
            user_id,
            profile
        )
        .execute(&mut *transaction)
        .await;

        if let Err(err) = insert_risk_profile_result {
            tracing::error!(user_id, "No se pudo insertar el perfil de riesgo: {}", err);
            // transaction drops here -> user insert is rolled back too
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred while saving the risk profile."
            }));
        }
    }

    let verification_token = match store_new_token(&mut *transaction, user_id).await {
        Ok(token) => token,
        Err(err) => {
            tracing::error!(
                user_id,
                "No se pudo guardar el token de verificacion de email: {}",
                err
            );
            // transaction drops here -> user insert is rolled back too
            return axum::response::Json(json!({
                "code": 500,
                "message": "An unexpected error occurred while saving the user."
            }));
        }
    };

    if let Err(err) = transaction.commit().await {
        tracing::error!(
            user_id,
            "No se pudo commitear la transaccion del registro: {}",
            err
        );
        return axum::response::Json(json!({
            "code": 500,
            "message": "An unexpected error occurred while saving the user."
        }));
    }

    // --- 4. Verification email (the account can't log in until it's verified) ---
    dispatch_verification_email(
        payload.email.trim().to_string(),
        payload.full_name.clone(),
        verification_token,
    );

    // --- 5. Success ---
    tracing::info!(user_id, "Usuario registrado");
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
