pub mod auth;
pub mod configuration;
pub mod database;
pub mod endpoints;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use sqlx::PgPool;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use crate::auth::jwt::JwtConfig;
use crate::auth::middleware::require_auth;
use crate::configuration::config::AppState;
use crate::endpoints::user::get_user_logic::{self, GetUserResponse};
use crate::endpoints::user::login_logic::{self, LoginUserRequest, LoginUserResponse};
use crate::endpoints::user::registration::registration_logic::{
    self, RegisterUserRequest, RegisterUserResponse,
};

pub struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.as_mut().expect("components missing");
        components.add_security_scheme(
            "bearer_auth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        endpoints::health::handler,
        endpoints::hello::handler,
        endpoints::user::registration::registration_logic::handler,
        endpoints::user::login_logic::handler,
        endpoints::user::get_user_logic::handler,
    ),
    components(
        schemas(
            RegisterUserRequest,
            RegisterUserResponse,
            LoginUserRequest,
            LoginUserResponse,
            GetUserResponse,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "Authentication", description = "Endpoints for user identity management"),
        (name = "User", description = "Endpoints for retrieving authenticated user information"),
        (name = "General", description = "General endpoints for knowing the status of the backend and other general information")
    ),
)]
pub struct ApiDoc;

pub fn app(pool: PgPool, session_layer: SessionManagerLayer<PostgresStore>) -> Router {
    let jwt_config = JwtConfig::from_env();
    app_with_state(AppState { pool, jwt_config }, session_layer)
}

pub fn app_with_state(
    state: AppState,
    session_layer: SessionManagerLayer<PostgresStore>,
) -> Router {
    // Rutas abiertas (sin sesión ni JWT)
    let normal_routes = Router::new()
        .route("/hello", get(endpoints::hello::handler))
        .route("/health", get(endpoints::health::handler))
        .route("/register", post(registration_logic::handler));

    // Rutas protegidas por JWT (middleware)
    let protected = Router::new()
        .route("/user", get(get_user_logic::handler))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    // /login usa session layer (server-side) además del JWT que devuelve en el body
    Router::new()
        .route("/login", post(login_logic::handler))
        .layer(session_layer)
        .merge(normal_routes)
        .merge(protected)
        .with_state(state)
}