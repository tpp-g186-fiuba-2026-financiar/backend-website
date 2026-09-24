// Cubre el ledger de operaciones (seed_existing_holdings) y el endpoint de
// balance historico contra un data-colector falso local, asi no dependen de
// la red ni del servicio real. Cada archivo de tests/ es su propio proceso,
// por lo que setear DATA_COLLECTOR_URL aca no afecta a otros tests.
use axum::{
    body::Body,
    extract::Path,
    http::{header, Request, StatusCode},
    routing::post,
    Json, Router,
};
use backend_website::endpoints::user_share::user_share_operations::seed_existing_holdings;
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use dotenvy::dotenv;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

const DAY_MS: i64 = 86_400_000;
const PRICE: f64 = 120.0;

static STUB: std::sync::Once = std::sync::Once::new();

// Tickers que empiezan con BAD responden 400; el resto devuelve un precio
// fijo (PRICE) para cada uno de los ultimos 40 dias, a las 00:00 UTC.
async fn history_stub(Path(ticker): Path<String>) -> axum::response::Response {
    use axum::response::IntoResponse;
    if ticker.starts_with("BAD") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let today = chrono::Utc::now().timestamp_millis().div_euclid(DAY_MS);
    let data: Vec<Value> = (0..40)
        .map(|offset| {
            json!({
                "close_amount": PRICE.to_string(),
                "ts": (today - offset) * DAY_MS,
            })
        })
        .collect();
    Json(json!({ "data": data })).into_response()
}

// Cada #[tokio::test] tiene su propio runtime y lo destruye al terminar, asi
// que el stub no puede colgar del runtime del primer test: vive en un hilo
// propio, con su runtime, durante todo el proceso.
fn start_stub() {
    STUB.call_once(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let app = Router::new().route("/historical-data/{ticker}", post(history_stub));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let address = rx.recv().unwrap();
        std::env::set_var("DATA_COLLECTOR_URL", format!("http://{address}"));
    });
}

async fn setup() -> AppState {
    dotenv().ok();
    start_stub();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database");
    AppState {
        pool,
        jwt_config: JwtConfig::new("test-secret-ledger", 24),
    }
}

async fn build_app(state: AppState) -> Router {
    let session_store = PostgresStore::new(state.pool.clone());
    session_store.migrate().await.expect("session migrate");
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);
    app_with_state(state, session_layer)
}

fn unique_email(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("ledger_{tag}_{nanos}@test.com")
}

