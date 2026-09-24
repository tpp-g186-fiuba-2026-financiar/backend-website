use backend_website::ApiDoc;
use utoipa::OpenApi;

#[test]
fn openapi_document_lists_the_public_endpoints() {
    let doc = serde_json::to_value(ApiDoc::openapi()).expect("openapi serializes");
    let paths = doc["paths"].as_object().expect("paths");

    for path in [
        "/login",
        "/register",
        "/user",
        "/user/2fa/setup",
        "/user/2fa/enable",
        "/user/2fa/disable",
        "/user/shares",
    ] {
        assert!(paths.contains_key(path), "missing {path} in OpenAPI");
    }
    assert!(doc["components"]["securitySchemes"]["bearer_auth"].is_object());
    assert!(doc["components"]["schemas"]["TwoFactorCodeRequest"].is_object());
}
