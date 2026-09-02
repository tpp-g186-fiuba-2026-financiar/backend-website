use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;

use crate::endpoints::user_share::trend_logic::fetch_trend;
use crate::mail::{send_trend_alert, MailConfig, TrendAlert};

const DEFAULT_CHECK_INTERVAL_HOURS: u64 = 24;

/// Arranca el job en background que, una vez por rueda, revisa si cambio la
/// tendencia de los tickers con suscripciones activas y manda las alertas
/// por mail correspondientes. No hace falta prediccion en tiempo real: las
/// predicciones de los modelos solo cambian una vez por dia (ver comentario
/// en `trend_logic::handler`), asi que alcanza con un chequeo diario.
pub fn spawn_daily_alert_job(pool: PgPool) {
    let interval_hours: u64 = std::env::var("ALERTS_CHECK_INTERVAL_HOURS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_CHECK_INTERVAL_HOURS);
    let mail_config = MailConfig::from_env();
    if mail_config.is_none() {
        tracing::warn!(
            "[Alerts] SMTP_HOST no esta configurado: el job de alertas va a correr igual (para mantener la cache de tendencias al dia) pero no va a enviar mails."
        );
    }

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(interval_hours * 3600)).await;
            tracing::info!("[Alerts] Revisando cambios de tendencia para suscripciones...");
            if let Err(err) = check_trend_changes(&pool, mail_config.as_ref()).await {
                tracing::error!("[Alerts] Fallo la revision de alertas: {}", err);
            }
        }
    });
}

async fn check_trend_changes(
    pool: &PgPool,
    mail_config: Option<&MailConfig>,
) -> Result<(), sqlx::Error> {
    let tickers = subscribed_tickers(pool).await?;
    if tickers.is_empty() {
        return Ok(());
    }

    let modal_lstm_url = std::env::var("MODAL_LSTM_URL")
        .unwrap_or_else(|_| "https://matimorales01--lstm-trend-model-main.modal.run".into());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("reqwest client");

    for ticker in tickers {
        let previous = current_cached_payload(pool, &ticker).await?;
        let fresh = fetch_trend(&client, &modal_lstm_url, &ticker).await;

        if fresh.get("available").and_then(Value::as_bool) != Some(true) {
            // Sin prediccion nueva disponible: no hay nada que comparar ni
            // que cachear, se deja el valor previo tal cual estaba.
            continue;
        }

        if let Some(previous) = &previous {
            if condition_changed(previous, &fresh) {
                notify_subscribers(pool, mail_config, &ticker, previous, &fresh).await?;
            }
        }
        // Si no habia valor previo cacheado, es la primera vez que se calcula
        // la tendencia para este ticker: se guarda como base pero no se
        // manda alerta (no hay "cambio" contra el cual comparar).

        upsert_cache(pool, &ticker, &fresh).await?;
    }

    Ok(())
}

/// Union de los tickers con suscripcion puntual y los tickers de la cartera
/// de cualquier usuario suscripto a "toda la cartera".
async fn subscribed_tickers(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT DISTINCT s.ticker
        FROM alert_subscriptions a
        JOIN shares s ON s.id = a.share_id
        WHERE a.share_id IS NOT NULL

        UNION

        SELECT DISTINCT s.ticker
        FROM alert_subscriptions a
        JOIN user_shares us ON us.user_id = a.user_id
        JOIN shares s ON s.id = us.share_id
        WHERE a.share_id IS NULL
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(ticker,)| ticker).collect())
}

async fn current_cached_payload(pool: &PgPool, ticker: &str) -> Result<Option<Value>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Value,)>(
        r#"SELECT payload FROM ticker_trend_cache WHERE ticker = $1"#,
    )
    .bind(ticker)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(payload,)| payload))
}

async fn upsert_cache(pool: &PgPool, ticker: &str, payload: &Value) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO ticker_trend_cache (ticker, payload, fetched_at)
        VALUES ($1, $2, now())
        ON CONFLICT (ticker)
        DO UPDATE SET payload = EXCLUDED.payload, fetched_at = EXCLUDED.fetched_at
        "#,
    )
    .bind(ticker)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(())
}

/// La tendencia relevante para el cliente (sobrecompra/sobreventa) vive en
/// el campo `condition`. Tambien se informa `signal` en el mail como
/// contexto adicional, pero lo que dispara la alerta es este campo.
fn condition_changed(previous: &Value, fresh: &Value) -> bool {
    let previous_condition = previous.get("condition").and_then(Value::as_str);
    let fresh_condition = fresh.get("condition").and_then(Value::as_str);
    previous_condition != fresh_condition && fresh_condition.is_some()
}

async fn notify_subscribers(
    pool: &PgPool,
    mail_config: Option<&MailConfig>,
    ticker: &str,
    previous: &Value,
    fresh: &Value,
) -> Result<(), sqlx::Error> {
    let subscribers = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT DISTINCT u.email, u.full_name
        FROM alert_subscriptions a
        JOIN users u ON u.id = a.user_id
        JOIN shares s ON s.id = a.share_id
        WHERE a.share_id IS NOT NULL AND s.ticker = $1 AND u.is_active

        UNION

        SELECT DISTINCT u.email, u.full_name
        FROM alert_subscriptions a
        JOIN users u ON u.id = a.user_id
        JOIN user_shares us ON us.user_id = a.user_id
        JOIN shares s ON s.id = us.share_id
        WHERE a.share_id IS NULL AND s.ticker = $1 AND u.is_active
        "#,
    )
    .bind(ticker)
    .fetch_all(pool)
    .await?;

    let previous_condition = previous
        .get("condition")
        .and_then(Value::as_str)
        .unwrap_or("desconocido");
    let new_condition = fresh
        .get("condition")
        .and_then(Value::as_str)
        .unwrap_or("desconocido");
    let signal = fresh.get("signal").and_then(Value::as_str);
    let as_of = fresh.get("as_of").and_then(Value::as_str);

    tracing::info!(
        "[Alerts] {} cambio de tendencia: {} -> {} ({} suscriptos)",
        ticker,
        previous_condition,
        new_condition,
        subscribers.len()
    );

    let Some(mail_config) = mail_config else {
        return Ok(());
    };

    let alert = TrendAlert {
        ticker,
        previous_condition,
        new_condition,
        signal,
        as_of,
    };

    for (email, full_name) in subscribers {
        if let Err(err) = send_trend_alert(mail_config, &email, &full_name, &alert).await {
            // Un mail que falla no debe frenar el resto ni la actualizacion
            // de la cache de tendencias.
            tracing::error!("[Alerts] No se pudo enviar mail a {}: {}", email, err);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn condition_changed_detects_difference() {
        let previous = json!({ "condition": "sobrecompra" });
        let fresh = json!({ "condition": "sobreventa" });
        assert!(condition_changed(&previous, &fresh));
    }

    #[test]
    fn condition_changed_is_false_when_equal() {
        let previous = json!({ "condition": "neutral" });
        let fresh = json!({ "condition": "neutral" });
        assert!(!condition_changed(&previous, &fresh));
    }

    #[test]
    fn condition_changed_is_false_when_fresh_missing_condition() {
        let previous = json!({ "condition": "neutral" });
        let fresh = json!({ "condition": Value::Null });
        assert!(!condition_changed(&previous, &fresh));
    }
}
