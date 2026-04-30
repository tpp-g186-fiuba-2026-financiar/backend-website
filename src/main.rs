use axum::http::{
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
    HeaderValue, Method,
};
use backend_website::{app, config::Config, db};
use dotenvy::dotenv;
use std::{env, net::SocketAddr};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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
    let router = app().layer(cors);
    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let listener = TcpListener::bind(addr).await.unwrap();

    tracing::info!("listening on {addr}");
    axum::serve(listener, router).await.unwrap();
}
