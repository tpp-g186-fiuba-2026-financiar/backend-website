use totp_rs::{Algorithm, Secret, TOTP};

const ISSUER: &str = "FinanciAr";

/// Genera un secreto aleatorio (base32) para enrolar una app autenticadora.
pub fn generate_secret() -> String {
    Secret::generate_secret().to_encoded().to_string()
}

fn build(secret_base32: &str, email: &str) -> Result<TOTP, String> {
    let bytes = Secret::Encoded(secret_base32.to_string())
        .to_bytes()
        .map_err(|err| format!("Secreto TOTP invalido: {err:?}"))?;
    // El label de otpauth no admite ':' (separador issuer:cuenta).
    let account = email.replace(':', "");
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        bytes,
        Some(ISSUER.to_string()),
        account,
    )
    .map_err(|err| format!("No se pudo construir el TOTP: {err}"))
}

/// URL `otpauth://` que las apps (Google Authenticator, Authy, etc.) importan.
pub fn otpauth_url(secret_base32: &str, email: &str) -> Result<String, String> {
    Ok(build(secret_base32, email)?.get_url())
}

/// QR (PNG en base64) equivalente a `otpauth_url`.
pub fn qr_base64(secret_base32: &str, email: &str) -> Result<String, String> {
    build(secret_base32, email)?.get_qr_base64()
}

/// Verifica un codigo de 6 digitos, con tolerancia de +-1 ventana de 30s.
pub fn verify_code(secret_base32: &str, email: &str, code: &str) -> bool {
    let code = code.trim();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match build(secret_base32, email) {
        Ok(totp) => totp.check_current(code).unwrap_or(false),
        Err(err) => {
            tracing::error!("{}", err);
            false
        }
    }
}

/// Codigo vigente para un secreto; lo usan los tests para simular la app.
pub fn current_code(secret_base32: &str, email: &str) -> Result<String, String> {
    build(secret_base32, email)?
        .generate_current()
        .map_err(|err| format!("No se pudo generar el codigo: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_code_verifies() {
        let secret = generate_secret();
        let code = current_code(&secret, "a@b.com").unwrap();
        assert!(verify_code(&secret, "a@b.com", &code));
    }

    #[test]
    fn malformed_codes_are_rejected() {
        let secret = generate_secret();
        assert!(!verify_code(&secret, "a@b.com", ""));
        assert!(!verify_code(&secret, "a@b.com", "12345"));
        assert!(!verify_code(&secret, "a@b.com", "abcdef"));
        assert!(!verify_code(&secret, "a@b.com", "1234567"));
    }

    #[test]
    fn otpauth_url_carries_issuer_and_account() {
        let secret = generate_secret();
        let url = otpauth_url(&secret, "a@b.com").unwrap();
        assert!(url.starts_with("otpauth://totp/"));
        assert!(url.contains("FinanciAr"));
        assert!(url.contains("a%40b.com") || url.contains("a@b.com"));
    }
}
