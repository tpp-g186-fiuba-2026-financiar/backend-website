//! Chequeo de PIN compartido para el tablero de retro.
//!
//! No es autenticacion de usuario (no hay JWT ni sesion, a proposito: las
//! tarjetas son anonimas). Es solo una traba minima para que la ruta no
//! quede abierta al mundo, dado que este repo es publico. Si `RETRO_PIN` no
//! esta seteada, se rechaza todo en vez de quedar abierto por defecto.

use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};

const HEADER_NAME: &str = "x-retro-pin";

pub fn check(headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    let configured = std::env::var("RETRO_PIN").unwrap_or_default();
    if configured.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "code": 503, "message": "RETRO_PIN no esta configurada" })),
        ));
    }

    let provided = headers
        .get(HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided == configured {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "code": 401, "message": "PIN incorrecto" })),
        ))
    }
}
