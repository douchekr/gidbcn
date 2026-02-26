use crate::models::messages::DailyCandle;
use crate::models::signal::Condition;

/// 기술적 시그널 평가: golden_cross, dead_cross, RSI
pub fn evaluate(condition: &Condition, candles: &[DailyCandle]) -> bool {
    match condition {
        Condition::GoldenCross {
            short_period,
            long_period,
        } => check_golden_cross(candles, *short_period as usize, *long_period as usize),
        Condition::DeadCross {
            short_period,
            long_period,
        } => check_dead_cross(candles, *short_period as usize, *long_period as usize),
        Condition::RsiAbove { threshold } => {
            let rsi = calc_rsi(candles, 14);
            rsi >= *threshold
        }
        Condition::RsiBelow { threshold } => {
            let rsi = calc_rsi(candles, 14);
            rsi <= *threshold
        }
        _ => false,
    }
}

fn sma(candles: &[DailyCandle], period: usize) -> Option<f64> {
    if candles.len() < period {
        return None;
    }
    let sum: f64 = candles[..period].iter().map(|c| c.close).sum();
    Some(sum / period as f64)
}

fn check_golden_cross(candles: &[DailyCandle], short: usize, long: usize) -> bool {
    // 최소 long+1 개 캔들 필요 (오늘 + 어제 비교)
    if candles.len() < long + 1 {
        return false;
    }

    // candles[0] = 최신 (오늘), candles[1] = 어제
    let today_short = sma(candles, short);
    let today_long = sma(candles, long);
    let yesterday_short = sma(&candles[1..], short);
    let yesterday_long = sma(&candles[1..], long);

    match (today_short, today_long, yesterday_short, yesterday_long) {
        (Some(ts), Some(tl), Some(ys), Some(yl)) => {
            // 어제: short <= long, 오늘: short > long → 상향돌파
            ys <= yl && ts > tl
        }
        _ => false,
    }
}

fn check_dead_cross(candles: &[DailyCandle], short: usize, long: usize) -> bool {
    if candles.len() < long + 1 {
        return false;
    }

    let today_short = sma(candles, short);
    let today_long = sma(candles, long);
    let yesterday_short = sma(&candles[1..], short);
    let yesterday_long = sma(&candles[1..], long);

    match (today_short, today_long, yesterday_short, yesterday_long) {
        (Some(ts), Some(tl), Some(ys), Some(yl)) => {
            // 어제: short >= long, 오늘: short < long → 하향돌파
            ys >= yl && ts < tl
        }
        _ => false,
    }
}

fn calc_rsi(candles: &[DailyCandle], period: usize) -> f64 {
    if candles.len() < period + 1 {
        return 50.0; // 데이터 부족 시 중립
    }

    let mut gains = 0.0;
    let mut losses = 0.0;

    // candles[0] = 최신, 역순이므로 i번째가 i+1번째보다 최신
    for i in 0..period {
        let diff = candles[i].close - candles[i + 1].close;
        if diff > 0.0 {
            gains += diff;
        } else {
            losses += diff.abs();
        }
    }

    let avg_gain = gains / period as f64;
    let avg_loss = losses / period as f64;

    if avg_loss == 0.0 {
        return 100.0;
    }

    let rs = avg_gain / avg_loss;
    100.0 - (100.0 / (1.0 + rs))
}
