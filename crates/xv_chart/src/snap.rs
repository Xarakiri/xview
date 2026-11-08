//! Anchor snapping: the OHLC magnet, the cmd High/Low snap, and the shift
//! angle/square constraints (screen space, plot-relative pixels).

use crate::{Viewport, index_to_time};
use xv_core::{Candle, Timeframe};
use xv_drawings::Anchor;

/// How the magnet snaps when it is on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum MagnetMode {
    /// Snaps to the nearest O/H/L/C only within `MAGNET_WEAK_PX` vertically.
    #[default]
    Weak,
    /// Always snaps to the nearest O/H/L/C.
    Strong,
}

/// Vertical reach of the weak magnet, in pixels.
pub const MAGNET_WEAK_PX: f32 = 12.0;

/// The unsnapped anchor under a plot-relative point.
fn raw(candles: &[Candle], tf: Timeframe, vp: &Viewport, p: (f32, f32)) -> Anchor {
    Anchor {
        time_ms: index_to_time(candles, tf, vp.index_of_x(p.0)),
        price: vp.price_of_y(p.1),
    }
}

/// The bar nearest to `x`, clamped to the loaded candles.
fn bar_at<'a>(candles: &'a [Candle], vp: &Viewport, x: f32) -> Option<&'a Candle> {
    let last = candles.len().checked_sub(1)?;
    let idx = vp.index_of_x(x).round().clamp(0.0, last as f64) as usize;
    candles.get(idx)
}

/// The level of `levels` nearest to `cursor`.
fn nearest(levels: &[f64], cursor: f64) -> f64 {
    levels
        .iter()
        .copied()
        .min_by(|a, b| (a - cursor).abs().total_cmp(&(b - cursor).abs()))
        .unwrap_or(cursor)
}

/// Snaps to the nearest bar (by x), then to the nearer of its High and Low.
/// `time_ms` is the bar's open time. With no candles: the raw point.
pub fn snap_high_low(candles: &[Candle], tf: Timeframe, vp: &Viewport, p: (f32, f32)) -> Anchor {
    let Some(c) = bar_at(candles, vp, p.0) else {
        return raw(candles, tf, vp, p);
    };
    Anchor {
        time_ms: c.open_time_ms,
        price: nearest(&[c.high, c.low], vp.price_of_y(p.1)),
    }
}

/// Snaps to the nearest bar (by x), then to the nearest of its O/H/L/C.
/// `None` when there are no candles, or the level is more than `max_px`
/// away vertically.
pub fn snap_ohlc(
    candles: &[Candle],
    _tf: Timeframe,
    vp: &Viewport,
    p: (f32, f32),
    max_px: Option<f32>,
) -> Option<Anchor> {
    let c = bar_at(candles, vp, p.0)?;
    let price = nearest(&[c.open, c.high, c.low, c.close], vp.price_of_y(p.1));
    if let Some(max) = max_px
        && (vp.y_of_price(price) - p.1).abs() > max
    {
        return None;
    }
    Some(Anchor {
        time_ms: c.open_time_ms,
        price,
    })
}

/// Snaps a plot-relative point to the nearest bar (by x), then to the nearest
/// of that bar's open/high/low/close (by y). `time_ms` is the bar's open time.
/// With no candles the point is converted without snapping.
pub fn snap_anchor(candles: &[Candle], tf: Timeframe, vp: &Viewport, p: (f32, f32)) -> Anchor {
    snap_ohlc(candles, tf, vp, p, None).unwrap_or_else(|| raw(candles, tf, vp, p))
}

/// Projects `moving` onto the nearest multiple of 45 degrees from `fixed`.
/// Horizontal and vertical results share `fixed`'s y or x exactly. A zero
/// length returns `moving` unchanged.
pub fn constrain_angle(fixed: (f32, f32), moving: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (moving.0 - fixed.0, moving.1 - fixed.1);
    if dx == 0.0 && dy == 0.0 {
        return moving;
    }
    let k = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round() as i32;
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let (ux, uy) = match k.rem_euclid(8) {
        0 => (1.0, 0.0),
        1 => (s, s),
        2 => (0.0, 1.0),
        3 => (-s, s),
        4 => (-1.0, 0.0),
        5 => (-s, -s),
        6 => (0.0, -1.0),
        _ => (s, -s),
    };
    let t = dx * ux + dy * uy;
    (fixed.0 + t * ux, fixed.1 + t * uy)
}

