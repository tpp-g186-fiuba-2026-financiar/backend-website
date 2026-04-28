pub mod config;
pub mod db;
pub mod health;

use axum::{routing::get, Router};

pub fn app() -> Router {
    Router::new().route("/health", get(health::handler))
}
