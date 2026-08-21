// GET /user/shares/{ticker}/history pega a data-colector para el historico
// de precios, sin fallback determinista si el servicio no responde (reintenta
// contra la URL real). Sin mockear esa dependencia, lo unico que se puede
// probar de forma determinista aca es que la ruta esta protegida por JWT.

use axum::{body::Body, http::Request, http::StatusCode, Router};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const JWT_SECRET: &str = "test-secret-for-share-history";
const JWT_EXP_HOURS: i64 = 24;

async fn setup() -> AppState {
    dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database");
    AppState {
        pool,
        jwt_config: JwtConfig::new(JWT_SECRET, JWT_EXP_HOURS),
    }
}

async fn build_app(state: AppState) -> Router {
    let session_store = PostgresStore::new(state.pool.clone());
    session_store
        .migrate()
        .await
        .expect("Failed to run session store migrations");
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);
    app_with_state(state, session_layer)
}

#[tokio::test]
async fn share_history_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares/GGAL/history")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
