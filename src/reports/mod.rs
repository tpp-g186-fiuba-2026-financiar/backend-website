use std::{collections::HashMap, time::Duration};

use chrono::{Datelike, Utc, Weekday};
use serde_json::Value;
use sqlx::PgPool;
use tokio::task::JoinSet;

use crate::mail::{send_weekly_report, MailConfig, WeeklyReport, WeeklyReportLine};

const DEFAULT_CHECK_INTERVAL_HOURS: u64 = 24;
const SECONDS_IN_A_WEEK: i64 = 7 * 24 * 3600;
const REPORT_WEEKDAY: Weekday = Weekday::Mon;

/// Arranca el job en background que, una vez por semana (lunes), manda un
/// mail resumen de cartera a cada usuario activo con al menos una accion
/// cargada. Mismo patron que `alerts::spawn_daily_alert_job`: un loop que
/// chequea una vez por dia y solo actua si corresponde -- no hace falta una
/// libreria de cron nueva, y las predicciones/cotizaciones tampoco cambian
/// mas de una vez por rueda.
pub fn spawn_weekly_report_job(pool: PgPool) {
    let interval_hours: u64 = std::env::var("WEEKLY_REPORT_CHECK_INTERVAL_HOURS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_CHECK_INTERVAL_HOURS);
    let mail_config = MailConfig::from_env();
    if mail_config.is_none() {
        tracing::warn!(
            "[WeeklyReport] SMTP_HOST no esta configurado: el job va a correr igual pero no va a enviar mails."
        );
    }

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(interval_hours * 3600)).await;
            if !is_report_day(Utc::now().weekday()) {
                continue;
            }
            tracing::info!("[WeeklyReport] Armando el resumen semanal de carteras...");
            if let Err(err) = send_weekly_reports(&pool, mail_config.as_ref()).await {
                tracing::error!("[WeeklyReport] Fallo el envio del resumen semanal: {}", err);
            }
        }
    });
}

fn is_report_day(today: Weekday) -> bool {
    today == REPORT_WEEKDAY
}

struct Holding {
    user_id: i32,
    email: String,
    full_name: String,
    ticker: String,
    quantity: i32,
    entry_price: Option<f64>,
}

/// Tenencias de todos los usuarios activos con al menos una accion cargada.
/// A diferencia de las alertas de tendencia (opt-in por ticker o cartera,
/// ver `alert_subscriptions`), el resumen semanal no tiene suscripcion
/// aparte: si tenes cartera, lo recibis.
async fn active_user_holdings(pool: &PgPool) -> Result<Vec<Holding>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (i32, String, String, String, i32, Option<f64>)>(
        r#"
        SELECT u.id, u.email, u.full_name, s.ticker, us.quantity, us.entry_price
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        JOIN users u ON u.id = us.user_id
        WHERE u.is_active
        ORDER BY u.id, s.ticker
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(user_id, email, full_name, ticker, quantity, entry_price)| Holding {
                user_id,
                email,
                full_name,
                ticker,
                quantity,
                entry_price,
            },
        )
        .collect())
}

async fn send_weekly_reports(
    pool: &PgPool,
    mail_config: Option<&MailConfig>,
) -> Result<(), sqlx::Error> {
    let holdings = active_user_holdings(pool).await?;
    if holdings.is_empty() {
        return Ok(());
    }

    let tickers: Vec<String> = {
        let mut tickers: Vec<String> = holdings.iter().map(|h| h.ticker.clone()).collect();
        tickers.sort();
        tickers.dedup();
        tickers
    };
    let prices = fetch_ticker_week_prices(&tickers).await;

    let mut by_user: HashMap<i32, (String, String, Vec<Holding>)> = HashMap::new();
    for holding in holdings {
        by_user
            .entry(holding.user_id)
            .or_insert_with(|| (holding.email.clone(), holding.full_name.clone(), Vec::new()))
            .2
            .push(holding);
    }

    tracing::info!(
        "[WeeklyReport] Enviando resumen semanal a {} usuarios ({} tickers distintos cotizados)",
        by_user.len(),
        prices.len()
    );

    let Some(mail_config) = mail_config else {
        return Ok(());
    };

    for (email, full_name, holdings) in by_user.into_values() {
        let mut lines = Vec::with_capacity(holdings.len());
        let mut total_current_value = 0.0;
        let mut total_current_value_with_week_ago = 0.0;
        let mut total_week_ago_value = 0.0;

        for holding in &holdings {
            let Some(week_prices) = prices.get(&holding.ticker) else {
                continue;
            };
            let current_price = week_prices.current;
            let quantity = f64::from(holding.quantity);
            let week_change_pct = week_change(current_price, week_prices.week_ago);
            let pnl_pct = pnl_percentage(current_price, holding.entry_price);

            total_current_value += current_price * quantity;
            if let Some(week_ago_price) = week_prices.week_ago {
                total_current_value_with_week_ago += current_price * quantity;
                total_week_ago_value += week_ago_price * quantity;
            }

            lines.push(WeeklyReportLine {
                ticker: &holding.ticker,
                quantity: holding.quantity,
                current_price,
                week_change_pct,
                pnl_pct,
            });
        }

        if lines.is_empty() {
            // No se pudo cotizar ningun ticker de esta cartera esta semana
            // (data-colector caido, todos tickers nuevos sin historia
            // todavia): no tiene sentido mandar un mail vacio.
            continue;
        }

        let total_week_change_pct = (total_week_ago_value > 0.0).then(|| {
            (total_current_value_with_week_ago - total_week_ago_value) / total_week_ago_value
                * 100.0
        });

        let report = WeeklyReport {
            lines: &lines,
            total_current_value,
            total_week_change_pct,
        };

        if let Err(err) = send_weekly_report(mail_config, &email, &full_name, &report).await {
            // Un mail que falla no debe frenar el resto.
            tracing::error!("[WeeklyReport] No se pudo enviar mail a {}: {}", email, err);
        }
    }

    Ok(())
}

