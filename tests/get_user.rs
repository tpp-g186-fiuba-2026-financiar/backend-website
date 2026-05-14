use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const JWT_SECRET: &str = "test-secret-for-get-user";
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
    format!("get_user_{tag}_{nanos}@test.com")
}

async fn register_and_login(
    state: &AppState,
    email: &str,
    password: &str,
    full_name: &str,
) -> String {
    let app = app_with_state(state.clone());

    let register_body = json!({
        "email": email,
        "password": password,
        "full_name": full_name,
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
    assert_eq!(json["code"], 200);
    json["token"]
        .as_str()
        .expect("token should be present in login response")
        .to_string()
}

#[tokio::test]
async fn get_user_without_token_returns_401() {
    let state = setup().await;
    let app = app_with_state(state);

    let response = app
        .oneshot(Request::builder().uri("/user").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 401);
}

#[tokio::test]
async fn get_user_with_invalid_token_returns_401() {
    let state = setup().await;
    let app = app_with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, "Bearer not-a-valid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_user_with_malformed_authorization_header_returns_401() {
    let state = setup().await;
    let app = app_with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_user_with_valid_token_returns_user_info() {
    let state = setup().await;
    let email = unique_email("happy");
    let token = register_and_login(&state, &email, "StrongPassword123!", "Get User").await;

    let app = app_with_state(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["email"], email);
    assert_eq!(json["full_name"], "Get User");
    assert_eq!(json["risk_profile"], "moderate");
    assert_eq!(json["is_active"], true);
    assert!(json["id"].is_i64());

    cleanup(&state.pool, &email).await;
}

#[tokio::test]
async fn get_user_with_token_signed_by_other_secret_returns_401() {
    let state = setup().await;
    let foreign_jwt = JwtConfig::new("a-completely-different-secret", JWT_EXP_HOURS);
    let foreign_token = foreign_jwt.encode_token(1, "intruder@test.com").unwrap();

    let app = app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {foreign_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_user_returns_404_when_user_was_deleted() {
    let state = setup().await;
    let email = unique_email("ghost");
    let token = register_and_login(&state, &email, "StrongPassword123!", "Ghost User").await;

    cleanup(&state.pool, &email).await;

    let app = app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/user")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 404);
}
