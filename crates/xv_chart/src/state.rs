//! GPUI-free navigation state machine: turns mouse/scroll/pinch input into
//! viewport changes. All coordinates are relative to the chart element origin.

use crate::{
    ChartColors, ChartLayout, Frame, FrameInput, MAX_BAR_SPACING, MIN_BAR_SPACING,
    RIGHT_MARGIN_BARS, RectF, Viewport, build_frame, time_to_index,
};
use crate::{
    ChartProjector,
    snap::{
        MAGNET_WEAK_PX, MagnetMode, constrain_angle, constrain_square, snap_anchor, snap_high_low,
        snap_ohlc,
    },
};
use std::collections::HashMap;
use xv_core::{Candle, Series, TzOffset, UpdateKind};
use xv_drawings::{
    Anchor, Drawing, DrawingId, DrawingKind, DrawingSet, Hit, PlaceResult, Projector, Style, Tool,
    Visibility, handles, hit_test, place_click, tool_of,
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
    /// A double-click on a drawing asks the host to open its settings.
    OpenSettings(DrawingId),
    /// `ChartState::tool_styles` changed (the host persists it).
    ToolStylesChanged,
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

/// A shift constraint for the moving point: an angle (or, for rectangles, a
/// square) around the `fixed` anchor.
#[derive(Copy, Clone, Debug)]
struct Constraint {
    fixed: Anchor,
    square: bool,
}

pub struct ChartState {
    pub series: Series,
    pub vp: Viewport,
    pub layout: ChartLayout,
    pub crosshair: Option<(f32, f32)>,
    pub drawings: DrawingSet,
    pub drawings_hidden: bool,
    pub drawings_locked: bool,
    /// Magnet on/off; `magnet_mode` says how it snaps.
    pub magnet: bool,
    pub magnet_mode: MagnetMode,
    /// Keep the drawing tool active after each placement.
    pub stay_in_drawing_mode: bool,
    /// Last-used style per tool; new drawings start from it.
    pub tool_styles: HashMap<Tool, Style>,
    pub tool: Tool,
    pub selected: Option<DrawingId>,
    pub loading_older: bool,
    pub tz: TzOffset,
    drag: Drag,
    /// First anchor of a click-click placement in progress.
    pending: Option<Anchor>,
    last_mods: Mods,
    /// Last pointer position (element space), for `modifiers_changed`.
    last_pointer: Option<(f32, f32)>,
    /// The edited drawing's `tool_styles` entries at `begin_edit`, restored
    /// by `cancel_edit`.
    edit_styles: Option<Vec<(Tool, Option<Style>)>>,
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
            magnet_mode: MagnetMode::default(),
            stay_in_drawing_mode: false,
            tool_styles: HashMap::new(),
            tool: Tool::Cursor,
            selected: None,
            loading_older: false,
            tz: TzOffset::default(),
            drag: Drag::None,
            pending: None,
            last_mods: Mods::default(),
            last_pointer: None,
            edit_styles: None,
        }
    }

    pub fn set_size(&mut self, w: f32, h: f32) {
        self.layout = ChartLayout::new(w, h);
        self.vp.width = self.layout.plot.w;
        self.vp.height = self.layout.plot.h;
        self.refit();
    }

    /// Replaces the series and resets the view to the latest candle, keeping
    /// the current zoom (`bar_spacing`). A selected drawing that the new
    /// timeframe does not show is deselected.
    pub fn set_series(&mut self, s: Series) -> Vec<ChartEffect> {
        self.series = s;
        self.loading_older = false;
        self.abort_drag();
        self.pending = None;
        let spacing = self.vp.bar_spacing;
        self.vp = Viewport::new(self.layout.plot.w, self.layout.plot.h);
        self.vp.bar_spacing = spacing;
        self.vp.scroll_to_latest(self.series.len());
        self.vp.autofit(self.series.candles());
        let mut fx = Vec::new();
        self.drop_invisible_selection(&mut fx);
        fx
    }

    pub fn set_drawings(&mut self, ds: Vec<Drawing>) {
        self.drawings = DrawingSet::new(ds);
        self.selected = None;
        self.pending = None;
        self.drag = Drag::None;
    }

    pub fn mouse_down(&mut self, p: (f32, f32), m: Mods, click_count: usize) -> Vec<ChartEffect> {
        self.last_mods = m;
        self.last_pointer = Some(p);
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
                if click_count == 2 {
                    fx.push(ChartEffect::OpenSettings(id));
                } else if let Some(d) = self.drawings.get(id).filter(|d| !d.locked).cloned() {
                    // A locked drawing is selected only: no drag starts.
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
        self.last_pointer = Some(p);
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
                self.drag = Drag::Handle {
                    start,
                    idx,
                    snapshot,
                    moved,
                };
                if moved {
                    self.drag_handle_to(p, m);
                }
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
        self.last_pointer = Some(p);
        self.finish_drag(p, m)
    }

    fn finish_drag(&mut self, p: (f32, f32), m: Mods) -> Vec<ChartEffect> {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        let mut fx = Vec::new();
        match drag {
            Drag::Place { start, first } => {
                if dist(start, p) > DRAG_THRESHOLD {
                    let to = self.anchor_at(p, m, self.constraint_from(first));
                    self.advance_placement(Some(first), to, &mut fx);
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
        self.last_pointer = None;
    }

    /// The modifiers changed without a mouse move: re-evaluates the last
    /// pointer position, so the crosshair, the placement preview and an
    /// active handle drag follow at once. Never starts or finishes a drag.
    pub fn modifiers_changed(&mut self, m: Mods) -> Vec<ChartEffect> {
        self.last_mods = m;
        if let (Some(p), Drag::Handle { moved: true, .. }) = (self.last_pointer, &self.drag) {
            self.drag_handle_to(p, m);
        }
        // The crosshair and the preview are derived from `last_mods` when
        // the frame is built.
        Vec::new()
    }

    /// The crosshair point to draw (element space): the snapped anchor under
    /// the pointer while a drawing tool is active, cmd is held or a handle is
    /// being dragged; otherwise the raw pointer.
    pub fn crosshair_snapped(&self) -> Option<(f32, f32)> {
        let c = self.crosshair?;
        let snapping = self.tool != Tool::Cursor
            || self.last_mods.cmd
            || matches!(self.drag, Drag::Handle { moved: true, .. });
        if !snapping {
            return Some(c);
        }
        let a = self.anchor_at(c, self.last_mods, self.active_constraint());
        let s = self.proj().to_screen(a);
        Some((s.0 + self.layout.plot.x, s.1 + self.layout.plot.y))
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
        let at = self.anchor_at(c, self.last_mods, self.constraint_from(first));
        match place_click(self.tool, Some(first), at)? {
            PlaceResult::Done(kind) => Some(Drawing::new(
                DrawingId(u64::MAX),
                kind,
                self.style_for(self.tool),
            )),
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
            crosshair: self.crosshair_snapped(),
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

    pub fn selected_drawing(&self) -> Option<&Drawing> {
        self.drawings.get(self.selected?)
    }

    /// Selects the topmost drawing under `p` (element space), or deselects.
    /// For the context menu.
    pub fn select_at(&mut self, p: (f32, f32)) -> (Option<DrawingId>, Vec<ChartEffect>) {
        let id = contains(&self.layout.plot, p)
            .then(|| self.hit(p).map(|(id, _)| id))
            .flatten();
        let mut fx = Vec::new();
        self.select(id, &mut fx);
        (id, fx)
    }

    /// Edits the selected drawing as one undo step (the id cannot change).
    /// Records its style as the last-used one for its tool.
    pub fn update_selected(&mut self, f: impl FnOnce(&mut Drawing)) -> Vec<ChartEffect> {
        let Some(cur) = self.selected_drawing().cloned() else {
            return Vec::new();
        };
        let mut d = cur.clone();
        f(&mut d);
        d.id = cur.id;
        if d == cur || !self.drawings.update(cur.id, |x| *x = d.clone()) {
            return Vec::new();
        }
        let mut fx = vec![ChartEffect::DrawingsChanged];
        self.record_style(&d, &mut fx);
        self.drop_invisible_selection(&mut fx);
        fx
    }

    /// Starts a settings-dialog edit: the selected drawing as it is now.
    /// Starts no gesture; pass the result back to `commit_edit`/`cancel_edit`.
    pub fn begin_edit(&mut self) -> Option<Drawing> {
        let d = self.selected_drawing()?.clone();
        // A dialog edit can turn a trend line into an extended line (and
        // back), so both entries of a trend line are kept.
        let tools = match tool_of(&d) {
            Tool::TrendLine | Tool::ExtendedLine => vec![Tool::TrendLine, Tool::ExtendedLine],
            t => vec![t],
        };
        self.edit_styles = Some(
            tools
                .into_iter()
                .map(|t| (t, self.tool_styles.get(&t).cloned()))
                .collect(),
        );
        Some(d)
    }

    /// Live preview of a dialog edit, without undo. Only for the selected id.
    pub fn preview_edit(&mut self, d: Drawing) -> Vec<ChartEffect> {
        if self.selected != Some(d.id) || !d.is_finite() || self.drawings.get(d.id) == Some(&d) {
            return Vec::new();
        }
        self.drawings.restore_without_undo(d.clone());
        let mut fx = vec![ChartEffect::DrawingsChanged];
        self.record_style(&d, &mut fx);
        fx
    }

    /// Ends a dialog edit: one undo step from `original` to the current state
    /// (none when nothing changed).
    pub fn commit_edit(&mut self, original: Drawing) -> Vec<ChartEffect> {
        self.edit_styles = None;
        let id = original.id;
        let Some(cur) = self.drawings.get(id).cloned() else {
            return Vec::new();
        };
        if cur == original {
            return Vec::new();
        }
        self.drawings.restore_without_undo(original);
        self.drawings.update(id, |x| *x = cur.clone());
        let mut fx = vec![ChartEffect::DrawingsChanged];
        self.record_style(&cur, &mut fx);
        self.drop_invisible_selection(&mut fx);
        fx
    }

    /// Abandons a dialog edit: puts `original` back without undo, and the
    /// last-used styles as they were at `begin_edit`.
    pub fn cancel_edit(&mut self, original: Drawing) -> Vec<ChartEffect> {
        let mut fx = Vec::new();
        if self
            .drawings
            .get(original.id)
            .is_some_and(|cur| *cur != original)
        {
            self.drawings.restore_without_undo(original);
            fx.push(ChartEffect::DrawingsChanged);
        }
        let mut changed = false;
        for (tool, saved) in self.edit_styles.take().unwrap_or_default() {
            if self.tool_styles.get(&tool) != saved.as_ref() {
                match saved {
                    Some(st) => self.tool_styles.insert(tool, st),
                    None => self.tool_styles.remove(&tool),
                };
                changed = true;
            }
        }
        if changed {
            fx.push(ChartEffect::ToolStylesChanged);
        }
        fx
    }

    /// Duplicates the selected drawing 10 px right and down, and selects it.
    pub fn clone_selected(&mut self) -> Vec<ChartEffect> {
        let Some(d) = self.selected_drawing().cloned() else {
            return Vec::new();
        };
        let shifted = d.translated(10.0, 10.0, &self.proj());
        self.insert_and_select(&shifted)
    }

    /// Inserts a copy of `d` (same anchors, new id) and selects it when it is
    /// shown on this timeframe.
    pub fn paste(&mut self, d: &Drawing) -> Vec<ChartEffect> {
        self.insert_and_select(d)
    }

    fn insert_and_select(&mut self, d: &Drawing) -> Vec<ChartEffect> {
        let Some(id) = self.drawings.insert_copy(d) else {
            return Vec::new();
        };
        let mut fx = vec![ChartEffect::DrawingsChanged];
        if self.drawings.get(id).is_some_and(|c| self.is_shown(c)) {
            self.select(Some(id), &mut fx);
        }
        fx
    }

    pub fn bring_selected_to_front(&mut self) -> Vec<ChartEffect> {
        self.reorder_selected(DrawingSet::bring_to_front)
    }

    pub fn send_selected_to_back(&mut self) -> Vec<ChartEffect> {
        self.reorder_selected(DrawingSet::send_to_back)
    }

    fn reorder_selected(&mut self, op: fn(&mut DrawingSet, DrawingId)) -> Vec<ChartEffect> {
        let Some(id) = self.selected else {
            return Vec::new();
        };
        let order = |s: &Self| s.drawings.all().iter().map(|d| d.id).collect::<Vec<_>>();
        let before = order(self);
        op(&mut self.drawings, id);
        if order(self) == before {
            Vec::new()
        } else {
            vec![ChartEffect::DrawingsChanged]
        }
    }

    pub fn set_selected_locked(&mut self, locked: bool) -> Vec<ChartEffect> {
        self.update_selected(|d| d.locked = locked)
    }

    /// Hides the selected drawing (one undo step) and deselects it.
    pub fn hide_selected(&mut self) -> Vec<ChartEffect> {
        self.update_selected(|d| d.hidden = true)
    }

    /// Shows every individually hidden drawing, as one undo step.
    pub fn unhide_all(&mut self) -> Vec<ChartEffect> {
        let hidden: Vec<Drawing> = self
            .drawings
            .all()
            .iter()
            .filter(|d| d.hidden)
            .cloned()
            .collect();
        if hidden.is_empty() {
            return Vec::new();
        }
        self.drawings.begin_gesture();
        for mut d in hidden {
            d.hidden = false;
            self.drawings.replace(d);
        }
        self.drawings.end_gesture();
        vec![ChartEffect::DrawingsChanged]
    }

    /// Number of individually hidden drawings.
    pub fn hidden_count(&self) -> usize {
        self.drawings.all().iter().filter(|d| d.hidden).count()
    }

    /// Records `d`'s style as its tool's last-used style. The text content
    /// is per drawing, and the timeframe visibility is left at the default so
    /// a new drawing never starts out invisible.
    fn record_style(&mut self, d: &Drawing, fx: &mut Vec<ChartEffect>) {
        let mut st = d.style.clone();
        st.text.content.clear();
        st.visibility = Visibility::default();
        let tool = tool_of(d);
        if self.tool_styles.get(&tool) != Some(&st) {
            self.tool_styles.insert(tool, st);
            fx.push(ChartEffect::ToolStylesChanged);
        }
    }

    /// The style a new drawing of `tool` starts with.
    fn style_for(&self, tool: Tool) -> Style {
        self.tool_styles
            .get(&tool)
            .cloned()
            .unwrap_or_else(|| tool.default_style())
    }

    /// Whether `d` is drawn and hit-testable on the current timeframe.
    fn is_shown(&self, d: &Drawing) -> bool {
        !d.hidden && d.style.visibility.shows(self.series.timeframe)
    }

    /// Deselects a selected drawing that is gone or no longer shown.
    fn drop_invisible_selection(&mut self, fx: &mut Vec<ChartEffect>) {
        if let Some(id) = self.selected
            && !self.drawings.get(id).is_some_and(|d| self.is_shown(d))
        {
            self.select(None, fx);
        }
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
                    self.drop_invisible_selection(&mut fx);
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
        if !self.is_shown(d) {
            return None;
        }
        let plot = &self.layout.plot;
        let pts = handles(d, &self.proj());
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

    fn proj(&self) -> ChartProjector<'_> {
        ChartProjector {
            candles: self.series.candles(),
            tf: self.series.timeframe,
            vp: &self.vp,
        }
    }

    /// Hit-tests the shown drawings at an element-space point.
    fn hit(&self, p: (f32, f32)) -> Option<(DrawingId, Hit)> {
        if self.drawings_hidden || self.drawings_locked {
            return None;
        }
        let shown: Vec<Drawing> = self
            .drawings
            .all()
            .iter()
            .filter(|d| self.is_shown(d))
            .cloned()
            .collect();
        hit_test(
            &shown,
            (p.0 - self.layout.plot.x, p.1 - self.layout.plot.y),
            &self.proj(),
        )
    }

    /// Anchor under an element-space point. Precedence: the shift constraint
    /// `c` (screen space, around its fixed anchor), then cmd High/Low, then
    /// the magnet (strong, or weak within `MAGNET_WEAK_PX`), then raw.
    fn anchor_at(&self, p: (f32, f32), m: Mods, c: Option<Constraint>) -> Anchor {
        let lp = (p.0 - self.layout.plot.x, p.1 - self.layout.plot.y);
        let proj = self.proj();
        let (cs, tf, vp) = (self.series.candles(), self.series.timeframe, &self.vp);
        if m.shift
            && let Some(c) = c
        {
            let f = proj.to_screen(c.fixed);
            let q = if c.square {
                constrain_square(f, lp)
            } else {
                constrain_angle(f, lp)
            };
            let mut a = proj.to_anchor(q);
            // Keep a shared axis bit-exact (no screen round trip).
            if q.1 == f.1 {
                a.price = c.fixed.price;
            }
            if q.0 == f.0 {
                a.time_ms = c.fixed.time_ms;
            }
            return a;
        }
        if m.cmd {
            return snap_high_low(cs, tf, vp, lp);
        }
        if self.magnet {
            match self.magnet_mode {
                MagnetMode::Strong => return snap_anchor(cs, tf, vp, lp),
                MagnetMode::Weak => {
                    if let Some(a) = snap_ohlc(cs, tf, vp, lp, Some(MAGNET_WEAK_PX)) {
                        return a;
                    }
                }
            }
        }
        proj.to_anchor(lp)
    }

    /// The shift constraint for the second point of the current tool.
    fn constraint_from(&self, first: Anchor) -> Option<Constraint> {
        match self.tool {
            Tool::TrendLine | Tool::ExtendedLine | Tool::Ray => Some(Constraint {
                fixed: first,
                square: false,
            }),
            Tool::Rectangle => Some(Constraint {
                fixed: first,
                square: true,
            }),
            Tool::Cursor | Tool::HorizontalLine | Tool::VerticalLine => None,
        }
    }

    /// The shift constraint for the moving point under the pointer now.
    fn active_constraint(&self) -> Option<Constraint> {
        match (&self.drag, self.pending) {
            (Drag::Handle { idx, snapshot, .. }, _) => handle_constraint(snapshot, *idx),
            (Drag::Place { first, .. }, _) => self.constraint_from(*first),
            (_, Some(first)) => self.constraint_from(first),
            _ => None,
        }
    }

    /// Moves the dragged handle to the (snapped) anchor under `p`.
    fn drag_handle_to(&mut self, p: (f32, f32), m: Mods) {
        let Drag::Handle { idx, snapshot, .. } = &self.drag else {
            return;
        };
        let to = self.anchor_at(p, m, handle_constraint(snapshot, *idx));
        let d = snapshot.with_handle(*idx, to);
        self.drawings.replace(d);
    }

    fn press_with_tool(&mut self, p: (f32, f32), m: Mods, fx: &mut Vec<ChartEffect>) {
        let c = self.pending.and_then(|first| self.constraint_from(first));
        let at = self.anchor_at(p, m, c);
        match self.pending {
            Some(first) => self.advance_placement(Some(first), at, fx),
            None => match place_click(self.tool, None, at) {
                Some(PlaceResult::Pending(first)) => {
                    // Wait for release: a drag finishes the shape, a click
                    // leaves the first point pending.
                    self.drag = Drag::Place { start: p, first };
                }
                Some(PlaceResult::Done(kind)) => self.finish_placement(kind, fx),
                None => {}
            },
        }
    }

    fn advance_placement(&mut self, first: Option<Anchor>, at: Anchor, fx: &mut Vec<ChartEffect>) {
        self.pending = None;
        match place_click(self.tool, first, at) {
            Some(PlaceResult::Done(kind)) => self.finish_placement(kind, fx),
            Some(PlaceResult::Pending(a)) => self.pending = Some(a),
            None => {}
        }
    }

    fn finish_placement(&mut self, kind: DrawingKind, fx: &mut Vec<ChartEffect>) {
        // A placement at a non-finite price (degenerate view) is dropped.
        let Some(id) = self.drawings.add(kind, self.style_for(self.tool)) else {
            return;
        };
        fx.push(ChartEffect::DrawingsChanged);
        self.select(Some(id), fx);
        if !self.stay_in_drawing_mode {
            self.tool = Tool::Cursor;
            fx.push(ChartEffect::ToolFinished);
        }
    }
}

/// The shift constraint for dragging handle `idx`: two-point lines keep the
/// other end fixed (angle), rectangles the opposite corner (square).
fn handle_constraint(d: &Drawing, idx: usize) -> Option<Constraint> {
    match (&d.kind, idx) {
        (DrawingKind::TrendLine { b, .. } | DrawingKind::Ray { b, .. }, 0) => Some(Constraint {
            fixed: *b,
            square: false,
        }),
        (DrawingKind::TrendLine { a, .. } | DrawingKind::Ray { a, .. }, 1) => Some(Constraint {
            fixed: *a,
            square: false,
        }),
        (DrawingKind::Rectangle { a, b }, i) if i < 4 => {
            // Corners: a, (b.time, a.price), b, (a.time, b.price).
            let corners = [
                *a,
                Anchor {
                    time_ms: b.time_ms,
                    price: a.price,
                },
                *b,
                Anchor {
                    time_ms: a.time_ms,
                    price: b.price,
                },
            ];
            Some(Constraint {
                fixed: corners[(i + 2) % 4],
                square: true,
            })
        }
        _ => None,
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
            name: None,
            locked: false,
            hidden: false,
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
    fn shift_no_longer_keeps_tool() {
        let mut s = state(500, false);
        s.set_tool(Tool::HorizontalLine);
        let fx = click(&mut s, (200.0, 150.0), sh());
        assert_eq!(s.drawings.all().len(), 1);
        assert_eq!(s.tool, Tool::Cursor);
        assert!(fx.contains(&ChartEffect::ToolFinished));
    }

    #[test]
    fn stay_in_drawing_mode_keeps_tool() {
        let mut s = state(500, false);
        s.stay_in_drawing_mode = true;
        s.set_tool(Tool::HorizontalLine);
        let fx = click(&mut s, (200.0, 150.0), no());
        assert_eq!(s.tool, Tool::HorizontalLine);
        assert!(!fx.contains(&ChartEffect::ToolFinished));
        click(&mut s, (200.0, 250.0), no());
        assert_eq!(s.drawings.all().len(), 2);
        assert_eq!(s.tool, Tool::HorizontalLine);
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
        assert!(s.update_selected(|d| d.style = Style::default()).is_empty());
        click(&mut s, (300.0, 125.0), no());
        let st = Style {
            color: 0xFF0000FF,
            width: 3,
            ..Style::default()
        };
        let fx = s.update_selected(|d| d.style = st.clone());
        assert_eq!(
            fx,
            vec![ChartEffect::DrawingsChanged, ChartEffect::ToolStylesChanged]
        );
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

    // ---- drawing polish: snapping, modes, state API ----

    fn sh() -> Mods {
        Mods {
            shift: true,
            ..no()
        }
    }

    fn cmd() -> Mods {
        Mods { cmd: true, ..no() }
    }

    /// Screen angle of b as seen from a, in degrees.
    fn angle(a: (f32, f32), b: (f32, f32)) -> f32 {
        (b.1 - a.1).atan2(b.0 - a.0).to_degrees()
    }

    fn on_45(deg: f32) -> bool {
        let r = deg.rem_euclid(45.0);
        r < 0.05 || r > 45.0 - 0.05
    }

    fn hline_price(s: &ChartState, i: usize) -> f64 {
        match s.drawings.all()[i].kind {
            DrawingKind::HorizontalLine { price } => price,
            ref k => panic!("not a horizontal line: {k:?}"),
        }
    }

    fn rect(s: &ChartState, i: usize) -> (Anchor, Anchor) {
        match s.drawings.all()[i].kind {
            DrawingKind::Rectangle { a, b } => (a, b),
            ref k => panic!("not a rectangle: {k:?}"),
        }
    }

    #[test]
    fn strong_magnet_always_snaps() {
        let mut s = state(500, false);
        s.magnet = true;
        s.magnet_mode = MagnetMode::Strong;
        let c = s.series.candles()[450];
        s.set_tool(Tool::HorizontalLine);
        // 40 px above the high.
        let p = (s.vp.x_of_index(450.0), s.vp.y_of_price(c.high) - 40.0);
        click(&mut s, p, no());
        assert_eq!(hline_price(&s, 0), c.high);
    }

    #[test]
    fn weak_magnet_snaps_only_near_levels() {
        let mut s = state(500, false);
        s.magnet = true;
        assert_eq!(s.magnet_mode, MagnetMode::Weak);
        let c = s.series.candles()[450];
        let x = s.vp.x_of_index(450.0);
        s.stay_in_drawing_mode = true;
        s.set_tool(Tool::HorizontalLine);
        let y = s.vp.y_of_price(c.high);
        click(&mut s, (x, y - 8.0), no());
        assert_eq!(hline_price(&s, 0), c.high);
        click(&mut s, (x, y - 40.0), no());
        assert_ne!(hline_price(&s, 1), c.high);
    }

    #[test]
    fn cmd_overrides_weak_magnet() {
        let mut s = state(500, false);
        s.magnet = true;
        let c = s.series.candles()[450];
        // 3 px from the close: the weak magnet alone picks the close.
        let p = (s.vp.x_of_index(450.0), s.vp.y_of_price(c.close) + 3.0);
        s.stay_in_drawing_mode = true;
        s.set_tool(Tool::HorizontalLine);
        click(&mut s, p, no());
        assert_eq!(hline_price(&s, 0), c.close);
        click(&mut s, p, cmd());
        assert_eq!(hline_price(&s, 1), c.high);
    }

    #[test]
    fn shift_constrains_second_point() {
        let mut s = state(500, false);
        s.set_tool(Tool::TrendLine);
        click(&mut s, (200.0, 150.0), sh());
        let first = at(&s, (200.0, 150.0));
        // ~5.7 degrees snaps to horizontal, with the exact same price.
        click(&mut s, (400.0, 170.0), sh());
        let (a, b) = ends(&s, s.drawings.all()[0].id);
        assert_eq!(a, first);
        assert_eq!(b.price, a.price);
        // ~42 degrees snaps to 45.
        s.set_tool(Tool::TrendLine);
        click(&mut s, (200.0, 150.0), no());
        click(&mut s, (300.0, 240.0), sh());
        let (a, b) = ends(&s, s.drawings.all()[1].id);
        assert_eq!(a, first);
        let deg = angle(scr(&s, a), scr(&s, b));
        assert!((deg - 45.0).abs() < 0.05, "{deg}");
        // Shift wins over cmd.
        s.set_tool(Tool::TrendLine);
        click(&mut s, (200.0, 150.0), no());
        let both = Mods {
            shift: true,
            cmd: true,
            ..no()
        };
        click(&mut s, (400.0, 163.0), both);
        let (a, b) = ends(&s, s.drawings.all().last().unwrap().id);
        assert_eq!(b.price, a.price);
        // Shift on a drag-placed ray end too.
        s.set_tool(Tool::Ray);
        s.mouse_down((200.0, 150.0), no(), 1);
        s.mouse_move((260.0, 90.0), sh(), true);
        s.mouse_up((290.0, 50.0), sh());
        let DrawingKind::Ray { a, b } = s.drawings.all()[3].kind else {
            panic!()
        };
        assert_eq!(a, first);
        assert!(on_45(angle(scr(&s, a), scr(&s, b))));
    }

    #[test]
    fn shift_constrains_handle_drag() {
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        let h = scr(&s, b0);
        s.mouse_down(h, sh(), 1);
        s.mouse_move((h.0 + 30.0, h.1 + 7.0), sh(), true);
        s.mouse_up((h.0 + 30.0, h.1 + 7.0), sh());
        let (a, b) = ends(&s, id);
        assert_eq!(a, a0);
        assert_ne!(b, b0);
        assert!(on_45(angle(scr(&s, a), scr(&s, b))));
        // Dragging handle 0 keeps b fixed.
        let ha = scr(&s, a);
        s.mouse_down(ha, sh(), 1);
        s.mouse_move((ha.0 - 40.0, ha.1 + 13.0), sh(), true);
        s.mouse_up((ha.0 - 40.0, ha.1 + 13.0), sh());
        let (a2, b2) = ends(&s, id);
        assert_eq!(b2, b);
        assert!(on_45(angle(scr(&s, b2), scr(&s, a2))));
    }

    #[test]
    fn rectangle_shift_is_square() {
        let mut s = state(500, false);
        s.set_tool(Tool::Rectangle);
        s.mouse_down((200.0, 150.0), no(), 1);
        s.mouse_move((300.0, 180.0), sh(), true);
        s.mouse_up((300.0, 180.0), sh());
        let (a, b) = rect(&s, 0);
        let (pa, pb) = (scr(&s, a), scr(&s, b));
        assert!(((pb.0 - pa.0) - 100.0).abs() < 0.05, "{pa:?} {pb:?}");
        assert!(((pb.1 - pa.1) - 100.0).abs() < 0.05, "{pa:?} {pb:?}");
        // A corner handle drag with shift keeps the opposite corner and squares.
        s.mouse_down(pb, sh(), 1);
        s.mouse_move((pb.0 + 40.0, pb.1 - 10.0), sh(), true);
        s.mouse_up((pb.0 + 40.0, pb.1 - 10.0), sh());
        let (a2, b2) = rect(&s, 0);
        assert_eq!(a2, a);
        let (pa, pb) = (scr(&s, a2), scr(&s, b2));
        assert!(
            ((pb.0 - pa.0) - (pb.1 - pa.1)).abs() < 0.05,
            "{pa:?} {pb:?}"
        );
        assert!((pb.0 - pa.0 - 140.0).abs() < 0.05);
    }

    #[test]
    fn body_drag_ignores_snapping() {
        let mut s = state(500, false);
        s.magnet = true;
        s.magnet_mode = MagnetMode::Strong;
        let (id, a0, _) = trend(&mut s);
        s.mouse_down((300.0, 125.0), cmd(), 1);
        s.mouse_move((320.0, 131.0), cmd(), true);
        s.mouse_up((320.0, 131.0), cmd());
        let (a, _) = ends(&s, id);
        let pa = scr(&s, a);
        let p0 = scr(&s, a0);
        assert!((pa.0 - p0.0 - 20.0).abs() < 0.01 && (pa.1 - p0.1 - 6.0).abs() < 0.01);
    }

    #[test]
    fn modifiers_changed_updates_preview_and_crosshair() {
        let mut s = state(500, false);
        s.set_tool(Tool::TrendLine);
        click(&mut s, (200.0, 150.0), no());
        s.mouse_move((400.0, 165.0), no(), false);
        let c = s.crosshair_snapped().unwrap();
        assert!((c.0 - 400.0).abs() < 0.01 && (c.1 - 165.0).abs() < 0.01);
        // Shift: the preview and crosshair go horizontal without a move.
        assert!(s.modifiers_changed(sh()).is_empty());
        let p = s.placement_preview().unwrap();
        let DrawingKind::TrendLine { a, b } = p.kind else {
            panic!()
        };
        assert_eq!(b.price, a.price);
        assert!((s.crosshair_snapped().unwrap().1 - 150.0).abs() < 0.01);
        // Cmd: the crosshair jumps to the bar's high or low.
        s.modifiers_changed(cmd());
        let idx = s.vp.index_of_x(400.0).round() as usize;
        let cd = s.series.candles()[idx];
        let y = s.crosshair_snapped().unwrap().1;
        let want = [cd.high, cd.low].map(|p| s.vp.y_of_price(p));
        assert!(want.iter().any(|w| (w - y).abs() < 0.01), "{y} {want:?}");
        assert_eq!(s.crosshair, Some((400.0, 165.0)));
        // Release: back to the raw point.
        s.modifiers_changed(no());
        assert!((s.crosshair_snapped().unwrap().1 - 165.0).abs() < 0.01);
        // Cursor mode without cmd: no snapping of the crosshair.
        s.set_tool(Tool::Cursor);
        s.magnet = true;
        s.modifiers_changed(no());
        assert_eq!(s.crosshair_snapped(), Some((400.0, 165.0)));

        // Active handle drag is re-applied with the new modifiers.
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        let h = scr(&s, b0);
        s.mouse_down(h, no(), 1);
        s.mouse_move((h.0 + 30.0, h.1 + 7.0), no(), true);
        let (_, b_raw) = ends(&s, id);
        s.modifiers_changed(sh());
        let (a, b) = ends(&s, id);
        assert_eq!(a, a0);
        assert_ne!(b, b_raw);
        assert!(on_45(angle(scr(&s, a), scr(&s, b))));
        s.modifiers_changed(no());
        assert_eq!(ends(&s, id).1, b_raw);
        s.mouse_up((h.0 + 30.0, h.1 + 7.0), no());
        s.key(ChartKey::Undo);
        assert_eq!(ends(&s, id), (a0, b0));
    }

    fn set_flags(s: &mut ChartState, id: DrawingId, f: impl FnOnce(&mut Drawing)) {
        let mut d = s.drawings.get(id).unwrap().clone();
        f(&mut d);
        s.drawings.restore_without_undo(d);
    }

    #[test]
    fn locked_drawing_selectable_not_draggable() {
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        set_flags(&mut s, id, |d| d.locked = true);
        let r0 = s.vp.right_index;
        let fx = s.mouse_down((300.0, 125.0), no(), 1);
        assert!(fx.contains(&ChartEffect::SelectionChanged));
        s.mouse_move((350.0, 145.0), no(), true);
        let fx = s.mouse_up((350.0, 145.0), no());
        assert!(!fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.selected, Some(id));
        assert_eq!(ends(&s, id), (a0, b0));
        assert_eq!(s.vp.right_index, r0);
        // Handles do not drag either.
        let h = scr(&s, b0);
        s.mouse_down(h, no(), 1);
        s.mouse_move((h.0 + 50.0, h.1), no(), true);
        s.mouse_up((h.0 + 50.0, h.1), no());
        assert_eq!(ends(&s, id), (a0, b0));
        // Delete still works.
        s.key(ChartKey::Delete);
        assert!(s.drawings.all().is_empty());
    }

    #[test]
    fn visibility_hidden_not_hittable_and_deselects_on_tf_change() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        set_flags(&mut s, id, |d| d.hidden = true);
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, None);
        assert_eq!(s.select_at((300.0, 125.0)).0, None);
        set_flags(&mut s, id, |d| {
            d.hidden = false;
            d.style.visibility.minutes.on = false;
        });
        // Not shown on M5.
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, None);
        set_flags(&mut s, id, |d| {
            d.style.visibility.minutes.on = true;
            d.style.visibility.hours.on = false;
        });
        click(&mut s, (300.0, 125.0), no());
        assert_eq!(s.selected, Some(id));
        assert!(s.selected_screen_bounds().is_some());
        // Switch to H1: the selection goes away.
        let sym = s.series.symbol.clone();
        let h1: Vec<Candle> = (0..300)
            .map(|i| Candle {
                open_time_ms: T0 + i * 3_600_000,
                ..candle(i)
            })
            .collect();
        let fx = s.set_series(Series::new(sym, Timeframe::H1, h1, false));
        assert_eq!(fx, vec![ChartEffect::SelectionChanged]);
        assert_eq!(s.selected, None);
        assert_eq!(s.selected_screen_bounds(), None);
    }

    #[test]
    fn double_click_emits_open_settings() {
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        let fx = s.mouse_down((300.0, 125.0), no(), 2);
        assert!(fx.contains(&ChartEffect::OpenSettings(id)));
        s.mouse_move((340.0, 125.0), no(), true);
        s.mouse_up((340.0, 125.0), no());
        assert_eq!(s.selected, Some(id));
        assert_eq!(ends(&s, id), (a0, b0));
        // Double-click on empty space opens nothing.
        let fx = s.mouse_down((100.0, 350.0), no(), 2);
        assert!(!fx.iter().any(|e| matches!(e, ChartEffect::OpenSettings(_))));
    }

    #[test]
    fn select_at_selects_topmost_or_clears() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        assert_eq!(
            s.select_at((300.0, 125.0)),
            (Some(id), vec![ChartEffect::SelectionChanged])
        );
        assert_eq!(s.selected, Some(id));
        assert_eq!(s.selected_drawing().map(|d| d.id), Some(id));
        assert_eq!(s.select_at((300.0, 125.0)), (Some(id), vec![]));
        assert_eq!(
            s.select_at((100.0, 350.0)),
            (None, vec![ChartEffect::SelectionChanged])
        );
        assert_eq!(s.selected, None);
        assert!(s.selected_drawing().is_none());
    }

    #[test]
    fn edit_cancel_restores_without_undo() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        assert!(s.begin_edit().is_none());
        click(&mut s, (300.0, 125.0), no());
        let orig = s.begin_edit().unwrap();
        let mut d = orig.clone();
        d.style.color = 0x00FF00FF;
        let fx = s.preview_edit(d.clone());
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.drawings.get(id).unwrap().style.color, 0x00FF00FF);
        assert_eq!(s.tool_styles[&Tool::TrendLine].color, 0x00FF00FF);
        // A preview for another id is ignored.
        let mut other = d.clone();
        other.id = DrawingId(77);
        assert!(s.preview_edit(other).is_empty());
        let fx = s.cancel_edit(orig.clone());
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert!(fx.contains(&ChartEffect::ToolStylesChanged));
        assert_eq!(s.drawings.get(id).unwrap(), &orig);
        assert!(s.tool_styles.is_empty());
        assert!(
            !s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
    }

    #[test]
    fn edit_commit_one_undo_step() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        // Committing an unchanged edit does nothing.
        let orig = s.begin_edit().unwrap();
        assert!(s.commit_edit(orig.clone()).is_empty());
        let orig = s.begin_edit().unwrap();
        let mut d = orig.clone();
        d.style.width = 3;
        s.preview_edit(d.clone());
        d.style.width = 4;
        d.name = Some("support".into());
        s.preview_edit(d.clone());
        let fx = s.commit_edit(orig.clone());
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert_eq!(s.drawings.get(id).unwrap(), &d);
        assert_eq!(s.tool_styles[&Tool::TrendLine].width, 4);
        assert!(
            s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
        assert_eq!(s.drawings.get(id).unwrap(), &orig);
        assert!(
            !s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
        s.key(ChartKey::Redo);
        assert_eq!(s.drawings.get(id).unwrap(), &d);
    }

    #[test]
    fn clone_paste_zorder_hide_unhide() {
        let mut s = state(500, false);
        let (id, a0, b0) = trend(&mut s);
        assert!(s.clone_selected().is_empty());
        click(&mut s, (300.0, 125.0), no());
        let fx = s.clone_selected();
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert!(fx.contains(&ChartEffect::SelectionChanged));
        let copy = s.selected.unwrap();
        assert_ne!(copy, id);
        assert_eq!(s.drawings.all().last().unwrap().id, copy);
        let (ca, cb) = ends(&s, copy);
        for (c, o) in [(ca, a0), (cb, b0)] {
            let (pc, po) = (scr(&s, c), scr(&s, o));
            assert!((pc.0 - po.0 - 10.0).abs() < 0.01 && (pc.1 - po.1 - 10.0).abs() < 0.01);
        }
        // Paste: same anchors, new id, selected, on top.
        let orig = s.drawings.get(id).unwrap().clone();
        let fx = s.paste(&orig);
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        let pasted = s.selected.unwrap();
        assert!(pasted != id && pasted != copy);
        assert_eq!(ends(&s, pasted), (a0, b0));
        assert_eq!(s.drawings.all().len(), 3);
        // Z-order.
        assert_eq!(
            s.send_selected_to_back(),
            vec![ChartEffect::DrawingsChanged]
        );
        assert_eq!(s.drawings.all()[0].id, pasted);
        assert!(s.send_selected_to_back().is_empty());
        assert_eq!(
            s.bring_selected_to_front(),
            vec![ChartEffect::DrawingsChanged]
        );
        assert_eq!(s.drawings.all().last().unwrap().id, pasted);
        assert!(s.bring_selected_to_front().is_empty());
        // Lock.
        s.set_selected_locked(true);
        assert!(s.drawings.get(pasted).unwrap().locked);
        // Hide / unhide.
        assert_eq!(s.hidden_count(), 0);
        let fx = s.hide_selected();
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert!(fx.contains(&ChartEffect::SelectionChanged));
        assert_eq!(s.selected, None);
        assert_eq!(s.hidden_count(), 1);
        let _ = s.select_at(scr(&s, ca));
        s.hide_selected();
        assert_eq!(s.hidden_count(), 2);
        assert_eq!(s.unhide_all(), vec![ChartEffect::DrawingsChanged]);
        assert_eq!(s.hidden_count(), 0);
        assert!(s.unhide_all().is_empty());
        // Unhide-all is one undo step.
        s.key(ChartKey::Undo);
        assert_eq!(s.hidden_count(), 2);
    }

    #[test]
    fn new_drawing_uses_tool_style() {
        let mut s = state(500, false);
        let st = Style {
            color: 0x123456FF,
            width: 3,
            ..Style::default()
        };
        s.tool_styles.insert(Tool::TrendLine, st.clone());
        s.set_tool(Tool::TrendLine);
        click(&mut s, (200.0, 150.0), no());
        s.mouse_move((300.0, 120.0), no(), false);
        assert_eq!(s.placement_preview().unwrap().style, st);
        click(&mut s, (400.0, 100.0), no());
        assert_eq!(s.drawings.all()[0].style, st);
        // No stored style: the tool's default.
        s.set_tool(Tool::ExtendedLine);
        click(&mut s, (200.0, 150.0), no());
        click(&mut s, (400.0, 100.0), no());
        assert_eq!(
            s.drawings.all()[1].style,
            Tool::ExtendedLine.default_style()
        );
    }

    #[test]
    fn update_selected_records_last_used() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        assert!(s.update_selected(|d| d.style.width = 4).is_empty());
        click(&mut s, (300.0, 125.0), no());
        let fx = s.update_selected(|d| {
            d.style.color = 0xABCDEFFF;
            d.style.text.content = "note".into();
        });
        assert!(fx.contains(&ChartEffect::DrawingsChanged));
        assert!(fx.contains(&ChartEffect::ToolStylesChanged));
        let d = s.drawings.get(id).unwrap();
        assert_eq!(d.style.text.content, "note");
        let last = &s.tool_styles[&Tool::TrendLine];
        assert_eq!(last.color, 0xABCDEFFF);
        assert!(last.text.content.is_empty());
        // A no-op update changes nothing.
        assert!(s.update_selected(|d| d.style.color = 0xABCDEFFF).is_empty());
        // One undo step.
        s.key(ChartKey::Undo);
        assert_eq!(s.drawings.get(id).unwrap().style, Style::default());
        assert!(
            !s.key(ChartKey::Undo)
                .contains(&ChartEffect::DrawingsChanged)
        );
        // Hiding visibility on the current timeframe deselects.
        click(&mut s, (300.0, 125.0), no());
        let fx = s.update_selected(|d| d.style.visibility.minutes.on = false);
        assert!(fx.contains(&ChartEffect::SelectionChanged));
        assert_eq!(s.selected, None);
        // The last-used style never carries a hiding visibility.
        assert_eq!(
            s.tool_styles[&Tool::TrendLine].visibility,
            xv_drawings::Visibility::default()
        );
    }

    #[test]
    fn paste_of_unshown_drawing_keeps_selection() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        let mut hidden = s.drawings.get(id).unwrap().clone();
        hidden.style.visibility.minutes.on = false;
        let fx = s.paste(&hidden);
        assert_eq!(fx, vec![ChartEffect::DrawingsChanged]);
        assert_eq!(s.selected, Some(id));
        assert_eq!(s.drawings.all().len(), 2);
        // Nothing selected before: still nothing, no SelectionChanged.
        s.key(ChartKey::Escape);
        hidden.hidden = true;
        assert_eq!(s.paste(&hidden), vec![ChartEffect::DrawingsChanged]);
        assert_eq!(s.selected, None);
    }

    #[test]
    fn cancel_edit_restores_only_the_edited_tool_entry() {
        let mut s = state(500, false);
        let (id, _, _) = trend(&mut s);
        click(&mut s, (300.0, 125.0), no());
        let orig = s.begin_edit().unwrap();
        let mut d = orig.clone();
        d.style.color = 0x00FF00FF;
        s.preview_edit(d.clone());
        // Another tool's last-used style changes meanwhile; it is kept.
        let ray = Style {
            width: 4,
            ..Style::default()
        };
        s.tool_styles.insert(Tool::Ray, ray.clone());
        // Extend Both makes it an extended line: that entry is restored too.
        d.style.extend = xv_drawings::Extend::Both;
        s.preview_edit(d);
        assert!(s.tool_styles.contains_key(&Tool::ExtendedLine));
        let fx = s.cancel_edit(orig.clone());
        assert!(fx.contains(&ChartEffect::ToolStylesChanged));
        assert_eq!(s.drawings.get(id).unwrap(), &orig);
        assert_eq!(s.tool_styles.len(), 1);
        assert_eq!(s.tool_styles[&Tool::Ray], ray);
    }

    #[test]
    fn edit_ends_after_mid_edit_delete() {
        for commit in [true, false] {
            let mut s = state(500, false);
            let (id, _, _) = trend(&mut s);
            click(&mut s, (300.0, 125.0), no());
            let orig = s.begin_edit().unwrap();
            let mut d = orig.clone();
            d.style.width = 4;
            s.preview_edit(d);
            s.key(ChartKey::Delete);
            assert!(s.drawings.get(id).is_none());
            let fx = if commit {
                s.commit_edit(orig.clone())
            } else {
                s.cancel_edit(orig.clone())
            };
            assert!(!fx.contains(&ChartEffect::DrawingsChanged));
            assert!(s.drawings.get(id).is_none());
            // Only the delete is undoable.
            assert!(s.key(ChartKey::Undo).contains(&ChartEffect::DrawingsChanged));
            assert!(s.drawings.get(id).is_some());
            assert!(!s.key(ChartKey::Undo).contains(&ChartEffect::DrawingsChanged));
        }
    }

    #[test]
    fn shift_with_off_screen_fixed_anchor() {
        let mut s = state(500, false);
        let a = at(&s, (-3000.0, 150.0));
        let b = at(&s, (400.0, 100.0));
        s.set_drawings(vec![Drawing::new(
            DrawingId(1),
            DrawingKind::TrendLine { a, b },
            Style::default(),
        )]);
        let h = scr(&s, b);
        s.mouse_down(h, sh(), 1);
        s.mouse_move((h.0 + 20.0, h.1 + 37.0), sh(), true);
        s.mouse_up((h.0 + 20.0, h.1 + 37.0), sh());
        let (a2, b2) = ends(&s, DrawingId(1));
        assert_eq!(a2, a);
        assert_ne!(b2, b);
        assert!(b2.price.is_finite());
        assert!(on_45(angle(scr(&s, a2), scr(&s, b2))));
    }

    #[test]
    fn rectangle_shift_corners_1_and_3_are_square() {
        for idx in [1usize, 3] {
            let mut s = state(500, false);
            let (a, b) = (at(&s, (200.0, 150.0)), at(&s, (300.0, 230.0)));
            s.set_drawings(vec![Drawing::new(
                DrawingId(1),
                DrawingKind::Rectangle { a, b },
                Style::default(),
            )]);
            let corners = |s: &ChartState| handles(s.drawings.get(DrawingId(1)).unwrap(), &proj(s));
            let c0 = corners(&s);
            let (h, fixed) = (c0[idx], c0[(idx + 2) % 4]);
            let to = (h.0 + 37.0, h.1 - 11.0);
            s.mouse_down(h, sh(), 1);
            s.mouse_move(to, sh(), true);
            s.mouse_up(to, sh());
            let c = corners(&s);
            assert!(near(c[(idx + 2) % 4], fixed), "{c:?} {fixed:?}");
            let (dx, dy) = (c[idx].0 - fixed.0, c[idx].1 - fixed.1);
            assert!((dx.abs() - dy.abs()).abs() < 0.05, "{idx}: {dx} {dy}");
            assert!(dx.abs() > 50.0);
        }
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05
    }
}
