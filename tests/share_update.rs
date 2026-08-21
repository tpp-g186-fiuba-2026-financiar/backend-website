// GET /shares/update sincroniza el catalogo local de tickers contra
// data-colector. Siempre pega la red (no tiene fallback a una URL por
// default como otros endpoints), asi que en vez de depender de que
// data-colector este arriba probamos el camino de error apuntando
// DATA_COLLECTOR_URL a una direccion que nadie escucha: es determinista y
// cubre que el handler no rompa (panic) cuando el servicio no responde.

use axum::{body::Body, http::Request, http::StatusCode, Router};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const JWT_SECRET: &str = "test-secret-for-share-update";
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
async fn share_update_when_data_collector_is_unreachable_returns_500() {
    std::env::set_var("DATA_COLLECTOR_URL", "http://127.0.0.1:9");

    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/shares/update")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 500);
}
