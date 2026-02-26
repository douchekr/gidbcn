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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candle(close: f64, volume: u64) -> DailyCandle {
        DailyCandle {
            date: String::new(),
            open: close,
            high: close,
            low: close,
            close,
            volume,
        }
    }

    #[test]
    fn sma_basic() {
        let candles = vec![make_candle(30.0, 0), make_candle(20.0, 0), make_candle(10.0, 0)];
        assert_eq!(sma(&candles, 3), Some(20.0));
        assert_eq!(sma(&candles, 2), Some(25.0)); // (30+20)/2
        assert_eq!(sma(&candles, 4), None);
    }

    #[test]
    fn golden_cross_detected() {
        // short=2, long=3
        // 오늘 candles[0..]: short_ma=(110+90)/2=100, long_ma=(110+90+80)/3=93.3 → short > long
        // 어제 candles[1..]: short_ma=(90+80)/2=85,  long_ma=(90+80+100)/3=90  → short < long
        let candles = vec![
            make_candle(110.0, 0), // 오늘
            make_candle(90.0, 0),
            make_candle(80.0, 0),
            make_candle(100.0, 0), // 가장 오래된
        ];
        assert!(check_golden_cross(&candles, 2, 3));
    }

    #[test]
    fn golden_cross_not_detected() {
        // 하락 추세 — 골든크로스 아님
        let candles = vec![
            make_candle(80.0, 0),
            make_candle(90.0, 0),
            make_candle(100.0, 0),
            make_candle(110.0, 0),
        ];
        assert!(!check_golden_cross(&candles, 2, 3));
    }

    #[test]
    fn dead_cross_detected() {
        // short=2, long=3
        // 오늘 candles[0..]: short_ma=(80+90)/2=85, long_ma=(80+90+70)/3=80 → short > long? No
        // 어제 candles[1..]: short_ma=(90+70)/2=80, long_ma=(90+70+60)/3=73.3 → short > long
        // 이건 데드크로스 아님. 다시 설계.
        //
        // 오늘: short < long, 어제: short >= long
        // candles[0..]: short=(70+100)/2=85, long=(70+100+90)/3=86.7 → short<long ✓
        // candles[1..]: short=(100+90)/2=95, long=(100+90+80)/3=90   → short>long ✓
        let candles = vec![
            make_candle(70.0, 0),
            make_candle(100.0, 0),
            make_candle(90.0, 0),
            make_candle(80.0, 0),
        ];
        assert!(check_dead_cross(&candles, 2, 3));
    }

    #[test]
    fn rsi_all_gains() {
        // 15일 연속 상승
        let candles: Vec<_> = (0..16).rev().map(|i| make_candle(100.0 + i as f64, 0)).collect();
        let rsi = calc_rsi(&candles, 14);
        assert_eq!(rsi, 100.0);
    }

    #[test]
    fn rsi_all_losses() {
        // candles[0]=최신=85, candles[15]=가장 오래된=100 (오름차순)
        // diff = candles[i] - candles[i+1] < 0 → 전부 loss
        let candles: Vec<_> = (0..16).map(|i| make_candle(85.0 + i as f64, 0)).collect();
        let rsi = calc_rsi(&candles, 14);
        assert!(rsi < 1.0, "rsi was {rsi}");
    }

    #[test]
    fn rsi_insufficient_data() {
        let candles = vec![make_candle(100.0, 0); 5];
        assert_eq!(calc_rsi(&candles, 14), 50.0);
    }

    #[test]
    fn evaluate_rsi_above() {
        let candles: Vec<_> = (0..16).rev().map(|i| make_candle(100.0 + i as f64, 0)).collect();
        let cond = Condition::RsiAbove { threshold: 70.0 };
        assert!(evaluate(&cond, &candles));
    }

    #[test]
    fn evaluate_rsi_below() {
        // 연속 하락: candles[0]=최신=최저
        let candles: Vec<_> = (0..16).map(|i| make_candle(85.0 + i as f64, 0)).collect();
        let cond = Condition::RsiBelow { threshold: 30.0 };
        assert!(evaluate(&cond, &candles));
    }
}
