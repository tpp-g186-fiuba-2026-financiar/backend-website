use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use sqlx::PgPool;

#[utoipa::path(
    post,
    path = "/share/sync",
    responses(
        (status = 200, description = "Tickers synced successfully", example = json!({
            "added": 3,
            "total": 10
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Share"
)]
pub async fn handler(State(pool): State<PgPool>) -> impl IntoResponse {
    let data_collector_url = std::env::var("DATA_COLLECTOR_URL").unwrap();

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/available-tickers", data_collector_url))
        .send()
        .await;

    let tickers = match response {
        Ok(res) => {
            let body: serde_json::Value = match res.json().await {
                Ok(b) => b,
                Err(err) => {
                    tracing::error!("Failed to parse data collector response: {}", err);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(
                            json!({ "code": 500, "message": "Failed to parse data collector response." }),
                        ),
                    );
                }
            };
            match body["message"]["tickers"].as_array() {
                Some(t) => t
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>(),
                None => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(
                            json!({ "code": 500, "message": "Unexpected response format from data collector." }),
                        ),
                    );
                }
            }
        }
        Err(err) => {
            tracing::error!("Failed to reach data collector: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "code": 500, "message": "Could not reach data collector." })),
            );
        }
    };

    let total = tickers.len();
    let mut added = 0;

    for ticker in &tickers {
        let result = sqlx::query(
            r#"INSERT INTO shares (ticker) VALUES ($1) ON CONFLICT (ticker) DO NOTHING"#,
        )
        .bind(ticker)
        .execute(&pool)
        .await;

        match result {
            Ok(r) => added += r.rows_affected() as usize,
            Err(err) => tracing::warn!("Failed to insert ticker {}: {}", ticker, err),
        }
    }

    (
        StatusCode::OK,
        Json(json!({ "added": added, "total": total })),
    )
}
