use std::{collections::HashMap, time::Duration};

use axum::{extract::Path, http::StatusCode, response::IntoResponse, Extension, Json};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::middleware::AuthUser;

const DEFAULT_LSTM_URL: &str = "https://matimorales01--lstm-trend-model-main.modal.run";
const DEFAULT_XGBOOST_URL: &str = "https://matimorales01--xgboost-trend-model-main.modal.run";
const DEFAULT_ARIMA_URL: &str = "https://matimorales01--arima-model-main.modal.run";
const DEFAULT_SVM_URL: &str = "https://matimorales01--svm-model-main.modal.run";
const DEFAULT_GARCH_URL: &str = "https://matimorales01--garch-model-main.modal.run";

#[derive(Serialize, ToSchema)]
pub struct ModelPredictionItem {
    pub available: bool,
    pub signal: Option<String>,
    pub condition: Option<String>,
    pub rsi: Option<f64>,
    pub horizon_days: Option<i64>,
    pub last_close: Option<f64>,
    pub predicted_close: Option<f64>,
    pub as_of: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub backtest: Option<Value>,
    pub volatility_forecast: Option<Value>,
    pub reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CompareTrendsResponse {
    pub symbol: String,
    pub as_of: Option<String>,
    pub default_model: Option<String>,
    pub predictions: HashMap<String, ModelPredictionItem>,
}

#[utoipa::path(
    get,
    path = "/user/shares/{ticker}/trends/compare",
    params(("ticker" = String, Path, description = "Ticker a comparar (ej: GGAL)")),
    responses(
        (status = 200, description = "Compara los modelos productivos de tendencia desplegados en Modal", body = CompareTrendsResponse),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 502, description = "Ningun modelo de Modal pudo responder")
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    Extension(_auth_user): Extension<AuthUser>,
    Path(ticker): Path<String>,
) -> impl IntoResponse {
    let ticker = ticker.trim().to_uppercase();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("reqwest client");
    let lstm_url = std::env::var("MODAL_LSTM_URL").unwrap_or_else(|_| DEFAULT_LSTM_URL.into());
    let xgboost_url =
        std::env::var("MODAL_XGBOOST_URL").unwrap_or_else(|_| DEFAULT_XGBOOST_URL.into());
    let arima_url = std::env::var("MODAL_ARIMA_URL").unwrap_or_else(|_| DEFAULT_ARIMA_URL.into());
    let svm_url = std::env::var("MODAL_SVM_URL").unwrap_or_else(|_| DEFAULT_SVM_URL.into());
    let garch_url = std::env::var("MODAL_GARCH_URL").unwrap_or_else(|_| DEFAULT_GARCH_URL.into());
    let api_ml_url = std::env::var("API_ML_URL").ok();

    let (lstm, xgboost, arima, svm, garch, api_ml_models) = tokio::join!(
        fetch_modal(&client, "lstm-modal", &lstm_url, &ticker),
        fetch_modal(&client, "xgboost-modal", &xgboost_url, &ticker),
        fetch_arima(&client, &arima_url, &ticker),
        fetch_svm(&client, &svm_url, &ticker),
        fetch_garch(&client, &garch_url, &ticker),
        fetch_api_ml_local_models(&client, api_ml_url.as_deref(), &ticker),
    );

    let as_of = lstm
        .get("as_of")
        .or_else(|| xgboost.get("as_of"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut predictions = serde_json::Map::new();
    predictions.insert("lstm-modal".into(), lstm);
    predictions.insert("xgboost-modal".into(), xgboost);
    predictions.insert("arima-modal".into(), arima);
    predictions.insert("svm-modal".into(), svm);
    predictions.insert("garch-modal".into(), garch);
    for (key, value) in api_ml_models {
        predictions.insert(key, value);
    }
    let default_model = pick_best_model(&predictions);
    (
        StatusCode::OK,
        Json(json!({
            "symbol": ticker,
            "as_of": as_of,
            "default_model": default_model,
            "predictions": predictions
        })),
    )
}

/// Elige, entre los modelos con backtest, el de mejor accuracy direccional
/// para este ticker puntual. El "default" ya no es un modelo fijo (antes
/// siempre "lstm-modal"): cada ticker puede tener un ganador distinto segun
/// como le fue prediciendolo. Si ninguno trae metricas todavia (Modal no
/// respondio, poca historia), se cae al default historico.
fn pick_best_model(predictions: &serde_json::Map<String, Value>) -> String {
    predictions
        .iter()
        .filter(|(_, value)| value.get("available").and_then(Value::as_bool) == Some(true))
        .filter_map(|(name, value)| {
            let accuracy = value
                .get("backtest")
                .and_then(|backtest| backtest.get("directional_accuracy"))
                .and_then(Value::as_f64)?;
            Some((name, accuracy))
        })
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| "lstm-modal".to_string())
}

/// Modelos locales de `api-ml` (lstm/xgboost/transformer/arima): un modelo
/// pooleado sobre todo el panel (a diferencia de los `-modal`, que son
/// per-ticker), con backtest walk-forward propio. Se muestran como
/// alternativa junto a los de Modal, no en reemplazo.
async fn fetch_api_ml_local_models(
    client: &reqwest::Client,
    api_ml_url: Option<&str>,
    ticker: &str,
) -> HashMap<String, Value> {
    const LOCAL_MODEL_KEYS: [&str; 4] = ["lstm", "xgboost", "transformer", "arima"];
    let Some(api_ml_url) = api_ml_url else {
        return LOCAL_MODEL_KEYS
            .into_iter()
            .map(|key| (key.to_string(), unavailable("api-ml no esta configurado")))
            .collect();
    };
    let url = format!(
        "{}/predict/trend/compare/{}",
        api_ml_url.trim_end_matches('/'),
        ticker
    );
    let body = match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) => Some(body),
            Err(error) => {
                tracing::error!(
                    "Respuesta invalida de api-ml al comparar tendencias: {}",
                    error
                );
                None
            }
        },
        Ok(response) => {
            tracing::warn!(
                "api-ml respondio {} al comparar tendencias",
                response.status()
            );
            None
        }
        Err(error) => {
            tracing::error!(
                "No se pudo contactar a api-ml para comparar tendencias: {}",
                error
            );
            None
        }
    };
    let predictions = body
        .as_ref()
        .and_then(|b| b.get("predictions"))
        .and_then(Value::as_object);
    LOCAL_MODEL_KEYS
        .into_iter()
        .map(|key| {
            let value = match predictions.and_then(|map| map.get(key)).cloned() {
                Some(mut entry) if entry.get("reason").is_none() => {
                    if let Some(object) = entry.as_object_mut() {
                        object.entry("available").or_insert(Value::Bool(true));
                    }
                    entry
                }
                Some(entry) => entry,
                None => unavailable("No se pudo contactar a api-ml"),
            };
            (key.to_string(), value)
        })
        .collect()
}

