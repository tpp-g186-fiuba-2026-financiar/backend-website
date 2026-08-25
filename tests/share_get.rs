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

const JWT_SECRET: &str = "test-secret-for-share-get";
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

async fn cleanup_user(pool: &sqlx::PgPool, email: &str) {
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
    format!("share_get_{tag}_{nanos}@test.com")
}

async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
    let app = build_app(state.clone()).await;

    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": "Share Tester",
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

// El endpoint valida el ticker contra data-colector (real, sin mockear) pero
// ademas exige que el ticker ya exista en el catalogo local `shares` (lo
// puebla normalmente GET /shares/update). Lo sembramos a mano aca para no
// depender de esa sincronizacion en el test.
async fn seed_catalog_ticker(pool: &sqlx::PgPool, ticker: &str) {
    sqlx::query("INSERT INTO shares (ticker) VALUES ($1) ON CONFLICT (ticker) DO NOTHING")
        .bind(ticker)
        .execute(pool)
        .await
        .expect("failed to seed shares catalog");
}

async fn create_share(pool: &sqlx::PgPool, app: Router, token: &str, ticker: &str, quantity: i32) {
    seed_catalog_ticker(pool, ticker).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/shares")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "ticker": ticker, "quantity": quantity }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn create_share_with_entry_price(
    pool: &sqlx::PgPool,
    app: Router,
    token: &str,
    ticker: &str,
    quantity: i32,
    entry_price: f64,
) {
    seed_catalog_ticker(pool, ticker).await;
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/shares")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "ticker": ticker, "quantity": quantity, "entry_price": entry_price })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn get_shares_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_shares_with_no_data_returns_empty_list() {
    let state = setup().await;
    let email = unique_email("empty");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["shares"].is_array());
    assert_eq!(json["shares"].as_array().unwrap().len(), 0);

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn get_shares_returns_only_authenticated_user_shares() {
    let state = setup().await;
    let email = unique_email("multi");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "GGAL",
        10,
    )
    .await;
    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "YPFD",
        5,
    )
    .await;
    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "PAMP",
        3,
    )
    .await;

    let app = build_app(state.clone()).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let shares = json["shares"].as_array().unwrap();
    assert_eq!(shares.len(), 3);

    let tickers: Vec<&str> = shares
        .iter()
        .map(|s| s["ticker"].as_str().unwrap())
        .collect();
    assert!(tickers.contains(&"GGAL"));
    assert!(tickers.contains(&"YPFD"));
    assert!(tickers.contains(&"PAMP"));

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn get_shares_includes_entry_price() {
    let state = setup().await;
    let email = unique_email("entry_price");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    create_share_with_entry_price(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "GGAL",
        10,
        1520.50,
    )
    .await;
    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "YPFD",
        5,
    )
    .await;

    let app = build_app(state.clone()).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let shares = json["shares"].as_array().unwrap();

    let ggal = shares.iter().find(|s| s["ticker"] == "GGAL").unwrap();
    assert_eq!(ggal["entry_price"], 1520.50);

    let ypfd = shares.iter().find(|s| s["ticker"] == "YPFD").unwrap();
    assert!(ypfd["entry_price"].is_null());

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn get_shares_does_not_leak_other_users_shares() {
    let state = setup().await;
    let email_a = unique_email("user_a");
    let email_b = unique_email("user_b");

    let token_a = register_and_login(&state, &email_a, "StrongPassword123!").await;
    let token_b = register_and_login(&state, &email_b, "StrongPassword123!").await;

    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token_a,
        "GGAL",
        10,
    )
    .await;
    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token_b,
        "PAMP",
        7,
    )
    .await;

    let app = build_app(state.clone()).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares")
                .header(header::AUTHORIZATION, format!("Bearer {token_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let shares = json["shares"].as_array().unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["ticker"], "GGAL");

    cleanup_user(&state.pool, &email_a).await;
    cleanup_user(&state.pool, &email_b).await;
}
