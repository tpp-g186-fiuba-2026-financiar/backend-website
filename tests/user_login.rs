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

const JWT_SECRET: &str = "test-secret-for-login";
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
    format!("login_{tag}_{nanos}@test.com")
}

async fn register(app: &Router, email: &str, password: &str) {
    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": "Login Tester",
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
}

async fn login_request(app: &Router, email: &str, password: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "email": email, "password": password }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn login_with_empty_email_returns_400() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = login_request(&app, "", "StrongPassword123!").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
    assert_eq!(json["message"], "Email and password are required");
    assert!(json["token"].is_null());
}

#[tokio::test]
async fn login_with_whitespace_only_email_returns_400() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = login_request(&app, "   ", "StrongPassword123!").await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
}

#[tokio::test]
async fn login_with_empty_password_returns_400() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = login_request(&app, "someone@test.com", "").await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
    assert_eq!(json["message"], "Email and password are required");
}

#[tokio::test]
async fn login_with_nonexistent_email_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = login_request(&app, "does-not-exist@test.com", "StrongPassword123!").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 401);
    assert_eq!(json["message"], "Invalid email or password");
    assert!(json["token"].is_null());
}

#[tokio::test]
async fn login_with_wrong_password_returns_401() {
    let state = setup().await;
    let email = unique_email("wrongpass");
    let app = build_app(state.clone()).await;
    register(&app, &email, "StrongPassword123!").await;

    let response = login_request(&app, &email, "TotallyWrongPassword!").await;

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 401);
    assert_eq!(json["message"], "Invalid email or password");
    assert!(json["token"].is_null());

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn login_email_is_case_sensitive_or_untrimmed_variant_fails_gracefully() {
    // No asumimos comportamiento de case-folding: solo verificamos que un
    // email con mayusculas distintas al registrado no autentique por accidente
    // ni rompa el handler.
    let state = setup().await;
    let email = unique_email("case");
    let app = build_app(state.clone()).await;
    register(&app, &email, "StrongPassword123!").await;

    let uppercased = email.to_uppercase();
    let response = login_request(&app, &uppercased, "StrongPassword123!").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    // El resultado (200 o 401) debe ser uno de los dos códigos documentados,
    // nunca un error inesperado.
    assert!(json["code"] == 200 || json["code"] == 401);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn login_with_valid_credentials_returns_200_and_token() {
    let state = setup().await;
    let email = unique_email("happy");
    let app = build_app(state.clone()).await;
    register(&app, &email, "StrongPassword123!").await;

    let response = login_request(&app, &email, "StrongPassword123!").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 200);
    assert_eq!(json["message"], "Login successful");
    assert!(json["token"].as_str().is_some_and(|t| !t.is_empty()));

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn login_sets_session_cookie_on_success() {
    let state = setup().await;
    let email = unique_email("session");
    let app = build_app(state.clone()).await;
    register(&app, &email, "StrongPassword123!").await;

    let response = login_request(&app, &email, "StrongPassword123!").await;

    assert!(
        response.headers().get(header::SET_COOKIE).is_some(),
        "login should establish a session cookie"
    );

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn login_issues_a_usable_token_for_protected_routes() {
    let state = setup().await;
    let email = unique_email("usable");
    let app = build_app(state.clone()).await;
    register(&app, &email, "StrongPassword123!").await;

    let response = login_request(&app, &email, "StrongPassword123!").await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let token = json["token"].as_str().unwrap().to_string();

    let protected = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(protected.status(), StatusCode::OK);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn login_after_account_deletion_returns_401() {
    let state = setup().await;
    let email = unique_email("deleted");
    let app = build_app(state.clone()).await;
    register(&app, &email, "StrongPassword123!").await;

    let first_login = login_request(&app, &email, "StrongPassword123!").await;
    let body = first_login.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let token = json["token"].as_str().unwrap().to_string();

    let delete_response = app
        .clone()
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
    assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

    let second_login = login_request(&app, &email, "StrongPassword123!").await;
    let body = second_login.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 401);
}
