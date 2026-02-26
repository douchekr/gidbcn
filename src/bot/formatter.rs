use crate::models::messages::PriceData;
use crate::models::portfolio::{Holding, Market};
use crate::models::signal::Signal;

pub fn format_holding_line(h: &Holding, price: &PriceData, _usd_krw: f64) -> String {
    let pnl_pct = if h.avg_price > 0.0 {
        (price.current_price - h.avg_price) / h.avg_price * 100.0
    } else {
        0.0
    };
    let sign = if pnl_pct >= 0.0 { "+" } else { "" };

    match h.market {
        Market::KRX => {
            let name = if price.name.is_empty() {
                &h.symbol
            } else {
                &price.name
            };
            format!(
                "• {} {} | {}주 | {:.0}→{:.0} | {sign}{:.1}%",
                h.symbol, name, h.quantity, h.avg_price, price.current_price, pnl_pct
            )
        }
        Market::NAS | Market::NYS | Market::AMS => {
            let name = if price.name.is_empty() {
                &h.symbol
            } else {
                &price.name
            };
            format!(
                "• {} {} | {}주 | ${:.2}→${:.2} | {sign}{:.1}%",
                h.symbol, name, h.quantity, h.avg_price, price.current_price, pnl_pct
            )
        }
        Market::BOND => {
            format!(
                "• {} | {} | {:.0}→{:.0} | {sign}{:.1}%",
                h.symbol, h.quantity, h.avg_price, price.current_price, pnl_pct
            )
        }
    }
}

pub fn format_info(h: &Holding, price: &PriceData, signals: &[&Signal]) -> String {
    let pnl = (price.current_price - h.avg_price) * h.quantity;
    let pnl_pct = if h.avg_price > 0.0 {
        (price.current_price - h.avg_price) / h.avg_price * 100.0
    } else {
        0.0
    };
    let eval = price.current_price * h.quantity;

    let (currency, price_fmt) = if h.market.is_domestic() {
        ("원", format!("{:.0}", price.current_price))
    } else {
        ("$", format!("{:.2}", price.current_price))
    };

    let sign = if price.change_pct >= 0.0 { "+" } else { "" };
    let name = if price.name.is_empty() {
        &h.symbol
    } else {
        &price.name
    };

    let mut msg = format!(
        "📈 {} {}\n현재가: {}{} (전일 대비 {sign}{:.1}%)\n매입가: {:.0} × {}주\n평가금액: {:.0}{}\n손익: {sign}{:.0}{} ({sign}{:.1}%)",
        h.symbol,
        name,
        price_fmt,
        currency,
        price.change_pct,
        h.avg_price,
        h.quantity,
        eval,
        currency,
        pnl,
        currency,
        pnl_pct,
    );

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
        "🚨 시그널 발동!\n{} {}\n조건: {}\n현재가: {:.0} ({sign}{:.1}%)",
        symbol, display_name, condition_desc, current_price, change_pct,
    );

    if let Some(avg) = avg_price {
        let profit_pct = if avg > 0.0 {
            (current_price - avg) / avg * 100.0
        } else {
            0.0
        };
        let ps = if profit_pct >= 0.0 { "+" } else { "" };
        msg.push_str(&format!(
            "\n\n💡 매입가: {:.0} | 수익률: {ps}{:.1}%",
            avg, profit_pct
        ));
    }

    msg
}