/// Makes `fixed`-`moving` a square: the longer side wins, signs are kept.
pub fn constrain_square(fixed: (f32, f32), moving: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (moving.0 - fixed.0, moving.1 - fixed.1);
    let side = dx.abs().max(dy.abs());
    let sign = |d: f32| if d < 0.0 { -1.0 } else { 1.0 };
    (fixed.0 + side * sign(dx), fixed.1 + side * sign(dy))
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

    fn setup() -> (Vec<Candle>, Viewport) {
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
        // 5 px per price unit.
        let mut vp = Viewport::new(100.0, 100.0);
        vp.bar_spacing = 10.0;
        vp.right_index = 10.0;
        vp.price = crate::PriceRange {
            min: 0.0,
            max: 20.0,
        };
        (cs, vp)
    }

    #[test]
    fn cmd_snaps_to_high_or_low_only() {
        let (cs, vp) = setup();
        let x = vp.x_of_index(3.0) + 2.0;
        // Nearest level is the close (12), but above the high/low midpoint.
        let a = snap_high_low(&cs, Timeframe::M1, &vp, (x, vp.y_of_price(11.9)));
        assert_eq!((a.time_ms, a.price), (3 * 60_000, 14.0));
        // Nearest level is the open (10), below the midpoint.
        let a = snap_high_low(&cs, Timeframe::M1, &vp, (x, vp.y_of_price(10.4)));
        assert_eq!(a.price, 8.0);
        let a = snap_high_low(&[], Timeframe::M1, &vp, (x, vp.y_of_price(10.4)));
        assert!((a.price - 10.4).abs() < 1e-3);
    }

    #[test]
    fn weak_magnet_threshold() {
        let (cs, vp) = setup();
        let x = vp.x_of_index(3.0);
        // 8 px from the high snaps.
        let p = (x, vp.y_of_price(15.6));
        let a = snap_ohlc(&cs, Timeframe::M1, &vp, p, Some(MAGNET_WEAK_PX)).unwrap();
        assert_eq!(a.price, 14.0);
        // 20 px from the high does not, unless unlimited.
        let p = (x, vp.y_of_price(18.0));
        assert_eq!(
            snap_ohlc(&cs, Timeframe::M1, &vp, p, Some(MAGNET_WEAK_PX)),
            None
        );
        assert_eq!(
            snap_ohlc(&cs, Timeframe::M1, &vp, p, None).unwrap().price,
            14.0
        );
        assert_eq!(snap_ohlc(&[], Timeframe::M1, &vp, p, None), None);
    }

    fn angle_deg(fixed: (f32, f32), p: (f32, f32)) -> f32 {
        (p.1 - fixed.1).atan2(p.0 - fixed.0).to_degrees()
    }

    #[test]
    fn constrain_angle_cases() {
        let f = (50.0, 50.0);
        for (from, to) in [
            (10.0f32, 0.0f32),
            (40.0, 45.0),
            (80.0, 90.0),
            (170.0, 180.0),
            (-40.0, -45.0),
            (-100.0, -90.0),
            (-170.0, -180.0),
            (130.0, 135.0),
        ] {
            let r = from.to_radians();
            let m = (f.0 + 100.0 * r.cos(), f.1 + 100.0 * r.sin());
            let c = constrain_angle(f, m);
            let got = angle_deg(f, c);
            let diff = (got - to).rem_euclid(360.0);
            assert!(
                diff < 1e-3 || diff > 360.0 - 1e-3,
                "{from} -> {got}, want {to}"
            );
            // Projection: length is |m - f| * cos(delta).
            let len = ((c.0 - f.0).powi(2) + (c.1 - f.1).powi(2)).sqrt();
            let want = 100.0 * (from - to).to_radians().cos();
            assert!((len - want).abs() < 1e-3, "{len} vs {want}");
        }
        // Exact axes stay exact.
        assert_eq!(constrain_angle(f, (150.0, 53.0)).1, 50.0);
        assert_eq!(constrain_angle(f, (52.0, 150.0)).0, 50.0);
        // Zero length.
        assert_eq!(constrain_angle(f, f), f);
    }

    #[test]
    fn constrain_square_cases() {
        let f = (50.0, 50.0);
        assert_eq!(constrain_square(f, (80.0, 60.0)), (80.0, 80.0));
        assert_eq!(constrain_square(f, (40.0, 20.0)), (20.0, 20.0));
        assert_eq!(constrain_square(f, (60.0, 10.0)), (90.0, 10.0));
        assert_eq!(constrain_square(f, f), f);
    }
}
