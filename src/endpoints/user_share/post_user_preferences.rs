use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

const MAX_TICKER_LEN: usize = 20;

#[derive(Deserialize, ToSchema)]
pub struct SaveUserPreferenceRequest {
    #[schema(example = "GGAL")]
    pub stock: String,
    #[schema(example = "lstm-modal")]
    pub model: String,
}

#[derive(Serialize, ToSchema)]
pub struct SaveUserPreferenceResponse {
    pub user_id: i32,
    pub stock: String,
    pub model: String,
}

#[utoipa::path(
    post,
    path = "/user/preferences",
    request_body = SaveUserPreferenceRequest,
    responses(
        (status = 200, description = "Preference saved for the authenticated user and stock", body = SaveUserPreferenceResponse),
        (status = 400, description = "Invalid stock ticker or unsupported model", example = json!({
            "code": 400,
            "message": "Invalid stock ticker or model."
        })),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "User"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
    Json(payload): Json<SaveUserPreferenceRequest>,
) -> impl IntoResponse {
    let stock = payload.stock.trim().to_uppercase();
    if !is_valid_ticker(&stock) || !is_allowed_model(&payload.model) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": 400,
                "message": "Invalid stock ticker or model."
            })),
        )
            .into_response();
    }

    let result = sqlx::query_as::<_, (i32, String, String)>(
        r#"
        INSERT INTO user_preferences (user_id, stock, model)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, stock)
        DO UPDATE SET model = EXCLUDED.model
        RETURNING user_id, stock, model
        "#,
    )
    .bind(auth_user.user_id)
    .bind(&stock)
    .bind(&payload.model)
    .fetch_one(&pool)
    .await;

    match result {
        Ok((user_id, stock, model)) => (
            StatusCode::OK,
            Json(SaveUserPreferenceResponse {
                user_id,
                stock,
                model,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("Failed to save user stock model preference: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            )
                .into_response()
        }
    }
}

fn is_valid_ticker(stock: &str) -> bool {
    !stock.is_empty()
        && stock.len() <= MAX_TICKER_LEN
        && stock
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '.')
        && stock
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
}

fn is_allowed_model(model: &str) -> bool {
    matches!(
        model,
        "arima-modal" | "lstm-modal" | "transformer-modal" | "xgboost-modal"
    )
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_model, is_valid_ticker};

    #[test]
    fn accepts_only_supported_models() {
        for model in [
            "arima-modal",
            "lstm-modal",
            "transformer-modal",
            "xgboost-modal",
        ] {
            assert!(is_allowed_model(model));
        }
        assert!(!is_allowed_model("unknown-modal"));
    }

    #[test]
    fn validates_ticker_format() {
        assert!(is_valid_ticker("GGAL"));
        assert!(is_valid_ticker("BRK.B"));
        assert!(!is_valid_ticker(""));
        assert!(!is_valid_ticker("INVALID-TICKER"));
    }
}
