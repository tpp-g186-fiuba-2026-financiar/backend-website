use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    Extension,
};
use serde::Deserialize;
use std::time::Duration;
use utoipa::IntoParams;

use crate::auth::middleware::AuthUser;
use crate::endpoints::upstream;

#[derive(Deserialize, IntoParams)]
pub struct VolatilityQuery {
    /// Ruedas a pronosticar (1 a 60; default el de api-ml)
    horizon: Option<u8>,
}

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/volatility",
    params(("ticker" = String, Path, description = "Ticker (ej: GGAL)"), VolatilityQuery),
    responses(
        (status = 200, description = "Pronostico GARCH(1,1) de cuanto puede moverse el precio (no la direccion), tal cual lo devuelve api-ml en GET /predict/volatility/{ticker}", body = serde_json::Value),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "Perfil de riesgo faltante o vencido"),
        (status = 422, description = "Pocas ruedas para ajustar GARCH"),
        (status = 502, description = "api-ml no pudo responder", example = json!({"code": 502, "message": "No se pudo contactar a api-ml"}))
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
    Query(query): Query<VolatilityQuery>,
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
    let url = format!("{api_ml_url}/predict/volatility/{ticker}");
    let label = format!("volatilidad de {ticker}");
    let horizon = query.horizon.map(|h| h.to_string());
    let response = upstream::send_with_retry(
        || {
            let request = client.get(&url);
            match &horizon {
                Some(h) => request.query(&[("horizon", h)]),
                None => request,
            }
        },
        &label,
    )
    .await;
    upstream::passthrough_api_ml(response, &label).await
}
