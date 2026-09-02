use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const JWT_SECRET: &str = "test-secret-for-alert-subscriptions";
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
    format!("alerts_{tag}_{nanos}@test.com")
}

async fn seed_catalog_ticker(pool: &sqlx::PgPool, ticker: &str) {
    sqlx::query("INSERT INTO shares (ticker) VALUES ($1) ON CONFLICT (ticker) DO NOTHING")
        .bind(ticker)
        .execute(pool)
        .await
        .expect("failed to seed shares catalog");
}

async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
    let app = build_app(state.clone()).await;

    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": "Alerts Tester",
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

/// Agrega un ticker a la cartera declarada del usuario (via /user/shares),
/// que es el prerequisito de dominio para poder suscribirse a alertas de ese
/// ticker puntual.
async fn add_share_to_portfolio(app: Router, token: &str, ticker: &str) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/shares")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "ticker": ticker, "quantity": 1 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "seeding the portfolio share should succeed"
    );
}

async fn request(app: Router, method: Method, uri: &str, token: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn subscribe_ticker_without_token_returns_401() {
    let state = setup().await;
    let app = build_app(state).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/alerts/subscriptions/GGAL")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn subscribe_ticker_not_in_portfolio_returns_404() {
    let state = setup().await;
    let email = unique_email("not_in_portfolio");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let (status, json) =
        request(app, Method::POST, "/user/alerts/subscriptions/GGAL", &token).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 404);

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn subscribe_ticker_returns_201_then_conflicts_on_duplicate() {
    let state = setup().await;
    let email = unique_email("dup");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    seed_catalog_ticker(&state.pool, "GGAL").await;
    add_share_to_portfolio(build_app(state.clone()).await, &token, "GGAL").await;

    let app = build_app(state.clone()).await;
    let (status, json) =
        request(app, Method::POST, "/user/alerts/subscriptions/ggal", &token).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(json["ticker"], "GGAL");

    let app = build_app(state.clone()).await;
    let (status, json) =
        request(app, Method::POST, "/user/alerts/subscriptions/GGAL", &token).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["code"], 409);

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn unsubscribe_ticker_removes_subscription() {
    let state = setup().await;
    let email = unique_email("unsub");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    seed_catalog_ticker(&state.pool, "YPFD").await;
    add_share_to_portfolio(build_app(state.clone()).await, &token, "YPFD").await;

    let app = build_app(state.clone()).await;
    let (status, _) = request(app, Method::POST, "/user/alerts/subscriptions/YPFD", &token).await;
    assert_eq!(status, StatusCode::CREATED);

    let app = build_app(state.clone()).await;
    let (status, _) = request(
        app,
        Method::DELETE,
        "/user/alerts/subscriptions/YPFD",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let app = build_app(state.clone()).await;
    let (status, json) = request(
        app,
        Method::DELETE,
        "/user/alerts/subscriptions/YPFD",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["code"], 404);

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn subscribe_and_unsubscribe_portfolio() {
    let state = setup().await;
    let email = unique_email("portfolio");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;

    let app = build_app(state.clone()).await;
    let (status, json) = request(
        app,
        Method::POST,
        "/user/alerts/subscriptions/portfolio",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(json["ticker"].is_null());

    let app = build_app(state.clone()).await;
    let (status, json) = request(
        app,
        Method::POST,
        "/user/alerts/subscriptions/portfolio",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["code"], 409);

    let app = build_app(state.clone()).await;
    let (status, _) = request(
        app,
        Method::DELETE,
        "/user/alerts/subscriptions/portfolio",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn list_subscriptions_returns_tickers_and_portfolio_flag() {
    let state = setup().await;
    let email = unique_email("list");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    seed_catalog_ticker(&state.pool, "GGAL").await;
    add_share_to_portfolio(build_app(state.clone()).await, &token, "GGAL").await;

    let app = build_app(state.clone()).await;
    let (status, _) = request(app, Method::POST, "/user/alerts/subscriptions/GGAL", &token).await;
    assert_eq!(status, StatusCode::CREATED);

    let app = build_app(state.clone()).await;
    let (status, _) = request(
        app,
        Method::POST,
        "/user/alerts/subscriptions/portfolio",
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let app = build_app(state.clone()).await;
    let (status, json) = request(app, Method::GET, "/user/alerts/subscriptions", &token).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["portfolio"], true);
    assert_eq!(json["tickers"].as_array().unwrap(), &vec![json!("GGAL")]);

    cleanup_user(&state.pool, &email).await;
}
