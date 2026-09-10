#[cfg(test)]
mod tests {
    use backend_website::endpoints::user_share::user_share_balance_logic::{
        calculate_portfolio_balance, calculate_share_balance,
    };

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
}
