use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use std::time::{Duration, Instant};
use utoipa::ToSchema;

use crate::endpoints::upstream;

const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MODAL_HEALTH_URL: &str = "https://matimorales01--lstm-trend-model-main.modal.run";

#[derive(Serialize, ToSchema)]
pub struct ServiceHealth {
    /// data-colector | api-ml | modal
    pub name: String,
    /// up | down | not_configured
    pub status: String,
    /// Latencia del chequeo en ms (null si no se llego a consultar)
    pub latency_ms: Option<u64>,
    /// Status HTTP que respondio el servicio, si respondio
    pub http_status: Option<u16>,
    /// Motivo de la falla, si la hubo
    pub error: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct AdminHealthResponse {
    /// true si todos los servicios estan `up`
    pub all_up: bool,
    pub services: Vec<ServiceHealth>,
}

#[utoipa::path(
    get,
    path = "/admin/health",
    responses(
        (status = 200, description = "Estado y latencia de los servicios externos (timeout 5s cada uno, en paralelo)", body = AdminHealthResponse, example = json!({
            "all_up": false,
            "services": [
                {"name": "data-colector", "status": "up", "latency_ms": 120, "http_status": 200, "error": null},
                {"name": "api-ml", "status": "up", "latency_ms": 85, "http_status": 200, "error": null},
                {"name": "modal", "status": "down", "latency_ms": 5001, "http_status": null, "error": "timeout"}
            ]
        })),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "El usuario no es administrador", example = json!({"code": 403, "error": "ADMIN_REQUIRED", "message": "Esta funcionalidad es solo para administradores."}))
    ),
    security(("bearer_auth" = [])),
    tag = "Admin"
)]
pub async fn handler() -> impl IntoResponse {
    let client = upstream::client(HEALTH_TIMEOUT);
    let data_collector = format!("{}/health", upstream::data_collector_url());
    let api_ml = upstream::api_ml_url().map(|url| format!("{url}/health"));
    // Modal no expone /health: cualquier respuesta < 500 del endpoint del
    // modelo (aunque sea 4xx por faltar el ticker) significa que esta arriba.
    let modal = std::env::var("MODAL_HEALTH_URL")
        .or_else(|_| std::env::var("MODAL_LSTM_URL"))
        .unwrap_or_else(|_| DEFAULT_MODAL_HEALTH_URL.into());

    let (data_collector, api_ml, modal) = tokio::join!(
        check(&client, "data-colector", Some(data_collector), false),
        check(&client, "api-ml", api_ml, false),
        check(&client, "modal", Some(modal), true),
    );
    let services = vec![data_collector, api_ml, modal];
    let all_up = services.iter().all(|service| service.status == "up");
    (
        StatusCode::OK,
        Json(AdminHealthResponse { all_up, services }),
    )
}

async fn check(
    client: &reqwest::Client,
    name: &str,
    url: Option<String>,
    any_response_is_up: bool,
) -> ServiceHealth {
    let Some(url) = url else {
        return ServiceHealth {
            name: name.into(),
            status: "not_configured".into(),
            latency_ms: None,
            http_status: None,
            error: Some("URL no configurada".into()),
        };
    };
    let started = Instant::now();
    let response = client.get(&url).send().await;
    let latency_ms = Some(started.elapsed().as_millis() as u64);
    match response {
        Ok(response) => {
            let status = response.status();
            let up = status.is_success() || (any_response_is_up && !status.is_server_error());
            ServiceHealth {
                name: name.into(),
                status: if up { "up" } else { "down" }.into(),
                latency_ms,
                http_status: Some(status.as_u16()),
                error: (!up).then(|| format!("respondio {status}")),
            }
        }
        Err(error) => ServiceHealth {
            name: name.into(),
            status: "down".into(),
            latency_ms,
            http_status: None,
            error: Some(if error.is_timeout() {
                "timeout".into()
            } else {
                "no se pudo conectar".into()
            }),
        },
    }
}
