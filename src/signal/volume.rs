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
