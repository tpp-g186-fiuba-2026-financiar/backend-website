// Data-colector falso para los tests que dan de alta acciones: POST
// /user/shares valida el ticker contra /model-ready-tickers y busca el precio
// en /historical-data. Contra el servicio real de Render el test fallaba con
// 503 cada vez que el servicio estaba dormido. Cada archivo de tests/ es su
// propio proceso, asi que setear DATA_COLLECTOR_URL aca no afecta a otros.
#![allow(dead_code)]

use axum::{extract::Path, http::StatusCode, routing::post, Json, Router};
use serde_json::{json, Value};

const DAY_MS: i64 = 86_400_000;
pub const PRICE: f64 = 120.0;
pub const READY: [&str; 5] = ["GGAL", "YPFD", "BMA", "PAMP", "TXAR"];

static STUB: std::sync::Once = std::sync::Once::new();

async fn ready() -> Json<Value> {
    Json(json!({ "message": { "tickers": READY } }))
}

async fn history(Path(ticker): Path<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !READY.contains(&ticker.as_str()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let today = chrono::Utc::now().timestamp_millis().div_euclid(DAY_MS);
    let data: Vec<Value> = (0..40)
        .map(|offset| json!({ "close_amount": PRICE.to_string(), "ts": (today - offset) * DAY_MS }))
        .collect();
    Json(json!({ "data": data })).into_response()
}

/// Levanta el stub una vez por proceso, en un hilo con su propio runtime (cada
/// #[tokio::test] destruye el suyo al terminar).
pub fn start_collector_stub() {
    STUB.call_once(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let app = Router::new()
                    .route("/model-ready-tickers", post(ready))
                    .route("/historical-data/{ticker}", post(history));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let address = rx.recv().unwrap();
        std::env::set_var("DATA_COLLECTOR_URL", format!("http://{address}"));
    });
}
