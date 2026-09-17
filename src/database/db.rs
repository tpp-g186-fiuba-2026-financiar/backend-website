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
            is_active     BOOLEAN NOT NULL DEFAULT TRUE,
            created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
    )
    .execute(pool)
    .await
    .expect("Failed to create tables");
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS user_investing_profiles (
            id            SERIAL PRIMARY KEY,
            user_id       INTEGER REFERENCES users(id) ON DELETE CASCADE,
            risk_profile  TEXT CHECK (risk_profile IN ('conservative', 'moderate', 'aggressive')),
            created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            expires_at    TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '6 months'),
            is_active     BOOLEAN NOT NULL DEFAULT TRUE
        )",
    )
    .execute(pool)
    .await
    .expect("Failed to create tables");
}

pub async fn update_expired_investing_profiles(pool: &PgPool) -> Result<(), sqlx::Error> {
    let query = sqlx::query(
        "UPDATE risk_profiles
         SET is_active = FALSE
         WHERE expires_at <= NOW() AND is_active = TRUE",
    )
    .execute(pool)
    .await;

    if let Err(e) = query {
        tracing::error!("Failed to update expired investing profiles: {}", e);
        return Err(e);
    }

    Ok(())
}
