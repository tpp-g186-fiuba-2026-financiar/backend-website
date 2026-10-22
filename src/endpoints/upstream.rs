//! Helpers compartidos para hablar con data-colector y api-ml: URL base por
//! env, client con timeout, reintentos en 5xx/transporte y errores en el
//! formato `{code, message}` que usa el resto de la API.

use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use std::time::Duration;

const DEFAULT_DATA_COLLECTOR_URL: &str = "https://data-colector-k7h6.onrender.com";
const ATTEMPTS: usize = 3;

pub fn data_collector_url() -> String {
    std::env::var("DATA_COLLECTOR_URL")
        .unwrap_or_else(|_| DEFAULT_DATA_COLLECTOR_URL.into())
        .trim_end_matches('/')
        .to_string()
}

pub fn api_ml_url() -> Option<String> {
    std::env::var("API_ML_URL")
        .ok()
        .map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

pub fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("reqwest client")
}

/// Manda el request armado por `build` hasta 3 veces mientras falle el
/// transporte o el upstream responda 5xx.
pub async fn send_with_retry<F>(build: F, label: &str) -> reqwest::Result<reqwest::Response>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut response = build().send().await;
    for attempt in 2..=ATTEMPTS {
        let retry = match &response {
            Err(_) => true,
            Ok(value) => value.status().is_server_error(),
        };
        if !retry {
            break;
        }
        tracing::warn!("Reintentando {} (intento {})", label, attempt);
        response = build().send().await;
    }
    response
}

pub fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({"code": status.as_u16(), "message": message})),
    )
}

pub fn bad_gateway(message: &str) -> (StatusCode, Json<Value>) {
    error(StatusCode::BAD_GATEWAY, message)
}

/// Reenvia la respuesta de api-ml: 2xx tal cual; 4xx con el mismo status y
/// el `detail` de FastAPI traducido a `{code, message}`; 5xx (ya agotados
/// los reintentos) o transporte caido como 502.
pub async fn passthrough_api_ml(
    response: reqwest::Result<reqwest::Response>,
    label: &str,
) -> (StatusCode, Json<Value>) {
    match response {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) => (StatusCode::OK, Json(body)),
            Err(err) => {
                tracing::error!("Respuesta invalida de api-ml en {}: {}", label, err);
                bad_gateway("Respuesta invalida de api-ml")
            }
        },
        Ok(response) if response.status().is_client_error() => {
            let status =
                StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let detail = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|body| body.get("detail").and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| "api-ml rechazo el pedido".to_string());
            tracing::warn!("api-ml respondio {} en {}: {}", status, label, detail);
            error(status, &detail)
        }
        Ok(response) => {
            tracing::warn!("api-ml respondio {} en {}", response.status(), label);
            bad_gateway("api-ml no pudo responder")
        }
        Err(err) => {
            tracing::error!("No se pudo contactar a api-ml en {}: {}", label, err);
            bad_gateway("No se pudo contactar a api-ml")
        }
    }
}

/// data-colector responde siempre HTTP 200 y pone el status logico en el
/// body (`{"status": 404, "message": {"error": "..."}}`). Devuelve el
/// mensaje de error si el body indica falla.
pub fn data_collector_body_error(body: &Value) -> Option<String> {
    let status = body.get("status").and_then(Value::as_u64)?;
    if status == 200 {
        return None;
    }
    let message = body
        .pointer("/message/error")
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| format!("data-colector respondio status {status}"));
    Some(message)
}

/// Acepta numeros que vienen como string (Decimal serializado) o como numero.
pub fn as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_error_detects_logical_status() {
        assert_eq!(data_collector_body_error(&json!({"status": 200})), None);
        assert_eq!(data_collector_body_error(&json!({"data": []})), None);
        assert_eq!(
            data_collector_body_error(&json!({"status": 404, "message": {"error": "sin datos"}})),
            Some("sin datos".into())
        );
        assert_eq!(
            data_collector_body_error(&json!({"status": 500})),
            Some("data-colector respondio status 500".into())
        );
    }

    #[test]
    fn as_f64_parses_strings_and_numbers() {
        assert_eq!(as_f64(&json!("1.5")), Some(1.5));
        assert_eq!(as_f64(&json!(2)), Some(2.0));
        assert_eq!(as_f64(&json!("NaN")), None);
        assert_eq!(as_f64(&json!(null)), None);
    }
}
