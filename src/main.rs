use axum::http::{
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    HeaderValue, Method,
};
use backend_website::{app, configuration::config::Config, database::db, ApiDoc};
use dotenvy::dotenv;
use std::{env, net::SocketAddr};
use time::Duration;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_sessions::{ExpiredDeletion, Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::PostgresStore;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Trick to auto-generate the swagger json without starting the server
    if std::env::var("EXPORT_OPENAPI").is_ok() {
        let openapi_json = ApiDoc::openapi()
            .to_pretty_json()
            .expect("Failed to serialize OpenAPI doc");
        tokio::fs::write("swagger-endpoints.json", openapi_json)
            .await
            .expect("Failed to write swagger-endpoints.json");
        tracing::info!("OpenAPI documentation exported to swagger-endpoints.json");
        return; // Exit without starting the Axum server
    }

    dotenv().ok();

    let cfg = Config::from_env();
    let pool = db::create_pool(&cfg.database_url).await;

    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run database migrations");

    // Adding CORS
    let allowed_origin = env::var("ALLOWED_ORIGIN").expect("ALLOWED_ORIGIN must be set");
    println!("Allowed origin: {}", allowed_origin);
    let cors = CorsLayer::new()
        .allow_origin(allowed_origin.parse::<HeaderValue>().unwrap())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([ACCEPT, AUTHORIZATION, CONTENT_TYPE]);

    let session_store = PostgresStore::new(pool.clone());
    session_store
        .migrate()
        .await
        .expect("Failed to run migrations");

    let session_expires_time = Expiry::OnInactivity(Duration::minutes(2));

    let session_layer = SessionManagerLayer::new(session_store.clone())
        .with_expiry(session_expires_time)
        .with_secure(!cfg.database_url.contains("localhost")); // Cleaner toggle

    let swagger = SwaggerUi::new("/swagger").url("/swagger-endpoints.json", ApiDoc::openapi());

    let router = app(pool, session_layer).layer(cors).merge(swagger);

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let listener = TcpListener::bind(addr)
        .await
        .expect("[Backend Website] Failed to bind to address");

    tracing::info!("listening on {addr}");

    tokio::spawn(async move {
        // Lucas disclaimer: This delete is only visual, the actual deletion is handled by the tower_sessions library.
        // This is just to make sure are deleted from the table and we keep a clean database.
        loop {
            tokio::time::sleep(std::time::Duration::from_hours(1)).await;
            tracing::info!("[Session Cleanup] Deleting expired sessions...");
            session_store.delete_expired().await.unwrap_or_else(|err| {
                tracing::error!("Failed to delete expired sessions: {}", err);
            });
        }
    });
    axum::serve(listener, router).await.unwrap();

}
