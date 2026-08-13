use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::board_logic::group_by_column;
use super::{pin, sprint};

#[derive(Deserialize)]
pub struct ArchiveRequest {
    pub label: Option<String>,
}

fn server_error(message: &str) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "code": 500, "message": message })),
    )
        .into_response()
}

/// Guarda una foto del tablero actual (con el numero de sprint y una
/// etiqueta, la fecha si no se manda una) y borra las tarjetas activas para
/// arrancar la proxima retro en limpio. Suma 1 al sprint para la proxima vez.
pub async fn create(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    Json(body): Json<ArchiveRequest>,
) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("Failed to start transaction for retro archive: {}", err);
            return server_error("No se pudo archivar");
        }
    };

    let rows = match sqlx::query_as::<_, (i32, String, String, DateTime<Utc>)>(
        r#"
        SELECT id, column_name, content, created_at
        FROM retro_cards
        ORDER BY created_at ASC
        "#,
    )
    .fetch_all(&mut *tx)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::error!("Failed to read retro board for archiving: {}", err);
            return server_error("No se pudo archivar");
        }
    };

    if rows.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "code": 400, "message": "el tablero esta vacio, no hay nada para archivar" })),
        )
            .into_response();
    }

    let sprint_number = match sprint::advance(&mut tx).await {
        Ok(n) => n,
        Err(err) => {
            tracing::error!("Failed to advance sprint counter: {}", err);
            return server_error("No se pudo archivar");
        }
    };

    let label = body
        .label
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| format!("Sprint {sprint_number}"));

    let snapshot = group_by_column(rows);

    let insert_result = sqlx::query_as::<_, (i32,)>(
        r#"
        INSERT INTO retro_archives (label, sprint_number, snapshot)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(&label)
    .bind(sprint_number)
    .bind(&snapshot)
    .fetch_one(&mut *tx)
    .await;

    let archive_id = match insert_result {
        Ok((id,)) => id,
        Err(err) => {
            tracing::error!("Failed to insert retro archive: {}", err);
            return server_error("No se pudo archivar");
        }
    };

    if let Err(err) = sqlx::query("DELETE FROM retro_cards")
        .execute(&mut *tx)
        .await
    {
        tracing::error!("Failed to clear retro board after archiving: {}", err);
        return server_error("No se pudo archivar");
    }

    if let Err(err) = tx.commit().await {
        tracing::error!("Failed to commit retro archive transaction: {}", err);
        return server_error("No se pudo archivar");
    }

    (
        StatusCode::CREATED,
        Json(json!({ "id": archive_id, "label": label, "sprint": sprint_number, "next_sprint": sprint_number + 1 })),
    )
        .into_response()
}

/// Deshace el ultimo archivado: restaura sus tarjetas al tablero activo
/// (se suman a lo que ya haya, no lo pisan), borra el registro de archivo, y
/// vuelve el sprint para atras. Pensado para "me equivoque al archivar",
/// no para reordenar el historial en general.
pub async fn undo(State(pool): State<PgPool>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("Failed to start transaction for retro undo: {}", err);
            return server_error("No se pudo deshacer");
        }
    };

    let last = sqlx::query_as::<_, (i32, Value)>(
        r#"
        SELECT id, snapshot
        FROM retro_archives
        ORDER BY archived_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await;

    let (archive_id, snapshot) = match last {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "code": 400, "message": "no hay ningun archivo para deshacer" })),
            )
                .into_response();
        }
        Err(err) => {
            tracing::error!("Failed to load last retro archive for undo: {}", err);
            return server_error("No se pudo deshacer");
        }
    };

    for column in ["bien", "mejorar", "acciones", "preguntas"] {
        let cards = snapshot
            .get(column)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for card in cards {
            let content = card.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.is_empty() {
                continue;
            }
            if let Err(err) =
                sqlx::query("INSERT INTO retro_cards (column_name, content) VALUES ($1, $2)")
                    .bind(column)
                    .bind(content)
                    .execute(&mut *tx)
                    .await
            {
                tracing::error!("Failed to restore retro card on undo: {}", err);
                return server_error("No se pudo deshacer");
            }
        }
    }

    if let Err(err) = sqlx::query("DELETE FROM retro_archives WHERE id = $1")
        .bind(archive_id)
        .execute(&mut *tx)
        .await
    {
        tracing::error!("Failed to delete retro archive on undo: {}", err);
        return server_error("No se pudo deshacer");
    }

    let sprint_number = match sprint::rewind(&mut tx).await {
        Ok(n) => n,
        Err(err) => {
            tracing::error!("Failed to rewind sprint counter: {}", err);
            return server_error("No se pudo deshacer");
        }
    };

    if let Err(err) = tx.commit().await {
        tracing::error!("Failed to commit retro undo transaction: {}", err);
        return server_error("No se pudo deshacer");
    }

    (StatusCode::OK, Json(json!({ "sprint": sprint_number }))).into_response()
}

pub async fn list(State(pool): State<PgPool>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    let rows = sqlx::query_as::<_, (i32, String, i32, DateTime<Utc>)>(
        r#"
        SELECT id, label, sprint_number, archived_at
        FROM retro_archives
        ORDER BY archived_at DESC
        "#,
    )
    .fetch_all(&pool)
    .await;

    match rows {
        Ok(rows) => {
            let archives: Vec<Value> = rows
                .into_iter()
                .map(|(id, label, sprint_number, archived_at)| {
                    json!({ "id": id, "label": label, "sprint": sprint_number, "archived_at": archived_at })
                })
                .collect();
            (StatusCode::OK, Json(json!({ "archives": archives }))).into_response()
        }
        Err(err) => {
            tracing::error!("Failed to list retro archives: {}", err);
            server_error("No se pudieron listar los archivos")
        }
    }
}

pub async fn get_one(
    State(pool): State<PgPool>,
    headers: HeaderMap,
    Path(id): Path<i32>,
) -> impl IntoResponse {
    if let Err(rejection) = pin::check(&headers) {
        return rejection.into_response();
    }

    let row = sqlx::query_as::<_, (i32, String, i32, Value, DateTime<Utc>)>(
        r#"
        SELECT id, label, sprint_number, snapshot, archived_at
        FROM retro_archives
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&pool)
    .await;

    match row {
        Ok(Some((id, label, sprint_number, snapshot, archived_at))) => (
            StatusCode::OK,
            Json(json!({
                "id": id, "label": label, "sprint": sprint_number,
                "snapshot": snapshot, "archived_at": archived_at
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": 404, "message": "archivo no encontrado" })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("Failed to load retro archive {}: {}", id, err);
            server_error("No se pudo cargar el archivo")
        }
    }
}
