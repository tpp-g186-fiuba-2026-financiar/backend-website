use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct HiddenModelsResponse {
    /// Claves de los modelos ocultos (las mismas de `predictions` en compare).
    pub hidden: Vec<String>,
}

async fn hidden(pool: &PgPool) -> impl IntoResponse {
    match sqlx::query_scalar::<_, String>("SELECT model FROM hidden_models ORDER BY model")
        .fetch_all(pool)
        .await
    {
        Ok(hidden) => (StatusCode::OK, Json(HiddenModelsResponse { hidden })).into_response(),
        Err(err) => {
            tracing::error!("no se pudieron leer los modelos ocultos: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "code": 500,
                    "message": "An unexpected error occurred. Please try again later."
                })),
            )
                .into_response()
        }
    }
}

fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 64
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn bad_model() -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"code": 400, "message": "Nombre de modelo invalido"})),
    )
        .into_response()
}

#[utoipa::path(
    get,
    path = "/models/hidden",
    responses(
        (status = 200, description = "Modelos que el admin oculto: el front no se los muestra al usuario", body = HiddenModelsResponse, example = json!({"hidden": ["macro"]})),
        (status = 401, description = "Missing or invalid authentication token")
    ),
    security(("bearer_auth" = [])),
    tag = "Admin"
)]
pub async fn list(State(pool): State<PgPool>) -> impl IntoResponse {
    hidden(&pool).await
}

#[utoipa::path(
    put,
    path = "/admin/models/{model}/hidden",
    params(("model" = String, Path, description = "Clave del modelo, ej: macro, xgboost-modal")),
    responses(
        (status = 200, description = "Modelo oculto; devuelve la lista completa", body = HiddenModelsResponse),
        (status = 400, description = "Nombre de modelo invalido"),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "El usuario no es administrador")
    ),
    security(("bearer_auth" = [])),
    tag = "Admin"
)]
pub async fn hide(State(pool): State<PgPool>, Path(model): Path<String>) -> impl IntoResponse {
    if !valid_model(&model) {
        return bad_model();
    }
    if let Err(err) =
        sqlx::query("INSERT INTO hidden_models (model) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(&model)
            .execute(&pool)
            .await
    {
        tracing::error!("no se pudo ocultar {model}: {err}");
    }
    hidden(&pool).await.into_response()
}

#[utoipa::path(
    delete,
    path = "/admin/models/{model}/hidden",
    params(("model" = String, Path, description = "Clave del modelo")),
    responses(
        (status = 200, description = "Modelo visible de nuevo; devuelve la lista completa", body = HiddenModelsResponse),
        (status = 400, description = "Nombre de modelo invalido"),
        (status = 401, description = "Missing or invalid authentication token"),
        (status = 403, description = "El usuario no es administrador")
    ),
    security(("bearer_auth" = [])),
    tag = "Admin"
)]
pub async fn show(State(pool): State<PgPool>, Path(model): Path<String>) -> impl IntoResponse {
    if !valid_model(&model) {
        return bad_model();
    }
    if let Err(err) = sqlx::query("DELETE FROM hidden_models WHERE model = $1")
        .bind(&model)
        .execute(&pool)
        .await
    {
        tracing::error!("no se pudo mostrar {model}: {err}");
    }
    hidden(&pool).await.into_response()
}
