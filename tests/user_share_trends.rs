use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const JWT_SECRET: &str = "test-secret-for-user-share-trends";
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

async fn cleanup(pool: &sqlx::PgPool, email: &str) {
    let _ = sqlx::query!("DELETE FROM users WHERE email = $1", email)
        .execute(pool)
        .await;
}

fn unique_email(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("trends_{tag}_{nanos}@test.com")
}

async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
    let app = build_app(state.clone()).await;

    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": "Trends Tester",
        "risk_profile": "moderate",
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(register_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "register should succeed");

    let login_body = json!({ "email": email, "password": password });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(login_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "login should succeed");

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    json["token"]
        .as_str()
        .expect("token should be present in login response")
        .to_string()
}

#[tokio::test]
async fn trends_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares/trends")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn trends_with_no_declared_shares_returns_empty_list() {
    let state = setup().await;
    let email = unique_email("empty");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares/trends")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Sin acciones declaradas no hay tickers que consultarle a api-ml, asi
    // que este caso no depende de que el servicio este levantado.
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["trends"].is_array());
    assert_eq!(json["trends"].as_array().unwrap().len(), 0);

    cleanup(&state.pool, &email).await;
}
