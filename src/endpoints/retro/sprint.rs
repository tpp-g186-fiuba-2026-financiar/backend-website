use sqlx::{PgPool, Postgres, Transaction};

pub async fn current(pool: &PgPool) -> Result<i32, sqlx::Error> {
    let (n,): (i32,) = sqlx::query_as("SELECT current_sprint FROM retro_sprint WHERE id = 1")
        .fetch_one(pool)
        .await?;
    Ok(n)
}

/// Suma 1 al sprint actual dentro de la misma transaccion del archivado, y
/// devuelve el numero *anterior* (el que se acaba de archivar).
pub async fn advance(tx: &mut Transaction<'_, Postgres>) -> Result<i32, sqlx::Error> {
    let (n,): (i32,) = sqlx::query_as(
        "UPDATE retro_sprint SET current_sprint = current_sprint + 1 WHERE id = 1 RETURNING current_sprint - 1",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(n)
}

/// Resta 1 al sprint actual (deshacer), sin bajar de 1.
pub async fn rewind(tx: &mut Transaction<'_, Postgres>) -> Result<i32, sqlx::Error> {
    let (n,): (i32,) = sqlx::query_as(
        "UPDATE retro_sprint SET current_sprint = GREATEST(1, current_sprint - 1) WHERE id = 1 RETURNING current_sprint",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(n)
}
