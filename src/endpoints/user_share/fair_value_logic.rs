use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension};
use std::time::Duration;

use crate::auth::middleware::AuthUser;
use crate::endpoints::upstream;

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/fair-value",
    params(("ticker" = String, Path, description = "Ticker (ej: GGAL)")),
    responses(
        (status = 200, description = "Precio estimado por regresion lineal (recta de tendencia del ultimo año, distancia del precio actual y canal de +-1/+-2 desvios), tal cual lo devuelve api-ml en GET /predict/fair-value/{ticker}", body = serde_json::Value),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "Perfil de riesgo faltante o vencido"),
        (status = 422, description = "Pocas ruedas para ajustar la regresion", example = json!({"code": 422, "message": "hacen falta al menos 60 ruedas para la regresion; hay 17"})),
        (status = 502, description = "api-ml no pudo responder", example = json!({"code": 502, "message": "No se pudo contactar a api-ml"}))
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();
    let Some(api_ml_url) = upstream::api_ml_url() else {
        tracing::error!("API_ML_URL no esta configurada");
        return upstream::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "An unexpected error occurred. Please try again later.",
        );
    };
    let client = upstream::client(Duration::from_secs(60));
    let url = format!("{api_ml_url}/predict/fair-value/{ticker}");
    let label = format!("precio estimado de {ticker}");
    let response = upstream::send_with_retry(|| client.get(&url), &label).await;
    upstream::passthrough_api_ml(response, &label).await
}
