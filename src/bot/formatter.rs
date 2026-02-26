use crate::models::messages::PriceData;
use crate::models::portfolio::{Holding, Market};
use crate::models::signal::Signal;

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

pub fn format_holding_line_no_price(h: &Holding) -> String {
    let name = display_name(h);
    match h.market {
        Market::KRX => format!(
            "• {} {} | {} | 매입 {} | 현재가: -",
            h.symbol, name, fmt_qty(h), fmt_price_krx(h.avg_price)
        ),
        Market::NAS | Market::NYS | Market::AMS => format!(
            "• {} {} | {} | 매입 {} | 현재가: -",
            h.symbol, name, fmt_qty(h), fmt_price_us(h.avg_price)
        ),
        Market::BOND => format!(
            "• {} {} | {} | 매입 {} | 현재가: -",
            h.symbol, name, fmt_qty(h), fmt_price_bond(h.avg_price)
        ),
    }
}

pub fn format_holding_line(h: &Holding, price: &PriceData, _usd_krw: f64) -> String {
    let pnl_pct = if h.avg_price > 0.0 {
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

    match h.market {
        Market::KRX => format!(
            "• {} {} | {} | {}→{} | {sign}{:.1}%",
            h.symbol, name, fmt_qty(h),
            fmt_price_krx(h.avg_price), fmt_price_krx(price.current_price), pnl_pct
        ),
        Market::NAS | Market::NYS | Market::AMS => format!(
            "• {} {} | {} | {}→{} | {sign}{:.1}%",
            h.symbol, name, fmt_qty(h),
            fmt_price_us(h.avg_price), fmt_price_us(price.current_price), pnl_pct
        ),
        Market::BOND => format!(
            "• {} {} | {} | {}→{} | {sign}{:.1}%",
            h.symbol, name, fmt_qty(h),
            fmt_price_bond(h.avg_price), fmt_price_bond(price.current_price), pnl_pct
        ),
    }
}

/// 캐시 가격으로 holding line 생성 (가격에 `*` 마커 추가)
pub fn format_holding_line_cached(h: &Holding, price: &PriceData, usd_krw: f64) -> String {
    let line = format_holding_line(h, price, usd_krw);
    // 마지막 `%` 뒤에 `*` 추가: "…| +3.6%" → "…| +3.6%*"
    format!("{line}*")
}

pub fn format_info(h: &Holding, price: &PriceData, signals: &[&Signal]) -> String {
    let pnl = (price.current_price - h.avg_price) * h.quantity;
    let pnl_pct = if h.avg_price > 0.0 {
        (price.current_price - h.avg_price) / h.avg_price * 100.0
    } else {
        0.0
    };
    let eval = price.current_price * h.quantity;
    let sign = if price.change_pct >= 0.0 { "+" } else { "" };
    let pnl_sign = if pnl_pct >= 0.0 { "+" } else { "" };
    let name = if !price.name.is_empty() {
        price.name.as_str()
    } else {
        display_name(h)
    };

    let mut msg = match h.market {
        Market::KRX => format!(
            "📈 {} {}\n현재가: {}원 (전일 대비 {sign}{:.1}%)\n매입가: {} × {}\n평가금액: {}원\n손익: {pnl_sign}{}원 ({pnl_sign}{:.1}%)",
            h.symbol, name,
            fmt_price_krx(price.current_price), price.change_pct,
            fmt_price_krx(h.avg_price), fmt_qty(h),
            fmt_int(eval), fmt_int(pnl), pnl_pct,
        ),
        Market::NAS | Market::NYS | Market::AMS => format!(
            "📈 {} {}\n현재가: {} (전일 대비 {sign}{:.1}%)\n매입가: {} × {}\n평가금액: {}\n손익: {pnl_sign}{} ({pnl_sign}{:.1}%)",
            h.symbol, name,
            fmt_price_us(price.current_price), price.change_pct,
            fmt_price_us(h.avg_price), fmt_qty(h),
            fmt_price_us(eval), fmt_price_us(pnl), pnl_pct,
        ),
        Market::BOND => format!(
            "📈 {} {}\n현재가: {}원 (전일 대비 {sign}{:.1}%)\n매입가: {} × {}\n평가금액: {}원\n손익: {pnl_sign}{}원 ({pnl_sign}{:.1}%)",
            h.symbol, name,
            fmt_price_bond(price.current_price), price.change_pct,
            fmt_price_bond(h.avg_price), fmt_qty(h),
            fmt_int(eval), fmt_int(pnl), pnl_pct,
        ),
    };

    if !signals.is_empty() {
        msg.push_str("\n\n⚡ 설정된 시그널:");
        for s in signals {
            msg.push_str(&format!("\n• {} → 알림", s.condition.display_description()));
        }
    }

    msg
}

pub fn format_signal_alert(
    symbol: &str,
    name: &str,
    condition_desc: &str,
    current_price: f64,
    change_pct: f64,
    avg_price: Option<f64>,
) -> String {
    let sign = if change_pct >= 0.0 { "+" } else { "" };
    let display_name = if name.is_empty() { symbol } else { name };

    let mut msg = format!(
        "🚨 시그널 발동!\n{} {}\n조건: {}\n현재가: {} ({sign}{:.1}%)",
        symbol, display_name, condition_desc, fmt_int(current_price), change_pct,
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
            fmt_int(avg), profit_pct
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
            change: 0.0,
            change_pct,
            volume: 0,
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
    fn format_alert_with_avg_price() {
        let msg = format_signal_alert("005930", "삼성전자", "가격 ≥ 80,000", 80500.0, 1.2, Some(70000.0));
        assert!(msg.contains("시그널 발동"));
        assert!(msg.contains("삼성전자"));
        assert!(msg.contains("80,500"));
        assert!(msg.contains("매입가"));
        assert!(msg.contains("70,000"));
    }

    #[test]
    fn format_alert_without_avg_price() {
        let msg = format_signal_alert("TSLA", "", "가격 ≥ 200", 205.0, 3.0, None);
        assert!(msg.contains("TSLA"));
        assert!(!msg.contains("매입가"));
    }
}