async fn fetch_arima(client: &reqwest::Client, url: &str, ticker: &str) -> Value {
    match client
        .get(url)
        .query(&[
            ("ticker", ticker),
            ("predictions", "5"),
            ("media_movil", "20"),
        ])
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) if body.get("error").is_none() => {
                let last_close = body.get("valor_actual").and_then(Value::as_f64);
                let predicted_close = body
                    .get("prediction")
                    .and_then(Value::as_array)
                    .and_then(|values| values.last())
                    .and_then(Value::as_f64);
                match (last_close, predicted_close) {
                    (Some(last), Some(predicted)) => {
                        let change = predicted / last - 1.0;
                        let signal = if change > 0.01 {
                            "alza"
                        } else if change < -0.01 {
                            "baja"
                        } else {
                            "neutral"
                        };
                        json!({
                            "available": true,
                            "signal": signal,
                            "condition": body.get("condition"),
                            "rsi": body.get("rsi"),
                            "horizon_days": 5,
                            "last_close": last,
                            "predicted_close": predicted,
                            "as_of": body.get("as_of"),
                            "model": "arima-modal",
                            "model_version": body.get("model_version"),
                            "backtest": body.get("backtest"),
                            "reason": null
                        })
                    }
                    _ => unavailable("ARIMA no devolvio precios validos"),
                }
            }
            Ok(body) => unavailable(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("ARIMA devolvio una respuesta invalida"),
            ),
            Err(error) => unavailable(&format!("Respuesta invalida de ARIMA: {error}")),
        },
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            unavailable("Servicio de predicciones no disponible")
        }
        Ok(response) => unavailable(&format!("ARIMA respondio HTTP {}", response.status())),
        Err(error) => unavailable(&format!("No se pudo contactar a ARIMA: {error}")),
    }
}

