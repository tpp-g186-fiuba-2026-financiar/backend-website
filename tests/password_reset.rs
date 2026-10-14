use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use backend_website::endpoints::user::email_verification::token::hash_token;
use backend_website::endpoints::user::password_reset::token::store_new_token;
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const PASSWORD: &str = "StrongPassword123!";
const NEW_PASSWORD: &str = "BrandNewPassword456!";
const FORGOT_MESSAGE: &str = "If an account exists for that email, a password reset email was sent";
const INVALID_TOKEN_MESSAGE: &str = "Invalid or expired reset token";

async fn build_app() -> (Router, sqlx::PgPool) {
    dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database");
    let state = AppState {
        pool: pool.clone(),
        jwt_config: JwtConfig::new("test-secret-for-password-reset", 24),
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
    format!("reset_{tag}_{nanos}@test.com")
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

/// Registra un usuario (queda sin verificar) y devuelve su id.
async fn register(app: &Router, pool: &sqlx::PgPool, email: &str) -> i32 {
    let (_, body) = post(
        app,
        "/register",
        json!({
            "email": email,
            "password": PASSWORD,
            "full_name": "Reset Tester",
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

async fn register_verified(app: &Router, pool: &sqlx::PgPool, email: &str) -> i32 {
    let id = register(app, pool, email).await;
    sqlx::query("UPDATE users SET email_verified = TRUE WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("mark email as verified");
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

async fn forgot(app: &Router, email: &str) -> (StatusCode, Value) {
    post(app, "/forgot-password", json!({ "email": email })).await
}

async fn reset(app: &Router, token: &str, new_password: &str) -> (StatusCode, Value) {
    post(
        app,
        "/reset-password",
        json!({ "token": token, "new_password": new_password }),
    )
    .await
}

async fn reset_state(pool: &sqlx::PgPool, email: &str) -> (Option<String>, bool, bool) {
    sqlx::query_as(
        r#"
        SELECT password_reset_token_hash,
               COALESCE(password_reset_expires_at > NOW(), FALSE),
               password_reset_sent_at IS NOT NULL
        FROM users WHERE email = $1
        "#,
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn expire_cooldown(pool: &sqlx::PgPool, user_id: i32) {
    sqlx::query(
        "UPDATE users SET password_reset_sent_at = NOW() - INTERVAL '2 minutes' WHERE id = $1",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn cleanup(pool: &sqlx::PgPool, email: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// POST /forgot-password
// ---------------------------------------------------------------------------

#[tokio::test]
async fn forgot_with_empty_email_returns_400() {
    let (app, _pool) = build_app().await;

    let (status, body) = forgot(&app, "   ").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 400);
    assert_eq!(body["message"], "Email is required");
}

#[tokio::test]
async fn forgot_for_unknown_email_returns_generic_message() {
    let (app, _pool) = build_app().await;

    let (status, body) = forgot(&app, &unique_email("ghost")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 200);
    assert_eq!(body["message"], FORGOT_MESSAGE);
}

#[tokio::test]
async fn forgot_response_is_identical_for_known_and_unknown_emails() {
    let (app, pool) = build_app().await;
    let email = unique_email("enumeration");
    register(&app, &pool, &email).await;

    let known = forgot(&app, &email).await;
    let unknown = forgot(&app, &unique_email("nobody")).await;
    assert_eq!(
        known, unknown,
        "la respuesta no debe delatar si el mail existe"
    );

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn forgot_for_existing_account_stores_a_pending_token() {
    let (app, pool) = build_app().await;
    let email = unique_email("pending");
    register(&app, &pool, &email).await;

    let (token_hash, _, sent) = reset_state(&pool, &email).await;
    assert!(token_hash.is_none(), "antes de pedirlo no hay token");
    assert!(!sent);

    let (status, body) = forgot(&app, &email).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], FORGOT_MESSAGE);

    let (token_hash, not_expired, sent) = reset_state(&pool, &email).await;
    assert!(token_hash.is_some());
    assert!(not_expired);
    assert!(sent);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn forgot_trims_the_email() {
    let (app, pool) = build_app().await;
    let email = unique_email("trim");
    register(&app, &pool, &email).await;

    let (status, _) = forgot(&app, &format!("  {email}  ")).await;
    assert_eq!(status, StatusCode::OK);
    let (token_hash, _, _) = reset_state(&pool, &email).await;
    assert!(token_hash.is_some());

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn forgot_respects_cooldown_and_then_rotates_token() {
    let (app, pool) = build_app().await;
    let email = unique_email("cooldown");
    let user_id = register(&app, &pool, &email).await;

    forgot(&app, &email).await;
    let (first_hash, _, _) = reset_state(&pool, &email).await;
    assert!(first_hash.is_some());

    let (status, body) = forgot(&app, &email).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], FORGOT_MESSAGE);
    let (second_hash, _, _) = reset_state(&pool, &email).await;
    assert_eq!(second_hash, first_hash);

    // Cooldown expira, el token rota.
    expire_cooldown(&pool, user_id).await;
    forgot(&app, &email).await;
    let (third_hash, not_expired, _) = reset_state(&pool, &email).await;
    assert!(third_hash.is_some());
    assert_ne!(third_hash, first_hash);
    assert!(not_expired);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn forgot_and_reset_reject_bodies_with_missing_fields() {
    let (app, _pool) = build_app().await;

    let (status, _) = post(&app, "/forgot-password", json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = post(&app, "/reset-password", json!({ "token": "abc" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = post(
        &app,
        "/reset-password",
        json!({ "new_password": NEW_PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------------------
// store_new_token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn store_new_token_saves_only_the_hash() {
    let (app, pool) = build_app().await;
    let email = unique_email("hashonly");
    let user_id = register(&app, &pool, &email).await;

    let token = store_new_token(&pool, user_id)
        .await
        .unwrap()
        .expect("primer token: sin cooldown");
    assert_eq!(token.len(), 64);

    let (token_hash, not_expired, sent) = reset_state(&pool, &email).await;
    assert_eq!(token_hash.as_deref(), Some(hash_token(&token).as_str()));
    assert_ne!(token_hash.as_deref(), Some(token.as_str()));
    assert!(not_expired);
    assert!(sent);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn store_new_token_expires_in_about_an_hour() {
    let (app, pool) = build_app().await;
    let email = unique_email("ttl");
    let user_id = register(&app, &pool, &email).await;
    store_new_token(&pool, user_id).await.unwrap().unwrap();

    // Vence en la hora, con un margen de 1 minuto para no depender de la exactitud del NOW() de la base.
    let (within_hour,): (bool,) = sqlx::query_as(
        r#"
        SELECT password_reset_expires_at > NOW()
           AND password_reset_expires_at <= NOW() + INTERVAL '61 minutes'
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(within_hour);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn store_new_token_returns_none_during_cooldown() {
    let (app, pool) = build_app().await;
    let email = unique_email("cooldownfn");
    let user_id = register(&app, &pool, &email).await;

    let first = store_new_token(&pool, user_id).await.unwrap();
    assert!(first.is_some());
    let second = store_new_token(&pool, user_id).await.unwrap();
    assert!(second.is_none());

    expire_cooldown(&pool, user_id).await;
    let third = store_new_token(&pool, user_id).await.unwrap();
    assert!(third.is_some());
    assert_ne!(first, third);

    cleanup(&pool, &email).await;
}

// ---------------------------------------------------------------------------
// POST /reset-password
// ---------------------------------------------------------------------------

#[tokio::test]
async fn valid_token_changes_password_and_old_one_stops_working() {
    let (app, pool) = build_app().await;
    let email = unique_email("happy");
    let user_id = register_verified(&app, &pool, &email).await;
    assert_eq!(login(&app, &email, PASSWORD).await["code"], 200);

    let token = store_new_token(&pool, user_id).await.unwrap().unwrap();

    let (status, body) = reset(&app, &format!("  {token}  "), NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 200);
    assert_eq!(body["message"], "Password updated successfully");

    let body = login(&app, &email, NEW_PASSWORD).await;
    assert_eq!(body["code"], 200);
    assert!(body["token"].as_str().is_some());

    let body = login(&app, &email, PASSWORD).await;
    assert_eq!(body["code"], 401);
    assert!(body["token"].is_null());

    let (token_hash, not_expired, _) = reset_state(&pool, &email).await;
    assert!(token_hash.is_none());
    assert!(!not_expired);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn reset_works_end_to_end_through_forgot_password() {
    let (app, pool) = build_app().await;
    let email = unique_email("e2e");
    let user_id = register_verified(&app, &pool, &email).await;

    // El token en claro solo viaja en el mail, asi que se reemplaza el que dejo
    // /forgot-password por uno conocido (pasado el cooldown) para poder usarlo.
    forgot(&app, &email).await;
    expire_cooldown(&pool, user_id).await;
    let token = store_new_token(&pool, user_id).await.unwrap().unwrap();

    let (status, _) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login(&app, &email, NEW_PASSWORD).await["code"], 200);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn token_is_single_use() {
    let (app, pool) = build_app().await;
    let email = unique_email("single");
    let user_id = register_verified(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap().unwrap();

    let (status, _) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = reset(&app, &token, "AnotherPassword789!").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], INVALID_TOKEN_MESSAGE);

    assert_eq!(login(&app, &email, NEW_PASSWORD).await["code"], 200);
    assert_eq!(
        login(&app, &email, "AnotherPassword789!").await["code"],
        401
    );

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn expired_token_is_rejected_and_password_is_unchanged() {
    let (app, pool) = build_app().await;
    let email = unique_email("expired");
    let user_id = register_verified(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap().unwrap();
    sqlx::query(
        "UPDATE users SET password_reset_expires_at = NOW() - INTERVAL '1 minute' WHERE id = $1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();

    let (status, body) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], INVALID_TOKEN_MESSAGE);

    assert_eq!(login(&app, &email, PASSWORD).await["code"], 200);
    assert_eq!(login(&app, &email, NEW_PASSWORD).await["code"], 401);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn unknown_or_empty_token_is_rejected() {
    let (app, _pool) = build_app().await;

    let (status, body) = reset(&app, "   ", NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 400);
    assert_eq!(body["message"], INVALID_TOKEN_MESSAGE);

    let (status, body) = reset(&app, &"0".repeat(64), NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], INVALID_TOKEN_MESSAGE);
}

#[tokio::test]
async fn a_new_token_invalidates_the_previous_one() {
    let (app, pool) = build_app().await;
    let email = unique_email("rotate");
    let user_id = register_verified(&app, &pool, &email).await;

    let old_token = store_new_token(&pool, user_id).await.unwrap().unwrap();
    expire_cooldown(&pool, user_id).await;
    let new_token = store_new_token(&pool, user_id).await.unwrap().unwrap();
    assert_ne!(old_token, new_token);

    let (status, body) = reset(&app, &old_token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], INVALID_TOKEN_MESSAGE);
    assert_eq!(login(&app, &email, PASSWORD).await["code"], 200);

    let (status, _) = reset(&app, &new_token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn weak_password_is_rejected_without_consuming_the_token() {
    let (app, pool) = build_app().await;
    let email = unique_email("weak");
    let user_id = register_verified(&app, &pool, &email).await;
    let token = store_new_token(&pool, user_id).await.unwrap().unwrap();

    for (weak, expected) in [
        ("Sh0rt!", "Password must be at least 8 characters long"),
        (
            "alllowercase123!",
            "Password must contain at least one uppercase letter",
        ),
        (
            "ALLUPPERCASE123!",
            "Password must contain at least one lowercase letter",
        ),
        ("NoDigitsHere!!", "Password must contain at least one digit"),
        (
            "NoSpecials1234",
            "Password must contain at least one special character",
        ),
    ] {
        let (status, body) = reset(&app, &token, weak).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{weak}");
        assert_eq!(body["code"], 400);
        assert_eq!(body["message"], expected, "{weak}");
    }

    // confirmamos que el token sigue siendo valido y la contrasena original sigue funcionando
    assert_eq!(login(&app, &email, PASSWORD).await["code"], 200);
    let (token_hash, not_expired, _) = reset_state(&pool, &email).await;
    assert_eq!(token_hash.as_deref(), Some(hash_token(&token).as_str()));
    assert!(not_expired);

    // Y con una contrasena valida el mismo link sigue sirviendo.
    let (status, _) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(login(&app, &email, NEW_PASSWORD).await["code"], 200);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn reset_does_not_verify_the_email_nor_touch_the_verification_token() {
    let (app, pool) = build_app().await;
    let email = unique_email("unverified");
    let user_id = register(&app, &pool, &email).await;

    let (verification_hash,): (Option<String>,) =
        sqlx::query_as("SELECT email_verification_token_hash FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(verification_hash.is_some());

    let token = store_new_token(&pool, user_id).await.unwrap().unwrap();
    let (status, _) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    // La contrasena nueva es la valida, pero la cuenta sigue sin verificar.
    let body = login(&app, &email, NEW_PASSWORD).await;
    assert_eq!(body["code"], 403);
    assert_eq!(body["email_verification_required"], true);
    assert_eq!(login(&app, &email, PASSWORD).await["code"], 401);

    let (verified, still_hash): (bool, Option<String>) = sqlx::query_as(
        "SELECT email_verified, email_verification_token_hash FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!verified);
    assert_eq!(still_hash, verification_hash);

    cleanup(&pool, &email).await;
}

#[tokio::test]
async fn reset_only_affects_the_account_that_owns_the_token() {
    let (app, pool) = build_app().await;
    let email_a = unique_email("owner");
    let email_b = unique_email("bystander");
    let user_a = register_verified(&app, &pool, &email_a).await;
    register_verified(&app, &pool, &email_b).await;

    let token = store_new_token(&pool, user_a).await.unwrap().unwrap();
    let (status, _) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(login(&app, &email_a, NEW_PASSWORD).await["code"], 200);
    // La otra cuenta conserva su contrasena original.
    assert_eq!(login(&app, &email_b, PASSWORD).await["code"], 200);
    assert_eq!(login(&app, &email_b, NEW_PASSWORD).await["code"], 401);

    cleanup(&pool, &email_a).await;
    cleanup(&pool, &email_b).await;
}
