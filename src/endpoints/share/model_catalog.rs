use std::{collections::HashSet, time::Duration};

use serde_json::Value;

pub async fn ready_tickers() -> Result<HashSet<String>, String> {
    let base = std::env::var("DATA_COLLECTOR_URL")
        .unwrap_or_else(|_| "https://data-colector.onrender.com".into());
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| error.to_string())?
        .post(format!(
            "{}/model-ready-tickers",
            base.trim_end_matches('/')
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let body = response.json::<Value>().await.map_err(|e| e.to_string())?;
    body.get("message")
        .and_then(|message| message.get("tickers"))
        .and_then(Value::as_array)
        .ok_or_else(|| "respuesta invalida de data-colector".to_string())
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}
