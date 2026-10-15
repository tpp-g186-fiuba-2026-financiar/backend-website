use axum::{extract::State, http::StatusCode, Json};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

pub async fn backfill_user_shares_handler(
    State(pool): State<PgPool>,
) -> Result<Json<BackfillReport>, StatusCode> {
    let (entry_prices_updated, entry_prices_skipped) =
        backfill_entry_prices(&pool).await.map_err(|err| {
            tracing::error!("entry_price backfill failed: {}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let operations_seeded = seed_existing_holdings(&pool).await.map_err(|err| {
        tracing::error!("ledger seed failed: {}", err);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(BackfillReport {
        entry_prices_updated,
        entry_prices_skipped,
        operations_seeded,
    }))
}

pub async fn seed_existing_holdings(pool: &PgPool) -> Result<usize, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        share_id: i32,
        user_id: i32,
        missing_quantity: i32,
        entry_price: Option<f64>,
        created_at: DateTime<Utc>,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT us.share_id,
               us.user_id,
               (us.quantity - COALESCE(net.qty, 0))::INTEGER AS missing_quantity,
               us.entry_price,
               us.created_at
        FROM user_shares us
        LEFT JOIN (
            SELECT user_id, share_id,
                   SUM(CASE WHEN operation_type = 'buy' THEN quantity ELSE -quantity END) AS qty
            FROM user_share_operations
            GROUP BY user_id, share_id
        ) net ON net.user_id = us.user_id AND net.share_id = us.share_id
        WHERE us.quantity > COALESCE(net.qty, 0)
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut seeded = 0;
    for row in rows {
        let Some(price) = row.entry_price else {
            tracing::error!(
                "Skipping seed for user {} / share {}: entry_price still NULL",
                row.user_id,
                row.share_id
            );
            continue;
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
            row.missing_quantity,
            price,
            Some(row.created_at),
        )
        .await
        {
            tracing::error!(
                "Failed to seed operation for user {} / share {}: {}",
                row.user_id,
                row.share_id,
                err
            );
            continue; // el tx se descarta y hace rollback solo
        }

        if tx.commit().await.is_ok() {
            seeded += 1;
        }
    }

    Ok(seeded)
}
use std::collections::HashMap;

use crate::endpoints::user_share::{
    user_share_balance_logic::{entry_price_for_purchase, fetch_ticker_history},
    user_share_operations::{record_operation_at, OperationType},
};

#[derive(Debug, serde::Serialize)]
pub struct BackfillReport {
    pub entry_prices_updated: usize,
    pub entry_prices_skipped: usize,
    pub operations_seeded: usize,
}

/// Completa `user_shares.entry_price` donde es NULL, usando el precio
/// histórico del día de compra (`created_at`). Devuelve (actualizadas, salteadas).
pub async fn backfill_entry_prices(pool: &PgPool) -> Result<(usize, usize), sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        user_id: i32,
        share_id: i32,
        ticker: String,
        created_at: DateTime<Utc>,
    }

    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT us.user_id, us.share_id, s.ticker, us.created_at
        FROM user_shares us
        JOIN shares s ON s.id = us.share_id
        WHERE us.entry_price IS NULL
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut by_ticker: HashMap<String, Vec<Row>> = HashMap::new();
    for row in rows {
        by_ticker.entry(row.ticker.clone()).or_default().push(row);
    }

    let (mut updated, mut skipped) = (0usize, 0usize);

    for (ticker, rows) in by_ticker {
        let history = match fetch_ticker_history(&ticker).await {
            Ok(history) => history.to_vec(),
            Err(err) => {
                tracing::error!("Skipping entry_price backfill for {}: {:?}", ticker, err);
                skipped += rows.len();
                continue;
            }
        };

        for row in rows {
            let price = match entry_price_for_purchase(&history, row.created_at.timestamp_millis())
            {
                Ok(Some(price)) => price,
                other => {
                    tracing::error!(
                        "No entry price for user {} / {}: {:?}",
                        row.user_id,
                        ticker,
                        other
                    );
                    skipped += 1;
                    continue;
                }
            };

            let result = sqlx::query(
                r#"
                UPDATE user_shares
                SET entry_price = $1
                WHERE user_id = $2 AND share_id = $3 AND entry_price IS NULL
                "#,
            )
            .bind(price)
            .bind(row.user_id)
            .bind(row.share_id)
            .execute(pool)
            .await?;

            updated += result.rows_affected() as usize;
        }
    }

    Ok((updated, skipped))
}
