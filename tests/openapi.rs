use backend_website::ApiDoc;
use utoipa::OpenApi;

#[test]
fn openapi_document_lists_the_public_endpoints() {
    let doc = serde_json::to_value(ApiDoc::openapi()).expect("openapi serializes");
    let paths = doc["paths"].as_object().expect("paths");

    for path in [
        "/login",
        "/register",
        "/verify-email",
        "/verify-email/resend",
        "/user",
        "/user/2fa/setup",
        "/user/2fa/enable",
        "/user/2fa/disable",
        "/user/shares",
        "/user/shares/{ticker}/candles",
        "/user/shares/{ticker}/explain",
        "/user/signals/history",
        "/market/series",
        "/admin/health",
        "/admin/models",
        "/admin/models/{model}/hidden",
        "/models/hidden",
        "/admin/user-shares/backfill",
    ] {
        assert!(paths.contains_key(path), "missing {path} in OpenAPI");
    }
    assert!(doc["components"]["securitySchemes"]["bearer_auth"].is_object());
    assert!(doc["components"]["schemas"]["TwoFactorCodeRequest"].is_object());
    assert!(doc["components"]["schemas"]["VerifyEmailRequest"].is_object());
}
