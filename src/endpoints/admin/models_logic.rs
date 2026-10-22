use axum::{http::StatusCode, response::IntoResponse};
use std::time::Duration;

use crate::endpoints::upstream;

#[utoipa::path(
    get,
    path = "/admin/models",
    responses(
        (status = 200, description = "Diagnostico de los modelos (backtests + metricas OOS de XGBoost), tal cual lo devuelve api-ml en GET /models/diagnostics", body = serde_json::Value),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "El usuario no es administrador", example = json!({"code": 403, "error": "ADMIN_REQUIRED", "message": "Esta funcionalidad es solo para administradores."})),
        (status = 502, description = "api-ml no pudo responder", example = json!({"code": 502, "message": "No se pudo contactar a api-ml"}))
    ),
    security(("bearer_auth" = [])),
    tag = "Admin"
)]
pub async fn handler() -> impl IntoResponse {
    let Some(api_ml_url) = upstream::api_ml_url() else {
        tracing::error!("API_ML_URL no esta configurada");
        return upstream::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "An unexpected error occurred. Please try again later.",
        );
    };
    let client = upstream::client(Duration::from_secs(30));
    let url = format!("{api_ml_url}/models/diagnostics");
    let response = upstream::send_with_retry(|| client.get(&url), "diagnostico de modelos").await;
    upstream::passthrough_api_ml(response, "diagnostico de modelos").await
}
