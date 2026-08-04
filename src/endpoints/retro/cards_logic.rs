use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use super::pin;

#[derive(Deserialize)]
pub struct CreateCardRequest {
    pub column: String,
    pub content: String,
}

pub async fn create(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    Json(body): Json<CreateCardRequest>,
) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    if !["bien", "mejorar", "acciones", "preguntas"].contains(&body.column.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "code": 400, "message": "columna invalida" })),
        )
            .into_response();
    }
    if body.content.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "code": 400, "message": "el contenido no puede estar vacio" })),
        )
            .into_response();
    }

    let result = sqlx::query_as::<_, (i32,)>(
        r#"
        INSERT INTO retro_cards (column_name, content)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(&body.column)
    .bind(body.content.trim())
    .fetch_one(&pool)
    .await;

    match result {
        Ok((id,)) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(err) => {
            tracing::error!("Failed to create retro card: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "code": 500, "message": "No se pudo crear la tarjeta" })),
            )
                .into_response()
        }
    }
}

pub async fn delete(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    Path(id): Path<i32>,
) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    let result = sqlx::query("DELETE FROM retro_cards WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;

    match result {
        Ok(res) if res.rows_affected() == 0 => (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": 404, "message": "tarjeta no encontrada" })),
        )
            .into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!("Failed to delete retro card: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "code": 500, "message": "No se pudo borrar la tarjeta" })),
            )
                .into_response()
        }
    }
}
