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

const JWT_SECRET: &str = "test-secret-for-share-pnl";
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
    format!("share_pnl_{tag}_{nanos}@test.com")
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

// El endpoint POST valida el ticker contra data-colector (real, sin
// mockear) pero ademas exige que el ticker ya exista en el catalogo local
// `shares` (lo puebla normalmente GET /shares/update). Lo sembramos a mano
// aca para no depender de esa sincronizacion en el test.
async fn seed_catalog_ticker(pool: &sqlx::PgPool, ticker: &str) {
    sqlx::query("INSERT INTO shares (ticker) VALUES ($1) ON CONFLICT (ticker) DO NOTHING")
        .bind(ticker)
        .execute(pool)
        .await
        .expect("failed to seed shares catalog");
}

async fn create_share(
    pool: &sqlx::PgPool,
    app: Router,
    token: &str,
    ticker: &str,
    quantity: i32,
    entry_price: Option<f64>,
) {
    seed_catalog_ticker(pool, ticker).await;
    let mut payload = json!({ "ticker": ticker, "quantity": quantity });
    if let Some(price) = entry_price {
        payload["entry_price"] = json!(price);
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/shares")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn get_pnl(app: Router, token: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares/pnl")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

#[tokio::test]
async fn pnl_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/user/shares/pnl")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn pnl_with_no_shares_returns_empty_portfolio() {
    let state = setup().await;
    let email = unique_email("empty");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let (status, json) = get_pnl(app, &token).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["shares"].as_array().unwrap().len(), 0);
    assert_eq!(json["portfolio"]["total_invested"], 0.0);
    assert_eq!(json["portfolio"]["total_current_value"], 0.0);
    assert_eq!(json["portfolio"]["total_pnl_amount"], 0.0);
    assert!(json["portfolio"]["total_pnl_percentage"].is_null());

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn pnl_without_entry_price_returns_null_pnl_fields() {
    let state = setup().await;
    let email = unique_email("no_entry_price");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "GGAL",
        10,
        None,
    )
    .await;

    let app = build_app(state.clone()).await;
    let (status, json) = get_pnl(app, &token).await;

    assert_eq!(status, StatusCode::OK);
    let shares = json["shares"].as_array().unwrap();
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0]["ticker"], "GGAL");
    assert!(shares[0]["entry_price"].is_null());
    assert!(shares[0]["pnl_amount"].is_null());
    assert!(shares[0]["pnl_percentage"].is_null());
    // Sin entry_price, esta tenencia no debe aportar a los agregados de P&L.
    assert_eq!(json["portfolio"]["total_invested"], 0.0);
    assert_eq!(json["portfolio"]["total_pnl_amount"], 0.0);

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn pnl_with_entry_price_computes_consistent_amounts() {
    let state = setup().await;
    let email = unique_email("with_entry_price");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    create_share(
        &state.pool,
        build_app(state.clone()).await,
        &token,
        "GGAL",
        10,
        Some(1.0),
    )
    .await;

    let app = build_app(state.clone()).await;
    let (status, json) = get_pnl(app, &token).await;

    assert_eq!(status, StatusCode::OK);
    let shares = json["shares"].as_array().unwrap();
    assert_eq!(shares.len(), 1);
    let share = &shares[0];
    assert_eq!(share["entry_price"], 1.0);

    // El precio actual depende de data-colector (real); si no pudo
    // resolverlo, el resto de los campos derivados deben ser null y
    // consistentes entre si.
    if share["current_price"].is_null() {
        assert!(share["pnl_amount"].is_null());
        assert!(share["pnl_percentage"].is_null());
    } else {
        let current = share["current_price"].as_f64().unwrap();
        let pnl_amount = share["pnl_amount"].as_f64().unwrap();
        let expected_amount = (current - 1.0) * 10.0;
        assert!((pnl_amount - expected_amount).abs() < 1e-6);

        let pnl_percentage = share["pnl_percentage"].as_f64().unwrap();
        let expected_percentage = expected_amount / (1.0 * 10.0) * 100.0;
        assert!((pnl_percentage - expected_percentage).abs() < 1e-6);

        let total_pnl_amount = json["portfolio"]["total_pnl_amount"].as_f64().unwrap();
        assert!((total_pnl_amount - expected_amount).abs() < 1e-6);
    }

    cleanup_user(&state.pool, &email).await;
}
