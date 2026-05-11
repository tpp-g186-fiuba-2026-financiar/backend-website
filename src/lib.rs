pub mod configuration;
pub mod database;
pub mod endpoints;

use axum::{
    routing::{get, post},
    Router,
};
use sqlx::PgPool;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;
use utoipa::OpenApi;

use crate::endpoints::user::login_logic::{self, LoginUserRequest};
use crate::endpoints::user::registration::registration_logic::{self, RegisterUserRequest};

#[derive(OpenApi)]
#[openapi(
    paths(
        endpoints::health::handler,
        endpoints::hello::handler,
        endpoints::user::registration::registration_logic::handler,
        endpoints::user::login_logic::handler
    ),
    components(
        schemas(RegisterUserRequest, LoginUserRequest)
    ),
    tags(
        (name = "Authentication", description = "Endpoints for user identity management"),
        (name = "General", description = "General endpoints for knowing the status of the backend and other general information")
    ),
)]
pub struct ApiDoc;

pub fn app(pool_state: PgPool, session_layer: SessionManagerLayer<PostgresStore>) -> Router {
    // 1. Open routes, requires neither session or sql pool.
    let normal_routes = Router::new()
        .route("/hello", get(endpoints::hello::handler))
        .route("/health", get(endpoints::health::handler))
        .route("/register", post(registration_logic::handler));

    Router::new()
        .route("/login", post(login_logic::handler))
        .layer(session_layer)
        .merge(normal_routes)
        .with_state(pool_state)
}

