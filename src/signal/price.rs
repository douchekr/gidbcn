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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_above_triggered() {
        let cond = Condition::PriceAbove { target: 80000.0 };
        assert!(evaluate(&cond, 80000.0, None));
        assert!(evaluate(&cond, 80500.0, None));
        assert!(!evaluate(&cond, 79999.0, None));
    }

    #[test]
    fn price_below_triggered() {
        let cond = Condition::PriceBelow { target: 60000.0 };
        assert!(evaluate(&cond, 60000.0, None));
        assert!(evaluate(&cond, 59000.0, None));
        assert!(!evaluate(&cond, 60001.0, None));
    }

    #[test]
    fn profit_above_triggered() {
        let cond = Condition::ProfitAbove { percentage: 10.0 };
        // 70000 → 77000 = +10%
        assert!(evaluate(&cond, 77000.0, Some(70000.0)));
        assert!(!evaluate(&cond, 76000.0, Some(70000.0)));
    }

    #[test]
    fn profit_below_triggered() {
        let cond = Condition::ProfitBelow { percentage: -5.0 };
        // 70000 → 66500 = -5%
        assert!(evaluate(&cond, 66500.0, Some(70000.0)));
        assert!(!evaluate(&cond, 67000.0, Some(70000.0)));
    }

    #[test]
    fn profit_no_avg_price() {
        let cond = Condition::ProfitAbove { percentage: 10.0 };
        assert!(!evaluate(&cond, 80000.0, None));
    }

}
