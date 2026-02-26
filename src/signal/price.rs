use crate::models::signal::Condition;

/// 가격 기반 시그널 평가: price_above, price_below, profit_above, profit_below
pub fn evaluate(condition: &Condition, current_price: f64, avg_price: Option<f64>) -> bool {
    match condition {
        Condition::PriceAbove { target } => current_price >= *target,
        Condition::PriceBelow { target } => current_price <= *target,
        Condition::ProfitAbove { percentage } => {
            if let Some(avg) = avg_price {
                if avg > 0.0 {
                    let pnl_pct = (current_price - avg) / avg * 100.0;
                    return pnl_pct >= *percentage;
                }
            }
            false
        }
        Condition::ProfitBelow { percentage } => {
            if let Some(avg) = avg_price {
                if avg > 0.0 {
                    let pnl_pct = (current_price - avg) / avg * 100.0;
                    return pnl_pct <= *percentage;
                }
            }
            false
        }
        _ => false,
    }
}
