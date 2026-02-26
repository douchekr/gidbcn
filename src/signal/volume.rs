use crate::models::messages::DailyCandle;
use crate::models::signal::Condition;

/// 거래량 시그널 평가: volume_surge
pub fn evaluate(condition: &Condition, candles: &[DailyCandle], current_volume: u64) -> bool {
    match condition {
        Condition::VolumeSurge { threshold_pct } => {
            check_volume_surge(candles, current_volume, *threshold_pct)
        }
        _ => false,
    }
}

fn check_volume_surge(candles: &[DailyCandle], current_volume: u64, threshold_pct: f64) -> bool {
    // 20일 평균 거래량 계산
    let period = 20.min(candles.len());
    if period == 0 {
        return false;
    }

    let avg_volume: f64 =
        candles[..period].iter().map(|c| c.volume as f64).sum::<f64>() / period as f64;

    if avg_volume <= 0.0 {
        return false;
    }

    let ratio = current_volume as f64 / avg_volume * 100.0;
    ratio >= threshold_pct
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candle(volume: u64) -> DailyCandle {
        DailyCandle {
            date: String::new(),
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            volume,
        }
    }

    #[test]
    fn volume_surge_triggered() {
        // 20일 평균 1000, 현재 2500 → 250% ≥ 200%
        let candles: Vec<_> = (0..20).map(|_| make_candle(1000)).collect();
        let cond = Condition::VolumeSurge { threshold_pct: 200.0 };
        assert!(evaluate(&cond, &candles, 2500));
    }

    #[test]
    fn volume_surge_not_triggered() {
        let candles: Vec<_> = (0..20).map(|_| make_candle(1000)).collect();
        let cond = Condition::VolumeSurge { threshold_pct: 200.0 };
        assert!(!evaluate(&cond, &candles, 1500)); // 150% < 200%
    }

    #[test]
    fn volume_surge_empty_candles() {
        let cond = Condition::VolumeSurge { threshold_pct: 200.0 };
        assert!(!evaluate(&cond, &[], 5000));
    }

    #[test]
    fn volume_surge_fewer_than_20_candles() {
        let candles: Vec<_> = (0..5).map(|_| make_candle(1000)).collect();
        let cond = Condition::VolumeSurge { threshold_pct: 200.0 };
        assert!(evaluate(&cond, &candles, 2000)); // 200% ≥ 200%
    }
}
