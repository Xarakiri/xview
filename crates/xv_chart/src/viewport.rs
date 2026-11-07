use std::ops::Range;
use xv_core::Candle;

pub const DEFAULT_BAR_SPACING: f32 = 8.0;
pub const MIN_BAR_SPACING: f32 = 0.5;
pub const MAX_BAR_SPACING: f32 = 60.0;
pub const RIGHT_MARGIN_BARS: f64 = 5.0;
pub const AUTOFIT_PAD: f64 = 0.1;
pub const VOLUME_FRACTION: f32 = 0.2;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PriceRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PriceScale {
    Linear,
}

#[derive(Clone, Debug)]
pub struct Viewport {
    /// Plot area size in pixels.
    pub width: f32,
    pub height: f32,
    pub bar_spacing: f32,
    /// Fractional bar index at the plot's right edge.
    pub right_index: f64,
    pub price: PriceRange,
    pub auto_scale: bool,
    pub scale: PriceScale,
}

impl Viewport {
    pub fn new(width: f32, height: f32) -> Self {
        Viewport {
            width,
            height,
            bar_spacing: DEFAULT_BAR_SPACING,
            right_index: RIGHT_MARGIN_BARS,
            price: PriceRange { min: 0.0, max: 1.0 },
            auto_scale: true,
            scale: PriceScale::Linear,
        }
    }

    pub fn x_of_index(&self, idx: f64) -> f32 {
        self.width - ((self.right_index - idx) * self.bar_spacing as f64) as f32
    }

    pub fn index_of_x(&self, x: f32) -> f64 {
        self.right_index - (self.width - x) as f64 / self.bar_spacing as f64
    }

    pub fn y_of_price(&self, price: f64) -> f32 {
        let span = self.price.max - self.price.min;
        self.height - ((price - self.price.min) / span * self.height as f64) as f32
    }

    pub fn price_of_y(&self, y: f32) -> f64 {
        let span = self.price.max - self.price.min;
        self.price.min + (self.height - y) as f64 / self.height as f64 * span
    }

    /// Bars intersecting the plot, plus one extra on each side, clamped to `0..len`.
    pub fn visible_range(&self, len: usize) -> Range<usize> {
        if len == 0 {
            return 0..0;
        }
        let left = self.index_of_x(0.0).floor() - 1.0;
        let right = self.index_of_x(self.width).ceil() + 2.0;
        let start = left.clamp(0.0, len as f64) as usize;
        let end = right.clamp(0.0, len as f64) as usize;
        start..end.max(start)
    }

    pub fn pan_pixels(&mut self, dx: f32) {
        self.right_index -= dx as f64 / self.bar_spacing as f64;
    }

    pub fn zoom_at(&mut self, factor: f32, anchor_x: f32) {
        let idx = self.index_of_x(anchor_x);
        self.bar_spacing = (self.bar_spacing * factor).clamp(MIN_BAR_SPACING, MAX_BAR_SPACING);
        self.right_index = idx + (self.width - anchor_x) as f64 / self.bar_spacing as f64;
    }

    pub fn scale_price(&mut self, factor: f32) {
        let c = (self.price.min + self.price.max) / 2.0;
        let half = (self.price.max - self.price.min) / 2.0 * factor as f64;
        self.price = PriceRange {
            min: c - half,
            max: c + half,
        };
        self.auto_scale = false;
    }

    /// Dragging down by `dy_px` moves the chart content down.
    pub fn scroll_price(&mut self, dy_px: f32) {
        let delta = dy_px as f64 / self.height as f64 * (self.price.max - self.price.min);
        self.price = PriceRange {
            min: self.price.min + delta,
            max: self.price.max + delta,
        };
        self.auto_scale = false;
    }

    pub fn autofit(&mut self, candles: &[Candle]) {
        self.auto_scale = true;
        let vis = &candles[self.visible_range(candles.len())];
        if vis.is_empty() {
            return;
        }
        let lo = vis.iter().map(|c| c.low).fold(f64::INFINITY, f64::min);
        let hi = vis.iter().map(|c| c.high).fold(f64::NEG_INFINITY, f64::max);
        if hi > lo {
            let pad = (hi - lo) * AUTOFIT_PAD;
            self.price = PriceRange {
                min: lo - pad,
                max: hi + pad,
            };
        } else {
            let pad = if lo == 0.0 { 1.0 } else { lo.abs() * 0.01 };
            self.price = PriceRange {
                min: lo - pad,
                max: hi + pad,
            };
        }
    }

