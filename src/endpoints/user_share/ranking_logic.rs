use axum::{http::StatusCode, response::IntoResponse, Extension};
use std::time::Duration;

use crate::auth::middleware::AuthUser;
use crate::endpoints::upstream;

#[utoipa::path(
    get,
    path = "/user/ranking",
    responses(
        (status = 200, description = "Ranking del dia del panel segun XGBoost (P de suba, puntaje 1-10 y senal), tal cual lo devuelve api-ml en GET /predict/ranking", body = serde_json::Value),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "Perfil de riesgo faltante o vencido"),
        (status = 502, description = "api-ml no pudo responder", example = json!({"code": 502, "message": "No se pudo contactar a api-ml"}))
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(Extension(_auth_user): Extension<AuthUser>) -> impl IntoResponse {
    let Some(api_ml_url) = upstream::api_ml_url() else {
        tracing::error!("API_ML_URL no esta configurada");
        return upstream::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "An unexpected error occurred. Please try again later.",
        );
    };
    let client = upstream::client(Duration::from_secs(90));
    let url = format!("{api_ml_url}/predict/ranking");
    let label = "ranking del dia".to_string();
    let response = upstream::send_with_retry(|| client.get(&url), &label).await;
    upstream::passthrough_api_ml(response, &label).await
}
