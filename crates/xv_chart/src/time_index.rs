use xv_core::{Candle, Timeframe, TzOffset};

fn bar_end(candles: &[Candle], tf: Timeframe, i: usize) -> i64 {
    let t = candles[i].open_time_ms;
    let end = tf.next_bucket(t, TzOffset::UTC);
    match candles.get(i + 1) {
        Some(n) => end.min(n.open_time_ms),
        None => end,
    }
}

/// Fractional bar index for a timestamp. Gaps map to the next bar's start.
pub fn time_to_index(candles: &[Candle], tf: Timeframe, ts_ms: i64) -> f64 {
    let nominal = tf.nominal_ms() as f64;
    let n = candles.len();
    if n == 0 {
        return ts_ms as f64 / nominal;
    }
    let pos = candles.partition_point(|c| c.open_time_ms <= ts_ms);
    if pos == 0 {
        return (ts_ms - candles[0].open_time_ms) as f64 / nominal;
    }
    let i = pos - 1;
    if i == n - 1 {
        return i as f64 + (ts_ms - candles[i].open_time_ms) as f64 / nominal;
    }
    let t = candles[i].open_time_ms;
    let end = bar_end(candles, tf, i);
    if ts_ms < end {
        i as f64 + (ts_ms - t) as f64 / (end - t) as f64
    } else {
        (i + 1) as f64
    }
}

/// Inverse of [`time_to_index`] on non-gap times.
pub fn index_to_time(candles: &[Candle], tf: Timeframe, idx: f64) -> i64 {
    let nominal = tf.nominal_ms() as f64;
    let n = candles.len();
    if n == 0 {
        return (idx * nominal).round() as i64;
    }
    if idx < 0.0 {
        return candles[0].open_time_ms + (idx * nominal).round() as i64;
    }
    let last = (n - 1) as f64;
    if idx >= last {
        return candles[n - 1].open_time_ms + ((idx - last) * nominal).round() as i64;
    }
    let i = idx.floor() as usize;
    let t = candles[i].open_time_ms;
    let end = bar_end(candles, tf, i);
    t + ((idx - i as f64) * (end - t) as f64).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;
    const DAY0: i64 = 1_700_000_000_000 - 1_700_000_000_000 % (24 * H); // a UTC midnight

    fn bar(t: i64) -> Candle {
        Candle {
            open_time_ms: t,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 0.0,
        }
    }
    fn at(h: i64, m: i64) -> i64 {
        DAY0 + h * H + m * 60_000
    }
    fn hourly(hs: &[i64]) -> Vec<Candle> {
        hs.iter().map(|h| bar(at(*h, 0))).collect()
    }

    #[test]
    fn fractional_index_inside_bar() {
        let c = hourly(&[13, 14, 15]);
        let ts = at(14, 37);
        assert!((time_to_index(&c, Timeframe::H1, ts) - (1.0 + 37.0 / 60.0)).abs() < 1e-9);
    }

    #[test]
    fn gap_maps_to_next_bar_start() {
        let c = vec![bar(at(23, 0)), bar(at(23, 0) + 57 * H)];
        assert_eq!(time_to_index(&c, Timeframe::H1, at(23, 0) + 13 * H), 1.0);
    }

    #[test]
    fn extrapolates_both_sides_and_round_trips() {
        let c = hourly(&[13, 14, 15]);
        assert!((time_to_index(&c, Timeframe::H1, at(17, 30)) - 4.5).abs() < 1e-9);
        assert!((time_to_index(&c, Timeframe::H1, at(11, 0)) + 2.0).abs() < 1e-9);
        for ts in [at(13, 5), at(14, 59), at(20, 0), at(9, 15)] {
            assert_eq!(
                index_to_time(&c, Timeframe::H1, time_to_index(&c, Timeframe::H1, ts)),
                ts
            );
        }
    }

    #[test]
    fn empty_series_is_defined() {
        let i = time_to_index(&[], Timeframe::H1, 5 * H);
        assert!((i - 5.0).abs() < 1e-9);
        assert_eq!(index_to_time(&[], Timeframe::H1, i), 5 * H);
    }
}
