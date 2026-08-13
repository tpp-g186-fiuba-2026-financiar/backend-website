use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct ShareTickerItem {
    pub id: i32,
    pub ticker: String,
    pub predictable: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ListAllSharesResponse {
    pub shares: Vec<ShareTickerItem>,
}

#[utoipa::path(
    get,
    path = "/shares/all",
    responses(
        (status = 200, description = "List of all available share tickers", body = ListAllSharesResponse, example = json!({
            "shares": [
                { "id": 1, "ticker": "GGAL" },
                { "id": 2, "ticker": "YPF" }
            ]
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    tag = "Share"
)]
pub async fn handler(State(pool): State<PgPool>) -> impl IntoResponse {
    let ready = match super::model_catalog::ready_tickers().await {
        Ok(ready) => ready,
        Err(error) => {
            tracing::error!("Failed to list model-ready tickers: {}", error);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({ "code": 503, "message": "No se pudo validar qué tickers son predecibles." }),
                ),
            );
        }
    };
    let rows = sqlx::query_as::<_, (i32, String)>(
        r#"
        SELECT id, ticker
        FROM shares
        ORDER BY ticker ASC
        "#,
    )
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => {
            let shares: Vec<_> = rows
                .into_iter()
                .map(|(id, ticker)| {
                    let predictable = ready.contains(&ticker);
                    json!({ "id": id, "ticker": ticker, "predictable": predictable })
                })
                .collect();
            (StatusCode::OK, Json(json!({ "shares": shares })))
        }
        Err(err) => {
            tracing::error!("Failed to list all shares: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            )
        }
    }
}
