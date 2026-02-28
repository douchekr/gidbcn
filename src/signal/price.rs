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

    #[test]
    fn profit_zero_avg_price() {
        // avg_price=0 → 0으로 나누기 방지, false 반환
        assert!(!evaluate(&Condition::ProfitAbove { percentage: 10.0 }, 80000.0, Some(0.0)));
        assert!(!evaluate(&Condition::ProfitBelow { percentage: -5.0 }, 50000.0, Some(0.0)));
    }

    #[test]
    fn profit_above_negative_threshold() {
        // 손실 허용: target=-5%, 현재 -3% → 트리거 (손실이 허용 범위 내)
        let cond = Condition::ProfitAbove { percentage: -5.0 };
        // 70000 → 67900 = -3%
        assert!(evaluate(&cond, 67900.0, Some(70000.0)));
        // 70000 → 66000 = -5.7% → 미트리거
        assert!(!evaluate(&cond, 66000.0, Some(70000.0)));
    }

    #[test]
    fn profit_below_positive_threshold() {
        // 수익 상한: target=20%, 현재 15% → 트리거 (수익이 상한 이하)
        let cond = Condition::ProfitBelow { percentage: 20.0 };
        // 70000 → 80500 = +15%
        assert!(evaluate(&cond, 80500.0, Some(70000.0)));
        // 70000 → 84700 = +21% → 미트리거
        assert!(!evaluate(&cond, 84700.0, Some(70000.0)));
    }

    #[test]
    fn price_conditions_with_decimals() {
        // 해외주식 소수점 가격
        let above = Condition::PriceAbove { target: 200.50 };
        assert!(evaluate(&above, 200.50, None));
        assert!(evaluate(&above, 200.51, None));
        assert!(!evaluate(&above, 200.49, None));

        let below = Condition::PriceBelow { target: 150.25 };
        assert!(evaluate(&below, 150.25, None));
        assert!(evaluate(&below, 150.24, None));
        assert!(!evaluate(&below, 150.26, None));
    }
}
