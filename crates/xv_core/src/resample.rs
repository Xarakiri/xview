use crate::{Candle, Timeframe, TzOffset};

/// Aggregates candles (sorted by time) into `to` buckets in the given timezone.
pub fn resample(candles: &[Candle], to: Timeframe, tz: TzOffset) -> Vec<Candle> {
    let mut out: Vec<Candle> = Vec::new();
    for c in candles {
        let key = to.bucket_start(c.open_time_ms, tz);
        match out.last_mut() {
            Some(b) if b.open_time_ms == key => {
                b.high = b.high.max(c.high);
                b.low = b.low.min(c.low);
                b.close = c.close;
                b.volume += c.volume;
            }
            _ => out.push(Candle { open_time_ms: key, ..*c }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn m(i: i64, t: i64) -> Candle {
        let f = i as f64;
        Candle { open_time_ms: t, open: f, high: f + 1.0, low: f - 1.0, close: f + 0.5, volume: 1.0 }
    }

    #[test]
    fn resample_1m_to_5m() {
        let cs: Vec<_> = (0..10).map(|i| m(i, i * MIN)).collect();
        let r = resample(&cs, Timeframe::M5, TzOffset::UTC);
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[0],
            Candle { open_time_ms: 0, open: 0.0, high: 5.0, low: -1.0, close: 4.5, volume: 5.0 }
        );
        assert_eq!(r[1].open_time_ms, 5 * MIN);
        assert_eq!(r[1].open, 5.0);
    }

    #[test]
    fn resample_respects_tz_for_daily() {
        let h = 3_600_000;
        let cs = vec![m(0, 18 * h), m(1, 20 * h)];
        assert_eq!(resample(&cs, Timeframe::D1, TzOffset::UTC).len(), 1);
        assert_eq!(resample(&cs, Timeframe::D1, TzOffset { seconds: 5 * 3600 }).len(), 2);
    }
}
