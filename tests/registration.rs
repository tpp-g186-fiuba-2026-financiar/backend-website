use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const JWT_SECRET: &str = "test-secret-for-registration";
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
    format!("register_{tag}_{nanos}@test.com")
}

async fn register_request(app: &Router, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn register_with_invalid_email_returns_400() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = register_request(
        &app,
        json!({
            "email": "not-an-email",
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
    // Nota: el utoipa::path del handler documenta "Invalid e-mail" como
    // mensaje de ejemplo, pero el EmailValidator real devuelve este otro
    // texto. Validamos el comportamiento real, no la doc (que está desactualizada).
    assert_eq!(json["message"], "Invalid email format");
}

#[tokio::test]
async fn register_with_empty_email_returns_400() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = register_request(
        &app,
        json!({
            "email": "",
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
}

#[tokio::test]
async fn register_with_weak_password_returns_400() {
    let state = setup().await;
    let email = unique_email("weak_pw");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "weak",
            "full_name": "Jane Doe",
        }),
    )
    .await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
    assert!(json["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
        .contains("password"));

    // No debe haber quedado un usuario a medio crear.
    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_password_validation_runs_even_with_valid_email() {
    let state = setup().await;
    let email = unique_email("weak_pw_only_digits");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "12345678",
            "full_name": "Jane Doe",
        }),
    )
    .await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_with_invalid_risk_profile_returns_400() {
    let state = setup().await;
    let email = unique_email("bad_profile");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
            "risk_profile": "yolo",
        }),
    )
    .await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
    assert_eq!(
        json["message"],
        "Invalid risk profile. Must be 'conservative', 'moderate', or 'aggressive'."
    );

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_without_risk_profile_succeeds() {
    let state = setup().await;
    let email = unique_email("no_profile");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 200);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_with_valid_data_returns_200() {
    let state = setup().await;
    let email = unique_email("happy");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
            "risk_profile": "moderate",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 200);
    assert_eq!(json["message"], "User registered successfully");

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_duplicate_email_returns_409() {
    let state = setup().await;
    let email = unique_email("dup");
    let app = build_app(state.clone()).await;

    let payload = json!({
        "email": email,
        "password": "StrongPassword123!",
        "full_name": "Jane Doe",
        "risk_profile": "moderate",
    });

    let first = register_request(&app, payload.clone()).await;
    assert_eq!(first.status(), StatusCode::OK);
    let body = first.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 200);

    let second = register_request(&app, payload).await;
    let body = second.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 409);
    assert_eq!(json["message"], "User already exists for that email!");

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_duplicate_email_is_case_or_whitespace_sensitive_check_documented() {
    // Confirmamos que registrar el mismo email con espacios alrededor
    // tambien es detectado como duplicado (dado que el handler hace
    // `.trim()` antes de guardar y antes de buscar coincidencias).
    let state = setup().await;
    let email = unique_email("dup_ws");
    let app = build_app(state.clone()).await;

    let first = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;
    let body = first.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 200);

    let padded_email = format!("  {email}  ");
    let second = register_request(
        &app,
        json!({
            "email": padded_email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;
    let body = second.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 409);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_persists_full_name_and_risk_profile() {
    let state = setup().await;
    let email = unique_email("persist");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Persisted Name",
            "risk_profile": "aggressive",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let login_response = app
        .clone()
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
    let token = json["token"].as_str().unwrap().to_string();

    let user_response = app
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = user_response.into_body().collect().await.unwrap().to_bytes();
    let user: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(user["full_name"], "Persisted Name");
    assert_eq!(user["risk_profile"], "aggressive");
    assert_eq!(user["is_active"], true);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_stores_hashed_password_not_plaintext() {
    let state = setup().await;
    let email = unique_email("hashed");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let row = sqlx::query("SELECT password_hash FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&state.pool)
        .await
        .expect("user should exist after registration");
    let stored_hash: String = row.try_get("password_hash").unwrap();

    assert_ne!(stored_hash, "StrongPassword123!");
    assert!(
        stored_hash.starts_with("$argon2"),
        "expected an argon2 hash, got: {stored_hash}"
    );

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn register_new_user_can_immediately_login() {
    let state = setup().await;
    let email = unique_email("login_after");
    let app = build_app(state.clone()).await;

    let response = register_request(
        &app,
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Jane Doe",
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

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
    assert_eq!(json["code"], 200);
    assert!(json["token"].as_str().is_some_and(|t| !t.is_empty()));

    cleanup(&state.pool, &email).await;
}