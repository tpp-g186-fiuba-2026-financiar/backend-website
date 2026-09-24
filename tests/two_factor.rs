use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use backend_website::endpoints::user::two_factor::totp;
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const PASSWORD: &str = "StrongPassword123!";

async fn build_app() -> (Router, sqlx::PgPool) {
    dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database");
    let state = AppState {
        pool: pool.clone(),
        jwt_config: JwtConfig::new("test-secret-for-2fa", 24),
    };
    let session_store = PostgresStore::new(pool.clone());
    session_store.migrate().await.expect("session migrate");
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);
    (app_with_state(state, session_layer), pool)
}

fn unique_email() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("twofa_{nanos}@test.com")
}

async fn post(app: &Router, uri: &str, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn register_and_login(app: &Router, email: &str) -> String {
    let (status, _) = post(
        app,
        "/register",
        None,
        json!({
            "email": email,
            "password": PASSWORD,
            "full_name": "2FA Tester",
            "risk_profile": "moderate",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = post(
        app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(body["code"], 200);
    body["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn two_factor_full_flow() {
    let (app, pool) = build_app().await;
    let email = unique_email();
    let token = register_and_login(&app, &email).await;

    // Requiere autenticacion.
    let (status, _) = post(&app, "/user/2fa/setup", None, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Enable sin setup previo falla.
    let (status, _) = post(
        &app,
        "/user/2fa/enable",
        Some(&token),
        json!({ "code": "000000" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Setup devuelve secreto, url y QR.
    let (status, setup) = post(&app, "/user/2fa/setup", Some(&token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_string();
    assert!(setup["otpauth_url"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/"));
    assert!(!setup["qr_base64"].as_str().unwrap().is_empty());

    // Mientras no se confirme, el login sigue sin pedir codigo.
    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(body["code"], 200);

    // Codigo incorrecto no activa.
    let (status, _) = post(
        &app,
        "/user/2fa/enable",
        Some(&token),
        json!({ "code": "abcdef" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Codigo correcto activa.
    let code = totp::current_code(&secret, &email).unwrap();
    let (status, _) = post(
        &app,
        "/user/2fa/enable",
        Some(&token),
        json!({ "code": code }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Setup ya no se puede repetir.
    let (status, _) = post(&app, "/user/2fa/setup", Some(&token), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Login sin codigo => pide 2FA y no da token.
    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(body["code"], 401);
    assert_eq!(body["two_factor_required"], true);
    assert!(body["token"].is_null());

    // Login con codigo incorrecto.
    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD, "totp_code": "000000" }),
    )
    .await;
    assert_eq!(body["code"], 401);
    assert!(body["token"].is_null());

    // Password incorrecta con codigo valido sigue fallando.
    let code = totp::current_code(&secret, &email).unwrap();
    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": "wrong", "totp_code": code }),
    )
    .await;
    assert_eq!(body["code"], 401);

    // Login con codigo correcto.
    let code = totp::current_code(&secret, &email).unwrap();
    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD, "totp_code": code }),
    )
    .await;
    assert_eq!(body["code"], 200);
    assert!(body["token"].is_string());

    // Disable exige codigo valido.
    let (status, _) = post(
        &app,
        "/user/2fa/disable",
        Some(&token),
        json!({ "code": "000000" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let code = totp::current_code(&secret, &email).unwrap();
    let (status, _) = post(
        &app,
        "/user/2fa/disable",
        Some(&token),
        json!({ "code": code }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Vuelve a loguear sin codigo.
    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD }),
    )
    .await;
    assert_eq!(body["code"], 200);

    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(&email)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn disable_without_2fa_enabled_returns_400() {
    let (app, pool) = build_app().await;
    let email = unique_email();
    let token = register_and_login(&app, &email).await;

    let (status, body) = post(
        &app,
        "/user/2fa/disable",
        Some(&token),
        json!({ "code": "123456" }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["message"], "Two-factor authentication is not enabled");
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(&email)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn two_factor_endpoints_return_404_when_user_no_longer_exists() {
    let (app, pool) = build_app().await;
    let email = unique_email();
    let token = register_and_login(&app, &email).await;
    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(&email)
        .execute(&pool)
        .await
        .unwrap();

    for uri in ["/user/2fa/setup", "/user/2fa/enable", "/user/2fa/disable"] {
        let (status, body) = post(&app, uri, Some(&token), json!({ "code": "123456" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(body["message"], "User not found");
    }
}

#[tokio::test]
async fn login_with_2fa_and_blank_code_asks_for_it() {
    let (app, pool) = build_app().await;
    let email = unique_email();
    let token = register_and_login(&app, &email).await;
    let (_, setup) = post(&app, "/user/2fa/setup", Some(&token), json!({})).await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    let code = totp::current_code(&secret, &email).unwrap();
    post(
        &app,
        "/user/2fa/enable",
        Some(&token),
        json!({ "code": code }),
    )
    .await;

    let (_, body) = post(
        &app,
        "/login",
        None,
        json!({ "email": email, "password": PASSWORD, "totp_code": "   " }),
    )
    .await;

    assert_eq!(body["code"], 401);
    assert_eq!(body["message"], "Two-factor code required");
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(&email)
        .execute(&pool)
        .await;
}
