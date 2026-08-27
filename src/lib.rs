pub mod auth;
pub mod configuration;
pub mod database;
pub mod endpoints;

use axum::{
    middleware,
    routing::{get, patch, post, put},
    Router,
};
use sqlx::PgPool;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;
use utoipa::{
    openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme},
    Modify, OpenApi,
};

use crate::auth::middleware::require_auth;
use crate::configuration::config::AppState;
use crate::endpoints::share::get_logic as share_get_logic;
use crate::endpoints::share::update_logic as share_update_logic;
use crate::endpoints::user::delete_logic as user_delete_logic;
use crate::endpoints::user::get_user_logic::{self, GetUserResponse};
use crate::endpoints::user::login_logic::{self, LoginUserRequest, LoginUserResponse};
use crate::endpoints::user::registration::registration_logic::{
    self, RegisterUserRequest, RegisterUserResponse,
};
use crate::endpoints::user_share::compare_trend_logic::{
    self as user_share_compare_trend_logic, CompareTrendsResponse, ModelPredictionItem,
};
use crate::endpoints::user_share::delete_logic as user_share_delete_logic;
use crate::endpoints::user_share::get_logic::{
    self as user_share_get_logic, ListSharesResponse, ShareItem,
};
use crate::endpoints::user_share::history_logic::{HistoricalPricePoint, ShareHistoryResponse};
use crate::endpoints::user_share::pnl_logic::{
    self as user_share_pnl_logic, PnlResponse, PortfolioPnlSummary, SharePnlItem,
};
use crate::endpoints::user_share::portfolio_logic as user_share_portfolio_logic;
use crate::endpoints::user_share::post_logic::{
    self as user_share_post_logic, CreateShareRequest, CreateShareResponse,
};
use crate::endpoints::user_share::put_logic::{
    self as user_share_put_logic, UpdateShareRequest, UpdateShareResponse,
};
use crate::endpoints::user_share::trend_logic::{
    self as user_share_trend_logic, ListTrendsResponse, ShareTrendItem,
};
use crate::user_share_portfolio_logic::PortfolioRecomendacionResponse;
use crate::{auth::jwt::JwtConfig, endpoints::user::update_risk_profile_logic};

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
        endpoints::user::delete_logic::handler,
        endpoints::user_share::get_logic::handler,
        endpoints::user_share::post_logic::handler,
        endpoints::user_share::delete_logic::handler,
        endpoints::user_share::put_logic::handler,
        endpoints::user_share::trend_logic::handler,
        endpoints::user_share::compare_trend_logic::handler,
        endpoints::user_share::history_logic::handler,
        endpoints::user_share::pnl_logic::handler,
        endpoints::share::get_logic::handler,
        endpoints::share::update_logic::handler,
        endpoints::user_share::portfolio_logic::handler,
    ),
    components(
        schemas(
            RegisterUserRequest,
            RegisterUserResponse,
            LoginUserRequest,
            LoginUserResponse,
            GetUserResponse,
            CreateShareRequest,
            CreateShareResponse,
            ListSharesResponse,
            ShareItem,
            UpdateShareRequest,
            UpdateShareResponse,
            ListTrendsResponse,
            ShareTrendItem,
            CompareTrendsResponse,
            ModelPredictionItem,
            ShareHistoryResponse,
            HistoricalPricePoint,
            PortfolioRecomendacionResponse,
            PnlResponse,
            SharePnlItem,
            PortfolioPnlSummary,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "Authentication", description = "Endpoints for user identity management"),
        (name = "User", description = "Endpoints for retrieving authenticated user information"),
        (name = "Share", description = "Endpoints for managing the authenticated user's declared stock portfolio"),
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
        .route("/register", post(registration_logic::handler))
        .route("/shares", get(share_get_logic::handler))
        .route("/shares/update", get(share_update_logic::handler));

    // Tablero de retro del equipo: no es del dominio del TP, no pasa por JWT
    // (tarjetas anonimas a proposito), protegido por un PIN compartido
    // (`endpoints::retro::pin`) en vez de por sesion de usuario. A proposito
    // no esta en `ApiDoc` (no aparece en /swagger).
    let retro_routes = Router::new()
        .route("/retro", get(endpoints::retro::page::handler))
        .route(
            "/retro/api/board",
            get(endpoints::retro::board_logic::handler),
        )
        .route(
            "/retro/api/cards",
            post(endpoints::retro::cards_logic::create),
        )
        .route(
            "/retro/api/cards/{id}",
            axum::routing::delete(endpoints::retro::cards_logic::delete),
        )
        .route(
            "/retro/api/archive",
            post(endpoints::retro::archive_logic::create),
        )
        .route(
            "/retro/api/undo",
            post(endpoints::retro::archive_logic::undo),
        )
        .route(
            "/retro/api/archives",
            get(endpoints::retro::archive_logic::list),
        )
        .route(
            "/retro/api/archives/{id}",
            get(endpoints::retro::archive_logic::get_one),
        );

    // Rutas protegidas por JWT (middleware)
    let protected = Router::new()
        .route(
            "/user",
            get(get_user_logic::handler).delete(user_delete_logic::handler),
        )
        .route(
            "/user/risk-profile",
            patch(update_risk_profile_logic::handler),
        )
        .route(
            "/user/shares",
            post(user_share_post_logic::handler).get(user_share_get_logic::handler),
        )
        .route(
            "/user/shares/{id}",
            put(user_share_put_logic::handler).delete(user_share_delete_logic::handler),
        )
        .route("/user/shares/trends", get(user_share_trend_logic::handler))
        .route(
            "/user/shares/{ticker}/trends/compare",
            get(user_share_compare_trend_logic::handler),
        )
        .route(
            "/user/shares/{ticker}/history",
            get(endpoints::user_share::history_logic::handler),
        )
        .route(
            "/user/shares/portfolio/recomendacion",
            get(user_share_portfolio_logic::handler),
        )
        .route("/user/shares/pnl", get(user_share_pnl_logic::handler))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    // /login usa session layer (server-side) además del JWT que devuelve en el body
    Router::new()
        .route("/login", post(login_logic::handler))
        .layer(session_layer)
        .merge(normal_routes)
        .merge(protected)
        .merge(retro_routes)
        .with_state(state)
}
