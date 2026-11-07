//! GPUI-free navigation state machine: turns mouse/scroll/pinch input into
//! viewport changes. All coordinates are relative to the chart element origin.

use crate::{
    ChartColors, ChartLayout, Frame, FrameInput, MAX_BAR_SPACING, MIN_BAR_SPACING,
    RIGHT_MARGIN_BARS, RectF, Viewport, build_frame, time_to_index,
};
use crate::{ChartProjector, snap::snap_anchor};
use xv_core::{Candle, Series, TzOffset, UpdateKind};
use xv_drawings::{
    Anchor, Drawing, DrawingId, DrawingKind, DrawingSet, Hit, PlaceResult, Projector, Style, Tool,
    handles, hit_test, place_click,
};

/// Pixels a press must travel before it counts as a drag rather than a click.
const DRAG_THRESHOLD: f32 = 3.0;

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

/// Active mouse-drag gesture.
#[derive(Clone, Debug)]
enum Drag {
    None,
    /// Press of a two-point placement tool; `first` is the anchor at press.
    Place {
        start: (f32, f32),
        first: Anchor,
    },
    /// Body drag; always translates from `snapshot` by the total offset.
    Body {
        start: (f32, f32),
        snapshot: Drawing,
        moved: bool,
    },
    Handle {
        start: (f32, f32),
        idx: usize,
        snapshot: Drawing,
        moved: bool,
    },
    Pan {
        last: (f32, f32),
    },
    PriceScale {
        last_y: f32,
    },
    TimeZoom {
        last_x: f32,
    },
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
    /// First anchor of a click-click placement in progress.
    pending: Option<Anchor>,
    last_mods: Mods,
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
            pending: None,
            last_mods: Mods::default(),
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
        self.abort_drag();
        self.pending = None;
        let spacing = self.vp.bar_spacing;
        self.vp = Viewport::new(self.layout.plot.w, self.layout.plot.h);
        self.vp.bar_spacing = spacing;
        self.vp.scroll_to_latest(self.series.len());
        self.vp.autofit(self.series.candles());
    }

    pub fn set_drawings(&mut self, ds: Vec<Drawing>) {
        self.drawings = DrawingSet::new(ds);
        self.selected = None;
        self.pending = None;
        self.drag = Drag::None;
    }

    pub fn mouse_down(&mut self, p: (f32, f32), m: Mods, click_count: usize) -> Vec<ChartEffect> {
        self.last_mods = m;
        // A drag still active here lost its mouse-up: finish it first.
        // A stale placement press is treated as a plain click.
        let at = match self.drag {
            Drag::Place { start, .. } => start,
            _ => p,
        };
        let mut fx = self.finish_drag(at, m);
        if contains(&self.layout.plot, p) {
            if self.tool != Tool::Cursor {
                self.press_with_tool(p, m, &mut fx);
            } else if let Some((id, hit)) = self.hit(p) {
                self.select(Some(id), &mut fx);
                if let Some(d) = self.drawings.get(id).cloned() {
                    self.drawings.begin_gesture();
                    self.drag = match hit {
                        Hit::Body => Drag::Body {
                            start: p,
                            snapshot: d,
                            moved: false,
                        },
                        Hit::Handle(idx) => Drag::Handle {
                            start: p,
                            idx,
                            snapshot: d,
                            moved: false,
                        },
                    };
                }
            } else {
                self.select(None, &mut fx);
                self.drag = Drag::Pan { last: p };
            }
        } else if contains(&self.layout.price_axis, p) {
            if click_count >= 2 {
                self.vp.autofit(self.series.candles());
            } else {
                self.drag = Drag::PriceScale { last_y: p.1 };
            }
        } else if contains(&self.layout.time_axis, p) {
            self.drag = Drag::TimeZoom { last_x: p.0 };
        }
        fx
    }

    pub fn mouse_move(&mut self, p: (f32, f32), m: Mods, pressed: bool) -> Vec<ChartEffect> {
        self.crosshair = contains(&self.layout.plot, p).then_some(p);
        self.last_mods = m;
        if !pressed {
            // Released outside the window: finish exactly like mouse_up.
            return self.finish_drag(p, m);
        }
        match self.drag.clone() {
            Drag::None | Drag::Place { .. } => return Vec::new(),
            Drag::Body {
                start,
                snapshot,
                moved,
            } => {
                let moved = moved || dist(start, p) > DRAG_THRESHOLD;
                if moved {
                    // Total offset from the press, applied to the snapshot, so
                    // repeated moves can never accumulate error.
                    let proj = ChartProjector {
                        candles: self.series.candles(),
                        tf: self.series.timeframe,
                        vp: &self.vp,
                    };
                    let d = snapshot.translated(p.0 - start.0, p.1 - start.1, &proj);
                    self.drawings.replace(d);
                }
                self.drag = Drag::Body {
                    start,
                    snapshot,
                    moved,
                };
                return Vec::new();
            }
            Drag::Handle {
                start,
                idx,
                snapshot,
                moved,
            } => {
                let moved = moved || dist(start, p) > DRAG_THRESHOLD;
                if moved {
                    let to = self.anchor_at(p, m);
                    self.drawings.replace(snapshot.with_handle(idx, to));
                }
                self.drag = Drag::Handle {
                    start,
                    idx,
                    snapshot,
                    moved,
                };
                return Vec::new();
            }
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

    pub fn mouse_up(&mut self, p: (f32, f32), m: Mods) -> Vec<ChartEffect> {
        self.last_mods = m;
        self.finish_drag(p, m)
    }

    fn finish_drag(&mut self, p: (f32, f32), m: Mods) -> Vec<ChartEffect> {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let mut fx = Vec::new();
        match drag {
            Drag::Place { start, first } => {
                if dist(start, p) > DRAG_THRESHOLD {
                    let to = self.anchor_at(p, m);
                    self.advance_placement(Some(first), to, m, &mut fx);
                } else {
                    self.pending = Some(first);
                }
            }
            Drag::Body { snapshot, .. } | Drag::Handle { snapshot, .. } => {
                self.drawings.end_gesture();
                if self.drawings.get(snapshot.id) != Some(&snapshot) {
                    fx.push(ChartEffect::DrawingsChanged);
                }
            }
            _ => {}
        }
        fx
    }

    pub fn mouse_left(&mut self) {
        self.crosshair = None;
    }

    pub fn scroll(&mut self, p: (f32, f32), dx: f32, dy: f32, m: Mods) -> Vec<ChartEffect> {
        if self.vp.width <= 0.0 || self.vp.height <= 0.0 {
            return Vec::new();
        }
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
        let was_following = self.vp.is_following(self.series.len());
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
        if was_following {
            self.vp.scroll_to_latest(self.series.len());
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

    /// Temporary drawing of the current tool from the pending first anchor to
    /// the anchor under the crosshair. Never added to the set.
    fn placement_preview(&self) -> Option<Drawing> {
        let first = match (&self.drag, self.pending) {
            (Drag::Place { first, .. }, _) => *first,
            (_, Some(a)) => a,
            _ => return None,
        };
        let c = self.crosshair?;
        match place_click(self.tool, Some(first), self.anchor_at(c, self.last_mods))? {
            PlaceResult::Done(kind) => Some(Drawing {
                id: DrawingId(u64::MAX),
                kind,
                style: Style::default(),
            }),
            PlaceResult::Pending(_) => None,
        }
    }

    pub fn frame(&self, colors: &ChartColors, source_name: &str) -> Frame {
        let mut all;
        let drawings = match self.placement_preview() {
            Some(d) => {
                all = self.drawings.all().to_vec();
                all.push(d);
                &all[..]
            }
            None => self.drawings.all(),
        };
        build_frame(&FrameInput {
            series: &self.series,
            vp: &self.vp,
            layout: &self.layout,
            crosshair: self.crosshair,
            drawings,
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
        self.check_need_older()
    }

    /// Emits `NeedOlder` (once) when the view is near the left edge of the
    /// loaded history. Hosts call this after retrying a failed page load.
    pub fn check_need_older(&mut self) -> Vec<ChartEffect> {
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

    /// The host reports that an older-page request failed.
    pub fn older_failed(&mut self) {
        self.loading_older = false;
    }

    // ---- drawings ----

    pub fn set_tool(&mut self, t: Tool) {
        self.tool = t;
        if t != Tool::Cursor {
            self.drawings_hidden = false;
        }
        self.pending = None;
        self.abort_drag();
    }

    /// True while a click-click placement has its first point set.
    pub fn placement_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn set_selected_style(&mut self, s: Style) -> Vec<ChartEffect> {
        let Some(mut d) = self.selected.and_then(|id| self.drawings.get(id)).cloned() else {
            return Vec::new();
        };
        if d.style == s {
            return Vec::new();
        }
        d.style = s;
        self.drawings.replace(d);
        vec![ChartEffect::DrawingsChanged]
    }

    pub fn clear_drawings(&mut self) -> Vec<ChartEffect> {
        self.abort_drag();
        self.pending = None;
        let mut fx = Vec::new();
        if !self.drawings.all().is_empty() {
            self.drawings.clear();
            fx.push(ChartEffect::DrawingsChanged);
        }
        self.select(None, &mut fx);
        fx
    }

    pub fn key(&mut self, k: ChartKey) -> Vec<ChartEffect> {
        let mut fx = Vec::new();
        match k {
            ChartKey::Delete => {
                if let Some(id) = self.selected
                    && !self.drawings_locked
                {
                    self.abort_drag();
                    self.drawings.remove(id);
                    fx.push(ChartEffect::DrawingsChanged);
                    self.select(None, &mut fx);
                }
            }
            ChartKey::Escape => {
                let dragging = !matches!(self.drag, Drag::None | Drag::Pan { .. });
                if dragging {
                    // Moved drawings are restored; a placement press is dropped.
                    self.abort_drag();
                } else if self.pending.take().is_some() {
                    // Pending placement cancelled.
                } else if self.selected.is_some() {
                    self.select(None, &mut fx);
                } else if self.tool != Tool::Cursor {
                    self.tool = Tool::Cursor;
                    fx.push(ChartEffect::ToolFinished);
                }
            }
            ChartKey::Undo | ChartKey::Redo => {
                self.abort_drag();
                self.pending = None;
                let changed = if k == ChartKey::Undo {
                    self.drawings.undo()
                } else {
                    self.drawings.redo()
                };
                if changed {
                    fx.push(ChartEffect::DrawingsChanged);
                    if self
                        .selected
                        .is_some_and(|id| self.drawings.get(id).is_none())
                    {
                        self.select(None, &mut fx);
                    }
                }
            }
        }
        fx
    }

    /// Bounding box (element coordinates) of the selected drawing's handles,
    /// clamped to the plot. `None` when nothing is selected or drawings are hidden.
    pub fn selected_screen_bounds(&self) -> Option<RectF> {
        if self.drawings_hidden {
            return None;
        }
        let d = self.drawings.get(self.selected?)?;
        let proj = ChartProjector {
            candles: self.series.candles(),
            tf: self.series.timeframe,
            vp: &self.vp,
        };
        let plot = &self.layout.plot;
        let pts = handles(d, &proj);
        let clamp = |p: &(f32, f32)| {
            (
                p.0.clamp(0.0, plot.w) + plot.x,
                p.1.clamp(0.0, plot.h) + plot.y,
            )
        };
        let pts: Vec<(f32, f32)> = pts.iter().map(clamp).collect();
        let first = *pts.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for p in &pts {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
        Some(RectF {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    /// Abandons the active drag: moved drawings go back to their drag-start
    /// state and no undo step remains.
    fn abort_drag(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Body { snapshot, .. } | Drag::Handle { snapshot, .. } => {
                self.drawings.replace(snapshot);
                self.drawings.end_gesture();
            }
            _ => {}
        }
    }

    pub fn set_locked(&mut self, locked: bool) -> Vec<ChartEffect> {
        self.abort_drag();
        self.drawings_locked = locked;
        let mut fx = Vec::new();
        if locked {
            self.select(None, &mut fx);
        }
        fx
    }

    pub fn set_hidden(&mut self, hidden: bool) -> Vec<ChartEffect> {
        self.abort_drag();
        self.drawings_hidden = hidden;
        let mut fx = Vec::new();
        if hidden {
            self.select(None, &mut fx);
        }
        fx
    }

    fn select(&mut self, id: Option<DrawingId>, fx: &mut Vec<ChartEffect>) {
        if self.selected != id {
            self.selected = id;
            fx.push(ChartEffect::SelectionChanged);
        }
    }

    /// Hit-tests the drawings at an element-space point.
    fn hit(&self, p: (f32, f32)) -> Option<(DrawingId, Hit)> {
        if self.drawings_hidden || self.drawings_locked {
            return None;
        }
        let proj = ChartProjector {
            candles: self.series.candles(),
            tf: self.series.timeframe,
            vp: &self.vp,
        };
        hit_test(
            self.drawings.all(),
            (p.0 - self.layout.plot.x, p.1 - self.layout.plot.y),
            &proj,
        )
    }

    /// Anchor under an element-space point, snapped to OHLC with cmd or magnet.
    fn anchor_at(&self, p: (f32, f32), m: Mods) -> Anchor {
        let lp = (p.0 - self.layout.plot.x, p.1 - self.layout.plot.y);
        if m.cmd || self.magnet {
            snap_anchor(self.series.candles(), self.series.timeframe, &self.vp, lp)
        } else {
            ChartProjector {
                candles: self.series.candles(),
                tf: self.series.timeframe,
                vp: &self.vp,
            }
            .to_anchor(lp)
        }
    }

    fn press_with_tool(&mut self, p: (f32, f32), m: Mods, fx: &mut Vec<ChartEffect>) {
        let at = self.anchor_at(p, m);
        match self.pending {
            Some(first) => self.advance_placement(Some(first), at, m, fx),
            None => match place_click(self.tool, None, at) {
                Some(PlaceResult::Pending(first)) => {
                    // Wait for release: a drag finishes the shape, a click
                    // leaves the first point pending.
                    self.drag = Drag::Place { start: p, first };
                }
                Some(PlaceResult::Done(kind)) => self.finish_placement(kind, m, fx),
                None => {}
            },
        }
    }

    fn advance_placement(
        &mut self,
        first: Option<Anchor>,
        at: Anchor,
        m: Mods,
        fx: &mut Vec<ChartEffect>,
    ) {
        self.pending = None;
        match place_click(self.tool, first, at) {
            Some(PlaceResult::Done(kind)) => self.finish_placement(kind, m, fx),
            Some(PlaceResult::Pending(a)) => self.pending = Some(a),
            None => {}
        }
    }

    fn finish_placement(&mut self, kind: DrawingKind, m: Mods, fx: &mut Vec<ChartEffect>) {
        // A placement at a non-finite price (degenerate view) is dropped.
        let Some(id) = self.drawings.add(kind, Style::default()) else {
            return;
        };
        fx.push(ChartEffect::DrawingsChanged);
        self.select(Some(id), fx);
        if !m.shift {
            self.tool = Tool::Cursor;
            fx.push(ChartEffect::ToolFinished);
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
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

    // ---- Task 12: drawing gestures ----

    use crate::{ChartProjector, Prim};
    use xv_core::{TzOffset, resample};
    use xv_drawings::{Anchor, DrawingId, DrawingKind, Projector, Style};

    fn proj(s: &ChartState) -> ChartProjector<'_> {
        ChartProjector {
            candles: s.series.candles(),
            tf: s.series.timeframe,
            vp: &s.vp,
        }
    }

    fn at(s: &ChartState, p: (f32, f32)) -> Anchor {
        proj(s).to_anchor(p)
    }

    fn scr(s: &ChartState, a: Anchor) -> (f32, f32) {
        proj(s).to_screen(a)
    }

    fn click(s: &mut ChartState, p: (f32, f32), m: Mods) -> Vec<ChartEffect> {
        let mut fx = s.mouse_down(p, m, 1);
        fx.extend(s.mouse_up(p, m));
        fx
    }

    fn trend(s: &mut ChartState) -> (DrawingId, Anchor, Anchor) {
        let (a, b) = (at(s, (200.0, 150.0)), at(s, (400.0, 100.0)));
        s.set_drawings(vec![Drawing {
            id: DrawingId(1),
            kind: DrawingKind::TrendLine { a, b },
            style: Style::default(),
        }]);
        (DrawingId(1), a, b)
    }

    fn ends(s: &ChartState, id: DrawingId) -> (Anchor, Anchor) {
        match s.drawings.get(id).unwrap().kind {
            DrawingKind::TrendLine { a, b } => (a, b),
            ref k => panic!("not a trendline: {k:?}"),
        }
    }

    #[test]
    fn two_click_trendline() {
        let mut s = state(500, false);
        s.set_tool(Tool::TrendLine);
        let fx = click(&mut s, (200.0, 150.0), no());
        assert_eq!(s.drawings.all().len(), 0);
        assert!(!fx.contains(&ChartEffect::ToolFinished));
        let fx = click(&mut s, (400.0, 100.0), no());
        assert_eq!(s.drawings.all().len(), 1);
        assert_eq!(s.tool, Tool::Cursor);
        assert!(fx.contains(&ChartEffect::ToolFinished));
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.selected, Some(s.drawings.all()[0].id));
        assert_eq!(s.drawings.all()[0].style, Style::default());
    }

    #[test]
    fn shift_keeps_tool() {
        let mut s = state(500, false);
        s.set_tool(Tool::HorizontalLine);
        let m = Mods {
            shift: true,
            ..no()
        };
        let fx = click(&mut s, (200.0, 150.0), m);
        assert_eq!(s.drawings.all().len(), 1);
        assert_eq!(s.tool, Tool::HorizontalLine);
        assert!(!fx.contains(&ChartEffect::ToolFinished));
    }

    #[test]
    fn drag_places_rectangle() {
        let mut s = state(500, false);
        s.set_tool(Tool::Rectangle);
        s.mouse_down((200.0, 150.0), no(), 1);
        s.mouse_move((250.0, 170.0), no(), true);
        s.mouse_move((300.0, 200.0), no(), true);
        let fx = s.mouse_up((300.0, 200.0), no());
        assert_eq!(s.drawings.all().len(), 1);
        assert!(matches!(
            s.drawings.all()[0].kind,
            DrawingKind::Rectangle { .. }
        ));
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.tool, Tool::Cursor);
        // The view did not pan while placing.
        assert_eq!(s.vp.right_index, 500.0 - 1.0 + RIGHT_MARGIN_BARS);
    }

    #[test]
    fn cmd_snaps_to_high() {
        let mut s = state(500, false);
        let k = 450usize;
        let c = s.series.candles()[k];
        let x = s.vp.x_of_index(k as f64) + 1.5;
        let y = s.vp.y_of_price(c.high) + 2.0;
        s.set_tool(Tool::TrendLine);
        let m = Mods { cmd: true, ..no() };
        click(&mut s, (x, y), m);
        click(&mut s, (x, y), m);
        let (a, _) = ends(&s, s.drawings.all()[0].id);
        assert_eq!(a.price, c.high);
        assert_eq!(a.time_ms, c.open_time_ms);
        // Without cmd the price is the raw cursor price.
        let mut s = state(500, false);
        s.set_tool(Tool::HorizontalLine);
        click(&mut s, (x, y), no());
        match s.drawings.all()[0].kind {
            DrawingKind::HorizontalLine { price } => assert_ne!(price, c.high),
            _ => panic!(),
        }
    }

    #[test]
    fn magnet_snaps_without_cmd() {
        let mut s = state(500, false);
        s.magnet = true;
        let c = s.series.candles()[450];
        s.set_tool(Tool::HorizontalLine);
        let pt = (s.vp.x_of_index(450.0), s.vp.y_of_price(c.low) - 2.0);
        click(&mut s, pt, no());
        assert_eq!(
            s.drawings.all()[0].kind,
            DrawingKind::HorizontalLine { price: c.low }
        );
    }

    #[test]
    fn drag_moves_without_drift() {
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        s.mouse_down((300.0, 125.0), no(), 1);
        assert_eq!(s.selected, Some(id));
        for i in 1..=10 {
            s.mouse_move((300.0 + i as f32 * 10.0, 125.0), no(), true);
        }
        let (a1, _) = ends(&s, id);
        assert_ne!(a1.time_ms, a0.time_ms);
        for i in (0..10).rev() {
            s.mouse_move((300.0 + i as f32 * 10.0, 125.0), no(), true);
        }
        s.mouse_up((300.0, 125.0), no());
        let (a, b) = ends(&s, id);
        assert!((a.time_ms - a0.time_ms).abs() <= 1 && (b.time_ms - b0.time_ms).abs() <= 1);
        assert!((a.price - a0.price).abs() < 1e-9 && (b.price - b0.price).abs() < 1e-9);
    }

    #[test]
    fn body_drag_moves_and_is_one_undo_step() {
        let mut s = state(500, false);
        let (id, a0, _) = trend(&mut s);
        s.mouse_down((300.0, 125.0), no(), 1);
        for i in 1..=5 {
            s.mouse_move(
                (300.0 + i as f32 * 10.0, 125.0 + i as f32 * 4.0),
                no(),
                true,
            );
        }
        let fx = s.mouse_up((350.0, 145.0), no());
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_ne!(ends(&s, id).0, a0);
        let fx = s.key(ChartKey::Undo);
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(ends(&s, id).0, a0);
    }

    #[test]
    fn press_release_without_move_selects_only() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        let before = s.drawings.all().to_vec();
        let fx = click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, Some(id));
        assert!(!fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.drawings.all(), &before[..]);
        let fx = s.key(ChartKey::Undo);
        assert!(!fx.contains(&ChartEffect::DrawingsChanged));
    }

    #[test]
    fn miss_deselects_and_pans() {
        let mut s = state(500, false);
        let (_, _, _) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        assert!(s.selected.is_some());
        let r0 = s.vp.right_index;
        s.mouse_down((100.0, 350.0), no(), 1);
        assert!(s.selected.is_none());
        s.mouse_move((140.0, 350.0), no(), true);
        s.mouse_up((140.0, 350.0), no());
        assert_ne!(s.vp.right_index, r0);
    }

    #[test]
    fn handle_drag_resizes_one_end() {
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        let h = scr(&s, b0);
        s.mouse_down(h, no(), 1);
        s.mouse_move((h.0 + 20.0, h.1 + 10.0), no(), true);
        s.mouse_move((h.0 + 50.0, h.1 + 30.0), no(), true);
        s.mouse_up((h.0 + 50.0, h.1 + 30.0), no());
        let (a, b) = ends(&s, id);
        assert_eq!(a, a0);
        assert_ne!(b, b0);
        let want = at(&s, (h.0 + 50.0, h.1 + 30.0));
        assert_eq!(b, want);
    }

    #[test]
    fn delete_and_undo_restore() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        let before = s.drawings.all().to_vec();
        click(&mut s, (300.0, 125.0), no());
        let fx = s.key(ChartKey::Delete);
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert!(s.drawings.all().is_empty());
        assert_eq!(s.selected, None);
        let fx = s.key(ChartKey::Undo);
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.drawings.all(), &before[..]);
        assert!(s.drawings.get(id).is_some());
        s.key(ChartKey::Redo);
        assert!(s.drawings.all().is_empty());
    }

    #[test]
    fn escape_cancels_pending_then_deselects() {
        let mut s = state(500, false);
        s.set_tool(Tool::TrendLine);
        click(&mut s, (200.0, 150.0), no());
        assert!(s.placement_pending());
        s.key(ChartKey::Escape);
        assert!(!s.placement_pending());
        assert_eq!(s.tool, Tool::TrendLine);
        // Next click starts a fresh placement rather than finishing one.
        click(&mut s, (400.0, 100.0), no());
        assert!(s.drawings.all().is_empty());
        assert!(s.placement_pending());
        s.key(ChartKey::Escape);
        s.set_tool(Tool::Cursor);
        let (id, _, _) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, Some(id));
        let fx = s.key(ChartKey::Escape);
        assert_eq!(s.selected, None);
        assert!(fx.contains(&ChartEffect::SelectionChanged));
    }

    #[test]
    fn locked_and_hidden_drawings_not_selectable() {
        let mut s = state(500, false);
        trend(&mut s);
        s.drawings_locked = true;
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, None);
        s.drawings_locked = false;
        s.drawings_hidden = true;
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, None);
        s.drawings_hidden = false;
        click(&mut s, (300.0, 125.0), no());
        assert!(s.selected.is_some());
        s.drawings_hidden = true;
        assert_eq!(s.selected_screen_bounds(), None);
    }

    #[test]
    fn style_and_clear() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        assert!(s.set_selected_style(Style::default()).is_empty());
        click(&mut s, (300.0, 125.0), no());
        let st = Style {
            color: 0xFF0000FF,
            width: 3,
        };
        let fx = s.set_selected_style(st);
        assert_eq!(fx, vec![ChartEffect::DrawingsChanged]);
        assert_eq!(s.drawings.get(id).unwrap().style, st);
        let fx = s.clear_drawings();
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert!(s.drawings.all().is_empty());
        assert_eq!(s.selected, None);
        s.key(ChartKey::Undo);
        assert_eq!(s.drawings.all().len(), 1);
    }

    #[test]
    fn selected_bounds_cover_handles_and_clamp() {
        let mut s = state(500, false);
        assert_eq!(s.selected_screen_bounds(), None);
        let (_, a, b) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        let r = s.selected_screen_bounds().unwrap();
        let (pa, pb) = (scr(&s, a), scr(&s, b));
        assert!((r.x - pa.0.min(pb.0)).abs() < 1e-3);
        assert!((r.y - pa.1.min(pb.1)).abs() < 1e-3);
        assert!((r.w - (pa.0 - pb.0).abs()).abs() < 1e-3);
        assert!((r.h - (pa.1 - pb.1).abs()).abs() < 1e-3);
        // Scroll the drawing far off to the left: bounds stay inside the plot.
        s.vp.right_index -= 400.0;
        let r = s.selected_screen_bounds().unwrap();
        assert!(r.x >= 0.0 && r.x + r.w <= s.layout.plot.w);
    }

    #[test]
    fn drawings_stay_put_across_timeframes() {
        const MIN: i64 = 60_000;
        const HR: i64 = 3_600_000;
        let h0 = 1_700_000_000_000 / HR * HR;
        let m1: Vec<Candle> = (0..180)
            .map(|i| Candle {
                open_time_ms: h0 + i * MIN,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.5,
                volume: 1.0,
            })
            .collect();
        let sym = series(0..1, false).symbol.clone();
        let mut s = ChartState::new(
            Series::new(sym.clone(), Timeframe::M1, m1.clone(), false),
            872.0,
            428.0,
        );
        // Click on the 14:37-equivalent candle (hour h0 + 1h + 37m).
        let target = 60 + 37;
        let x = s.vp.x_of_index(target as f64);
        s.set_tool(Tool::VerticalLine);
        click(&mut s, (x, 200.0), no());
        let a = match s.drawings.all()[0].kind {
            DrawingKind::VerticalLine { time_ms } => time_ms,
            _ => panic!(),
        };
        assert!((a - (h0 + target * MIN)).abs() <= 1000);
        let h1 = resample(&m1, Timeframe::H1, TzOffset::UTC);
        s.set_series(Series::new(sym, Timeframe::H1, h1, false));
        let anchor = Anchor {
            time_ms: a,
            price: 100.0,
        };
        let sx = proj(&s).to_screen(anchor).0;
        let idx = time_to_index(s.series.candles(), s.series.timeframe, a);
        assert_eq!(sx, s.vp.x_of_index(idx));
        assert!((idx.fract() - 37.0 / 60.0).abs() < 0.01, "{}", idx.fract());
        assert_eq!(idx.floor(), 1.0);
    }

    #[test]
    fn host_hooks_and_review_followups() {
        // check_need_older is callable by the host.
        let mut s = state(500, true);
        s.vp.right_index = 20.0;
        let fx = s.check_need_older();
        assert_eq!(fx.len(), 1);
        assert!(s.loading_older);
        assert!(s.check_need_older().is_empty());
        s.older_failed();
        assert!(!s.loading_older);
        assert_eq!(s.check_need_older().len(), 1);
        // scroll with a degenerate viewport does nothing.
        let mut s = state(500, true);
        s.set_size(0.0, 0.0);
        let r = s.vp.right_index;
        assert!(s.scroll((0.0, 0.0), 10.0, 0.0, no()).is_empty());
        assert_eq!(s.vp.right_index, r);
        // merge_backfill keeps following.
        let mut s = state(500, false);
        assert!(s.vp.is_following(s.series.len()));
        let extra: Vec<Candle> = (1500..1510).map(candle).collect();
        s.merge_backfill(extra);
        assert_eq!(s.series.len(), 510);
        assert!(s.vp.is_following(510));
        assert_eq!(s.vp.right_index, 509.0 + RIGHT_MARGIN_BARS);
    }

    fn drag_body(s: &mut ChartState) -> DrawingId {
        let (id, _, _) = trend(s);
        s.mouse_down((300.0, 125.0), no(), 1);
        for i in 1..=5 {
            s.mouse_move((300.0 + i as f32 * 10.0, 125.0), no(), true);
        }
        id
    }

    #[test]
    fn release_outside_window_commits_drag() {
        let mut s = state(500, false);
        let (_, a0, _) = trend(&mut s);
        let id = drag_body(&mut s);
        let fx = s.mouse_move((350.0, 125.0), no(), false);
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_ne!(ends(&s, id).0, a0);
        let moved = ends(&s, id).0;
        assert!(s.mouse_move((360.0, 125.0), no(), false).is_empty());
        assert_eq!(ends(&s, id).0, moved);
        s.key(ChartKey::Undo);
        assert_eq!(ends(&s, id).0, a0);
        assert!(
            !s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
    }

    #[test]
    fn set_series_mid_drag_restores_snapshot() {
        let mut s = state(500, false);
        let (_, a0, _) = trend(&mut s);
        let id = drag_body(&mut s);
        assert_ne!(ends(&s, id).0, a0);
        let ser = series(1000..1500, false);
        s.set_series(ser);
        assert_eq!(ends(&s, id).0, a0);
        assert!(
            !s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
    }

    #[test]
    fn stale_mouse_down_commits_previous_drag() {
        let mut s = state(500, false);
        let (_, a0, _) = trend(&mut s);
        let id = drag_body(&mut s);
        let fx = s.mouse_down((100.0, 350.0), no(), 1);
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_ne!(ends(&s, id).0, a0);
        s.mouse_up((100.0, 350.0), no());
        s.key(ChartKey::Undo);
        assert_eq!(ends(&s, id).0, a0);
    }

    #[test]
    fn escape_aborts_in_progress_drags() {
        let mut s = state(500, false);
        let (_, a0, _) = trend(&mut s);
        let id = drag_body(&mut s);
        s.key(ChartKey::Escape);
        assert_eq!(ends(&s, id).0, a0);
        assert!(s.mouse_up((350.0, 125.0), no()).is_empty());
        assert!(
            !s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
        // Placement drag.
        let mut s = state(500, false);
        s.set_tool(Tool::Rectangle);
        s.mouse_down((200.0, 150.0), no(), 1);
        s.mouse_move((300.0, 200.0), no(), true);
        s.key(ChartKey::Escape);
        let fx = s.mouse_up((300.0, 200.0), no());
        assert!(fx.is_empty() && s.drawings.all().is_empty());
        assert!(!s.placement_pending());
    }

    #[test]
    fn escape_with_tool_and_nothing_else_returns_to_cursor() {
        let mut s = state(500, false);
        s.set_tool(Tool::Ray);
        let fx = s.key(ChartKey::Escape);
        assert_eq!(s.tool, Tool::Cursor);
        assert_eq!(fx, vec![ChartEffect::ToolFinished]);
    }

    #[test]
    fn set_tool_unhides_and_lock_hide_deselect() {
        let mut s = state(500, false);
        trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        assert!(s.selected.is_some());
        assert_eq!(s.set_locked(true), vec![ChartEffect::SelectionChanged]);
        assert!(s.drawings_locked && s.selected.is_none());
        s.set_locked(false);
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.set_hidden(true), vec![ChartEffect::SelectionChanged]);
        assert!(s.drawings_hidden && s.selected.is_none());
        s.set_tool(Tool::TrendLine);
        assert!(!s.drawings_hidden);
    }

    #[test]
    fn pending_placement_renders_preview() {
        let lines = |f: &Frame| {
            f.plot
                .iter()
                .filter(|p| matches!(p, Prim::Line { .. }))
                .count()
        };
        let mut s = state(500, false);
        s.set_tool(Tool::TrendLine);
        s.mouse_move((300.0, 100.0), no(), false);
        let base = lines(&s.frame(&ChartColors::dark(), "t"));
        click(&mut s, (200.0, 150.0), no());
        assert!(s.placement_pending());
        s.mouse_move((300.0, 100.0), no(), false);
        let f = s.frame(&ChartColors::dark(), "t");
        assert_eq!(lines(&f), base + 1);
        assert!(s.drawings.all().is_empty());
    }

    #[test]
    fn frame_builds() {
        let s = state(100, false);
        let f = s.frame(&ChartColors::dark(), "test");
        assert!(!f.plot.is_empty());
    }
}
