use crate::{Viewport, index_to_time, time_to_index};
use xv_core::{Candle, Timeframe};
use xv_drawings::{Anchor, Projector};

/// Maps anchors to pixels relative to the plot area origin.
pub struct ChartProjector<'a> {
    pub candles: &'a [Candle],
    pub tf: Timeframe,
    pub vp: &'a Viewport,
}

impl Projector for ChartProjector<'_> {
    fn to_screen(&self, a: Anchor) -> (f32, f32) {
        let idx = time_to_index(self.candles, self.tf, a.time_ms);
        (self.vp.x_of_index(idx), self.vp.y_of_price(a.price))
    }

    fn to_anchor(&self, p: (f32, f32)) -> Anchor {
        let idx = self.vp.index_of_x(p.0);
        Anchor {
            time_ms: index_to_time(self.candles, self.tf, idx),
            price: self.vp.price_of_y(p.1),
        }
    }

    fn width(&self) -> f32 {
        self.vp.width
    }

    fn height(&self) -> f32 {
        self.vp.height
    }

    /// Shifts in f64 index/price space so repeated drags do not drift, and an
    /// untouched axis stays bit-exact.
    fn shift_anchor(&self, a: Anchor, dx: f32, dy: f32) -> Anchor {
        let time_ms = if dx == 0.0 {
            a.time_ms
        } else {
            let idx = time_to_index(self.candles, self.tf, a.time_ms)
                + dx as f64 / self.vp.bar_spacing as f64;
            index_to_time(self.candles, self.tf, idx)
        };
        let price = if dy == 0.0 {
            a.price
        } else {
            let per_px = (self.vp.price.max - self.vp.price.min) / self.vp.height as f64;
            a.price - dy as f64 * per_px
        };
        Anchor { time_ms, price }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PriceRange;

    const H: i64 = 3_600_000;

    fn candles() -> Vec<Candle> {
        (0..100)
            .map(|i| Candle {
                open_time_ms: 1_700_000_000_000 - 1_700_000_000_000 % H + i * H,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 0.0,
            })
            .collect()
    }

    #[test]
    fn chart_projector_shift_is_exact() {
        let cs = candles();
        let mut vp = Viewport::new(800.0, 400.0);
        vp.right_index = 50.0;
        // 1 price unit per pixel keeps the price arithmetic exactly representable.
        vp.price = PriceRange {
            min: 0.0,
            max: 400.0,
        };
        let p = ChartProjector {
            candles: &cs,
            tf: Timeframe::H1,
            vp: &vp,
        };
        let a = Anchor {
            time_ms: cs[10].open_time_ms + 17 * 60_000,
            price: 250.5,
        };
        let moved = p.shift_anchor(a, 100.0, 100.0);
        assert_ne!(moved, a);
        assert_eq!(p.shift_anchor(moved, -100.0, -100.0), a);
        // Untouched axes stay exact.
        assert_eq!(p.shift_anchor(a, 0.0, 33.0).time_ms, a.time_ms);
        assert_eq!(p.shift_anchor(a, 33.0, 0.0).price, a.price);
    }

    #[test]
    fn to_screen_round_trips() {
        let cs = candles();
        let mut vp = Viewport::new(800.0, 400.0);
        vp.right_index = 50.0;
        vp.price = PriceRange {
            min: 0.0,
            max: 400.0,
        };
        let p = ChartProjector {
            candles: &cs,
            tf: Timeframe::H1,
            vp: &vp,
        };
        let a = Anchor {
            time_ms: cs[45].open_time_ms,
            price: 100.0,
        };
        let s = p.to_screen(a);
        assert_eq!(p.to_anchor(s).time_ms, a.time_ms);
        assert!((p.to_anchor(s).price - 100.0).abs() < 1e-3);
    }
}
