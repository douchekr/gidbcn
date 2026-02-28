use crate::models::messages::PriceData;
use crate::models::portfolio::{Holding, Market};
use crate::models::signal::{Condition, Signal};

/// 정수를 3자리 콤마 포맷 (예: 70000 → "70,000")
pub fn fmt_int(v: f64) -> String {
    let n = v as i64;
    if n == 0 {
        return "0".to_string();
    }
    let neg = n < 0;
    let s = n.unsigned_abs().to_string();
    let len = s.len();
    let mut result = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    if neg {
        format!("-{result}")
    } else {
        result
    }
}

/// 소수점 2자리 + 콤마 포맷 (예: 1350.50 → "1,350.50")
fn fmt_dec2(v: f64) -> String {
    let neg = v < 0.0;
    let abs = v.abs();
    let int_str = fmt_int(abs.trunc());
    let frac = (abs.fract() * 100.0).round() as u64;
    let s = format!("{int_str}.{frac:02}");
    if neg { format!("-{s}") } else { s }
}

pub fn fmt_quantity(v: f64) -> String {
    fmt_int(v)
}

pub fn fmt_price(market: &Market, v: f64) -> String {
    match market {
        Market::NAS | Market::NYS | Market::AMS => fmt_price_us(v),
        _ => fmt_int(v),
    }
}

fn fmt_qty(h: &Holding) -> String {
    fmt_int(h.quantity)
}

fn fmt_price_krx(v: f64) -> String {
    fmt_int(v)
}

fn fmt_price_us(v: f64) -> String {
    format!("${}", fmt_dec2(v))
}

fn fmt_price_bond(v: f64) -> String {
    fmt_int(v)
}

fn display_name(h: &Holding) -> &str {
    if h.name.is_empty() { "-" } else { &h.name }
}

fn acct_tag(account: &str) -> String {
    if account.is_empty() { String::new() } else { format!(" [@{account}]") }
}

pub fn format_holding_line_no_price(h: &Holding) -> String {
    let name = display_name(h);
    let acct = acct_tag(&h.account);
    match h.market {
        Market::KRX | Market::CART => format!(
            "• {} {}{} | {} | 매입 {} | 현재가: -",
            h.symbol, name, acct, fmt_qty(h), fmt_price_krx(h.avg_price)
        ),
        Market::NAS | Market::NYS | Market::AMS => format!(
            "• {} {}{} | {} | 매입 {} | 현재가: -",
            h.symbol, name, acct, fmt_qty(h), fmt_price_us(h.avg_price)
        ),
        Market::BOND => format!(
            "• {} {}{} | {} | 매입 {} | 현재가: -",
            h.symbol, name, acct, fmt_qty(h), fmt_price_bond(h.avg_price)
        ),
    }
}

pub fn format_holding_line(h: &Holding, price: &PriceData, _usd_krw: f64) -> String {
    let pnl_pct = if h.quantity > 0.0 && h.avg_price > 0.0 {
        (price.current_price - h.avg_price) / h.avg_price * 100.0
    } else {
        0.0
    };
    let sign = if pnl_pct >= 0.0 { "+" } else { "" };
    let name = if !price.name.is_empty() {
        price.name.as_str()
    } else {
        display_name(h)
    };
    let acct = acct_tag(&h.account);

    match h.market {
        Market::KRX | Market::CART => format!(
            "• {} {}{} | {} | {}→{} | {sign}{:.1}%",
            h.symbol, name, acct, fmt_qty(h),
            fmt_price_krx(h.avg_price), fmt_price_krx(price.current_price), pnl_pct
        ),
        Market::NAS | Market::NYS | Market::AMS => format!(
            "• {} {}{} | {} | {}→{} | {sign}{:.1}%",
            h.symbol, name, acct, fmt_qty(h),
            fmt_price_us(h.avg_price), fmt_price_us(price.current_price), pnl_pct
        ),
        Market::BOND => format!(
            "• {} {}{} | {} | {}→{} | {sign}{:.1}%",
            h.symbol, name, acct, fmt_qty(h),
            fmt_price_bond(h.avg_price), fmt_price_bond(price.current_price), pnl_pct
        ),
    }
}

/// 캐시 가격으로 holding line 생성 (가격에 `⏱` 마커 추가)
pub fn format_holding_line_cached(h: &Holding, price: &PriceData, usd_krw: f64) -> String {
    let line = format_holding_line(h, price, usd_krw);
    format!("{line}⏱")
}

