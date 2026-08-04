use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;

use super::{pin, sprint};

/// Agrupa filas de `retro_cards` en un objeto `{bien, mejorar, acciones,
/// preguntas}`, cada una una lista de tarjetas. Se usa tanto para el board
/// activo como para armar el snapshot al archivar.
pub fn group_by_column(rows: Vec<(i32, String, String, DateTime<Utc>)>) -> serde_json::Value {
    let mut bien = Vec::new();
    let mut mejorar = Vec::new();
    let mut acciones = Vec::new();
    let mut preguntas = Vec::new();
    for (id, column_name, content, created_at) in rows {
        let card = json!({ "id": id, "content": content, "created_at": created_at });
        match column_name.as_str() {
            "bien" => bien.push(card),
            "mejorar" => mejorar.push(card),
            "acciones" => acciones.push(card),
            "preguntas" => preguntas.push(card),
            _ => {}
        }
    }
    json!({ "bien": bien, "mejorar": mejorar, "acciones": acciones, "preguntas": preguntas })
}

pub async fn handler(State(pool): State<PgPool>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    let rows = sqlx::query_as::<_, (i32, String, String, DateTime<Utc>)>(
        r#"
        SELECT id, column_name, content, created_at
        FROM retro_cards
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(&pool)
    .await;

    let current_sprint = match sprint::current(&pool).await {
        Ok(n) => n,
        Err(err) => {
            tracing::error!("Failed to load current sprint: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "code": 500, "message": "No se pudo cargar el tablero" })),
            )
                .into_response();
        }
    };

    match rows {
        Ok(rows) => {
            let mut board = group_by_column(rows);
            board["sprint"] = json!(current_sprint);
            (StatusCode::OK, Json(board)).into_response()
        }
        Err(err) => {
            tracing::error!("Failed to load retro board: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "code": 500, "message": "No se pudo cargar el tablero" })),
            )
                .into_response()
        }
    }
}
