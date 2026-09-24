use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::time::Duration;

#[utoipa::path(
	get,
	path = "/shares/{ticker}/sector",
	params(("ticker" = String, Path, description = "Ticker de la accion (ej: GGAL)")),
	responses(
		(status = 200, description = "Sector de la accion obtenido de data-collector"),
		(status = 502, description = "data-collector no pudo responder")
	),
	tag = "Share"
)]
pub async fn handler(Path(ticker): Path<String>) -> impl IntoResponse {
	let ticker = ticker.trim().to_uppercase();
	let base = std::env::var("DATA_COLLECTOR_URL")
		.unwrap_or_else(|_| "https://data-colector.onrender.com".into());
	let client = match reqwest::Client::builder()
		.timeout(Duration::from_secs(45))
		.build()
	{
		Ok(client) => client,
		Err(error) => {
			tracing::error!("No se pudo crear cliente para sector de {}: {}", ticker, error);
			return bad_gateway("No se pudo contactar a data-collector");
		}
	};
	let url = format!(
		"{}/ticker/sector/{}",
		base.trim_end_matches('/'),
		ticker
	);

	match client.get(url).send().await {
		Ok(response) if response.status().is_success() => match response.json::<Value>().await {
			Ok(body) => (StatusCode::OK, Json(body)),
			Err(error) => {
				tracing::error!("Sector invalido para {}: {}", ticker, error);
				bad_gateway("data-collector devolvio una respuesta invalida")
			}
		},
		Ok(response) => {
			tracing::warn!("data-collector respondio {} para sector de {}", response.status(), ticker);
			bad_gateway("data-collector no pudo obtener el sector")
		}
		Err(error) => {
			tracing::error!("No se pudo obtener sector de {}: {}", ticker, error);
			bad_gateway("No se pudo contactar a data-collector")
		}
	}
}

fn bad_gateway(message: &str) -> (StatusCode, Json<Value>) {
	(
		StatusCode::BAD_GATEWAY,
		Json(json!({"code": 502, "message": message})),
	)
}
