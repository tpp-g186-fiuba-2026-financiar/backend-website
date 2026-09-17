use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use backend_website::{app_with_state, auth::jwt::JwtConfig, configuration::config::AppState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

async fn setup() -> AppState {
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("Failed to connect to the database");
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run database migrations");
    sqlx::query("DELETE FROM retro_archives")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM retro_cards")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE retro_sprint SET current_sprint = 15 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    AppState {
        pool,
        jwt_config: JwtConfig::new("retro-test-secret", 24),
    }
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
    pin: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(pin) = pin {
        builder = builder.header("x-retro-pin", pin);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let response = app.oneshot(builder.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn retro_board_complete_lifecycle_and_validation() {
    let state = setup().await;
    let app = build_app(state.clone()).await;

    std::env::remove_var("RETRO_PIN");
    let (status, body) = request(app.clone(), Method::GET, "/retro/api/board", None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], 503);

    std::env::set_var("RETRO_PIN", "2468");
    let (status, body) = request(
        app.clone(),
        Method::GET,
        "/retro/api/board",
        Some("wrong"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 401);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/retro")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = response.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&html).contains("Retro del equipo"));

    let (status, board) = request(
        app.clone(),
        Method::GET,
        "/retro/api/board",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(board["sprint"], 15);

    let (status, _) = request(
        app.clone(),
        Method::POST,
        "/retro/api/cards",
        Some("2468"),
        Some(json!({"column": "otra", "content": "texto"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request(
        app.clone(),
        Method::POST,
        "/retro/api/cards",
        Some("2468"),
        Some(json!({"column": "bien", "content": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut ids = Vec::new();
    for (column, content) in [
        ("bien", "Buen trabajo"),
        ("mejorar", "Mejorar tiempos"),
        ("acciones", "Automatizar pruebas"),
        ("preguntas", "¿Qué aprendimos?"),
    ] {
        let (status, body) = request(
            app.clone(),
            Method::POST,
            "/retro/api/cards",
            Some("2468"),
            Some(json!({"column": column, "content": format!("  {content}  ")})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        ids.push(body["id"].as_i64().unwrap());
    }

    let (status, board) = request(
        app.clone(),
        Method::GET,
        "/retro/api/board",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(board["bien"][0]["content"], "Buen trabajo");
    assert_eq!(board["mejorar"].as_array().unwrap().len(), 1);
    assert_eq!(board["acciones"].as_array().unwrap().len(), 1);
    assert_eq!(board["preguntas"].as_array().unwrap().len(), 1);

    let (status, _) = request(
        app.clone(),
        Method::DELETE,
        &format!("/retro/api/cards/{}", ids[0]),
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = request(
        app.clone(),
        Method::DELETE,
        "/retro/api/cards/2147483647",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], 404);

    let (status, archived) = request(
        app.clone(),
        Method::POST,
        "/retro/api/archive",
        Some("2468"),
        Some(json!({"label": "Retro de entrega"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(archived["sprint"], 15);
    assert_eq!(archived["next_sprint"], 16);
    let archive_id = archived["id"].as_i64().unwrap();

    let (status, body) = request(
        app.clone(),
        Method::POST,
        "/retro/api/archive",
        Some("2468"),
        Some(json!({"label": null})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 400);

    let (status, list) = request(
        app.clone(),
        Method::GET,
        "/retro/api/archives",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["archives"][0]["label"], "Retro de entrega");
    let (status, one) = request(
        app.clone(),
        Method::GET,
        &format!("/retro/api/archives/{archive_id}"),
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        one["snapshot"]["acciones"][0]["content"],
        "Automatizar pruebas"
    );
    let (status, _) = request(
        app.clone(),
        Method::GET,
        "/retro/api/archives/2147483647",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, undo) = request(
        app.clone(),
        Method::POST,
        "/retro/api/undo",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(undo["sprint"], 15);

    let (status, archived) = request(
        app.clone(),
        Method::POST,
        "/retro/api/archive",
        Some("2468"),
        Some(json!({"label": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(archived["label"], "Sprint 15");
    let (status, _) = request(
        app.clone(),
        Method::POST,
        "/retro/api/undo",
        Some("2468"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("DELETE FROM retro_cards")
        .execute(&state.pool)
        .await
        .unwrap();
    let (status, body) = request(app, Method::POST, "/retro/api/undo", Some("2468"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 400);
}