    pub fn scroll_to_latest(&mut self, len: usize) {
        self.right_index = len as f64 - 1.0 + RIGHT_MARGIN_BARS;
    }

    pub fn is_following(&self, len: usize) -> bool {
        if len == 0 {
            return false;
        }
        let x = self.x_of_index((len - 1) as f64);
        (0.0..=self.width).contains(&x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(w: f32, h: f32) -> Viewport {
        let mut v = Viewport::new(w, h);
        v.right_index = 2.0 + RIGHT_MARGIN_BARS;
        v
    }
    fn flat(p: f64) -> Candle {
        Candle {
            open_time_ms: 0,
            open: p,
            high: p,
            low: p,
            close: p,
            volume: 0.0,
        }
    }

    #[test]
    fn zoom_keeps_index_under_cursor() {
        let mut v = vp(800.0, 400.0);
        let before = v.index_of_x(300.0);
        v.zoom_at(1.5, 300.0);
        assert!((v.index_of_x(300.0) - before).abs() < 1e-6);
        v.zoom_at(1000.0, 300.0);
        assert_eq!(v.bar_spacing, MAX_BAR_SPACING);
    }

    #[test]
    fn autofit_flat_series_pads_range() {
        let mut v = vp(800.0, 400.0);
        v.autofit(&[flat(100.0); 3]);
        assert!(v.price.max > v.price.min && v.price.min.is_finite());
        v.autofit(&[flat(0.0)]);
        assert!(v.price.max > v.price.min);
        v.autofit(&[]);
        assert!(v.price.max > v.price.min);
    }

    #[test]
    fn autofit_pads_ten_percent() {
        let mut v = vp(800.0, 400.0);
        let c = [Candle {
            open_time_ms: 0,
            open: 1.0,
            high: 110.0,
            low: 100.0,
            close: 1.0,
            volume: 0.0,
        }];
        v.autofit(&c);
        assert!((v.price.min - 99.0).abs() < 1e-9 && (v.price.max - 111.0).abs() < 1e-9);
        assert!(v.auto_scale);
    }

    #[test]
    fn coordinate_round_trips_and_pan() {
        let mut v = vp(800.0, 400.0);
        v.price = PriceRange {
            min: 10.0,
            max: 20.0,
        };
        assert!((v.price_of_y(v.y_of_price(13.0)) - 13.0).abs() < 1e-4);
        assert!((v.index_of_x(v.x_of_index(1.5)) - 1.5).abs() < 1e-4);
        let r = v.right_index;
        v.pan_pixels(16.0);
        assert!((v.right_index - (r - 2.0)).abs() < 1e-9);
    }

    #[test]
    fn visible_range_and_following() {
        let mut v = vp(80.0, 400.0); // 10 bars visible
        v.scroll_to_latest(100);
        assert!(v.is_following(100));
        assert_eq!(v.visible_range(100).end, 100);
        v.right_index = 50.0;
        assert!(!v.is_following(100));
        let r = v.visible_range(100);
        assert!(r.start <= 40 && r.end >= 50 && r.end <= 100);
        assert_eq!(v.visible_range(0), 0..0);
        v.right_index = 99.0 - 20.0;
        assert!(!v.is_following(100)); // last bar beyond the right edge
        v.right_index = 99.0 + 50.0;
        assert!(!v.is_following(100)); // last bar off the left edge
    }

    #[test]
    fn manual_price_ops_disable_auto() {
        let mut v = vp(800.0, 400.0);
        v.price = PriceRange {
            min: 0.0,
            max: 10.0,
        };
        v.scale_price(2.0);
        assert!(!v.auto_scale && v.price.min == -5.0 && v.price.max == 15.0);
        v.auto_scale = true;
        v.scroll_price(40.0);
        assert!(!v.auto_scale);
    }
}
