use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use utoipa::{IntoParams, ToSchema};

use crate::auth::{middleware::AuthUser, profile::CurrentProfile};

#[derive(Serialize)]
struct TenenciaItem {
    ticker: String,
    // ASUNCION: api-ml espera "cantidad" (convencion en espanol que usa el
    // resto del codigo de api-ml, ej. `cantidades_por_ticker`). Si
    // `schemas.py` define otro nombre de campo, cambiar aca.
    cantidad: i32,
}

#[derive(Serialize)]
struct UsuarioPayload {
    perfil_riesgo: String,
    tenencias: Vec<TenenciaItem>,
}

#[derive(Deserialize, IntoParams)]
pub struct PortfolioQuery {
    /// Modelo de prediccion de tendencia a usar (ver /models en api-ml para el default vigente)
    model: Option<String>,
    /// Estrategia de cartera ancla: propia | equal_weight | mercado
    cartera_ancla: Option<String>,
    /// Cuantas acciones que el usuario no tiene puede sumar (default 2 en api-ml; 0 = solo las suyas)
    max_nuevas: Option<u8>,
    /// true = puede usar todo el panel
    todas: Option<bool>,
}

/// Detalle Black-Litterman por ticker (se reenvia tal cual desde api-ml).
#[derive(Serialize, ToSchema)]
pub struct PortfolioDetalleItem {
    pub ticker: String,
    pub peso_actual: f64,
    pub peso_recomendado: f64,
    /// Retorno de equilibrio (prior)
    pub pi: f64,
    /// View del modelo (retorno diario esperado)
    pub q: f64,
    /// Incertidumbre de la view
    pub omega: f64,
    /// Retorno posterior Black-Litterman
    pub mu_bl: f64,
    pub horizon_days: i64,
}

/// El body de api-ml se reenvia tal cual; este schema documenta sus campos.
#[derive(Serialize, ToSchema)]
pub struct PortfolioRecomendacionResponse {
    pub pesos_recomendados: std::collections::HashMap<String, f64>,
    pub detalle: Vec<PortfolioDetalleItem>,
    /// Ticker -> motivo por el que quedo afuera de la optimizacion
    pub tickers_excluidos: std::collections::HashMap<String, String>,
    /// Estrategia de cartera ancla usada: propia | equal_weight | mercado
    pub cartera_ancla: String,
    /// Acciones que el usuario no tenia y la recomendacion suma
    pub acciones_nuevas: Vec<String>,
    /// Fraccion de la cartera que cambia de manos
    pub rotacion: f64,
    /// Costo estimado de hacer los movimientos, como fraccion del valor de la cartera
    pub costo_estimado: f64,
    /// Costo por unidad operada que se uso (comision + derechos + spread)
    pub costo_operacion: f64,
}

