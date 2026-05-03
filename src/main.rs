use axum::http::{
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    HeaderValue, Method,
};
use backend_website::{app, configuration::config::Config, database::db, ApiDoc};
use dotenvy::dotenv;
use std::{env, net::SocketAddr};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
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

    db::create_tables(&pool).await;

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

    // Creating server
    let swagger = SwaggerUi::new("/swagger").url("/swagger-endpoints.json", ApiDoc::openapi());

    let router = app(pool).layer(cors).merge(swagger);

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let listener = TcpListener::bind(addr).await.unwrap();

    tracing::info!("listening on {addr}");
    axum::serve(listener, router).await.unwrap();
}
