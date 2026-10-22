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
                    {"ts": 1_700_000_000_000_i64, "open_amount": "99", "high_amount": "101", "low_amount": "98", "close_amount": "100.0", "volume": 10},
                    {"ts": 1_705_000_000_000_i64, "close_amount": "invalid"},
                    {"ts": 1_710_000_000_000_i64, "open_amount": "120", "high_amount": "126", "low_amount": "119", "close_amount": "125.5", "volume": 20}
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

async fn sector(Path(ticker): Path<String>) -> Json<Value> {
    Json(json!({"ticker": ticker, "sector": "Financials"}))
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
        "backtest": {"directional_accuracy": 0.65, "observations": 12}
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
        "backtest": {"directional_accuracy": 0.7, "observations": 12}
    }))
}

async fn svm_model() -> Json<Value> {
    Json(json!({
        "prediction": "Buy",
        "model_version": "coverage-v1",
        "backtest": {"directional_accuracy": 0.75, "observations": 12}
    }))
}

async fn garch_model() -> Json<Value> {
    Json(json!({
        "prediction": {"h.2": {"0": 0.09}, "h.1": {"0": 0.04}},
        "model_version": "coverage-v1",
        "backtest": {"variance_mae": 0.01}
    }))
}

async fn garch_ann_model() -> Json<Value> {
    Json(json!({
        "prediction": {"next_day_abs_return_pct": 1.234},
        "model_version": "coverage-v1",
        "backtest": {"abs_return_mae": 0.9}
    }))
}

async fn local_models(Path(_ticker): Path<String>) -> Json<Value> {
    Json(json!({
        "predictions": {
            "lstm": {"signal": "alza", "backtest": {"directional_accuracy": 0.6, "observations": 12}},
            "xgboost": {"available": false, "reason": "sin artefacto"},
            "transformer": {"signal": "neutral", "backtest": {"directional_accuracy": 0.5, "observations": 12}},
            "macro": {"signal": "baja", "horizon_days": 20, "probability_down": 0.61},
            "contexto": {"signal": "alza", "horizon_days": 20, "probability_down": 0.18}
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

async fn rate_series(Path(series): Path<String>) -> Json<Value> {
    if series == "RESERVAS" {
        return Json(
            json!({"status": 500, "message": {"error": "Failed to fetch interest rate series"}}),
        );
    }
    Json(json!({
        "status": 200,
        "series": series,
        "data": [
            {"series_id": series, "ts": 1_710_000_000_000_i64, "value": "4.25"},
            {"series_id": series, "ts": 1_700_000_000_000_i64, "value": "4.0"}
        ],
        "cached": true
    }))
}

async fn explain(Path(ticker): Path<String>) -> (StatusCode, Json<Value>) {
    if ticker == "NOPE" {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"detail": "sin artefacto para NOPE"})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "symbol": ticker,
            "probability_up": 0.61,
            "contributions": [{"feature": "rsi", "value": 55.0, "contribution": 0.02}]
        })),
    )
}

async fn signals_history(Query(query): Query<HashMap<String, String>>) -> Json<Value> {
    Json(json!({"symbols": query.get("symbols"), "limit": query.get("limit"), "signals": []}))
}

async fn diagnostics() -> Json<Value> {
    Json(json!({"models": {"xgboost": {"auc": 0.55}}}))
}

