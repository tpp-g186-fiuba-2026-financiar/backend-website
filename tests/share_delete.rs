// use axum::{
//     body::Body,
//     http::{header, Request, StatusCode},
//     Router,
// };
// use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
// use dotenvy::dotenv;
// use http_body_util::BodyExt;
// use serde_json::{json, Value};
// use tower::ServiceExt;
// use tower_sessions::SessionManagerLayer;
// use tower_sessions_sqlx_store::PostgresStore;

// const JWT_SECRET: &str = "test-secret-for-share-delete";
// const JWT_EXP_HOURS: i64 = 24;

// async fn setup() -> AppState {
//     dotenv().ok();
//     let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
//     let pool = sqlx::PgPool::connect(&database_url)
//         .await
//         .expect("Failed to connect to the database");
//     AppState {
//         pool,
//         jwt_config: JwtConfig::new(JWT_SECRET, JWT_EXP_HOURS),
//     }
// }

// async fn build_app(state: AppState) -> Router {
//     let session_store = PostgresStore::new(state.pool.clone());
//     session_store
//         .migrate()
//         .await
//         .expect("Failed to run session store migrations");
//     let session_layer = SessionManagerLayer::new(session_store).with_secure(false);
//     app_with_state(state, session_layer)
// }

// async fn cleanup_user(pool: &sqlx::PgPool, email: &str) {
//     let _ = sqlx::query("DELETE FROM users WHERE email = $1")
//         .bind(email)
//         .execute(pool)
//         .await;
// }

// fn unique_email(tag: &str) -> String {
//     let nanos = std::time::SystemTime::now()
//         .duration_since(std::time::UNIX_EPOCH)
//         .unwrap()
//         .as_nanos();
//     format!("share_delete_{tag}_{nanos}@test.com")
// }

// async fn register_and_login(state: &AppState, email: &str, password: &str) -> String {
//     let app = build_app(state.clone()).await;

//     let register_body = json!({
//         "email": email,
//         "password": password,
//         "full_name": "Share Tester",
//         "risk_profile": "moderate",
//     });

//     let response = app
//         .clone()
//         .oneshot(
//             Request::builder()
//                 .method("POST")
//                 .uri("/register")
//                 .header(header::CONTENT_TYPE, "application/json")
//                 .body(Body::from(register_body.to_string()))
//                 .unwrap(),
//         )
//         .await
//         .unwrap();
//     assert_eq!(response.status(), StatusCode::OK, "register should succeed");

//     let login_body = json!({ "email": email, "password": password });
//     let response = app
//         .oneshot(
//             Request::builder()
//                 .method("POST")
//                 .uri("/login")
//                 .header(header::CONTENT_TYPE, "application/json")
//                 .body(Body::from(login_body.to_string()))
//                 .unwrap(),
//         )
//         .await
//         .unwrap();
//     assert_eq!(response.status(), StatusCode::OK, "login should succeed");

//     let body = response.into_body().collect().await.unwrap().to_bytes();
//     let json: Value = serde_json::from_slice(&body).unwrap();
//     json["token"]
//         .as_str()
//         .expect("token should be present in login response")
//         .to_string()
// }

// async fn create_share(app: Router, token: &str, ticker: &str, quantity: i32) -> i32 {
//     let response = app
//         .oneshot(
//             Request::builder()
//                 .method("POST")
//                 .uri("/shares")
//                 .header(header::CONTENT_TYPE, "application/json")
//                 .header(header::AUTHORIZATION, format!("Bearer {token}"))
//                 .body(Body::from(
//                     json!({ "ticker": ticker, "quantity": quantity }).to_string(),
//                 ))
//                 .unwrap(),
//         )
//         .await
//         .unwrap();
//     assert_eq!(response.status(), StatusCode::CREATED);
//     let body = response.into_body().collect().await.unwrap().to_bytes();
//     let json: Value = serde_json::from_slice(&body).unwrap();
//     json["id"].as_i64().unwrap() as i32
// }

// async fn delete_share(app: Router, token: &str, share_id: i32) -> StatusCode {
//     let response = app
//         .oneshot(
//             Request::builder()
//                 .method("DELETE")
//                 .uri(format!("/shares/{share_id}"))
//                 .header(header::AUTHORIZATION, format!("Bearer {token}"))
//                 .body(Body::empty())
//                 .unwrap(),
//         )
//         .await
//         .unwrap();
//     response.status()
// }

// async fn count_shares_for_user(state: &AppState, token: &str) -> usize {
//     let app = build_app(state.clone()).await;
//     let response = app
//         .oneshot(
//             Request::builder()
//                 .uri("/shares")
//                 .header(header::AUTHORIZATION, format!("Bearer {token}"))
//                 .body(Body::empty())
//                 .unwrap(),
//         )
//         .await
//         .unwrap();
//     let body = response.into_body().collect().await.unwrap().to_bytes();
//     let json: Value = serde_json::from_slice(&body).unwrap();
//     json["shares"].as_array().unwrap().len()
// }

// #[tokio::test]
// async fn delete_share_without_token_returns_401() {
//     let state = setup().await;
//     let app = build_app(state).await;

//     let response = app
//         .oneshot(
//             Request::builder()
//                 .method("DELETE")
//                 .uri("/shares/1")
//                 .body(Body::empty())
//                 .unwrap(),
//         )
//         .await
//         .unwrap();

//     assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
// }

// #[tokio::test]
// async fn delete_share_with_valid_id_returns_204() {
//     let state = setup().await;
//     let email = unique_email("happy");
//     let token = register_and_login(&state, &email, "StrongPassword123!").await;

//     let share_id = create_share(build_app(state.clone()).await, &token, "GGAL", 10).await;
//     assert_eq!(count_shares_for_user(&state, &token).await, 1);

//     let status = delete_share(build_app(state.clone()).await, &token, share_id).await;
//     assert_eq!(status, StatusCode::NO_CONTENT);

//     assert_eq!(count_shares_for_user(&state, &token).await, 0);

//     cleanup_user(&state.pool, &email).await;
// }

// #[tokio::test]
// async fn delete_share_with_nonexistent_id_returns_404() {
//     let state = setup().await;
//     let email = unique_email("missing");
//     let token = register_and_login(&state, &email, "StrongPassword123!").await;

//     let status = delete_share(build_app(state.clone()).await, &token, 999_999).await;
//     assert_eq!(status, StatusCode::NOT_FOUND);

//     cleanup_user(&state.pool, &email).await;
// }

// #[tokio::test]
// async fn delete_share_owned_by_other_user_returns_404() {
//     let state = setup().await;
//     let email_a = unique_email("owner");
//     let email_b = unique_email("intruder");

//     let token_a = register_and_login(&state, &email_a, "StrongPassword123!").await;
//     let token_b = register_and_login(&state, &email_b, "StrongPassword123!").await;

//     let share_id = create_share(build_app(state.clone()).await, &token_a, "GGAL", 10).await;

//     let status = delete_share(build_app(state.clone()).await, &token_b, share_id).await;
//     assert_eq!(status, StatusCode::NOT_FOUND);

//     assert_eq!(count_shares_for_user(&state, &token_a).await, 1);

//     cleanup_user(&state.pool, &email_a).await;
//     cleanup_user(&state.pool, &email_b).await;
// }
