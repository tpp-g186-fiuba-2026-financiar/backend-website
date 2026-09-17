use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};

use crate::endpoints::user_share::user_share_balance_logic::{
    entry_price_for_purchase, fetch_ticker_history, latest_price, PriceFetchError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Buy,
    Sell,
}

impl OperationType {
    fn as_db_str(self) -> &'static str {
        match self {
            OperationType::Buy => "buy",
            OperationType::Sell => "sell",
        }
    }
}

/// Fetches the current price for `ticker` by hitting the data-collector's
/// historical endpoint and taking the most recent point — the same source
/// the balance endpoint uses, so "current price" means the same thing
/// everywhere in the app.
pub async fn fetch_current_price(ticker: &str) -> Result<f64, PriceFetchError> {
    let history = fetch_ticker_history(ticker).await?.to_vec();
    latest_price(&history)
}

/// Records one buy/sell in the operations ledger with `created_at =
/// NOW()`, inside an existing transaction so it's atomic with whatever
/// change to `user_shares` triggered it.
pub async fn record_operation(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i32,
    share_id: i32,
    operation_type: OperationType,
    quantity: i32,
    price: f64,
) -> Result<(), sqlx::Error> {
    record_operation_at(tx, user_id, share_id, operation_type, quantity, price, None).await
}

/// Same as `record_operation`, but lets the caller set an explicit
/// `created_at` — only used by `seed_existing_holdings` to backdate the
/// retroactive entries to when the position was actually opened.
pub async fn record_operation_at(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i32,
    share_id: i32,
    operation_type: OperationType,
    quantity: i32,
    price: f64,
    created_at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO user_share_operations (user_id, share_id, operation_type, quantity, price, created_at)
        VALUES ($1, $2, $3, $4, $5, COALESCE($6, NOW()))
        "#,
    )
    .bind(user_id)
    .bind(share_id)
    .bind(operation_type.as_db_str())
    .bind(quantity)
    .bind(price)
    .bind(created_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// One-off backfill: for every existing `user_shares` row that doesn't
/// already have a matching ledger entry, inserts a 'buy' operation —
/// using the row's `entry_price` if it's set, or resolving one from
/// historical data (same day-of-purchase rule as the entry_price backfill
/// in `user_share_balance_logic`) if not. Rows whose price can't be
/// resolved either way are skipped and logged, not failed.
///
/// This is NOT wired into any request path — it's meant to be invoked
/// once, manually (e.g. from a temporary call in `main`, or an ignored
/// test), after this migration has been deployed.
pub async fn seed_existing_holdings(pool: &PgPool) -> Result<usize, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        share_id: i32,
        ticker: String,
        user_id: i32,
        quantity: i32,
        entry_price: Option<f64>,
        created_at: DateTime<Utc>,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT us.share_id, s.ticker, us.user_id, us.quantity, us.entry_price, us.created_at
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE NOT EXISTS (
            SELECT 1 FROM user_share_operations uso
            WHERE uso.user_id = us.user_id AND uso.share_id = us.share_id
        )
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut seeded = 0;
    for row in rows {
        let price = match row.entry_price {
            Some(price) => price,
            None => {
                let history = match fetch_ticker_history(&row.ticker).await {
                    Ok(history) => history.to_vec(),
                    Err(err) => {
                        tracing::error!(
                            "Skipping seed for user {} / {}: {:?}",
                            row.user_id,
                            row.ticker,
                            err
                        );
                        continue;
                    }
                };
                match entry_price_for_purchase(&history, row.created_at.timestamp_millis()) {
                    Ok(Some(price)) => price,
                    Ok(None) => {
                        tracing::error!(
                            "Skipping seed for user {} / {}: no historical price before {}",
                            row.user_id,
                            row.ticker,
                            row.created_at
                        );
                        continue;
                    }
                    Err(err) => {
                        tracing::error!(
                            "Skipping seed for user {} / {}: {:?}",
                            row.user_id,
                            row.ticker,
                            err
                        );
                        continue;
                    }
                }
            }
        };

        let mut tx = match pool.begin().await {
            Ok(tx) => tx,
            Err(err) => {
                tracing::error!("Failed to start seed transaction: {}", err);
                continue;
            }
        };

        if let Err(err) = record_operation_at(
            &mut tx,
            row.user_id,
            row.share_id,
            OperationType::Buy,
            row.quantity,
            price,
            Some(row.created_at),
        )
        .await
        {
            tracing::error!(
                "Failed to seed operation for user {} / {}: {}",
                row.user_id,
                row.ticker,
                err
            );
            continue;
        }

        if tx.commit().await.is_ok() {
            seeded += 1;
        }
    }

    Ok(seeded)
}