async fn stub_health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn start_stub() -> (String, StubState) {
    let state = StubState::default();
    let app = Router::new()
        .route("/historical-data/{ticker}", post(historical))
        .route("/ticker/sector/{ticker}", get(sector))
        .route("/available-tickers", post(available))
        .route("/model-ready-tickers", post(ready))
        .route("/lstm", get(trend_model))
        .route("/xgboost", get(xgboost_model))
        .route("/arima", get(arima_model))
        .route("/svm", get(svm_model))
        .route("/garch", get(garch_model))
        .route("/garch-ann", get(garch_ann_model))
        .route("/predict/trend/compare/{ticker}", get(local_models))
        .route("/portfolio/recomendacion", post(portfolio))
        .route("/macro/argdatos/{series}", post(rate_series))
        .route("/interest-rate/ar/{series}", post(rate_series))
        .route("/interest-rate/us/{series}", post(rate_series))
        .route("/predict/trend/xgboost/explain/{ticker}", get(explain))
        .route("/signals/history", get(signals_history))
        .route("/models/diagnostics", get(diagnostics))
        .route("/health", get(stub_health))
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

    sqlx::query(
        "INSERT INTO user_investing_profiles (user_id, risk_profile) VALUES ($1, 'moderate')",
    )
    .bind(user_id)
    .execute(&pool)
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
    sqlx::query("DELETE FROM ticker_compare_cache WHERE ticker = 'COVR'")
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
    std::env::set_var("MODAL_GARCH_ANN_URL", format!("{base}/garch-ann"));

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
    assert_eq!(
        body["predictions"]["garch-ann-modal"]["volatility_forecast"][0]["volatility_pct"],
        1.23
    );
    assert_eq!(body["predictions"]["macro"]["signal"], "baja");
    assert_eq!(body["predictions"]["macro"]["horizon_days"], 20);
    assert_eq!(body["predictions"]["macro"]["available"], true);
    assert_eq!(body["predictions"]["contexto"]["signal"], "alza");
    assert_eq!(body["predictions"]["contexto"]["available"], true);
    // El stub no tiene todos los modelos (xgboost local sin artefacto): queda
    // guardada como incompleta, que igual se reusa por unos minutos.
    let cached_complete: bool =
        sqlx::query_scalar("SELECT complete FROM ticker_compare_cache WHERE ticker = 'COVR'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(!cached_complete);
    let (status, cached) = request(
        app.clone(),
        Method::GET,
        "/user/shares/COVR/trends/compare",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cached["predictions"], body["predictions"]);

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

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/covr/candles?limit=1",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ticker"], "COVR");
    assert_eq!(body["candles"].as_array().unwrap().len(), 1);
    assert_eq!(body["candles"][0]["close"], 125.5);
    assert_eq!(body["candles"][0]["open"], 120.0);
    assert_eq!(body["candles"][0]["volume"], 20);
    assert_eq!(body["candles"][0]["date"], "2024-03-09");
    let (status, _) = request(
        app.clone(),
        Method::GET,
        "/user/shares/BAD/candles",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let (status, body) = request(app.clone(), Method::GET, "/market/series", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    let keys = body["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|series| series["key"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            "CCL",
            "MEP",
            "RIESGO_PAIS",
            "TPM",
            "BADLAR",
            "TNX",
            "GOLD",
            "OIL"
        ]
    );
    assert_eq!(body["series"][0]["points"][0]["ts"], 1_700_000_000_000_i64);
    assert_eq!(body["series"][0]["points"][1]["value"], 4.25);
    assert_eq!(body["series"][6]["points"][1]["value"], 125.5);
    assert_eq!(body["unavailable"][0]["key"], "RESERVAS");
    assert_eq!(
        body["unavailable"][0]["reason"],
        "Failed to fetch interest rate series"
    );
    let (status, _) = request(app.clone(), Method::GET, "/market/series", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/covr/explain",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["symbol"], "COVR");
    assert_eq!(body["contributions"][0]["feature"], "rsi");
    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/shares/NOPE/explain",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "sin artefacto para NOPE");

    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/user/signals/history",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["symbols"], "COVR");
    assert_eq!(body["limit"], "200");

    let (status, body) = request(app.clone(), Method::GET, "/user", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["role"], "user");

    for (method, uri) in [
        (Method::GET, "/admin/health"),
        (Method::GET, "/admin/models"),
        (Method::POST, "/admin/user-shares/backfill"),
    ] {
        let (status, _) = request(app.clone(), method.clone(), uri, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} sin token");
        let (status, body) = request(app.clone(), method, uri, Some(&token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri} sin rol admin");
        assert_eq!(body["error"], "ADMIN_REQUIRED");
    }

    sqlx::query("UPDATE users SET role = 'admin' WHERE email = $1")
        .bind(&email)
        .execute(&state.pool)
        .await
        .unwrap();
    let (status, body) = request(app.clone(), Method::GET, "/user", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["role"], "admin");

    std::env::set_var("MODAL_HEALTH_URL", format!("{base}/lstm"));
    let (status, body) = request(app.clone(), Method::GET, "/admin/health", Some(&token)).await;
    std::env::remove_var("MODAL_HEALTH_URL");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["all_up"], true);
    assert_eq!(body["services"][0]["name"], "data-colector");
    assert_eq!(body["services"][1]["http_status"], 200);
    assert!(body["services"][2]["latency_ms"].is_u64());

    let (status, body) = request(app.clone(), Method::GET, "/admin/models", Some(&token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["models"]["xgboost"]["auc"], 0.55);

    let (status, _) = request(
        app.clone(),
        Method::POST,
        "/admin/user-shares/backfill",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

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
