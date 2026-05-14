use axum::{body::Body, http::Request, http::StatusCode};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn create_basic_pool() -> sqlx::PgPool {
    // set enviroment variables
    dotenv().ok();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database")
}

async fn create_basic_session_layer(
) -> tower_sessions::SessionManagerLayer<tower_sessions_sqlx_store::PostgresStore> {
    let pool = create_basic_pool().await;
    let session_store = tower_sessions_sqlx_store::PostgresStore::new(pool.clone());
    session_store
        .migrate()
        .await
        .expect("Failed to run migrations");
    tower_sessions::SessionManagerLayer::new(session_store)
}

#[tokio::test]
async fn health_returns_ok() {
    let pool = create_basic_pool().await;
    let dummy_session = create_basic_session_layer().await;
    let app = backend_website::app(pool, dummy_session);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}
#[tokio::test]
async fn hello_returns_ok() {
    let pool = create_basic_pool().await;
    let dummy_session = create_basic_session_layer().await;
    let app = backend_website::app(pool, dummy_session);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/hello")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["message"], "Hello from Financiar backend!");
}
