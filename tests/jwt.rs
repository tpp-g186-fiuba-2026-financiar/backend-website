use backend_website::auth::jwt::JwtConfig;

#[test]
fn round_trip_encodes_and_decodes_claims() {
    let cfg = JwtConfig::new("super-secret-key", 1);
    let token = cfg.encode_token(42, "user@test.com").unwrap();
    let claims = cfg.decode_token(&token).unwrap();

    assert_eq!(claims.sub, 42);
    assert_eq!(claims.email, "user@test.com");
    assert!(claims.exp > claims.iat);
}

#[test]
fn token_signed_with_other_secret_fails_to_decode() {
    let issuer = JwtConfig::new("secret-A", 1);
    let verifier = JwtConfig::new("secret-B", 1);

    let token = issuer.encode_token(1, "user@test.com").unwrap();
    assert!(verifier.decode_token(&token).is_err());
}

#[test]
fn expired_token_is_rejected() {
    let cfg = JwtConfig::new("super-secret-key", -1);
    let token = cfg.encode_token(1, "user@test.com").unwrap();
    assert!(cfg.decode_token(&token).is_err());
}

#[test]
fn garbage_token_is_rejected() {
    let cfg = JwtConfig::new("super-secret-key", 1);
    assert!(cfg.decode_token("not.a.jwt").is_err());
    assert!(cfg.decode_token("").is_err());
}
