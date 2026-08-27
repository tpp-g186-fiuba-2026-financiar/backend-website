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

const JWT_SECRET: &str = "test-secret-for-update-risk-profile";
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
    format!("risk_profile_{tag}_{nanos}@test.com")
}

async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
    let app = build_app(state.clone()).await;

    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": "Risk Profile Tester",
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

async fn patch_risk_profile(
    app: &Router,
    token: &str,
    risk_profile: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/user/risk-profile")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "risk_profile": risk_profile }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get_user(app: &Router, token: &str) -> Value {
    let response = app
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
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn update_risk_profile_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/user/risk-profile")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "risk_profile": "moderate" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn update_risk_profile_with_invalid_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/user/risk-profile")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer not-a-valid-token")
                .body(Body::from(
                    json!({ "risk_profile": "moderate" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn update_risk_profile_with_invalid_value_returns_400() {
    let state = setup().await;
    let email = unique_email("invalid_value");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    let response = patch_risk_profile(&app, &token, "yolo").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 400);
    assert_eq!(json["message"], "Invalid risk_profile value");

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn update_risk_profile_with_empty_value_returns_400() {
    let state = setup().await;
    let email = unique_email("empty_value");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    let response = patch_risk_profile(&app, &token, "").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn update_risk_profile_with_valid_value_returns_200() {
    let state = setup().await;
    let email = unique_email("valid_value");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    let response = patch_risk_profile(&app, &token, "aggressive").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 200);

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn update_risk_profile_accepts_every_documented_value() {
    let state = setup().await;
    let email = unique_email("all_values");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    for value in ["conservative", "moderate", "aggressive"] {
        let response = patch_risk_profile(&app, &token, value).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "expected {value} to be accepted"
        );
    }

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn update_risk_profile_persists_the_new_value() {
    let state = setup().await;
    let email = unique_email("persists");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    let response = patch_risk_profile(&app, &token, "conservative").await;
    assert_eq!(response.status(), StatusCode::OK);

    let user = get_user(&app, &token).await;
    assert_eq!(user["risk_profile"], "conservative");

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn update_risk_profile_rejects_invalid_value_without_mutating_existing_one() {
    let state = setup().await;
    let email = unique_email("no_mutate");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    // El usuario se registra con "moderate".
    let bad_response = patch_risk_profile(&app, &token, "not_a_real_profile").await;
    assert_eq!(bad_response.status(), StatusCode::BAD_REQUEST);

    let user = get_user(&app, &token).await;
    assert_eq!(user["risk_profile"], "moderate");

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn update_risk_profile_returns_404_when_user_was_deleted() {
    let state = setup().await;
    let email = unique_email("deleted");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

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

    let response = patch_risk_profile(&app, &token, "moderate").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 404);
}

#[tokio::test]
async fn update_risk_profile_is_case_sensitive_for_whitelist_values() {
    let state = setup().await;
    let email = unique_email("case_sensitive");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let app = build_app(state.clone()).await;

    let response = patch_risk_profile(&app, &token, "Moderate").await;

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "whitelist check is case-sensitive, 'Moderate' should not match 'moderate'"
    );

    cleanup(&state.pool, &email).await;
}
