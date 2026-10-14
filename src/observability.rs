//! Logs del backend: inicializacion de `tracing`, request id, log de acceso
//! HTTP y captura de panics.
//!
//! Variables de entorno:
//! - `RUST_LOG`: filtro de `tracing` (si no esta, se usa `DEFAULT_FILTER`).
//! - `LOG_FORMAT=json`: una linea JSON por evento (para Render); si no, texto.

use std::{any::Any, io::IsTerminal, time::Duration};

use axum::{
    body::Body,
    extract::{MatchedPath, Request},
    http::{HeaderName, HeaderValue, Response, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestId, MakeRequestUuid, RequestId},
    trace::TraceLayer,
};
use tracing::{field::Empty, Level, Span};
use tracing_subscriber::EnvFilter;

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Nivel por defecto: `info` para el back y `tower_http`, solo `warn` para las
/// librerias que en `debug`/`info` llenan el log de ruido (pool de conexiones,
/// cada query de SQLx, etc).
pub const DEFAULT_FILTER: &str = "info,sqlx=warn,hyper=warn,hyper_util=warn,reqwest=warn,h2=warn,tower_sessions=warn,tower_sessions_core=warn";

const MAX_REQUEST_ID_LEN: usize = 64;

/// Codigo de negocio de una respuesta que va con HTTP 200 pero lleva el
/// codigo real en el body (`/login`, `/register`). El log de acceso lo usa
/// para elegir el nivel y mostrarlo junto al status HTTP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyCode(pub u16);

/// Inicializa el subscriber de `tracing` y el panic hook. Llamar una sola vez,
/// despues de cargar el `.env` para que tome su `RUST_LOG`.
pub fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if is_json_format(std::env::var("LOG_FORMAT").ok().as_deref()) {
        builder
            .json()
            .with_current_span(true)
            .with_span_list(false)
            .init();
    } else {
        // Sin terminal (docker, Render) los colores ANSI quedan como basura en el log
        builder.with_ansi(std::io::stdout().is_terminal()).init();
    }
    std::panic::set_hook(Box::new(|info| {
        let ubicacion = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        tracing::error!(%ubicacion, "Panic: {}", panic_message(info.payload()));
    }));
}

fn is_json_format(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("json"))
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "panic sin mensaje"
    }
}

/// Agrega request id, log de acceso y captura de panics a todas las rutas.
pub fn with_http_logging(router: Router) -> Router {
    router
        .layer(CatchPanicLayer::custom(panic_response))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(make_span)
                .on_request(())
                .on_response(log_response)
                .on_failure(()),
        )
        .layer(middleware::from_fn(request_id))
}

/// Usa el `X-Request-Id` que manda el cliente (si es valido) o genera uno, y
/// lo devuelve en la respuesta para poder cruzar logs del front y del back.
async fn request_id(mut request: Request, next: Next) -> axum::response::Response {
    let id = match request.headers().get(&REQUEST_ID_HEADER) {
        Some(value) if is_valid_request_id(value) => value.clone(),
        _ => MakeRequestUuid
            .make_request_id(&request)
            .map(RequestId::into_header_value)
            .unwrap_or_else(|| HeaderValue::from_static("-")),
    };
    request.headers_mut().insert(REQUEST_ID_HEADER, id.clone());
    let mut response = next.run(request).await;
    response.headers_mut().insert(REQUEST_ID_HEADER, id);
    response
}

fn is_valid_request_id(value: &HeaderValue) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_REQUEST_ID_LEN
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
}

fn make_span(request: &Request) -> Span {
    // La ruta matcheada (`/user/shares/{id}`) agrupa mejor que la URI y no
    // trae la query string
    let ruta = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| request.uri().path().to_owned());
    let request_id = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    tracing::info_span!(
        "http",
        metodo = %request.method(),
        ruta = %ruta,
        request_id = %request_id,
        user_id = Empty,
    )
}

fn log_response<B>(response: &Response<B>, latency: Duration, _span: &Span) {
    let status = response.status().as_u16();
    let body_code = response.extensions().get::<BodyCode>().map(|c| c.0);
    let latencia_ms = latency.as_millis() as u64;
    match response_level(status, body_code) {
        Level::ERROR => {
            tracing::error!(status, code = body_code, latencia_ms, "Respuesta HTTP")
        }
        Level::WARN => tracing::warn!(status, code = body_code, latencia_ms, "Respuesta HTTP"),
        _ => tracing::info!(status, code = body_code, latencia_ms, "Respuesta HTTP"),
    }
}

fn response_level(status: u16, body_code: Option<u16>) -> Level {
    match body_code.unwrap_or(status).max(status) {
        500.. => Level::ERROR,
        400..=499 => Level::WARN,
        _ => Level::INFO,
    }
}

fn panic_response(_err: Box<dyn Any + Send + 'static>) -> Response<Body> {
    // El panic ya lo loguea el hook de `init_tracing` (con mensaje y ubicacion)
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "code": 500,
            "message": "An unexpected error occurred. Please try again later."
        })),
    )
        .into_response()
}

/// Respuesta JSON con HTTP 200 que marca el `code` del body para el log de
/// acceso (ver `BodyCode`).
pub fn json_with_body_code(body: Value) -> axum::response::Response {
    let code = body
        .get("code")
        .and_then(Value::as_u64)
        .and_then(|code| u16::try_from(code).ok());
    let mut response = Json(body).into_response();
    if let Some(code) = code {
        response.extensions_mut().insert(BodyCode(code));
    }
    response
}

