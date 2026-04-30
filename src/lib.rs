pub mod config;
pub mod db;
pub mod health;
pub mod hello;
use axum::{routing::get, Router};

pub fn app() -> Router {
    Router::new()
        .route("/health", get(health::handler))
        .route("/hello", get(hello::handler))
}
