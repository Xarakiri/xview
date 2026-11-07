use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    pub open_time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// Decimal places of `v` as printed (shortest round-trip form).
fn decimals(v: f64) -> u32 {
    let s = format!("{v}");
    s.split_once('.').map_or(0, |(_, frac)| frac.len() as u32)
}

/// Price step implied by the prices themselves: 10^-d for the most decimal
/// places any open/high/low/close shows, capped at 8. For sources that don't
/// state a tick size (CSV files, a symbol whose metadata failed to load).
pub fn infer_tick_size(candles: &[Candle]) -> f64 {
    let dec = candles
        .iter()
        .flat_map(|c| [c.open, c.high, c.low, c.close])
        .filter(|v| v.is_finite())
        .map(decimals)
        .max()
        .unwrap_or(0)
        .min(8);
    10f64.powi(-(dec as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_tick_size_takes_the_most_decimals() {
        let c = |o: f64, cl: f64| Candle { open_time_ms: 0, open: o, high: o, low: o, close: cl, volume: 0.0 };
        assert_eq!(infer_tick_size(&[c(76056.11, 76094.9)]), 0.01);
        assert_eq!(infer_tick_size(&[c(1.0, 2.0)]), 1.0);
        assert_eq!(infer_tick_size(&[c(0.000012345678912, 1.0)]), 1e-8);
        assert_eq!(infer_tick_size(&[]), 1.0);
    }

    #[test]
    fn candle_json_round_trip() {
        let c = Candle { open_time_ms: 1, open: 1.0, high: 2.0, low: 0.5, close: 1.5, volume: 3.0 };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Candle>(&s).unwrap(), c);
    }
}
