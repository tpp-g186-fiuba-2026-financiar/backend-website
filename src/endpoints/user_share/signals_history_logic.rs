use axum::{extract::State, http::StatusCode, response::IntoResponse, Extension, Json};
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;

use crate::auth::middleware::AuthUser;
use crate::endpoints::upstream;

const SIGNALS_LIMIT: &str = "200";

#[utoipa::path(
    get,
    path = "/user/signals/history",
    responses(
        (status = 200, description = "Historial de senales emitidas por los modelos para los tickers del usuario, tal cual lo devuelve api-ml en GET /signals/history. Sin tenencias devuelve `{\"signals\": []}` sin consultar a api-ml", body = serde_json::Value),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "Perfil de riesgo faltante o vencido"),
        (status = 502, description = "api-ml no pudo responder", example = json!({"code": 502, "message": "No se pudo contactar a api-ml"})),
        (status = 500, description = "Internal server error")
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
) -> impl IntoResponse {
    let tickers = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT s.ticker
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.user_id = $1
        ORDER BY s.ticker ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    let tickers = match tickers {
        Ok(tickers) => tickers,
        Err(err) => {
            tracing::error!("Failed to list tickers for signals history: {}", err);
            return upstream::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "An unexpected error occurred. Please try again later.",
            );
        }
    };
    if tickers.is_empty() {
        return (StatusCode::OK, Json(json!({"signals": []})));
    }

    let Some(api_ml_url) = upstream::api_ml_url() else {
        tracing::error!("API_ML_URL no esta configurada");
        return upstream::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "An unexpected error occurred. Please try again later.",
        );
    };
    let client = upstream::client(Duration::from_secs(30));
    let url = format!("{api_ml_url}/signals/history");
    let symbols = tickers.join(",");
    let response = upstream::send_with_retry(
        || {
            client
                .get(&url)
                .query(&[("symbols", symbols.as_str()), ("limit", SIGNALS_LIMIT)])
        },
        "historial de senales",
    )
    .await;
    upstream::passthrough_api_ml(response, "historial de senales").await
}
