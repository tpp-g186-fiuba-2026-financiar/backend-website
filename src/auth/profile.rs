use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde_json::json;

use crate::auth::middleware::AuthUser;
use crate::configuration::config::AppState;

#[derive(Clone, Debug)]
pub struct CurrentProfile {
    pub risk_profile: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn require_profile(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // require_auth runs before us and puts AuthUser here. If it's missing,
    // the router is wired wrong — fail loud in logs, generic 500 to the client.
    let Some(auth_user) = request.extensions().get::<AuthUser>().cloned() else {
        tracing::error!("require_profile ran without AuthUser in extensions — check layer order");
        return internal_error();
    };

    let row = sqlx::query!(
        r#"
        SELECT risk_profile, expires_at
        FROM user_investing_profiles
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        auth_user.user_id
    )
    .fetch_optional(&state.pool)
    .await;

    match row {
        Ok(Some(p)) if p.expires_at > Utc::now() => {
            request.extensions_mut().insert(CurrentProfile {
                risk_profile: p.risk_profile,
                expires_at: p.expires_at,
            });
            next.run(request).await
        }
        Ok(Some(p)) => profile_error(
            "RISK_PROFILE_EXPIRED",
            "Tu perfil de riesgo vencio. Completalo de nuevo para continuar.",
            Some(p.expires_at),
        ),
        Ok(None) => profile_error(
            "RISK_PROFILE_REQUIRED",
            "Configura tu perfil de riesgo antes de usar esta funcionalidad.",
            None,
        ),
        Err(err) => {
            tracing::error!("risk profile lookup failed: {}", err);
            internal_error()
        }
    }
}

fn profile_error(code: &str, message: &str, expired_at: Option<DateTime<Utc>>) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "code": 403,
            "error": code,
            "message": message,
            "expired_at": expired_at,
        })),
    )
        .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
    )
        .into_response()
}
