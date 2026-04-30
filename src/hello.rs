use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct HelloResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub message: &'static str,
}

pub async fn handler() -> Json<HelloResponse> {
    Json(HelloResponse {
        status: "ok",
        message: "Hello from Financiar backend!",
        version: env!("CARGO_PKG_VERSION"),
    })
}
