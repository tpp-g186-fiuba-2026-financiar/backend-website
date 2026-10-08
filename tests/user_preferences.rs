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

const JWT_SECRET: &str = "test-secret-for-user-preferences";
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

fn unique_email(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("preferences_{tag}_{nanos}@test.com")
}

async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
    let app = build_app(state.clone()).await;

    let register_body = json!(
        {
            "email": email,
            "password": password,
            "full_name": "Preference Tester",
            "risk_profile": "moderate",
        }
    );

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

async fn cleanup_user(pool: &sqlx::PgPool, email: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await;
}

async fn get_user_id(pool: &sqlx::PgPool, email: &str) -> i32 {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .expect("user should exist")
}

async fn seed_share_for_user(pool: &sqlx::PgPool, user_id: i32, ticker: &str) {
    let share_id: i32 = sqlx::query_scalar(
        "INSERT INTO shares (ticker) VALUES ($1) ON CONFLICT (ticker) DO UPDATE SET ticker = EXCLUDED.ticker RETURNING id",
    )
    .bind(ticker)
    .fetch_one(pool)
    .await
    .expect("failed to seed share catalog");

    sqlx::query(
        "INSERT INTO user_shares (user_id, share_id, quantity, entry_price) VALUES ($1, $2, 5, 100.0)",
    )
    .bind(user_id)
    .bind(share_id)
    .execute(pool)
    .await
    .expect("failed to seed user share");
}

async fn send_post_preference(app: Router, token: &str, payload: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/user/preferences")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(payload.to_string()))
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
async fn setting_a_user_preference_returns_saved_values() {
    let state = setup().await;
    let email = unique_email("set");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let user_id = get_user_id(&state.pool, &email).await;

    let app = build_app(state.clone()).await;
    let (status, body) = send_post_preference(
        app,
        &token,
        json!({ "stock": "ggal", "model": "transformer-modal" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["user_id"], user_id);
    assert_eq!(body["stock"], "GGAL");
    assert_eq!(body["model"], "transformer-modal");

    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn user_preferences_are_used_when_fetching_trends() {
    let state = setup().await;
    let email = unique_email("trend");
    let token = register_and_login(&state, &email, "StrongPassword123!").await;
    let user_id = get_user_id(&state.pool, &email).await;

    let app = build_app(state.clone()).await;
    let (status, _) = send_post_preference(
        app.clone(),
        &token,
        json!({ "stock": "ggal", "model": "xgboost-modal" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    seed_share_for_user(&state.pool, user_id, "GGAL").await;

    let trend_response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/user/shares/trends")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(trend_response.status(), StatusCode::OK);
    let body = trend_response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();

    let trend = json["trends"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["ticker"] == "GGAL")
        .expect("preference should produce a trend for GGAL");

    assert_eq!(trend["model"], "xgboost-modal");
    assert_eq!(trend["ticker"], "GGAL");

    cleanup_user(&state.pool, &email).await;
}