pub fn format_info(h: &Holding, price: &PriceData, signals: &[&Signal], usd_krw: f64) -> String {
    let factor = h.market.value_factor();
    let pnl = (price.current_price - h.avg_price) * h.quantity * factor;
    let pnl_pct = if h.quantity > 0.0 && h.avg_price > 0.0 {
        (price.current_price - h.avg_price) / h.avg_price * 100.0
    } else {
        0.0
    };
    let eval = price.current_price * h.quantity * factor;
    let sign = if price.change_pct >= 0.0 { "+" } else { "" };
    let pnl_sign = if pnl_pct >= 0.0 { "+" } else { "" };
    let name = if !price.name.is_empty() {
        price.name.as_str()
    } else {
        display_name(h)
    };
    let acct = acct_tag(&h.account);

    let mut msg = match h.market {
        Market::KRX | Market::CART => format!(
            "📈 {} {}{}\n현재가: {}원 (전일 대비 {sign}{:.1}%)\n매입가: {} × {}\n평가금액: {}원\n손익: {pnl_sign}{}원 ({pnl_sign}{:.1}%)",
            h.symbol, name, acct,
            fmt_price_krx(price.current_price), price.change_pct,
            fmt_price_krx(h.avg_price), fmt_qty(h),
            fmt_int(eval), fmt_int(pnl), pnl_pct,
        ),
        Market::NAS | Market::NYS | Market::AMS => format!(
            "📈 {} {}{}\n현재가: {} (전일 대비 {sign}{:.1}%)\n매입가: {} × {}\n평가금액: {} (약 {}원)\n손익: {pnl_sign}{} ({pnl_sign}{:.1}%)\n💱 USD/KRW: {}",
            h.symbol, name, acct,
            fmt_price_us(price.current_price), price.change_pct,
            fmt_price_us(h.avg_price), fmt_qty(h),
            fmt_price_us(eval), fmt_int(eval * usd_krw),
            fmt_price_us(pnl), pnl_pct,
            fmt_int(usd_krw),
        ),
        Market::BOND => format!(
            "📈 {} {}{}\n현재가: {}원 (전일 대비 {sign}{:.1}%)\n매입가: {} × {}\n평가금액: {}원\n손익: {pnl_sign}{}원 ({pnl_sign}{:.1}%)",
            h.symbol, name, acct,
            fmt_price_bond(price.current_price), price.change_pct,
            fmt_price_bond(h.avg_price), fmt_qty(h),
            fmt_int(eval), fmt_int(pnl), pnl_pct,
        ),
    };

    if !signals.is_empty() {
        msg.push_str("\n\n⚡ 설정된 시그널:");
        for s in signals {
            msg.push_str(&format!("\n• {} → 알림", format_condition(&s.condition, &h.market)));
        }
    }

    msg
}

pub fn format_condition(cond: &Condition, market: &Market) -> String {
    match cond {
        Condition::PriceAbove { target } => format!("가격 ≥ {}", fmt_price(market, *target)),
        Condition::PriceBelow { target } => format!("가격 ≤ {}", fmt_price(market, *target)),
        Condition::ProfitAbove { percentage } => format!("수익률 ≥ {percentage}%"),
        Condition::ProfitBelow { percentage } => format!("수익률 ≤ {percentage}%"),
    }
}

