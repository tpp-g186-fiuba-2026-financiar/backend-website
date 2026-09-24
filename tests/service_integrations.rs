use std::{collections::HashMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Method, Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

#[derive(Clone, Default)]
struct StubState {
    mode: Arc<std::sync::atomic::AtomicU8>,
}

async fn historical(Path(ticker): Path<String>) -> impl IntoResponse {
    match ticker.as_str() {
        "BAD" => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "upstream"})),
        ),
        "INVALID" => (StatusCode::OK, Json(json!({"data": "not-an-array"}))),
        _ => (
            StatusCode::OK,
            Json(json!({
                "data": [
                    {"ts": 1_700_000_000_000_i64, "close_amount": "100.0"},
                    {"ts": 1_705_000_000_000_i64, "close_amount": "invalid"},
                    {"ts": 1_710_000_000_000_i64, "close_amount": "125.5"}
                ],
                "status": 200,
                "cached": false
            })),
        ),
    }
}

async fn available(State(state): State<StubState>) -> impl IntoResponse {
    if state.mode.load(std::sync::atomic::Ordering::Relaxed) == 1 {
        Json(json!({"unexpected": true}))
    } else {
        Json(json!({"message": {"tickers": ["COVR", "SYNC"]}}))
    }
}

async fn ready() -> Json<Value> {
    Json(json!({"message": {"tickers": ["COVR"]}}))
}

async fn trend_model() -> Json<Value> {
    Json(json!({
        "signal": "alza",
        "condition": "neutral",
        "rsi": 55.0,
        "horizon_days": 5,
        "last_close": 100.0,
        "predicted_close": 110.0,
        "as_of": "2026-09-17",
        "model_version": "coverage-v1",
        "backtest": {"directional_accuracy": 0.82, "mae": 0.04, "observations": 12}
    }))
}

async fn xgboost_model() -> Json<Value> {
    Json(json!({
        "signal": "neutral",
        "condition": "neutral",
        "rsi": 50.0,
        "horizon_days": 5,
        "last_close": 100.0,
        "predicted_close": 100.5,
        "as_of": "2026-09-17",
        "model_version": "coverage-v1",
        "backtest": {"directional_accuracy": 0.65}
    }))
}

async fn arima_model() -> Json<Value> {
    Json(json!({
        "valor_actual": 100.0,
        "prediction": [101.0, 102.0, 103.0],
        "condition": "neutral",
        "rsi": 51.0,
        "as_of": "2026-09-17",
        "model_version": "coverage-v1",
        "backtest": {"directional_accuracy": 0.7}
    }))
}

async fn svm_model() -> Json<Value> {
    Json(json!({
        "prediction": "Buy",
        "model_version": "coverage-v1",
        "backtest": {"directional_accuracy": 0.75}
    }))
}

async fn garch_model() -> Json<Value> {
    Json(json!({
        "prediction": {"h.2": {"0": 0.09}, "h.1": {"0": 0.04}},
        "model_version": "coverage-v1",
        "backtest": {"variance_mae": 0.01}
    }))
}

async fn local_models(Path(_ticker): Path<String>) -> Json<Value> {
    Json(json!({
        "predictions": {
            "lstm": {"signal": "alza", "backtest": {"directional_accuracy": 0.6}},
            "xgboost": {"available": false, "reason": "sin artefacto"},
            "transformer": {"signal": "neutral", "backtest": {"directional_accuracy": 0.5}}
        }
    }))
}

async fn portfolio(Query(query): Query<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    if query.get("model").is_some_and(|value| value == "unknown") {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"detail": "modelo sin datos suficientes"})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({"pesos_recomendados": {"COVR": 1.0}})),
    )
}

async fn start_stub() -> (String, StubState) {
    let state = StubState::default();
    let app = Router::new()
        .route("/historical-data/{ticker}", post(historical))
        .route("/share/sector/{ticker}", get(sector))
        .route("/available-tickers", post(available))
        .route("/model-ready-tickers", post(ready))
        .route("/lstm", get(trend_model))
        .route("/xgboost", get(xgboost_model))
        .route("/arima", get(arima_model))
        .route("/svm", get(svm_model))
        .route("/garch", get(garch_model))
        .route("/predict/trend/compare/{ticker}", get(local_models))
        .route("/portfolio/recomendacion", post(portfolio))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), state)
}