struct TickerWeekPrices {
    current: f64,
    week_ago: Option<f64>,
}

async fn fetch_ticker_week_prices(tickers: &[String]) -> HashMap<String, TickerWeekPrices> {
    let collector_url = std::env::var("DATA_COLLECTOR_URL")
        .unwrap_or_else(|_| "https://data-colector.onrender.com".into());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .expect("reqwest client");

    let mut requests = JoinSet::new();
    for ticker in tickers {
        let ticker = ticker.clone();
        let client = client.clone();
        let collector_url = collector_url.clone();
        requests.spawn(async move {
            let points = fetch_ticker_history_points(&client, &collector_url, &ticker).await;
            (ticker, points)
        });
    }

    let mut prices = HashMap::new();
    while let Some(result) = requests.join_next().await {
        let Ok((ticker, points)) = result else {
            continue;
        };
        if let Some(week_prices) = week_prices_from_points(&points) {
            prices.insert(ticker, week_prices);
        }
    }
    prices
}

/// Mismo endpoint y parseo que `pnl_logic::fetch_current_price` /
/// `history_logic::handler`, pero devolviendo todos los puntos: aca hace
/// falta ademas del precio actual, el mas cercano a hace 7 dias.
async fn fetch_ticker_history_points(
    client: &reqwest::Client,
    base_url: &str,
    ticker: &str,
) -> Vec<(i64, f64)> {
    let url = format!(
        "{}/historical-data/{}",
        base_url.trim_end_matches('/'),
        ticker
    );
    let response = match client.post(&url).send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::warn!(
                "[WeeklyReport] data-colector respondio {} para {}",
                response.status(),
                ticker
            );
            return Vec::new();
        }
        Err(error) => {
            tracing::error!(
                "[WeeklyReport] No se pudo contactar a data-colector para {}: {}",
                ticker,
                error
            );
            return Vec::new();
        }
    };

    let body: Value = match response.json().await {
        Ok(body) => body,
        Err(error) => {
            tracing::error!(
                "[WeeklyReport] Respuesta invalida de data-colector para {}: {}",
                ticker,
                error
            );
            return Vec::new();
        }
    };

    body.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let ts = row.get("ts")?.as_i64()?;
            let raw_close = row.get("close_amount")?;
            let close = raw_close
                .as_f64()
                .or_else(|| raw_close.as_str()?.parse::<f64>().ok())?;
            close.is_finite().then_some((ts, close))
        })
        .collect()
}

/// `points` no necesita venir ordenado. El precio "actual" es el cierre mas
/// reciente disponible; el de "hace una semana" es el cierre mas cercano
/// (pero no posterior) a 7 dias antes de ese ultimo cierre -- se ancla al
/// ultimo dato real, no al reloj, para no fallar por fines de semana o
/// feriados en los que no hubo rueda. `ts` viene en milisegundos (ver
/// `historical_data.rs` en data-colector). None si no hay ningun punto
/// (ticker sin historia todavia).
fn week_prices_from_points(points: &[(i64, f64)]) -> Option<TickerWeekPrices> {
    let &(latest_ts, current) = points.iter().max_by_key(|(ts, _)| *ts)?;
    let cutoff = latest_ts - SECONDS_IN_A_WEEK * 1000;
    let week_ago = points
        .iter()
        .filter(|(ts, _)| *ts <= cutoff)
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, close)| *close);
    Some(TickerWeekPrices { current, week_ago })
}

