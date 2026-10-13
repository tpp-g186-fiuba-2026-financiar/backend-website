use sqlx::PgPool;

use crate::endpoints::user::email_verification::token::{generate_token, hash_token};
use crate::mail::{self, MailConfig};

/// Minutos de validez del link de reseteo
pub const TOKEN_TTL_MINUTES: i64 = 60;

/// Segundos minimos entre dos mails de reseteo al mismo usuario
pub const RESEND_COOLDOWN_SECONDS: i64 = 60;

const DEFAULT_FRONTEND_URL: &str = "http://localhost:5173";

/// Link del front que muestra el formulario de nueva contraseña
pub fn reset_link(token: &str) -> String {
    let frontend_url =
        std::env::var("FRONTEND_URL").unwrap_or_else(|_| DEFAULT_FRONTEND_URL.to_string());
    format!(
        "{}/restore-password?token={token}",
        frontend_url.trim_end_matches('/')
    )
}

pub async fn store_new_token(pool: &PgPool, user_id: i32) -> Result<Option<String>, sqlx::Error> {
    let token = generate_token();
    let updated = sqlx::query(
        r#"
        UPDATE users
        SET password_reset_token_hash = $1,
            password_reset_expires_at = NOW() + make_interval(mins => $2),
            password_reset_sent_at = NOW()
        WHERE id = $3
          AND (password_reset_sent_at IS NULL
               OR password_reset_sent_at < NOW() - make_interval(secs => $4))
        "#,
    )
    .bind(hash_token(&token))
    .bind(TOKEN_TTL_MINUTES as i32)
    .bind(user_id)
    .bind(RESEND_COOLDOWN_SECONDS as f64)
    .execute(pool)
    .await?;

    Ok((updated.rows_affected() > 0).then_some(token))
}

pub fn dispatch_password_reset_email(email: String, full_name: String, token: String) {
    let link = reset_link(&token);
    let Some(config) = MailConfig::from_env() else {
        tracing::warn!(
            "[PasswordReset] SMTP_HOST no esta configurado: no se envia el mail a {email}. Link de reseteo: {link}"
        );
        return;
    };

    tokio::spawn(async move {
        if let Err(err) = mail::send_password_reset(&config, &email, &full_name, &link).await {
            tracing::error!(
                "[PasswordReset] No se pudo enviar el mail de reseteo a {email}: {err}"
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reset_link_uses_frontend_url() {
        let _guard = crate::ENV_TEST_LOCK.lock().await;

        std::env::remove_var("FRONTEND_URL");
        assert_eq!(
            reset_link("abc"),
            "http://localhost:5173/restore-password?token=abc"
        );

        std::env::set_var("FRONTEND_URL", "https://frontend.example.test/");
        assert_eq!(
            reset_link("abc"),
            "https://frontend.example.test/restore-password?token=abc"
        );

        std::env::remove_var("FRONTEND_URL");
    }
}