/// SVM (repo `models`, issue #159): clasificador binario Buy/Sell sobre el
/// retorno del dia siguiente. No predice un precio (a diferencia de
/// lstm/xgboost/arima), asi que `last_close`/`predicted_close` quedan en
/// null -- igual criterio que `rsi`/`condition` en ARIMA, que tampoco los
/// puede calcular. Se preserva el `backtest` (`directional_accuracy`) tal
/// cual para que compita de igual a igual en `pick_best_model`.
async fn fetch_svm(client: &reqwest::Client, url: &str, ticker: &str) -> Value {
    match client.get(url).query(&[("ticker", ticker)]).send().await {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) if body.get("error").is_none() => {
                let signal = match body.get("prediction").and_then(Value::as_str) {
                    Some("Buy") => "alza",
                    Some("Sell") => "baja",
                    _ => return unavailable("SVM no devolvio una prediccion valida"),
                };
                json!({
                    "available": true,
                    "signal": signal,
                    "condition": null,
                    "rsi": null,
                    "horizon_days": 1,
                    "last_close": null,
                    "predicted_close": null,
                    "as_of": null,
                    "model": "svm-modal",
                    "model_version": body.get("model_version"),
                    "backtest": body.get("backtest"),
                    "reason": null
                })
            }
            Ok(body) => unavailable(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("SVM devolvio una respuesta invalida"),
            ),
            Err(error) => unavailable(&format!("Respuesta invalida de SVM: {error}")),
        },
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            unavailable("Servicio de predicciones no disponible")
        }
        Ok(response) => unavailable(&format!("SVM respondio HTTP {}", response.status())),
        Err(error) => unavailable(&format!("No se pudo contactar a SVM: {error}")),
    }
}

/// El `prediction` de GARCH en Modal viene como
/// `{"h.1": {"<indice_pandas>": varianza}, "h.2": {...}, ...}` (un
/// `DataFrame.to_dict()` de una sola fila): el indice interno no tiene
/// significado para el front, asi que se aplana a una lista ordenada por
/// horizonte con la volatilidad ya en las mismas unidades que los retornos
/// (`%`, ver `garch_model.py`: la volatilidad es la raiz de la varianza, no
/// hace falta reescalar).
fn parse_garch_volatility(prediction: &Value) -> Option<Vec<Value>> {
    let map = prediction.as_object()?;
    let mut points: Vec<(i64, f64)> = map
        .iter()
        .filter_map(|(key, value)| {
            let horizon = key.strip_prefix("h.")?.parse::<i64>().ok()?;
            let variance = value.as_object()?.values().next()?.as_f64()?;
            Some((horizon, variance))
        })
        .collect();
    if points.is_empty() {
        return None;
    }
    points.sort_by_key(|(horizon, _)| *horizon);
    Some(
        points
            .into_iter()
            .map(|(horizon, variance)| {
                let volatility_pct = (variance.max(0.0).sqrt() * 100.0).round() / 100.0;
                json!({ "horizon_days": horizon, "volatility_pct": volatility_pct })
            })
            .collect(),
    )
}

/// GARCH (repo `models`, issue #159) pronostica volatilidad (varianza a 5
/// dias), no una direccion: no tiene "signal" ni precio, asi que no
/// participa del ranking de `pick_best_model` (no tiene
/// `directional_accuracy`) ni se muestra como una prediccion de tendencia
/// mas -- se marca `available: false` con motivo explicito en vez de
/// inventar un signal "neutral" que seria enganoso. El backtest
/// (`variance_mae`) y la proyeccion de volatilidad (`volatility_forecast`)
/// igual se exponen, para que el front la muestre como medida de riesgo,
/// no de tendencia (ver `TickerDetail.tsx`).
async fn fetch_garch(client: &reqwest::Client, url: &str, ticker: &str) -> Value {
    match client.get(url).query(&[("ticker", ticker)]).send().await {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(body) if body.get("error").is_none() => {
                let volatility_forecast = body.get("prediction").and_then(parse_garch_volatility);
                json!({
                    "available": false,
                    "signal": null,
                    "condition": null,
                    "rsi": null,
                    "horizon_days": null,
                    "last_close": null,
                    "predicted_close": null,
                    "as_of": null,
                    "model": "garch-modal",
                    "model_version": body.get("model_version"),
                    "backtest": body.get("backtest"),
                    "volatility_forecast": volatility_forecast,
                    "reason": "GARCH proyecta volatilidad, no una direccion: no participa del comparador de tendencia"
                })
            }
            Ok(body) => unavailable(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("GARCH devolvio una respuesta invalida"),
            ),
            Err(error) => unavailable(&format!("Respuesta invalida de GARCH: {error}")),
        },
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            unavailable("Servicio de predicciones no disponible")
        }
        Ok(response) => unavailable(&format!("GARCH respondio HTTP {}", response.status())),
        Err(error) => unavailable(&format!("No se pudo contactar a GARCH: {error}")),
    }
}

