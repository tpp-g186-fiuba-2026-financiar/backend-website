use sqlx::PgPool;

pub async fn create_pool(database_url: &str) -> PgPool {
    PgPool::connect(database_url)
        .await
        .expect("Failed to connect to database")
}

pub async fn create_tables(pool: &PgPool) {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id            SERIAL PRIMARY KEY,
            email         TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            full_name     TEXT NOT NULL,
            risk_profile  TEXT CHECK (risk_profile IN ('conservative', 'moderate', 'aggressive')),
            is_active     BOOLEAN NOT NULL DEFAULT TRUE,
            created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
    )
    .execute(pool)
    .await
    .expect("Failed to create tables");
}
