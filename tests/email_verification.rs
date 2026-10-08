use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use backend_website::endpoints::user::email_verification::token::{hash_token, store_new_token};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const PASSWORD: &str = "StrongPassword123!";
const RESEND_MESSAGE: &str =
    "If the account exists and is not verified yet, a new verification email was sent";

async fn build_app() -> (Router, sqlx::PgPool) {
    dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database");
    let state = AppState {
        pool: pool.clone(),
        jwt_config: JwtConfig::new("test-secret-for-email-verification", 24),
    };
    let session_store = PostgresStore::new(pool.clone());
    session_store.migrate().await.expect("session migrate");
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);
    (app_with_state(state, session_layer), pool)
}

fn unique_email(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("verify_{tag}_{nanos}@test.com")
}

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn register(app: &Router, pool: &sqlx::PgPool, email: &str) -> i32 {
    let (_, body) = post(
        app,
        "/register",
        json!({
            "email": email,
            "password": PASSWORD,
            "full_name": "Verification Tester",
            "risk_profile": "moderate",
        }),
    )
    .await;
    assert_eq!(body["code"], 200, "register should succeed");

    let (id,): (i32,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap();
    id
}

async fn login(app: &Router, email: &str, password: &str) -> Value {
    post(
        app,
        "/login",
        json!({ "email": email, "password": password }),
    )
    .await
    .1
}

/// Devuelve (email_verified, token_hash, expires_at > NOW()).
async fn verification_state(pool: &sqlx::PgPool, email: &str) -> (bool, Option<String>, bool) {
    sqlx::query_as(
        r#"
        SELECT email_verified,
               email_verification_token_hash,
               COALESCE(email_verification_expires_at > NOW(), FALSE)
        FROM users WHERE email = $1
        "#,
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn cleanup(pool: &sqlx::PgPool, email: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn register_leaves_account_unverified_with_a_pending_token() {
    let (app, pool) = build_app().await;
    let email = unique_email("pending");
    register(&app, &pool, &email).await;

    let (verified, token_hash, not_expired) = verification_state(&pool, &email).await;
    assert!(!verified);
    assert!(token_hash.is_some());
    assert!(not_expired);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn unverified_account_cannot_log_in() {
    let (app, pool) = build_app().await;
    let email = unique_email("blocked");
    register(&app, &pool, &email).await;

    let body = login(&app, &email, PASSWORD).await;
    assert_eq!(body["code"], 403);
    assert_eq!(body["message"], "Email not verified");
    assert_eq!(body["email_verification_required"], true);
    assert!(body["token"].is_null());

    // Con contrasena incorrecta no se revela que la cuenta esta sin verificar.
    let body = login(&app, &email, "WrongPassword123!").await;
    assert_eq!(body["code"], 401);
    assert!(body["email_verification_required"].is_null());

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn valid_token_verifies_account_and_enables_login() {
    let (app, pool) = build_app().await;
    let email = unique_email("happy");
    let user_id = register(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap();

    let (status, body) = post(&app, "/verify-email", json!({ "token": token })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 200);
    assert_eq!(body["message"], "Email verified successfully");

    let (verified, token_hash, _) = verification_state(&pool, &email).await;
    assert!(verified);
    assert!(token_hash.is_none());

    let body = login(&app, &email, PASSWORD).await;
    assert_eq!(body["code"], 200);
    assert!(body["token"].as_str().is_some());

    // El token es de un solo uso.
    let (status, _) = post(&app, "/verify-email", json!({ "token": token })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn expired_token_is_rejected() {
    let (app, pool) = build_app().await;
    let email = unique_email("expired");
    let user_id = register(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap();
    sqlx::query(
        "UPDATE users SET email_verification_expires_at = NOW() - INTERVAL '1 hour' WHERE id = $1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = post(&app, "/verify-email", json!({ "token": token })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "Invalid or expired verification token");

    let (verified, _, _) = verification_state(&pool, &email).await;
    assert!(!verified);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn unknown_or_empty_token_is_rejected() {
    let (app, _pool) = build_app().await;

    let (status, body) = post(&app, "/verify-email", json!({ "token": "   " })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 400);

    let (status, _) = post(
        &app,
        "/verify-email",
        json!({ "token": "0000000000000000000000000000000000000000000000000000000000000000" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn resend_for_unknown_email_returns_generic_message() {
    let (app, _pool) = build_app().await;

    let (status, body) = post(
        &app,
        "/verify-email/resend",
        json!({ "email": unique_email("ghost") }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], RESEND_MESSAGE);
}

#[tokio::test]
async fn resend_respects_cooldown_and_then_rotates_token() {
    let (app, pool) = build_app().await;
    let email = unique_email("resend");
    let user_id = register(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap();

    // Recien enviado: no se genera un token nuevo.
    let (status, body) = post(&app, "/verify-email/resend", json!({ "email": email })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], RESEND_MESSAGE);
    let (_, token_hash, _) = verification_state(&pool, &email).await;
    assert_eq!(token_hash.as_deref(), Some(hash_token(&token).as_str()));

    // Pasado el cooldown se reemplaza el token y el anterior deja de servir.
    sqlx::query(
        "UPDATE users SET email_verification_sent_at = NOW() - INTERVAL '2 minutes' WHERE id = $1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    let (status, _) = post(&app, "/verify-email/resend", json!({ "email": email })).await;
    assert_eq!(status, StatusCode::OK);
    let (verified, token_hash, not_expired) = verification_state(&pool, &email).await;
    assert!(!verified);
    assert!(not_expired);
    assert_ne!(token_hash.as_deref(), Some(hash_token(&token).as_str()));

    let (status, _) = post(&app, "/verify-email", json!({ "token": token })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn resend_for_verified_account_does_nothing() {
    let (app, pool) = build_app().await;
    let email = unique_email("already");
    let user_id = register(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap();
    post(&app, "/verify-email", json!({ "token": token })).await;

    let (status, body) = post(&app, "/verify-email/resend", json!({ "email": email })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], RESEND_MESSAGE);

    let (verified, token_hash, _) = verification_state(&pool, &email).await;
    assert!(verified);
    assert!(token_hash.is_none());

    cleanup(&pool, &email).await;
}