async fn setup() -> (AppState, String, String) {
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let jwt = JwtConfig::new("service-integration-secret", 24);
    let email = format!(
        "service_{}@test.com",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let user_id: i32 = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, full_name) VALUES ($1, 'hash', 'Coverage') RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();

    let user_investing_profile = sqlx::query_scalar(
        "INSERT INTO user_investing_profiles (user_id, risk_profile) VALUES ($1, 'moderate') RETURNING user_id",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let share_id: i32 = sqlx::query_scalar(
        "INSERT INTO shares (ticker) VALUES ('COVR') ON CONFLICT (ticker) DO UPDATE SET ticker = EXCLUDED.ticker RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_shares (user_id, share_id, quantity, entry_price) VALUES ($1, $2, 2, 100.0)")
        .bind(user_id)
        .bind(share_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ticker_trend_cache WHERE ticker = 'COVR'")
        .execute(&pool)
        .await
        .unwrap();
    let token = jwt.encode_token(user_id, &email).unwrap();
    (
        AppState {
            pool,
            jwt_config: jwt,
        },
        token,
        email,
    )
}

async fn build_app(state: AppState) -> Router {
    let session_store = PostgresStore::new(state.pool.clone());
    session_store.migrate().await.unwrap();
    app_with_state(
        state,
        SessionManagerLayer::new(session_store).with_secure(false),
    )
}

async fn request(
    app: Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn external_service_endpoints_cover_success_and_error_paths() {
    let (base, stub_state) = start_stub().await;
    std::env::set_var("DATA_COLLECTOR_URL", &base);
    std::env::set_var("API_ML_URL", &base);
    std::env::set_var("MODAL_LSTM_URL", format!("{base}/lstm"));
    std::env::set_var("MODAL_XGBOOST_URL", format!("{base}/xgboost"));
    std::env::set_var("MODAL_ARIMA_URL", format!("{base}/arima"));
    std::env::set_var("MODAL_SVM_URL", format!("{base}/svm"));
    std::env::set_var("MODAL_GARCH_URL", format!("{base}/garch"));

    let (state, token, email) = setup().await;
    let app = build_app(state.clone()).await;

    let (status, body) = request(app.clone(), Method::GET, "/shares", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["shares"]
        .as_array()
        .unwrap()
        .iter()
        .any(|share| share["ticker"] == "COVR" && share["predictable"] == true));

    let (status, body) = request(app.clone(), Method::GET, "/shares/update", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/COVR/history",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ticker"], "COVR");
    assert_eq!(body["prices"].as_array().unwrap().len(), 2);

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/balance",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["shares"][0]["current_price"], 125.5);

    let (status, body) = request(app.clone(), Method::GET, "/user/shares/pnl", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["shares"][0]["current_price"], 125.5);

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/trends",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["trends"][0]["available"], true);

    let (status, cached) = request(
        app.clone(),
        Method::GET,
        "/user/shares/trends",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cached["trends"][0]["ticker"], "COVR");

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/covr/trends/compare",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["symbol"], "COVR");
    assert_eq!(body["default_model"], "lstm-modal");
    assert_eq!(body["predictions"]["svm-modal"]["signal"], "alza");
    assert_eq!(
        body["predictions"]["garch-modal"]["volatility_forecast"][0]["horizon_days"],
        1
    );

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/portfolio/recomendacion?model=lstm&cartera_ancla=propia",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pesos_recomendados"]["COVR"], 1.0);
    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/portfolio/recomendacion?model=unknown",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["message"], "modelo sin datos suficientes");

    let (status, _) = request(
        app.clone(),
        Method::GET,
        "/user/shares/BAD/history",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/INVALID/history",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["prices"].as_array().unwrap().is_empty());

    stub_state
        .mode
        .store(1, std::sync::atomic::Ordering::Relaxed);
    let (status, body) = request(app, Method::GET, "/shares/update", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["code"], 500);

    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(&state.pool)
        .await
        .unwrap();
}
