//! OHLC snapping for drawing anchors (the "magnet").

use crate::{Viewport, index_to_time};
use xv_core::{Candle, Timeframe};
use xv_drawings::Anchor;

/// Snaps a plot-relative point to the nearest bar (by x), then to the nearest
/// of that bar's open/high/low/close (by y). `time_ms` is the bar's open time.
/// With no candles the point is converted without snapping.
pub fn snap_anchor(candles: &[Candle], tf: Timeframe, vp: &Viewport, p: (f32, f32)) -> Anchor {
    if candles.is_empty() {
        return Anchor {
            time_ms: index_to_time(candles, tf, vp.index_of_x(p.0)),
            price: vp.price_of_y(p.1),
        };
    }
    let idx = vp
        .index_of_x(p.0)
        .round()
        .clamp(0.0, (candles.len() - 1) as f64) as usize;
    let c = &candles[idx];
    let cursor = vp.price_of_y(p.1);
    let price = [c.open, c.high, c.low, c.close]
        .into_iter()
        .min_by(|a, b| (a - cursor).abs().total_cmp(&(b - cursor).abs()))
        .unwrap_or(cursor);
    Anchor {
        time_ms: c.open_time_ms,
        price,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_nearest_bar_and_level() {
        let cs: Vec<Candle> = (0..10)
            .map(|i| Candle {
                open_time_ms: i * 60_000,
                open: 10.0,
                high: 14.0,
                low: 8.0,
                close: 12.0,
                volume: 0.0,
            })
            .collect();
        let mut vp = Viewport::new(100.0, 100.0);
        vp.bar_spacing = 10.0;
        vp.right_index = 10.0;
        vp.price = crate::PriceRange {
            min: 0.0,
            max: 20.0,
        };
        let p = (vp.x_of_index(3.0) + 2.0, vp.y_of_price(13.2));
        let a = snap_anchor(&cs, Timeframe::M1, &vp, p);
        assert_eq!((a.time_ms, a.price), (3 * 60_000, 14.0));
        let a = snap_anchor(&[], Timeframe::M1, &vp, p);
        assert!((a.price - 13.2).abs() < 1e-3);
    }
}