fn week_change(current: f64, week_ago: Option<f64>) -> Option<f64> {
    let week_ago = week_ago?;
    (week_ago > 0.0).then(|| (current - week_ago) / week_ago * 100.0)
}

fn pnl_percentage(current: f64, entry: Option<f64>) -> Option<f64> {
    let entry = entry?;
    (entry > 0.0).then(|| (current - entry) / entry * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Path, routing::post, Json, Router};
    use serde_json::json;

    fn ts(day: i64) -> i64 {
        day * 86_400_000
    }

    #[test]
    fn week_prices_picks_latest_close_and_closest_before_cutoff() {
        let points = vec![
            (ts(0), 100.0),
            (ts(6), 108.0),
            (ts(7), 110.0),
            (ts(14), 130.0),
        ];
        let result = week_prices_from_points(&points).unwrap();
        assert_eq!(result.current, 130.0);
        assert_eq!(result.week_ago, Some(110.0));
    }

    #[test]
    fn week_prices_returns_none_week_ago_when_ticker_is_new() {
        let points = vec![(ts(0), 100.0), (ts(1), 105.0)];
        let result = week_prices_from_points(&points).unwrap();
        assert_eq!(result.current, 105.0);
        assert_eq!(result.week_ago, None);
    }

    #[test]
    fn week_prices_returns_none_when_no_points() {
        assert!(week_prices_from_points(&[]).is_none());
    }

    #[test]
    fn week_prices_ignores_order_of_input() {
        let points = vec![(ts(14), 130.0), (ts(0), 100.0), (ts(7), 110.0)];
        let result = week_prices_from_points(&points).unwrap();
        assert_eq!(result.current, 130.0);
        assert_eq!(result.week_ago, Some(110.0));
    }

    #[test]
    fn week_change_computes_percentage_up_and_down() {
        assert_eq!(week_change(110.0, Some(100.0)), Some(10.0));
        assert_eq!(week_change(90.0, Some(100.0)), Some(-10.0));
    }

    #[test]
    fn week_change_is_none_without_a_week_ago_price() {
        assert_eq!(week_change(100.0, None), None);
    }

    #[test]
    fn pnl_percentage_computes_percentage() {
        assert_eq!(pnl_percentage(120.0, Some(100.0)), Some(20.0));
    }

    #[test]
    fn pnl_percentage_is_none_without_entry_price() {
        assert_eq!(pnl_percentage(120.0, None), None);
    }

    #[test]
    fn is_report_day_matches_only_monday() {
        assert!(is_report_day(Weekday::Mon));
        assert!(!is_report_day(Weekday::Tue));
        assert!(!is_report_day(Weekday::Sun));
    }

    async fn history_stub(Path(_ticker): Path<String>) -> Json<Value> {
        Json(json!({
            "data": [
                {"ts": ts(0), "close_amount": "100.0"},
                {"ts": ts(7), "close_amount": 110.0},
                {"ts": ts(14), "close_amount": "125.0"},
                {"ts": ts(13), "close_amount": "invalid"}
            ]
        }))
    }

    #[tokio::test]
    async fn weekly_report_loads_holdings_prices_and_builds_mail() {
        let _env_guard = crate::ENV_TEST_LOCK.lock().await;
        dotenvy::dotenv().ok();
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let email = format!("weekly_report_{suffix}@test.com");
        let ticker = format!("R{}", suffix % 1_000_000);
        let user_id: i32 = sqlx::query_scalar(
            "INSERT INTO users (email, password_hash, full_name, risk_profile) VALUES ($1, 'hash', 'Weekly Report', 'moderate') RETURNING id",
        )
        .bind(&email)
        .fetch_one(&pool)
        .await
        .unwrap();
        let share_id: i32 =
            sqlx::query_scalar("INSERT INTO shares (ticker) VALUES ($1) RETURNING id")
                .bind(&ticker)
                .fetch_one(&pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO user_shares (user_id, share_id, quantity, entry_price) VALUES ($1, $2, 3, 90.0)",
        )
        .bind(user_id)
        .bind(share_id)
        .execute(&pool)
        .await
        .unwrap();

        let stub = Router::new().route("/historical-data/{ticker}", post(history_stub));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, stub).await.unwrap() });
        std::env::set_var("DATA_COLLECTOR_URL", format!("http://{address}"));

        send_weekly_reports(&pool, None).await.unwrap();
        let mail = MailConfig {
            host: "invalid host".to_string(),
            port: 587,
            username: "user".to_string(),
            password: "pass".to_string(),
            from_email: "alertas@financiar.test".to_string(),
            from_name: "FinanciAr".to_string(),
        };
        send_weekly_reports(&pool, Some(&mail)).await.unwrap();

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM shares WHERE id = $1")
            .bind(share_id)
            .execute(&pool)
            .await
            .unwrap();
    }
}
