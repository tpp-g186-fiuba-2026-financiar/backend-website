#[cfg(test)]
mod tests {
    use backend_website::endpoints::user_share::balance_history_logic::{
        build_checkpoints, build_daily_timeline, holdings_on_day, LedgerOp,
    };
    use backend_website::endpoints::user_share::user_share_balance_logic::PricePoint;
    use std::collections::HashMap;

    const DAY: i64 = 86_400_000;

    fn buy(day: i64, quantity: i32, price: f64) -> LedgerOp {
        LedgerOp {
            is_buy: true,
            quantity,
            price,
            created_at_ms: day * DAY,
        }
    }

    fn sell(day: i64, quantity: i32, price: f64) -> LedgerOp {
        LedgerOp {
            is_buy: false,
            quantity,
            price,
            created_at_ms: day * DAY,
        }
    }

    fn price_point(day: i64, close: &str) -> PricePoint {
        PricePoint {
            ts: day * DAY,
            close_amount: close.to_string(),
        }
    }

    // -------------------------------------------------------------
    // build_checkpoints / holdings_on_day
    // -------------------------------------------------------------

    #[test]
    fn single_buy_gives_its_own_price_as_avg_cost() {
        let checkpoints = build_checkpoints(vec![buy(1, 10, 100.0)]);
        assert_eq!(holdings_on_day(&checkpoints, 1), (10, 100.0));
        assert_eq!(holdings_on_day(&checkpoints, 5), (10, 100.0));
    }

    #[test]
    fn second_buy_at_different_price_blends_average_cost() {
        // 10 @ 100 then 10 @ 200 -> 20 shares @ avg 150
        let checkpoints = build_checkpoints(vec![buy(1, 10, 100.0), buy(5, 10, 200.0)]);
        assert_eq!(holdings_on_day(&checkpoints, 1), (10, 100.0));
        assert_eq!(holdings_on_day(&checkpoints, 4), (10, 100.0));
        assert_eq!(holdings_on_day(&checkpoints, 5), (20, 150.0));
    }

    #[test]
    fn partial_sell_keeps_average_cost_of_remaining_shares() {
        // 10 @ 100, then sell 4 -> 6 shares remain, still @ avg 100
        let checkpoints = build_checkpoints(vec![buy(1, 10, 100.0), sell(5, 4, 999.0)]);
        assert_eq!(holdings_on_day(&checkpoints, 5), (6, 100.0));
    }

    #[test]
    fn selling_the_whole_position_resets_avg_cost_to_zero() {
        let checkpoints = build_checkpoints(vec![buy(1, 10, 100.0), sell(5, 10, 150.0)]);
        assert_eq!(holdings_on_day(&checkpoints, 5), (0, 0.0));
    }

    #[test]
    fn holdings_before_any_operation_are_zero() {
        let checkpoints = build_checkpoints(vec![buy(10, 5, 100.0)]);
        assert_eq!(holdings_on_day(&checkpoints, 1), (0, 0.0));
    }

    #[test]
    fn same_day_operations_use_the_last_one_chronologically() {
        // Buy then sell same calendar day: final state should reflect both.
        let checkpoints = build_checkpoints(vec![
            LedgerOp {
                is_buy: true,
                quantity: 10,
                price: 100.0,
                created_at_ms: 1 * DAY,
            },
            LedgerOp {
                is_buy: false,
                quantity: 3,
                price: 120.0,
                created_at_ms: 1 * DAY + 3_600_000,
            },
        ]);
        assert_eq!(holdings_on_day(&checkpoints, 1), (7, 100.0));
    }

    // -------------------------------------------------------------
    // build_daily_timeline
    // -------------------------------------------------------------

    #[test]
    fn single_ticker_single_buy_timeline() {
        let checkpoints = build_checkpoints(vec![buy(1, 10, 100.0)]);
        let history = vec![price_point(1, "100.0"), price_point(3, "120.0")];
        let mut tickers = HashMap::new();
        tickers.insert("GGAL".to_string(), (checkpoints, history));

        let timeline = build_daily_timeline(&tickers, 1, 3).unwrap();
        assert_eq!(timeline.len(), 3);

        assert_eq!(timeline[0].total_current_value, 1000.0);
        assert_eq!(timeline[0].total_cost_basis, 1000.0);
        assert_eq!(timeline[0].total_profit_loss, 0.0);

        // Day 2 has no session; falls back to day 1's close (100.0).
        assert_eq!(timeline[1].total_current_value, 1000.0);

        // Day 3 has its own close.
        assert_eq!(timeline[2].total_current_value, 1200.0);
        assert_eq!(timeline[2].total_profit_loss, 200.0);
    }

    #[test]
    fn day_before_any_holding_reports_zero_balance() {
        let checkpoints = build_checkpoints(vec![buy(5, 10, 100.0)]);
        let history = vec![price_point(1, "90.0"), price_point(5, "100.0")];
        let mut tickers = HashMap::new();
        tickers.insert("GGAL".to_string(), (checkpoints, history));

        let timeline = build_daily_timeline(&tickers, 1, 5).unwrap();
        assert_eq!(timeline[0].total_current_value, 0.0);
        assert_eq!(timeline[0].total_profit_loss, 0.0);
        assert_eq!(timeline[4].total_current_value, 1000.0);
    }

    #[test]
    fn multiple_tickers_sum_into_the_same_day() {
        let ggal_checkpoints = build_checkpoints(vec![buy(1, 10, 100.0)]);
        let ypfd_checkpoints = build_checkpoints(vec![buy(1, 5, 50.0)]);
        let mut tickers = HashMap::new();
        tickers.insert(
            "GGAL".to_string(),
            (ggal_checkpoints, vec![price_point(1, "100.0")]),
        );
        tickers.insert(
            "YPFD".to_string(),
            (ypfd_checkpoints, vec![price_point(1, "50.0")]),
        );

        let timeline = build_daily_timeline(&tickers, 1, 1).unwrap();
        // GGAL: 10*100 = 1000, YPFD: 5*50 = 250 -> total 1250
        assert_eq!(timeline[0].total_current_value, 1250.0);
        assert_eq!(timeline[0].total_cost_basis, 1250.0);
    }

    #[test]
    fn missing_price_history_before_purchase_day_skips_that_ticker_for_that_day() {
        let checkpoints = build_checkpoints(vec![buy(1, 10, 100.0)]);
        // History only starts on day 5, later than the purchase and the
        // requested range's early days.
        let history = vec![price_point(5, "100.0")];
        let mut tickers = HashMap::new();
        tickers.insert("GGAL".to_string(), (checkpoints, history));

        let timeline = build_daily_timeline(&tickers, 1, 5).unwrap();
        assert_eq!(timeline[0].total_current_value, 0.0);
        assert_eq!(timeline[4].total_current_value, 1000.0);
    }
}
