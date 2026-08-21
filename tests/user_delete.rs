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

const JWT_SECRET: &str = "test-secret-for-user-delete";
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
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await;
}

fn unique_email(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("user_delete_{tag}_{nanos}@test.com")
}

async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
    let app = build_app(state.clone()).await;

    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": "Delete Tester",
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
async fn delete_user_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/user")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_user_with_valid_token_returns_204_and_removes_account() {
    let state = setup().await;
    let email = unique_email("happy");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    // El usuario ya no deberia existir: la misma cuenta no puede loguearse.
    // /login siempre responde HTTP 200 (ver login_logic.rs); el resultado
    // real viaja en el campo "code" del body.
    let app = build_app(state.clone()).await;
    let login_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": "StrongPassword123!" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = login_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 401);
    assert!(json["token"].is_null());

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn delete_user_called_twice_returns_404_on_second_call() {
    let state = setup().await;
    let email = unique_email("twice");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let first = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);

    let app = build_app(state.clone()).await;
    let second = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(second.status(), StatusCode::NOT_FOUND);
    let body = second.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 404);

    cleanup(&state.pool, &email).await;
}