#[utoipa::path(
    get,
    path = "/user/shares/portfolio/recomendacion",
    params(PortfolioQuery),
    responses(
        (status = 200, description = "Pesos de cartera recomendados (Black-Litterman) para el usuario autenticado", body = PortfolioRecomendacionResponse, example = json!({
            "pesos_recomendados": {
                "GGAL": 0.58,
                "YPFD": 0.42
            },
            "detalle": [{
                "ticker": "GGAL",
                "peso_actual": 0.5,
                "peso_recomendado": 0.58,
                "pi": 0.0004,
                "q": 0.0011,
                "omega": 0.00002,
                "mu_bl": 0.0007,
                "horizon_days": 5
            }],
            "tickers_excluidos": {"PAMP": "sin prediccion disponible"},
            "cartera_ancla": "propia"
        })),
        (status = 400, description = "El usuario no tiene perfil de riesgo configurado", example = json!({
            "code": 400,
            "message": "Configura tu perfil de riesgo antes de pedir una recomendacion."
        })),
        (status = 401, description = "Missing or invalid authentication token", example = json!({
            "code": 401,
            "message": "Invalid or expired token"
        })),
        (status = 404, description = "Modelo de prediccion desconocido", example = json!({
            "code": 404,
            "message": "modelo desconocido"
        })),
        (status = 422, description = "No hay suficientes datos para calcular la recomendacion", example = json!({
            "code": 422,
            "message": "no hay fechas en comun entre los tickers disponibles"
        })),
        (status = 501, description = "Estrategia de cartera ancla solicitada todavia no implementada", example = json!({
            "code": 501,
            "message": "TipoCarteraAncla.MERCADO todavia no esta implementada"
        })),
        (status = 502, description = "api-ml no pudo responder", example = json!({
            "code": 502,
            "message": "No se pudo contactar a api-ml"
        })),
        (status = 500, description = "Internal server error", example = json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        }))
    ),
    security(("bearer_auth" = [])),
    tag = "Share"
)]
pub async fn handler(
    State(pool): State<PgPool>,
    Extension(auth_user): Extension<AuthUser>,
    Extension(profile): Extension<CurrentProfile>,
    Query(query): Query<PortfolioQuery>,
) -> impl IntoResponse {
    // Perfil de riesgo vigente (lo resuelve el middleware require_profile).
    let perfil_riesgo = profile.risk_profile.clone();

    // Tenencias del usuario (ticker + cantidad). Cartera vacia es valida:
    // api-ml cae a equal_weight sola si no se fuerza cartera_ancla=propia.
    let tenencias = sqlx::query_as::<_, (String, i32)>(
        r#"
        SELECT s.ticker, us.quantity
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.user_id = $1
        ORDER BY s.ticker ASC
        "#,
    )
    .bind(auth_user.user_id)
    .fetch_all(&pool)
    .await;

    let tenencias = match tenencias {
        Ok(rows) => rows
            .into_iter()
            .map(|(ticker, quantity)| TenenciaItem {
                ticker,
                cantidad: quantity,
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            tracing::error!(
                "Failed to list tenencias for portfolio recomendacion: {}",
                err
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
    };

    let api_ml_url = match std::env::var("API_ML_URL") {
        Ok(url) => url,
        Err(_) => {
            tracing::error!("API_ML_URL no esta configurada");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            );
        }
    };

    let payload = UsuarioPayload {
        perfil_riesgo,
        tenencias,
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("reqwest client");
    let mut request = client.post(format!(
        "{}/portfolio/recomendacion",
        api_ml_url.trim_end_matches('/'),
    ));

    // Se reenvian tal cual: api-ml valida sus propios valores (404 si el
    // modelo no existe, 501 si la cartera_ancla pedida no esta
    // implementada), no hace falta duplicar esa validacion aca.
    let mut query_params = Vec::new();
    if let Some(model) = &query.model {
        query_params.push(("model", model.to_string()));
    }
    if let Some(cartera_ancla) = &query.cartera_ancla {
        query_params.push(("cartera_ancla", cartera_ancla.to_string()));
    }
    if let Some(max_nuevas) = query.max_nuevas {
        query_params.push(("max_nuevas", max_nuevas.to_string()));
    }
    if query.todas == Some(true) {
        query_params.push(("todas", "true".to_string()));
    }
    if !query_params.is_empty() {
        request = request.query(&query_params);
    }

    let response = request.json(&payload).send().await;

    match response {
        Ok(res) if res.status().is_success() => match res.json::<Value>().await {
            Ok(body) => (StatusCode::OK, Json(body)),
            Err(err) => {
                tracing::error!(
                    "Respuesta invalida de api-ml al recomendar cartera: {}",
                    err
                );
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "code": 502,
                        "message": "Respuesta invalida de api-ml"
                    })),
                )
            }
        },
        // api-ml devuelve {"detail": "..."} (convencion FastAPI) en sus
        // errores (404/422/501/502/503); se reenvia el mismo status code
        // con el detail traducido a nuestro formato {code, message}.
        Ok(res) => {
            let status = res.status();
            let detail = res
                .json::<Value>()
                .await
                .ok()
                .and_then(|body| body.get("detail").and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| "api-ml no pudo calcular la recomendacion".to_string());

            tracing::warn!(
                "api-ml respondio {} al recomendar cartera: {}",
                status,
                detail
            );

            let axum_status =
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

            (
                axum_status,
                Json(json!({ "code": axum_status.as_u16(), "message": detail })),
            )
        }
        Err(err) => {
            tracing::error!(
                "No se pudo contactar a api-ml para recomendar cartera: {}",
                err
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "code": 502,
                    "message": "No se pudo contactar a api-ml"
                })),
            )
        }
    }
}
