pub mod configuration;
pub mod database;
pub mod endpoints;

use axum::{
    routing::{get, post},
    Router,
};
use sqlx::PgPool;
use utoipa::OpenApi;

use crate::endpoints::user::registration::registration_logic::{self, RegisterUserRequest};

#[derive(OpenApi)]
#[openapi(
    paths(
        endpoints::health::handler,
        endpoints::hello::handler,
        endpoints::user::registration::registration_logic::handler
    ),
    components(
        schemas(RegisterUserRequest)
    ),
    tags(
        (name = "Authentication", description = "Endpoints for user identity management"),
        (name = "General", description = "General endpoints for knowing the status of the backend and other general information")
    ),
)]
pub struct ApiDoc;

pub fn app(pool_state: PgPool) -> Router {
    Router::new()
        .route("/health", get(endpoints::health::handler))
        .route("/hello", get(endpoints::hello::handler))
        .route("/register", post(registration_logic::handler))
        .with_state(pool_state)
}
