#[cfg(test)]
mod tests {
    use backend_website::endpoints::user_share::user_share_balance_logic::{
        calculate_portfolio_balance, calculate_share_balance, entry_price_for_purchase,
        latest_price, PriceFetchError, PricePoint,
    };

    // -------------------------------------------------------------
    // calculate_share_balance / calculate_portfolio_balance — unchanged
    // -------------------------------------------------------------

    #[test]
    fn profit_when_current_price_above_entry() {
        let balance = calculate_share_balance("GGAL", 10, Some(100.0), 150.0);
        assert_eq!(balance.current_value, 1500.0);
        assert_eq!(balance.cost_basis, Some(1000.0));
        assert_eq!(balance.profit_loss, Some(500.0));
        assert_eq!(balance.profit_loss_percent, Some(50.0));
    }

    #[test]
    fn loss_when_current_price_below_entry() {
        let balance = calculate_share_balance("GGAL", 10, Some(200.0), 150.0);
        assert_eq!(balance.current_value, 1500.0);
        assert_eq!(balance.cost_basis, Some(2000.0));
        assert_eq!(balance.profit_loss, Some(-500.0));
        assert_eq!(balance.profit_loss_percent, Some(-25.0));
    }

    #[test]
    fn breakeven_when_current_price_equals_entry() {
        let balance = calculate_share_balance("GGAL", 5, Some(300.0), 300.0);
        assert_eq!(balance.profit_loss, Some(0.0));
        assert_eq!(balance.profit_loss_percent, Some(0.0));
    }

    #[test]
    fn no_entry_price_means_no_pnl() {
        let balance = calculate_share_balance("GGAL", 5, None, 300.0);
        assert_eq!(balance.current_value, 1500.0);
        assert_eq!(balance.cost_basis, None);
        assert_eq!(balance.profit_loss, None);
        assert_eq!(balance.profit_loss_percent, None);
    }

    #[test]
    fn zero_quantity_gives_zero_value() {
        let balance = calculate_share_balance("GGAL", 0, Some(100.0), 150.0);
        assert_eq!(balance.current_value, 0.0);
        assert_eq!(balance.cost_basis, Some(0.0));
        assert_eq!(balance.profit_loss, Some(0.0));
        // cost_basis is 0.0 here, so percent stays None to avoid a divide-by-zero
        assert_eq!(balance.profit_loss_percent, None);
    }

    #[test]
    fn ticker_is_normalized_to_uppercase() {
        let balance = calculate_share_balance("ggal", 1, Some(1.0), 1.0);
        assert_eq!(balance.ticker, "GGAL");
    }

    #[test]
    fn portfolio_totals_sum_across_shares() {
        let shares = vec![
            calculate_share_balance("GGAL", 10, Some(100.0), 150.0),
            calculate_share_balance("YPFD", 5, Some(300.0), 300.0),
        ];
        let portfolio = calculate_portfolio_balance(shares);
        assert_eq!(portfolio.total_current_value, 3000.0);
        assert_eq!(portfolio.total_cost_basis, Some(2500.0));
        assert_eq!(portfolio.total_profit_loss, Some(500.0));
    }

    #[test]
    fn portfolio_cost_basis_is_none_if_any_lot_missing_entry_price() {
        let shares = vec![
            calculate_share_balance("GGAL", 10, Some(100.0), 150.0),
            calculate_share_balance("YPFD", 5, None, 300.0),
        ];
        let portfolio = calculate_portfolio_balance(shares);
        assert_eq!(portfolio.total_cost_basis, None);
        assert_eq!(portfolio.total_profit_loss, None);
    }

    #[test]
    fn empty_portfolio_has_zero_cost_basis() {
        let portfolio = calculate_portfolio_balance(Vec::new());
        assert_eq!(portfolio.total_current_value, 0.0);
        assert_eq!(portfolio.total_cost_basis, Some(0.0));
        assert_eq!(portfolio.total_profit_loss, Some(0.0));
    }

    // -------------------------------------------------------------
    // latest_price / entry_price_for_purchase — new
    // -------------------------------------------------------------

    const DAY: i64 = 86_400_000;
    const HOUR: i64 = 3_600_000;

    fn point(ts: i64, close: &str) -> PricePoint {
        PricePoint {
            ts,
            close_amount: close.to_string(),
        }
    }

    #[test]
    fn latest_price_picks_highest_ts_regardless_of_order() {
        let history = vec![
            point(3 * DAY, "110.0"),
            point(1 * DAY, "100.0"),
            point(2 * DAY, "105.0"),
        ];
        assert_eq!(latest_price(&history).unwrap(), 110.0);
    }

    #[test]
    fn latest_price_empty_history_errors() {
        assert!(matches!(latest_price(&[]), Err(PriceFetchError::Empty)));
    }

    #[test]
    fn latest_price_invalid_number_errors() {
        let history = vec![point(DAY, "not-a-number")];
        assert!(matches!(
            latest_price(&history),
            Err(PriceFetchError::InvalidPrice(_))
        ));
    }

    #[test]
    fn entry_price_same_day_uses_that_days_close() {
        // Compra a las 15hs del día 10; el cierre de ese mismo día se
        // registra más tarde (después del cierre de mercado) y aun así
        // debe matchear.
        let purchase_ts = 10 * DAY + 15 * HOUR;
        let history = vec![point(9 * DAY, "95.0"), point(10 * DAY + 20 * HOUR, "100.0")];
        assert_eq!(
            entry_price_for_purchase(&history, purchase_ts).unwrap(),
            Some(100.0)
        );
    }

    #[test]
    fn entry_price_weekend_falls_back_to_last_prior_trading_day() {
        // Compra el día 12 (sin sesión ese día); el último día hábil
        // anterior fue el día 10. El cierre del día 9 debe ser ignorado
        // en favor del más reciente (día 10).
        let purchase_ts = 12 * DAY + 10 * HOUR;
        let history = vec![point(9 * DAY, "90.0"), point(10 * DAY, "100.0")];
        assert_eq!(
            entry_price_for_purchase(&history, purchase_ts).unwrap(),
            Some(100.0)
        );
    }

    #[test]
    fn entry_price_ignores_points_after_purchase_day() {
        let purchase_ts = 10 * DAY;
        let history = vec![point(10 * DAY, "100.0"), point(11 * DAY, "999.0")];
        assert_eq!(
            entry_price_for_purchase(&history, purchase_ts).unwrap(),
            Some(100.0)
        );
    }

    #[test]
    fn entry_price_no_prior_data_returns_none() {
        let purchase_ts = 5 * DAY;
        let history = vec![point(10 * DAY, "100.0")]; // sólo hay data posterior a la compra
        assert_eq!(
            entry_price_for_purchase(&history, purchase_ts).unwrap(),
            None
        );
    }

    #[test]
    fn entry_price_invalid_close_propagates_error() {
        let purchase_ts = 10 * DAY;
        let history = vec![point(10 * DAY, "garbage")];
        assert!(matches!(
            entry_price_for_purchase(&history, purchase_ts),
            Err(PriceFetchError::InvalidPrice(_))
        ));
    }
}
