use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::{PgExecutor, PgPool};

use crate::mail::{self, MailConfig};

/// Horas de validez del link de verificacion.
pub const TOKEN_TTL_HOURS: i64 = 24;

/// Segundos minimos entre dos envios del mail de verificacion al mismo usuario.
pub const RESEND_COOLDOWN_SECONDS: i64 = 60;

const DEFAULT_FRONTEND_URL: &str = "http://localhost:5173";

/// Genera un token aleatorio de 32 bytes en hexadecimal. Es lo que viaja en el
/// link; en la base solo se guarda su hash (ver `hash_token`).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    to_hex(&bytes)
}

pub fn hash_token(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Link del front que confirma la cuenta. `FRONTEND_URL` apunta al front
/// (en Render, https://frontend-mfnd.onrender.com).
pub fn verification_link(token: &str) -> String {
    let frontend_url =
        std::env::var("FRONTEND_URL").unwrap_or_else(|_| DEFAULT_FRONTEND_URL.to_string());
    format!(
        "{}/verificar-email?token={token}",
        frontend_url.trim_end_matches('/')
    )
}

/// Guarda un token nuevo para el usuario (reemplaza al anterior) y lo devuelve.
pub async fn store_new_token<'e>(
    executor: impl PgExecutor<'e>,
    user_id: i32,
) -> Result<String, sqlx::Error> {
    let token = generate_token();
    sqlx::query(
        r#"
        UPDATE users
        SET email_verification_token_hash = $1,
            email_verification_expires_at = NOW() + make_interval(hours => $2),
            email_verification_sent_at = NOW()
        WHERE id = $3
        "#,
    )
    .bind(hash_token(&token))
    .bind(TOKEN_TTL_HOURS as i32)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(token)
}

/// Como `store_new_token`, pero solo si el usuario sigue sin verificar y no se
/// le mando otro mail en los ultimos `RESEND_COOLDOWN_SECONDS`. Devuelve `None`
/// si no corresponde reenviar.
pub async fn store_new_token_for_resend(
    pool: &PgPool,
    user_id: i32,
) -> Result<Option<String>, sqlx::Error> {
    let token = generate_token();
    let updated = sqlx::query(
        r#"
        UPDATE users
        SET email_verification_token_hash = $1,
            email_verification_expires_at = NOW() + make_interval(hours => $2),
            email_verification_sent_at = NOW()
        WHERE id = $3
          AND email_verified = FALSE
          AND (email_verification_sent_at IS NULL
               OR email_verification_sent_at < NOW() - make_interval(secs => $4))
        "#,
    )
    .bind(hash_token(&token))
    .bind(TOKEN_TTL_HOURS as i32)
    .bind(user_id)
    .bind(RESEND_COOLDOWN_SECONDS as f64)
    .execute(pool)
    .await?;

    Ok((updated.rows_affected() > 0).then_some(token))
}

/// Manda el mail de verificacion en segundo plano para no demorar la respuesta.
/// Sin SMTP configurado (entorno local) no se manda nada y el link queda en el
/// log, asi se puede verificar la cuenta igual.
pub fn dispatch_verification_email(email: String, full_name: String, token: String) {
    let link = verification_link(&token);
    let Some(config) = MailConfig::from_env() else {
        tracing::warn!(
            "[EmailVerification] SMTP_HOST no esta configurado: no se envia el mail a {email}. Link de verificacion: {link}"
        );
        return;
    };

    tokio::spawn(async move {
        if let Err(err) = mail::send_email_verification(&config, &email, &full_name, &link).await {
            tracing::error!(
                "[EmailVerification] No se pudo enviar el mail de verificacion a {email}: {err}"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_random_hex() {
        let first = generate_token();
        let second = generate_token();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn hash_is_deterministic_and_differs_from_token() {
        let token = generate_token();
        assert_eq!(hash_token(&token), hash_token(&token));
        assert_ne!(hash_token(&token), token);
        assert_eq!(hash_token(&token).len(), 64);
    }

    #[tokio::test]
    async fn verification_link_uses_frontend_url() {
        let _guard = crate::ENV_TEST_LOCK.lock().await;

        std::env::remove_var("FRONTEND_URL");
        assert_eq!(
            verification_link("abc"),
            "http://localhost:5173/verificar-email?token=abc"
        );

        std::env::set_var("FRONTEND_URL", "https://frontend.example.test/");
        assert_eq!(
            verification_link("abc"),
            "https://frontend.example.test/verificar-email?token=abc"
        );

        std::env::remove_var("FRONTEND_URL");
    }
}