async fn fetch_modal(client: &reqwest::Client, name: &str, url: &str, ticker: &str) -> Value {
    match client
        .get(url)
        .query(&[("ticker", ticker), ("horizon", "5")])
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(mut body) if body.get("error").is_none() => {
                if let Some(object) = body.as_object_mut() {
                    object.insert("available".into(), Value::Bool(true));
                    object.insert("model".into(), Value::String(name.into()));
                    object.insert("reason".into(), Value::Null);
                }
                body
            }
            Ok(body) => unavailable(
                body.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Modal devolvio una respuesta invalida"),
            ),
            Err(error) => unavailable(&format!("Respuesta invalida de Modal: {error}")),
        },
        Ok(response) if response.status() == StatusCode::NOT_FOUND => {
            unavailable("Servicio de predicciones no disponible")
        }
        Ok(response) => unavailable(&format!("Modal respondio HTTP {}", response.status())),
        Err(error) => unavailable(&format!("No se pudo contactar a Modal: {error}")),
    }
}

fn unavailable(reason: &str) -> Value {
    json!({
        "available": false,
        "signal": null,
        "condition": null,
        "rsi": null,
        "horizon_days": null,
        "last_close": null,
        "predicted_close": null,
        "as_of": null,
        "model": null,
        "model_version": null,
        "backtest": null,
        "reason": reason
    })
}

#[cfg(test)]
mod pick_best_model_tests {
    use super::pick_best_model;
    use serde_json::{json, Value};

    fn predictions(entries: &[(&str, Value)]) -> serde_json::Map<String, Value> {
        entries
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn prefers_higher_directional_accuracy() {
        let predictions = predictions(&[
            (
                "lstm-modal",
                json!({"available": true, "backtest": {"directional_accuracy": 0.6}}),
            ),
            (
                "xgboost-modal",
                json!({"available": true, "backtest": {"directional_accuracy": 0.8}}),
            ),
        ]);
        assert_eq!(pick_best_model(&predictions), "xgboost-modal");
    }

    #[test]
    fn ignores_unavailable_models() {
        let predictions = predictions(&[
            (
                "lstm-modal",
                json!({"available": false, "reason": "No se pudo contactar a Modal"}),
            ),
            (
                "xgboost-modal",
                json!({"available": true, "backtest": {"directional_accuracy": 0.55}}),
            ),
        ]);
        assert_eq!(pick_best_model(&predictions), "xgboost-modal");
    }

    #[test]
    fn svm_can_win_if_it_has_better_accuracy() {
        let predictions = predictions(&[
            (
                "lstm-modal",
                json!({"available": true, "backtest": {"directional_accuracy": 0.5}}),
            ),
            (
                "svm-modal",
                json!({"available": true, "backtest": {"directional_accuracy": 0.65, "observations": 60}}),
            ),
        ]);
        assert_eq!(pick_best_model(&predictions), "svm-modal");
    }

    #[test]
    fn garch_never_wins_because_it_has_no_directional_accuracy() {
        let predictions = predictions(&[
            (
                "lstm-modal",
                json!({"available": true, "backtest": {"directional_accuracy": 0.5}}),
            ),
            (
                "garch-modal",
                json!({
                    "available": false,
                    "backtest": {"variance_mae": 10.6, "observations": 30},
                    "reason": "GARCH proyecta volatilidad, no una direccion: no participa del comparador de tendencia"
                }),
            ),
        ]);
        assert_eq!(pick_best_model(&predictions), "lstm-modal");
    }

    #[test]
    fn falls_back_to_lstm_modal_when_nothing_has_metrics() {
        let predictions = predictions(&[
            ("lstm-modal", json!({"available": true})),
            ("arima-modal", json!({"available": false, "reason": "..."})),
        ]);
        assert_eq!(pick_best_model(&predictions), "lstm-modal");
    }
}

#[cfg(test)]
mod parse_garch_volatility_tests {
    use super::parse_garch_volatility;
    use serde_json::json;

    #[test]
    fn flattens_and_sorts_by_horizon() {
        let prediction = json!({
            "h.5": {"2441": 8.729070722313933},
            "h.1": {"2441": 6.836204657283052},
            "h.2": {"2441": 7.316327591867336},
        });
        let points = parse_garch_volatility(&prediction).expect("deberia parsear");
        assert_eq!(
            points,
            vec![
                json!({"horizon_days": 1, "volatility_pct": 2.61}),
                json!({"horizon_days": 2, "volatility_pct": 2.7}),
                json!({"horizon_days": 5, "volatility_pct": 2.95}),
            ]
        );
    }

    #[test]
    fn returns_none_when_shape_is_unexpected() {
        assert!(parse_garch_volatility(&json!("no es un objeto")).is_none());
        assert!(parse_garch_volatility(&json!({})).is_none());
    }
}