async fn post_json(app: &Router, uri: &str, body: Value) -> Value {
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
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Registra al usuario y devuelve (id, token).
async fn create_user(state: &AppState, email: &str) -> (i32, String) {
    let app = build_app(state.clone()).await;
    post_json(
        &app,
        "/register",
        json!({
            "email": email,
            "password": "StrongPassword123!",
            "full_name": "Ledger Tester",
            "risk_profile": "moderate",
        }),
    )
    .await;
    let login = post_json(
        &app,
        "/login",
        json!({ "email": email, "password": "StrongPassword123!" }),
    )
    .await;
    let token = login["token"].as_str().expect("token").to_string();
    let (id,): (i32,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    (id, token)
}

async fn seed_share(pool: &sqlx::PgPool, ticker: &str) -> i32 {
    sqlx::query("INSERT INTO shares (ticker) VALUES ($1) ON CONFLICT (ticker) DO NOTHING")
        .bind(ticker)
        .execute(pool)
        .await
        .unwrap();
    let (id,): (i32,) = sqlx::query_as("SELECT id FROM shares WHERE ticker = $1")
        .bind(ticker)
        .fetch_one(pool)
        .await
        .unwrap();
    id
}

async fn insert_op(
    pool: &sqlx::PgPool,
    user_id: i32,
    share_id: i32,
    kind: &str,
    quantity: i32,
    price: f64,
    days_ago: i64,
) {
    sqlx::query(
        "INSERT INTO user_share_operations (user_id, share_id, operation_type, quantity, price, created_at) \
         VALUES ($1, $2, $3, $4, $5, NOW() - ($6 || ' days')::interval)",
    )
    .bind(user_id)
    .bind(share_id)
    .bind(kind)
    .bind(quantity)
    .bind(price)
    .bind(days_ago.to_string())
    .execute(pool)
    .await
    .unwrap();
}

async fn get_history(app: &Router, token: Option<&str>) -> (StatusCode, Value) {
    let mut req = Request::builder().uri("/user/shares/balance/history");
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn cleanup_user(pool: &sqlx::PgPool, email: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn history_requires_authentication() {
    let state = setup().await;
    let app = build_app(state).await;
    let (status, _) = get_history(&app, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn history_without_operations_is_empty() {
    let state = setup().await;
    let email = unique_email("empty");
    let (_, token) = create_user(&state, &email).await;
    let app = build_app(state.clone()).await;

    let (status, body) = get_history(&app, Some(&token)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 0);
    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn history_replays_the_ledger_into_a_daily_timeline() {
    let state = setup().await;
    let email = unique_email("timeline");
    let (user_id, token) = create_user(&state, &email).await;
    let share_id = seed_share(&state.pool, "GGAL").await;
    // Compra 10 @ 100 hace 7 dias, vende 4 hace 3 dias => quedan 6 @ 100.
    insert_op(&state.pool, user_id, share_id, "buy", 10, 100.0, 7).await;
    insert_op(&state.pool, user_id, share_id, "sell", 4, 110.0, 3).await;
    let app = build_app(state.clone()).await;

    let (status, body) = get_history(&app, Some(&token)).await;

    assert_eq!(status, StatusCode::OK);
    let points = body.as_array().unwrap();
    assert!(points.len() >= 8, "one point per day since the first buy");
    let first = &points[0];
    assert_eq!(first["total_current_value"], 10.0 * PRICE);
    assert_eq!(first["total_cost_basis"], 1000.0);
    let last = points.last().unwrap();
    assert_eq!(last["total_current_value"], 6.0 * PRICE);
    assert_eq!(last["total_cost_basis"], 600.0);
    assert_eq!(last["total_profit_loss"], 6.0 * PRICE - 600.0);
    cleanup_user(&state.pool, &email).await;
}

#[tokio::test]
async fn history_returns_502_when_price_history_is_unavailable() {
    let state = setup().await;
    let email = unique_email("badgateway");
    let (user_id, token) = create_user(&state, &email).await;
    let share_id = seed_share(&state.pool, "BADHIST").await;
    insert_op(&state.pool, user_id, share_id, "buy", 1, 100.0, 1).await;
    let app = build_app(state.clone()).await;

    let (status, body) = get_history(&app, Some(&token)).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["code"], 502);
    cleanup_user(&state.pool, &email).await;
    let _ = sqlx::query("DELETE FROM shares WHERE ticker = 'BADHIST'")
        .execute(&state.pool)
        .await;
}

#[tokio::test]
async fn seed_existing_holdings_backfills_missing_ledger_entries() {
    let state = setup().await;
    let email = unique_email("seed");
    let (user_id, _) = create_user(&state, &email).await;
    let with_price = seed_share(&state.pool, "GGAL").await;
    let without_price = seed_share(&state.pool, "YPFD").await;
    let unresolvable = seed_share(&state.pool, "BADSEED").await;
    for (share_id, quantity, entry_price) in [
        (with_price, 10, Some(95.0)),
        (without_price, 5, None),
        (unresolvable, 3, None),
    ] {
        sqlx::query(
            "INSERT INTO user_shares (user_id, share_id, quantity, entry_price) VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(share_id)
        .bind(quantity)
        .bind(entry_price)
        .execute(&state.pool)
        .await
        .unwrap();
    }

    let seeded = seed_existing_holdings(&state.pool).await.unwrap();
    assert!(seeded >= 2);

    let ops: Vec<(i32, String, f64)> = sqlx::query_as(
        "SELECT share_id, operation_type, price FROM user_share_operations WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(ops.len(), 2, "the unresolvable ticker is skipped");
    assert!(ops.iter().all(|(_, kind, _)| kind == "buy"));
    let price_of = |share_id: i32| ops.iter().find(|(id, _, _)| *id == share_id).unwrap().2;
    assert_eq!(price_of(with_price), 95.0, "uses the stored entry_price");
    assert_eq!(price_of(without_price), PRICE, "resolves it from history");

    // Segunda corrida: no duplica lo ya sembrado.
    seed_existing_holdings(&state.pool).await.unwrap();
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM user_share_operations WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(count, 2);

    cleanup_user(&state.pool, &email).await;
    let _ = sqlx::query("DELETE FROM shares WHERE ticker = 'BADSEED'")
        .execute(&state.pool)
        .await;
}
