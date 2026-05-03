use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct HelloResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub message: &'static str,
}

#[utoipa::path(
    get,
    path = "/hello",
    responses(
        (status = 200, description = "Simple Hello Response!", body = HelloResponse, example = json!({
            "status": "ok",
            "message": "Hello from Financiar backend!",
            "version": env!("CARGO_PKG_VERSION")
        }))
    ),
    tag = "General"
)]
pub async fn handler() -> Json<HelloResponse> {
    Json(HelloResponse {
        status: "ok",
        message: "Hello from Financiar backend!",
        version: env!("CARGO_PKG_VERSION"),
    })
}