pub fn format_signal_alert(
    market: &Market,
    symbol: &str,
    name: &str,
    account: &str,
    condition_desc: &str,
    current_price: f64,
    change_pct: f64,
    avg_price: Option<f64>,
) -> String {
    let sign = if change_pct >= 0.0 { "+" } else { "" };
    let name_part = if name.is_empty() { String::new() } else { format!(" {name}") };
    let acct = acct_tag(account);

    let mut msg = format!(
        "🚨 시그널 발동!\n{}{}{}\n조건: {}\n현재가: {} ({sign}{:.1}%)",
        symbol, name_part, acct, condition_desc, fmt_price(market, current_price), change_pct,
    );

    if let Some(avg) = avg_price {
        let profit_pct = if avg > 0.0 {
            (current_price - avg) / avg * 100.0
        } else {
            0.0
        };
        let ps = if profit_pct >= 0.0 { "+" } else { "" };
        msg.push_str(&format!(
            "\n\n💡 매입가: {} | 수익률: {ps}{:.1}%",
            fmt_price(market, avg), profit_pct
        ));
    }

    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    fn make_holding(market: Market, symbol: &str, qty: f64, avg: f64) -> Holding {
        Holding {
            market,
            symbol: symbol.into(),
            name: String::new(),
            account: String::new(),
            quantity: qty,
            avg_price: avg,
            added_at: Utc::now().with_timezone(&FixedOffset::east_opt(9 * 3600).unwrap()),
            cached_price: None,
            cached_at: None,
        }
    }

    fn make_price(name: &str, current: f64, change_pct: f64) -> PriceData {
        PriceData {
            name: name.into(),
            current_price: current,
            change_pct,
        }
    }

    #[test]
    fn fmt_int_comma() {
        assert_eq!(fmt_int(70000.0), "70,000");
        assert_eq!(fmt_int(1234567.0), "1,234,567");
        assert_eq!(fmt_int(500.0), "500");
        assert_eq!(fmt_int(0.0), "0");
        assert_eq!(fmt_int(-1234.0), "-1,234");
    }

    #[test]
    fn fmt_dec2_comma() {
        assert_eq!(fmt_dec2(1350.50), "1,350.50");
        assert_eq!(fmt_dec2(180.5), "180.50");
        assert_eq!(fmt_dec2(0.99), "0.99");
    }

    #[test]
    fn format_krx_holding() {
        let h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        let p = make_price("삼성전자", 72500.0, 1.2);
        let line = format_holding_line(&h, &p, 1350.0);
        assert!(line.contains("005930"));
        assert!(line.contains("삼성전자"));
        assert!(line.contains("| 10 |"));
        assert!(!line.contains("주"));
        assert!(line.contains("70,000→72,500"));
        assert!(line.contains("+3.6%"));
    }

    #[test]
    fn format_us_holding() {
        let h = make_holding(Market::NAS, "TSLA", 5.0, 180.5);
        let p = make_price("테슬라", 195.2, 2.0);
        let line = format_holding_line(&h, &p, 1350.0);
        assert!(line.contains("$180.50"));
        assert!(line.contains("$195.20"));
        assert!(!line.contains("주"));
    }

    #[test]
    fn format_bond_holding() {
        let h = make_holding(Market::BOND, "KR103502G9C8", 100.0, 9850.0);
        let p = make_price("", 9920.0, 0.5);
        let line = format_holding_line(&h, &p, 1350.0);
        assert!(line.contains("- |")); // name empty → "-"
        assert!(line.contains("100"));
        assert!(line.contains("9,850→9,920"));
        assert!(!line.contains("주"));
    }

    #[test]
    fn format_no_price_krx() {
        let h = make_holding(Market::KRX, "005930", 1000.0, 70000.0);
        let line = format_holding_line_no_price(&h);
        assert!(line.contains("005930 -"));
        assert!(line.contains("1,000"));
        assert!(line.contains("70,000"));
        assert!(line.contains("현재가: -"));
        assert!(!line.contains("주"));
    }

    #[test]
    fn format_no_price_with_cached_name() {
        let mut h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        h.name = "삼성전자".into();
        let line = format_holding_line_no_price(&h);
        assert!(line.contains("005930 삼성전자"));
        assert!(line.contains("현재가: -"));
    }

    #[test]
    fn format_alert_krx_with_avg_price() {
        let msg = format_signal_alert(&Market::KRX, "005930", "삼성전자", "", "가격 ≥ 80,000", 80500.0, 1.2, Some(70000.0));
        assert!(msg.contains("시그널 발동"));
        assert!(msg.contains("삼성전자"));
        assert!(msg.contains("80,500"));
        assert!(msg.contains("매입가"));
        assert!(msg.contains("70,000"));
    }

    #[test]
    fn format_alert_us_with_dollar() {
        let msg = format_signal_alert(&Market::NAS, "TSLA", "테슬라", "", "가격 ≥ $200.00", 205.5, 3.0, Some(180.5));
        assert!(msg.contains("$205.50"));
        assert!(msg.contains("$180.50"));
    }

    #[test]
    fn format_alert_without_avg_price() {
        let msg = format_signal_alert(&Market::NAS, "TSLA", "", "", "가격 ≥ $200.00", 205.0, 3.0, None);
        assert!(msg.contains("TSLA"));
        assert!(msg.contains("$205.00"));
        assert!(!msg.contains("매입가"));
    }

    #[test]
    fn format_condition_krx() {
        assert_eq!(
            format_condition(&Condition::PriceAbove { target: 80000.0 }, &Market::KRX),
            "가격 ≥ 80,000"
        );
    }

    #[test]
    fn format_condition_us() {
        assert_eq!(
            format_condition(&Condition::PriceAbove { target: 200.5 }, &Market::NAS),
            "가격 ≥ $200.50"
        );
        assert_eq!(
            format_condition(&Condition::ProfitBelow { percentage: -10.0 }, &Market::NAS),
            "수익률 ≤ -10%"
        );
    }

    #[test]
    fn format_condition_bond() {
        assert_eq!(
            format_condition(&Condition::PriceBelow { target: 9500.0 }, &Market::BOND),
            "가격 ≤ 9,500"
        );
    }

    #[test]
    fn format_holding_line_cached_marker() {
        let h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        let p = make_price("삼성전자", 72500.0, 0.0);
        let line = format_holding_line_cached(&h, &p, 1350.0);
        assert!(line.ends_with('⏱'));
        assert!(line.contains("70,000→72,500"));
    }

    #[test]
    fn format_holding_line_with_account() {
        let mut h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        h.account = "IRP".into();
        let p = make_price("삼성전자", 72500.0, 1.2);
        let line = format_holding_line(&h, &p, 1350.0);
        assert!(line.contains("[@IRP]"));
    }

    #[test]
    fn format_holding_line_cart() {
        let mut h = make_holding(Market::CART, "비트코인", 2.0, 50000000.0);
        h.name = "비트코인".into();
        let p = make_price("비트코인", 55000000.0, 0.0);
        let line = format_holding_line(&h, &p, 0.0);
        assert!(line.contains("비트코인"));
        assert!(line.contains("50,000,000→55,000,000"));
        assert!(!line.contains("$")); // CART는 원화
    }

    #[test]
    fn format_no_price_us() {
        let h = make_holding(Market::NAS, "TSLA", 5.0, 180.5);
        let line = format_holding_line_no_price(&h);
        assert!(line.contains("$180.50"));
        assert!(line.contains("현재가: -"));
    }

    #[test]
    fn format_holding_line_negative_pnl() {
        let h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        let p = make_price("삼성전자", 65000.0, -2.5);
        let line = format_holding_line(&h, &p, 1350.0);
        assert!(line.contains("-7.1%"));
    }

    #[test]
    fn format_info_krx() {
        let h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        let p = make_price("삼성전자", 72500.0, 1.2);
        let msg = format_info(&h, &p, &[], 0.0);
        assert!(msg.contains("72,500원"));
        assert!(msg.contains("70,000 × 10"));
        assert!(msg.contains("725,000원")); // 평가금액
        assert!(msg.contains("+25,000원")); // 손익
        assert!(!msg.contains("USD/KRW"));
        assert!(!msg.contains("⚡")); // 시그널 없음
    }

    #[test]
    fn format_info_us() {
        let h = make_holding(Market::NAS, "TSLA", 5.0, 180.5);
        let p = make_price("테슬라", 195.2, 2.0);
        let msg = format_info(&h, &p, &[], 1450.0);
        assert!(msg.contains("$195.20"));
        assert!(msg.contains("$180.50 × 5"));
        assert!(msg.contains("$976.00")); // eval
        assert!(msg.contains("USD/KRW"));
        assert!(msg.contains("1,450"));
    }

    #[test]
    fn format_info_bond() {
        let h = make_holding(Market::BOND, "KR103502G990", 50000.0, 7435.0);
        let p = make_price("국고01125", 7485.0, 0.5);
        let msg = format_info(&h, &p, &[], 0.0);
        assert!(msg.contains("7,485원")); // 현재가
        assert!(msg.contains("7,435 × 50,000")); // 매입가 × 수량
        // 평가금액: 7485 * 50000 * 0.1 = 37,425,000
        assert!(msg.contains("37,425,000원"));
    }

    #[test]
    fn format_info_with_signals() {
        use crate::models::signal::Signal;
        let h = make_holding(Market::KRX, "005930", 10.0, 70000.0);
        let p = make_price("삼성전자", 72500.0, 1.2);
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let sig = Signal {
            id: "test".into(),
            symbol: "005930".into(),
            account: String::new(),
            condition: Condition::PriceAbove { target: 80000.0 },
            active: true,
            created_at: Utc::now().with_timezone(&kst),
        };
        let signals: Vec<&Signal> = vec![&sig];
        let msg = format_info(&h, &p, &signals, 0.0);
        assert!(msg.contains("⚡ 설정된 시그널:"));
        assert!(msg.contains("가격 ≥ 80,000"));
    }

    #[test]
    fn format_alert_with_account() {
        let msg = format_signal_alert(&Market::KRX, "005930", "삼성전자", "IRP", "가격 ≥ 80,000", 80500.0, 1.2, None);
        assert!(msg.contains("[@IRP]"));
    }

    #[test]
    fn fmt_price_dispatching() {
        assert_eq!(fmt_price(&Market::KRX, 70000.0), "70,000");
        assert_eq!(fmt_price(&Market::NAS, 195.2), "$195.20");
        assert_eq!(fmt_price(&Market::NYS, 195.2), "$195.20");
        assert_eq!(fmt_price(&Market::AMS, 195.2), "$195.20");
        assert_eq!(fmt_price(&Market::BOND, 9850.0), "9,850");
        assert_eq!(fmt_price(&Market::CART, 55000000.0), "55,000,000");
    }

    #[test]
    fn fmt_dec2_negative() {
        assert_eq!(fmt_dec2(-73.5), "-73.50");
    }
}
