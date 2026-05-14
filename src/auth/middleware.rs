use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::auth::jwt::{Claims, JwtConfig};

#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
}

impl From<Claims> for AuthUser {
    fn from(claims: Claims) -> Self {
        Self {
            user_id: claims.sub,
            email: claims.email,
        }
    }
}

pub async fn require_auth(
    State(jwt_config): State<JwtConfig>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => return unauthorized("Missing or malformed Authorization header"),
    };

    let claims = match jwt_config.decode_token(token) {
        Ok(claims) => claims,
        Err(err) => {
            tracing::warn!("JWT validation failed: {}", err);
            return unauthorized("Invalid or expired token");
        }
    };

    request.extensions_mut().insert(AuthUser::from(claims));
    next.run(request).await
}

fn extract_bearer_token(request: &Request) -> Option<&str> {
    let header = request.headers().get(AUTHORIZATION)?.to_str().ok()?;
    header.strip_prefix("Bearer ").map(|t| t.trim())
}

fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "code": 401,
            "message": message
        })),
    )
        .into_response()
}