/// Suma el usuario autenticado al span de la request: todos los logs de esa
/// request lo llevan sin tener que pasarlo a mano.
pub fn record_user_id(user_id: i32) {
    Span::current().record("user_id", user_id);
}

/// `fede@gmail.com` -> `f***@gmail.com`: alcanza para reconocer el caso sin
/// dejar el mail completo en el log.
pub fn mask_email(email: &str) -> String {
    match email.trim().split_once('@') {
        Some((user, domain)) if !user.is_empty() => {
            let first: String = user.chars().take(1).collect();
            format!("{first}***@{domain}")
        }
        _ => "***".to_string(),
    }
}

/// Un link con token (verificacion, reseteo) solo se puede loguear si apunta a
/// un front local: en un deploy, quien lea el log podria usarlo.
pub fn is_local_link(link: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"]
        .iter()
        .any(|prefix| link.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_router() -> Router {
        with_http_logging(
            Router::new()
                .route("/ok", get(|| async { "ok" }))
                .route(
                    "/login-fallido",
                    get(|| async { json_with_body_code(json!({"code": 401})) }),
                )
                .route(
                    "/panic",
                    get(|| async {
                        if true {
                            panic!("explota");
                        }
                        "nunca"
                    }),
                ),
        )
    }

    async fn call(uri: &str, request_id: Option<&str>) -> axum::response::Response {
        let mut builder = Request::builder().uri(uri);
        if let Some(id) = request_id {
            builder = builder.header(REQUEST_ID_HEADER, id);
        }
        test_router()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn genera_request_id_si_no_viene() {
        let response = call("/ok", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers().get(&REQUEST_ID_HEADER).unwrap();
        assert!(is_valid_request_id(id));
        assert_eq!(id.len(), 36);
    }

    #[tokio::test]
    async fn respeta_request_id_valido_del_cliente() {
        let response = call("/ok", Some("front-abc_123")).await;
        assert_eq!(
            response.headers().get(&REQUEST_ID_HEADER).unwrap(),
            "front-abc_123"
        );
    }

    #[tokio::test]
    async fn reemplaza_request_id_invalido() {
        let largo = "a".repeat(MAX_REQUEST_ID_LEN + 1);
        for invalido in ["con espacio", "<script>", largo.as_str()] {
            let response = call("/ok", Some(invalido)).await;
            let id = response.headers().get(&REQUEST_ID_HEADER).unwrap();
            assert_ne!(id, invalido);
            assert!(is_valid_request_id(id));
        }
    }

    #[tokio::test]
    async fn panic_en_handler_devuelve_500_json() {
        let response = call("/panic", None).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.headers().contains_key(&REQUEST_ID_HEADER));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], 500);
    }

    #[tokio::test]
    async fn json_with_body_code_marca_el_codigo() {
        let response = call("/login-fallido", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.extensions().get::<BodyCode>(),
            Some(&BodyCode(401))
        );

        let sin_code = json_with_body_code(json!({"message": "x"}));
        assert!(sin_code.extensions().get::<BodyCode>().is_none());
    }

    #[test]
    fn nivel_segun_status_y_codigo_del_body() {
        assert_eq!(response_level(200, None), Level::INFO);
        assert_eq!(response_level(302, None), Level::INFO);
        assert_eq!(response_level(404, None), Level::WARN);
        assert_eq!(response_level(503, None), Level::ERROR);
        assert_eq!(response_level(200, Some(200)), Level::INFO);
        assert_eq!(response_level(200, Some(401)), Level::WARN);
        assert_eq!(response_level(200, Some(500)), Level::ERROR);
        assert_eq!(response_level(500, Some(200)), Level::ERROR);
    }

    #[test]
    fn formato_json_solo_si_se_pide() {
        assert!(is_json_format(Some("json")));
        assert!(is_json_format(Some(" JSON ")));
        assert!(!is_json_format(Some("text")));
        assert!(!is_json_format(None));
    }

    #[test]
    fn mensaje_de_panic() {
        let estatico: Box<dyn Any + Send> = Box::new("estatico");
        let dinamico: Box<dyn Any + Send> = Box::new(String::from("dinamico"));
        let otro: Box<dyn Any + Send> = Box::new(42);
        assert_eq!(panic_message(estatico.as_ref()), "estatico");
        assert_eq!(panic_message(dinamico.as_ref()), "dinamico");
        assert_eq!(panic_message(otro.as_ref()), "panic sin mensaje");
    }

    #[test]
    fn enmascara_emails() {
        assert_eq!(mask_email("fede@gmail.com"), "f***@gmail.com");
        assert_eq!(mask_email(" ana@x.com "), "a***@x.com");
        assert_eq!(mask_email("@x.com"), "***");
        assert_eq!(mask_email("sin-arroba"), "***");
    }

    #[test]
    fn solo_links_locales() {
        assert!(is_local_link("http://localhost:5173/verify-email?token=x"));
        assert!(is_local_link(
            "http://127.0.0.1:5173/reset-password?token=x"
        ));
        assert!(!is_local_link(
            "https://frontend-mfnd.onrender.com/verify-email?token=x"
        ));
    }

    #[test]
    fn record_user_id_sin_span_no_falla() {
        record_user_id(7);
    }
}
