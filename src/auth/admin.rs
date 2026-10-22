use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::auth::middleware::AuthUser;
use crate::configuration::config::AppState;

pub async fn require_admin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // require_auth runs before us and puts AuthUser here. If it's missing,
    // the router is wired wrong — fail loud in logs, generic 500 to the client.
    let Some(auth_user) = request.extensions().get::<AuthUser>().cloned() else {
        tracing::error!("require_admin ran without AuthUser in extensions — check layer order");
        return internal_error();
    };

    let role = sqlx::query_scalar::<_, String>("SELECT role FROM users WHERE id = $1")
        .bind(auth_user.user_id)
        .fetch_optional(&state.pool)
        .await;

    match role {
        Ok(Some(role)) if role == "admin" => next.run(request).await,
        Ok(_) => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "code": 403,
                "error": "ADMIN_REQUIRED",
                "message": "Esta funcionalidad es solo para administradores.",
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("role lookup failed: {}", err);
            internal_error()
        }
    }
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
