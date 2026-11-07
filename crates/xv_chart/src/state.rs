//! GPUI-free navigation state machine: turns mouse/scroll/pinch input into
//! viewport changes. All coordinates are relative to the chart element origin.

use crate::{
    ChartColors, ChartLayout, Frame, FrameInput, MAX_BAR_SPACING, MIN_BAR_SPACING,
    RIGHT_MARGIN_BARS, RectF, Viewport, build_frame, time_to_index,
};
use xv_core::{Candle, Series, TzOffset, UpdateKind};
use xv_drawings::{Drawing, DrawingId, DrawingSet, Tool};

pub const OLDER_THRESHOLD_BARS: f64 = 50.0;

#[derive(Copy, Clone, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub cmd: bool,
    pub alt: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChartEffect {
    NeedOlder { end_before_ms: i64 },
    DrawingsChanged,
    ToolFinished,
    SelectionChanged,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChartKey {
    Delete,
    Escape,
    Undo,
    Redo,
}

/// Active mouse-drag gesture. Task 12 extends this with drawing variants.
#[derive(Copy, Clone, Debug)]
enum Drag {
    None,
    Pan { last: (f32, f32) },
    PriceScale { last_y: f32 },
    TimeZoom { last_x: f32 },
}

pub struct ChartState {
    pub series: Series,
    pub vp: Viewport,
    pub layout: ChartLayout,
    pub crosshair: Option<(f32, f32)>,
    pub drawings: DrawingSet,
    pub drawings_hidden: bool,
    pub drawings_locked: bool,
    pub magnet: bool,
    pub tool: Tool,
    pub selected: Option<DrawingId>,
    pub loading_older: bool,
    pub tz: TzOffset,
    drag: Drag,
}

fn contains(r: &RectF, p: (f32, f32)) -> bool {
    p.0 >= r.x && p.0 <= r.x + r.w && p.1 >= r.y && p.1 <= r.y + r.h
}

impl ChartState {
    pub fn new(series: Series, width: f32, height: f32) -> Self {
        let layout = ChartLayout::new(width, height);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        vp.scroll_to_latest(series.len());
        vp.autofit(series.candles());
        ChartState {
            series,
            vp,
            layout,
            crosshair: None,
            drawings: DrawingSet::default(),
            drawings_hidden: false,
            drawings_locked: false,
            magnet: false,
            tool: Tool::Cursor,
            selected: None,
            loading_older: false,
            tz: TzOffset::default(),
            drag: Drag::None,
        }
    }

    pub fn set_size(&mut self, w: f32, h: f32) {
        self.layout = ChartLayout::new(w, h);
        self.vp.width = self.layout.plot.w;
        self.vp.height = self.layout.plot.h;
        self.refit();
    }

    /// Replaces the series and resets the view to the latest candle, keeping
    /// the current zoom (`bar_spacing`).
    pub fn set_series(&mut self, s: Series) {
        self.series = s;
        self.loading_older = false;
        self.drag = Drag::None;
        let spacing = self.vp.bar_spacing;
        self.vp = Viewport::new(self.layout.plot.w, self.layout.plot.h);
        self.vp.bar_spacing = spacing;
        self.vp.scroll_to_latest(self.series.len());
        self.vp.autofit(self.series.candles());
    }

    pub fn set_drawings(&mut self, ds: Vec<Drawing>) {
        self.drawings = DrawingSet::new(ds);
        self.selected = None;
    }

    pub fn mouse_down(&mut self, p: (f32, f32), _m: Mods, click_count: usize) -> Vec<ChartEffect> {
        self.drag = Drag::None;
        if contains(&self.layout.plot, p) {
            self.drag = Drag::Pan { last: p };
        } else if contains(&self.layout.price_axis, p) {
            if click_count >= 2 {
                self.vp.autofit(self.series.candles());
            } else {
                self.drag = Drag::PriceScale { last_y: p.1 };
            }
        } else if contains(&self.layout.time_axis, p) {
            self.drag = Drag::TimeZoom { last_x: p.0 };
        }
        Vec::new()
    }

    pub fn mouse_move(&mut self, p: (f32, f32), _m: Mods, pressed: bool) -> Vec<ChartEffect> {
        self.crosshair = contains(&self.layout.plot, p).then_some(p);
        if !pressed {
            self.drag = Drag::None;
            return Vec::new();
        }
        match self.drag {
            Drag::None => return Vec::new(),
            Drag::Pan { last } => {
                self.vp.pan_pixels(p.0 - last.0);
                if !self.vp.auto_scale {
                    self.vp.scroll_price(p.1 - last.1);
                }
                self.drag = Drag::Pan { last: p };
            }
            Drag::PriceScale { last_y } => {
                self.vp.scale_price(((p.1 - last_y) * 0.005).exp());
                self.drag = Drag::PriceScale { last_y: p.1 };
            }
            Drag::TimeZoom { last_x } => {
                let anchor = self.vp.width;
                self.vp.zoom_at(((p.0 - last_x) * 0.005).exp(), anchor);
                self.drag = Drag::TimeZoom { last_x: p.0 };
            }
        }
        self.after_navigation()
    }

    pub fn mouse_up(&mut self, _p: (f32, f32), _m: Mods) -> Vec<ChartEffect> {
        self.drag = Drag::None;
        Vec::new()
    }

    pub fn mouse_left(&mut self) {
        self.crosshair = None;
    }

    pub fn scroll(&mut self, p: (f32, f32), dx: f32, dy: f32, m: Mods) -> Vec<ChartEffect> {
        if dx.abs() > dy.abs() {
            self.vp.pan_pixels(dx);
        } else if m.alt {
            self.vp.scroll_price(dy);
        } else {
            self.vp
                .zoom_at((-dy * 0.002).exp(), p.0 - self.layout.plot.x);
        }
        self.after_navigation()
    }

    pub fn pinch(&mut self, p: (f32, f32), delta: f32) -> Vec<ChartEffect> {
        self.vp
            .zoom_at((1.0 + delta).max(0.01), p.0 - self.layout.plot.x);
        self.after_navigation()
    }

    pub fn apply_update(&mut self, c: Candle) {
        let was_following = self.vp.is_following(self.series.len());
        let kind = self.series.apply_update(c);
        if kind == UpdateKind::Appended && was_following {
            self.vp.scroll_to_latest(self.series.len());
        }
        self.refit();
    }

    pub fn prepend_older(&mut self, page_candles: Vec<Candle>, has_more: bool) {
        let added = self.series.prepend_older(page_candles, has_more);
        self.vp.right_index += added as f64;
        self.loading_older = false;
        self.refit();
    }

    pub fn merge_backfill(&mut self, cs: Vec<Candle>) {
        let first_ts = self.series.candles().first().map(|c| c.open_time_ms);
        self.series.merge_range(cs);
        if let Some(ts) = first_ts {
            // Keep the view stable if candles were inserted before the old first one.
            let shift = self
                .series
                .candles()
                .partition_point(|c| c.open_time_ms < ts);
            self.vp.right_index += shift as f64;
        }
        self.refit();
    }

    /// Fits the last `span_ms` of data (or all data for `None`) into the plot.
    /// The span is laid out in the plot width together with the right margin.
    pub fn show_span(&mut self, span_ms: Option<i64>) {
        let len = self.series.len();
        if len == 0 {
            return;
        }
        let last_idx = (len - 1) as f64;
        let bars = match span_ms {
            Some(span) => {
                let start = self.series.candles()[len - 1].open_time_ms - span;
                let idx0 = time_to_index(self.series.candles(), self.series.timeframe, start);
                last_idx - idx0.max(0.0)
            }
            None => last_idx + 1.0,
        }
        .max(1.0);
        let spacing = self.vp.width as f64 / (bars + RIGHT_MARGIN_BARS);
        self.vp.bar_spacing = (spacing as f32).clamp(MIN_BAR_SPACING, MAX_BAR_SPACING);
        self.vp.scroll_to_latest(len);
        self.vp.autofit(self.series.candles());
    }

    pub fn frame(&self, colors: &ChartColors, source_name: &str) -> Frame {
        build_frame(&FrameInput {
            series: &self.series,
            vp: &self.vp,
            layout: &self.layout,
            crosshair: self.crosshair,
            drawings: self.drawings.all(),
            drawings_hidden: self.drawings_hidden,
            selected: self.selected,
            colors,
            tz: self.tz,
            source_name,
        })
    }

    /// Re-applies autofit when enabled (price range follows the visible bars).
    fn refit(&mut self) {
        if self.vp.auto_scale {
            self.vp.autofit(self.series.candles());
        }
    }

    fn after_navigation(&mut self) -> Vec<ChartEffect> {
        self.refit();
        let mut fx = Vec::new();
        if self.series.has_older
            && !self.loading_older
            && self.vp.index_of_x(0.0) < OLDER_THRESHOLD_BARS
            && let Some(first) = self.series.candles().first()
        {
            self.loading_older = true;
            fx.push(ChartEffect::NeedOlder {
                end_before_ms: first.open_time_ms,
            });
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_to_time;
    use xv_core::{SourceId, Symbol, Timeframe};

    const M5: i64 = 5 * 60_000;
    const T0: i64 = 1_700_000_000_000 / M5 * M5;

    fn candle(i: i64) -> Candle {
        let p = 100.0 + (i % 17) as f64;
        Candle {
            open_time_ms: T0 + i * M5,
            open: p,
            high: p + 2.0,
            low: p - 2.0,
            close: p + 1.0,
            volume: 1.0,
        }
    }

    fn series(range: std::ops::Range<i64>, has_older: bool) -> Series {
        let sym = Symbol {
            source: SourceId("t".into()),
            id: "X".into(),
            display: "X".into(),
            description: String::new(),
            base: "X".into(),
            quote: "Y".into(),
            tick_size: 0.01,
        };
        Series::new(sym, Timeframe::M5, range.map(candle).collect(), has_older)
    }

    // Chart 872x428 -> plot 800x400.
    fn state(n: i64, has_older: bool) -> ChartState {
        ChartState::new(series(1000..1000 + n, has_older), 872.0, 428.0)
    }

    fn no() -> Mods {
        Mods::default()
    }

    #[test]
    fn drag_in_plot_pans() {
        let mut s = state(500, false);
        let r0 = s.vp.right_index;
        s.mouse_down((400.0, 200.0), no(), 1);
        s.mouse_move((480.0, 200.0), no(), true);
        s.mouse_up((480.0, 200.0), no());
        let expect = r0 - 80.0 / s.vp.bar_spacing as f64;
        assert!((s.vp.right_index - expect).abs() < 1e-9);
    }

    #[test]
    fn vertical_scroll_zooms_at_cursor() {
        let mut s = state(500, false);
        let before = s.vp.index_of_x(300.0);
        let sp = s.vp.bar_spacing;
        s.scroll((300.0, 100.0), 0.0, -50.0, no());
        assert!(s.vp.bar_spacing > sp);
        assert!((s.vp.index_of_x(300.0) - before).abs() < 1e-6);
    }

    #[test]
    fn horizontal_scroll_pans() {
        let mut s = state(500, false);
        let (r0, sp) = (s.vp.right_index, s.vp.bar_spacing);
        s.scroll((300.0, 100.0), 40.0, 3.0, no());
        assert_eq!(s.vp.bar_spacing, sp);
        assert!((s.vp.right_index - (r0 - 40.0 / sp as f64)).abs() < 1e-9);
    }

    #[test]
    fn alt_scroll_moves_price_and_disables_autoscale() {
        let mut s = state(500, false);
        let p0 = s.vp.price;
        let m = Mods { alt: true, ..no() };
        s.scroll((300.0, 100.0), 0.0, 20.0, m);
        assert!(!s.vp.auto_scale);
        assert_ne!(s.vp.price, p0);
    }

    #[test]
    fn price_axis_drag_scales_and_double_click_restores_autofit() {
        let mut s = state(500, false);
        let auto = s.vp.price;
        let x = s.layout.price_axis.x + 10.0;
        s.mouse_down((x, 100.0), no(), 1);
        s.mouse_move((x, 140.0), no(), true);
        s.mouse_up((x, 140.0), no());
        assert!(!s.vp.auto_scale);
        assert_ne!(s.vp.price, auto);
        s.mouse_down((x, 100.0), no(), 2);
        s.mouse_up((x, 100.0), no());
        assert!(s.vp.auto_scale);
        assert_eq!(s.vp.price, auto);
    }

    #[test]
    fn time_axis_drag_zooms_anchored_at_right_edge() {
        let mut s = state(500, false);
        let right = s.vp.right_index;
        let sp = s.vp.bar_spacing;
        let y = s.layout.time_axis.y + 5.0;
        s.mouse_down((100.0, y), no(), 1);
        s.mouse_move((200.0, y), no(), true);
        assert!(s.vp.bar_spacing > sp);
        assert!((s.vp.right_index - right).abs() < 1e-6);
    }

    #[test]
    fn need_older_emitted_once_near_left_edge() {
        let mut s = state(500, true);
        let first_ts = s.series.candles()[0].open_time_ms;
        s.mouse_down((400.0, 200.0), no(), 1);
        let mut effects = Vec::new();
        for i in 1..=60 {
            effects.extend(s.mouse_move((400.0 + i as f32 * 100.0, 200.0), no(), true));
        }
        assert_eq!(
            effects,
            vec![ChartEffect::NeedOlder {
                end_before_ms: first_ts
            }]
        );
        assert!(s.loading_older);
        let older: Vec<Candle> = (500..1000).map(candle).collect();
        s.prepend_older(older, true);
        assert!(!s.loading_older);
        // Still near the (new) left edge? Only emits again once pulled there.
        let n = s.mouse_move((900.0, 200.0), no(), true);
        assert!(n.len() <= 1);
    }

    #[test]
    fn no_need_older_without_more_history() {
        let mut s = state(500, false);
        s.mouse_down((400.0, 200.0), no(), 1);
        let mut n = 0;
        for i in 1..=60 {
            n += s
                .mouse_move((400.0 + i as f32 * 100.0, 200.0), no(), true)
                .len();
        }
        assert_eq!(n, 0);
    }

    #[test]
    fn prepend_keeps_view_stable() {
        let mut s = state(500, true);
        let t = |s: &ChartState| {
            index_to_time(
                s.series.candles(),
                s.series.timeframe,
                s.vp.index_of_x(400.0),
            )
        };
        let before = t(&s);
        let older: Vec<Candle> = (500..1000).map(candle).collect();
        s.prepend_older(older, true);
        assert_eq!(s.series.len(), 1000);
        assert!((t(&s) - before).abs() <= 1);
    }

    #[test]
    fn live_update_follows_when_at_latest() {
        let mut s = state(500, false);
        assert!(s.vp.is_following(s.series.len()));
        s.apply_update(candle(1500));
        assert_eq!(s.series.len(), 501);
        assert!(s.vp.is_following(501));
        assert_eq!(s.vp.right_index, 500.0 + RIGHT_MARGIN_BARS);

        s.vp.right_index = 200.0;
        s.apply_update(candle(1501));
        assert_eq!(s.series.len(), 502);
        assert_eq!(s.vp.right_index, 200.0);
    }

    #[test]
    fn show_span_fits_last_day() {
        let mut s = state(2000, false);
        s.show_span(Some(24 * 3_600_000));
        let visible = (0..s.series.len())
            .filter(|&i| (0.0..=s.vp.width).contains(&s.vp.x_of_index(i as f64)))
            .count();
        assert!((visible as i64 - 288).abs() <= 2, "visible {visible}");
        let mut s = state(1000, false);
        s.show_span(None);
        assert!(s.vp.index_of_x(0.0) <= 0.5);
    }

    #[test]
    fn set_size_keeps_viewport_in_sync_and_set_series_keeps_spacing() {
        let mut s = state(500, false);
        s.set_size(472.0, 328.0);
        assert_eq!((s.vp.width, s.vp.height), (400.0, 300.0));
        s.vp.bar_spacing = 12.0;
        s.set_series(series(0..300, false));
        assert_eq!(s.vp.bar_spacing, 12.0);
        assert!(s.vp.is_following(300));
    }

    #[test]
    fn crosshair_follows_plot_moves_and_clears() {
        let mut s = state(100, false);
        s.mouse_move((10.0, 10.0), no(), false);
        assert_eq!(s.crosshair, Some((10.0, 10.0)));
        s.mouse_move((s.layout.price_axis.x + 5.0, 10.0), no(), false);
        assert_eq!(s.crosshair, None);
        s.mouse_move((10.0, 10.0), no(), false);
        s.mouse_left();
        assert_eq!(s.crosshair, None);
    }

    #[test]
    fn frame_builds() {
        let s = state(100, false);
        let f = s.frame(&ChartColors::dark(), "test");
        assert!(!f.plot.is_empty());
    }
}
