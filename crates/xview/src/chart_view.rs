//! GPUI chart view: paints `xv_chart::Frame` primitives in a `canvas` and
//! forwards mouse, scroll, pinch and keyboard input to the GPUI-free
//! `ChartState`. All chart logic lives in `xv_chart`; this is the thin layer.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ContentMask, Context, DispatchPhase, EventEmitter, FocusHandle, Focusable, Font,
    FontStyle, FontWeight, Hitbox, HitboxBehavior, Hsla, KeyBinding, KeyDownEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, PathStyle, PinchEvent, Pixels, Point, Render,
    ScrollDelta, ScrollEvent, SharedString, StrokeOptions, Subscription, TextRun, Window, actions, canvas, div,
    fill, point, prelude::*, px, quad, size, transparent_black,
};
use xv_chart::{
    ChartColors, ChartEffect, ChartKey, ChartLayout, ChartState, DEFAULT_TEXT_SIZE, Dash, Frame,
    Legend, Mods, Prim, RectF, TextAlign, clamp_span,
};
use lyon_tessellation::LineCap;
use xv_drawings::DrawingId;
use xv_core::{Candle, Series};

use crate::theme::u32_to_hsla;

actions!(chart, [Delete, Cancel, Undo, Redo, CopyDrawing, PasteDrawing, CloneDrawing]);

/// Key context of the chart; the chart key bindings only apply inside it.
pub const KEY_CONTEXT: &str = "ChartView";

/// Pixels per line for `ScrollDelta::Lines` (mouse wheels).
const LINE_SCROLL_PX: f32 = 20.0;
const AXIS_FONT_SIZE: f32 = DEFAULT_TEXT_SIZE;
const TITLE_FONT_SIZE: f32 = 14.0;
/// Line box used to vertically center axis text on its anchor.
const TEXT_LINE_HEIGHT: f32 = 16.0;
/// Background quad behind labelled axis text.
const LABEL_HEIGHT: f32 = xv_chart::AXIS_LABEL_HEIGHT;
const LABEL_PAD_X: f32 = 4.0;
const LABEL_RADIUS: f32 = 3.0;
/// Gap kept between clamped drawing text and the plot edge.
const TEXT_CLAMP_PAD: f32 = 2.0;
const HANDLE_RADIUS: f32 = 4.5;
const HANDLE_BORDER: f32 = 1.5;
/// Crosshair and last-price lines.
const GUIDE_DASH: [f32; 2] = [4.0, 4.0];
/// Drawing line styles. Dots have round caps, so each "on" segment paints
/// as a dot; the dot pattern is scaled by the line width so thick dotted
/// lines keep their gaps (a 1 px line: 2 px dots, 2 px gaps).
const DRAWING_DASH: [f32; 2] = [6.0, 4.0];
const DRAWING_DOT: [f32; 2] = [1.0, 3.0];
const LEGEND_X: f32 = 10.0;
const LEGEND_Y: f32 = 16.0;
const LEGEND_GAP: f32 = 12.0;
/// Frames per line of the `XVIEW_FRAME_LOG` paint-time log.
const FRAME_LOG_EVERY: u32 = 60;

pub fn bind_keys(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Delete, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("cmd-z", Undo, ctx),
        KeyBinding::new("cmd-shift-z", Redo, ctx),
        KeyBinding::new("cmd-c", CopyDrawing, ctx),
        KeyBinding::new("cmd-v", PasteDrawing, ctx),
        KeyBinding::new("cmd-d", CloneDrawing, ctx),
    ]);
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChartViewEvent {
    NeedOlder(i64),
    DrawingsChanged,
    ToolFinished,
    SelectionChanged,
    /// A letter was typed while the chart had focus: open the symbol search
    /// pre-filled with it.
    TypeToSearch(String),
    /// Right-click on a drawing (now selected); `position` is in window
    /// coordinates.
    ContextMenu {
        id: DrawingId,
        position: Point<Pixels>,
    },
    /// Double-click on a drawing: open its settings.
    OpenSettings(DrawingId),
    /// `ChartState::tool_styles` changed.
    ToolStylesChanged,
    /// Esc while a drawing-toolbar popover is open: close it instead of
    /// deselecting.
    DismissPopover,
}

pub struct ChartView {
    pub state: ChartState,
    pub source_name: SharedString,
    focus: FocusHandle,
    colors: ChartColors,
    /// A left-button press started on the chart and has not been released, so
    /// moves and the release are forwarded even outside the canvas.
    dragging: bool,
    /// The pointer is over the canvas, so modifier changes are forwarded.
    hovered: bool,
    /// Window position of the canvas origin, from the last prepaint.
    origin: Point<Pixels>,
    /// A drawing-toolbar popover is open, so Esc closes it (set by the
    /// toolbar).
    pub popover_open: bool,
    /// The drawing settings dialog is open (set by the workspace): the chart
    /// takes no shortcuts.
    pub modal_open: bool,
    /// Element size last passed to `ChartState::set_size`.
    size: Option<(f32, f32)>,
    stats: Option<Rc<RefCell<FrameStats>>>,
    _activation: Subscription,
}

impl EventEmitter<ChartViewEvent> for ChartView {}

impl Focusable for ChartView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ChartView {
    pub fn new(
        series: Series,
        source_name: impl Into<SharedString>,
        colors: ChartColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The real size arrives in the first prepaint.
        let state = ChartState::new(series, 0.0, 0.0);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                // Modifier releases are not seen while inactive (cmd-tab).
                let fx = this.state.modifiers_changed(Mods::default());
                this.state.mouse_left();
                this.hovered = false;
                this.apply_effects(fx, cx);
            }
        });
        let stats = std::env::var_os("XVIEW_FRAME_LOG")
            .is_some_and(|v| !v.is_empty() && v != "0")
            .then(|| Rc::new(RefCell::new(FrameStats::default())));
        ChartView {
            state,
            source_name: source_name.into(),
            focus: cx.focus_handle(),
            colors,
            dragging: false,
            hovered: false,
            origin: Point::default(),
            popover_open: false,
            modal_open: false,
            size: None,
            stats,
            _activation: activation,
        }
    }

    /// Replaces the series (the loader landed a new symbol or timeframe).
    pub fn set_series(&mut self, s: Series, cx: &mut Context<Self>) {
        self.dragging = false;
        let mut fx = self.state.set_series(s);
        // A wide window may already show the left edge of the first page.
        fx.extend(self.state.check_need_older());
        self.apply_effects(fx, cx);
    }

    pub fn set_source_name(&mut self, name: impl Into<SharedString>, cx: &mut Context<Self>) {
        let name = name.into();
        if name != self.source_name {
            self.source_name = name;
            cx.notify();
        }
    }

    pub fn prepend_older(&mut self, cs: Vec<Candle>, has_more: bool, cx: &mut Context<Self>) {
        self.state.prepend_older(cs, has_more);
        // Keep fetching while the page still does not fill the window.
        let fx = self.state.check_need_older();
        self.apply_effects(fx, cx);
    }

    pub fn older_failed(&mut self, cx: &mut Context<Self>) {
        self.state.older_failed();
        cx.notify();
    }

    pub fn apply_update(&mut self, c: Candle, cx: &mut Context<Self>) {
        self.state.apply_update(c);
        cx.notify();
    }

    pub fn merge_backfill(&mut self, cs: Vec<Candle>, cx: &mut Context<Self>) {
        self.state.merge_backfill(cs);
        let fx = self.state.check_need_older();
        self.apply_effects(fx, cx);
    }

    pub fn set_colors(&mut self, colors: ChartColors, cx: &mut Context<Self>) {
        self.colors = colors;
        cx.notify();
    }

    /// Emits every effect as a `ChartViewEvent` and repaints. Called after
    /// every forwarded input, since even effect-free input (crosshair, drag
    /// previews) changes what is drawn.
    pub fn apply_effects(&mut self, fx: Vec<ChartEffect>, cx: &mut Context<Self>) {
        for e in fx {
            cx.emit(match e {
                ChartEffect::NeedOlder { end_before_ms } => {
                    ChartViewEvent::NeedOlder(end_before_ms)
                }
                ChartEffect::DrawingsChanged => ChartViewEvent::DrawingsChanged,
                ChartEffect::ToolFinished => ChartViewEvent::ToolFinished,
                ChartEffect::SelectionChanged => ChartViewEvent::SelectionChanged,
                ChartEffect::OpenSettings(id) => ChartViewEvent::OpenSettings(id),
                ChartEffect::ToolStylesChanged => ChartViewEvent::ToolStylesChanged,
            });
        }
        cx.notify();
    }

    /// Edit shortcuts are swallowed while a toolbar popover or the settings
    /// dialog is open.
    pub fn input_blocked(&self) -> bool {
        self.popover_open || self.modal_open
    }

    /// Window position of the chart element's origin.
    pub fn origin(&self) -> Point<Pixels> {
        self.origin
    }

    fn key(&mut self, k: ChartKey, cx: &mut Context<Self>) {
        let fx = self.state.key(k);
        self.apply_effects(fx, cx);
    }

    /// Right-click at `p` (element coordinates): selects the drawing under it
    /// (or deselects) and asks for its context menu.
    pub fn right_click(&mut self, p: (f32, f32), cx: &mut Context<Self>) {
        let (id, fx) = self.state.select_at(p);
        self.apply_effects(fx, cx);
        if let Some(id) = id {
            let position = self.origin + point(px(p.0), px(p.1));
            cx.emit(ChartViewEvent::ContextMenu { id, position });
        }
    }

    fn modifiers_changed(&mut self, m: &Modifiers, cx: &mut Context<Self>) {
        if self.hovered || self.dragging {
            let fx = self.state.modifiers_changed(mods(m));
            self.apply_effects(fx, cx);
        }
    }

    /// Prepaint: syncs the size and builds the frame.
    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> (Frame, ChartColors, ChartLayout) {
        self.origin = bounds.origin;
        let sz = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if self.size != Some(sz) {
            self.size = Some(sz);
            self.state.set_size(sz.0, sz.1);
            // A resize can expose the left edge of the loaded history. Emit
            // without notifying: this frame already reflects the new size.
            for e in self.state.check_need_older() {
                if let ChartEffect::NeedOlder { end_before_ms } = e {
                    cx.emit(ChartViewEvent::NeedOlder(end_before_ms));
                }
            }
        }
        let frame = self.state.frame(&self.colors, &self.source_name);
        (frame, self.colors.clone(), self.state.layout)
    }
}

/// The letter that opens the symbol search when typed on the chart, or `None`
/// for anything else (shortcuts with cmd/ctrl/alt/fn, digits, named keys).
pub fn type_to_search_letter(k: &Keystroke) -> Option<String> {
    let m = &k.modifiers;
    if m.platform || m.control || m.alt || m.function {
        return None;
    }
    let ch = k.key_char.as_deref()?;
    let mut chars = ch.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_alphabetic() => Some(ch.to_string()),
        _ => None,
    }
}

fn mods(m: &Modifiers) -> Mods {
    Mods {
        shift: m.shift,
        cmd: m.platform,
        alt: m.alt,
    }
}

fn scroll_px(d: &ScrollDelta) -> (f32, f32) {
    match d {
        ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
        ScrollDelta::Lines(l) => (l.x * LINE_SCROLL_PX, l.y * LINE_SCROLL_PX),
    }
}

struct Prepared {
    frame: Frame,
    colors: ChartColors,
    layout: ChartLayout,
    hitbox: Hitbox,
    build: Duration,
}

impl Render for ChartView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let paint_view = view.clone();
        let stats = self.stats.clone();
        div()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            // While a toolbar popover is open, edit shortcuts are swallowed:
            // they would change the drawing under an open picker session.
            .on_action(cx.listener(|this, _: &Delete, _, cx| {
                if !this.input_blocked() {
                    this.key(ChartKey::Delete, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                if this.modal_open {
                    return;
                }
                if this.popover_open {
                    cx.emit(ChartViewEvent::DismissPopover);
                } else {
                    this.key(ChartKey::Escape, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if !this.input_blocked() {
                    this.key(ChartKey::Undo, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if !this.input_blocked() {
                    this.key(ChartKey::Redo, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CloneDrawing, _, cx| {
                if this.input_blocked() {
                    return;
                }
                let fx = this.state.clone_selected();
                this.apply_effects(fx, cx);
            }))
            // Modifier events go to the focused path, which the canvas below
            // is not on, so the listener sits here.
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                this.modifiers_changed(&e.modifiers, cx)
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if this.input_blocked() {
                    return;
                }
                if let Some(letter) = type_to_search_letter(&e.keystroke) {
                    cx.stop_propagation();
                    cx.emit(ChartViewEvent::TypeToSearch(letter));
                }
            }))
            .size_full()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let t0 = Instant::now();
                        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                        let (frame, colors, layout) =
                            view.update(cx, |this, cx| this.prepare(bounds, cx)).ok()?;
                        Some(Prepared {
                            frame,
                            colors,
                            layout,
                            hitbox,
                            build: t0.elapsed(),
                        })
                    },
                    move |bounds, prepared, window, cx| {
                        let Some(prepared) = prepared else { return };
                        let t0 = Instant::now();
                        paint_frame(bounds, &prepared, window, cx);
                        // The pointer may rest on the chart since before the
                        // first mouse move (e.g. at launch, or after a popover
                        // closed): take the hover from the hitbox so a cmd /
                        // shift press snaps at once. GPUI's hit test is only
                        // current from paint on, so this is not in prepaint.
                        // Only ever set here; moves and exits clear it.
                        if prepared.hitbox.is_hovered(window) {
                            let _ = paint_view.update(cx, |this, _| this.hovered = true);
                        }
                        register_input(bounds, &prepared.hitbox, &paint_view, window);
                        if let Some(stats) = &stats {
                            let prims = prepared.frame.plot.len()
                                + prepared.frame.price_axis.len()
                                + prepared.frame.time_axis.len();
                            stats
                                .borrow_mut()
                                .record(prepared.build, t0.elapsed(), prims);
                        }
                    },
                )
                .size_full(),
            )
    }
}

// ---- painting ----

fn rect_bounds(origin: Point<Pixels>, r: RectF) -> Bounds<Pixels> {
    Bounds {
        origin: origin + point(px(r.x), px(r.y)),
        size: size(px(r.w), px(r.h)),
    }
}

fn paint_frame(bounds: Bounds<Pixels>, p: &Prepared, window: &mut Window, cx: &mut App) {
    let painter = Painter {
        origin: bounds.origin,
        scale: window.scale_factor(),
        font: window.text_style().font(),
        background: u32_to_hsla(p.colors.background),
        plot: p.layout.plot,
    };
    let c = &p.colors;
    let l = &p.layout;
    window.paint_quad(fill(bounds, painter.background));

    let plot_mask = Some(ContentMask {
        bounds: rect_bounds(bounds.origin, l.plot),
    });
    window.with_content_mask(plot_mask, |window| {
        for prim in &p.frame.plot {
            painter.prim(prim, window, cx);
        }
    });

    // Axis separators.
    let sep = u32_to_hsla(c.grid);
    let (px_, py_) = (l.plot.x + l.plot.w, l.plot.y + l.plot.h);
    painter.line(
        (px_, l.plot.y),
        (px_, py_ + l.time_axis.h),
        sep,
        1.0,
        Dash::Solid,
        window,
    );
    painter.line(
        (l.plot.x, py_),
        (px_ + l.price_axis.w, py_),
        sep,
        1.0,
        Dash::Solid,
        window,
    );

    for (rect, prims) in [
        (l.price_axis, &p.frame.price_axis),
        (l.time_axis, &p.frame.time_axis),
    ] {
        let mask = Some(ContentMask {
            bounds: rect_bounds(bounds.origin, rect),
        });
        window.with_content_mask(mask, |window| {
            for prim in prims {
                painter.prim(prim, window, cx);
            }
        });
    }

    window.with_content_mask(plot_mask, |window| {
        painter.legend(&p.frame.legend, l.plot, c, window, cx);
    });
}

/// Color, font and size of one `Prim::Text`.
struct TextLook<'a> {
    color: Hsla,
    font: &'a Font,
    size: f32,
    /// Keep the text and its background inside the plot.
    clamp: bool,
}

struct Painter {
    origin: Point<Pixels>,
    scale: f32,
    font: Font,
    background: Hsla,
    /// Plot rect (element coordinates), for clamped text.
    plot: RectF,
}

impl Painter {
    fn snap(&self, v: f32) -> f32 {
        (v * self.scale).round() / self.scale
    }

    /// Snaps the center of a line of logical width `w` so its edges fall on
    /// device pixels. Returns (snapped center, snapped width).
    fn snap_line(&self, c: f32, w: f32) -> (f32, f32) {
        let wd = (w * self.scale).round().max(1.0);
        (
            (((c * self.scale) - wd / 2.0).round() + wd / 2.0) / self.scale,
            wd / self.scale,
        )
    }

    /// Element-relative rect to window bounds, edges snapped to device
    /// pixels and at least one device pixel in each dimension.
    fn snapped(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        let min = 1.0 / self.scale;
        let (x0, y0) = (self.snap(x), self.snap(y));
        let x1 = self.snap(x + w).max(x0 + min);
        let y1 = self.snap(y + h).max(y0 + min);
        Bounds {
            origin: self.origin + point(px(x0), px(y0)),
            size: size(px(x1 - x0), px(y1 - y0)),
        }
    }

    fn at(&self, p: (f32, f32)) -> Point<Pixels> {
        self.origin + point(px(p.0), px(p.1))
    }

    fn prim(&self, prim: &Prim, window: &mut Window, cx: &mut App) {
        match prim {
            Prim::Rect { r, color } => {
                window.paint_quad(fill(self.snapped(r.x, r.y, r.w, r.h), u32_to_hsla(*color)));
            }
            Prim::StrokeRect { r, color, width } => {
                window.paint_quad(quad(
                    self.snapped(r.x, r.y, r.w, r.h),
                    px(0.),
                    transparent_black(),
                    px(*width),
                    u32_to_hsla(*color),
                    Default::default(),
                ));
            }
            Prim::Line {
                a,
                b,
                color,
                width,
                dash,
            } => self.line(*a, *b, u32_to_hsla(*color), *width, *dash, window),
            Prim::Text {
                pos,
                text,
                color,
                align,
                bg,
                size,
                bold,
                italic,
                clamp,
            } => {
                let mut font = self.font.clone();
                if *bold {
                    font.weight = FontWeight::SEMIBOLD;
                }
                if *italic {
                    font.style = FontStyle::Italic;
                }
                self.text(
                    *pos,
                    text,
                    TextLook {
                        color: u32_to_hsla(*color),
                        font: &font,
                        size: *size,
                        clamp: *clamp,
                    },
                    *align,
                    bg.map(u32_to_hsla),
                    window,
                    cx,
                );
            }
            Prim::Polygon { pts, color } => {
                let Some((first, rest)) = pts.split_first() else {
                    return;
                };
                let mut p = PathBuilder::fill();
                p.move_to(self.at(*first));
                for q in rest {
                    p.line_to(self.at(*q));
                }
                p.close();
                if let Ok(path) = p.build() {
                    window.paint_path(path, u32_to_hsla(*color));
                }
            }
            Prim::Handle {
                center,
                color,
                filled,
            } => {
                let r = HANDLE_RADIUS;
                window.paint_quad(quad(
                    Bounds {
                        origin: self.at((center.0 - r, center.1 - r)),
                        size: size(px(2. * r), px(2. * r)),
                    },
                    px(r),
                    // Chart background inside a colored ring; a locked
                    // drawing's handles are just the ring.
                    if *filled {
                        self.background
                    } else {
                        transparent_black()
                    },
                    px(HANDLE_BORDER),
                    u32_to_hsla(*color),
                    Default::default(),
                ));
            }
        }
    }

    fn line(
        &self,
        a: (f32, f32),
        b: (f32, f32),
        color: Hsla,
        width: f32,
        dash: Dash,
        window: &mut Window,
    ) {
        let (mut a, mut b) = (a, b);
        let vertical = a.0 == b.0;
        let horizontal = a.1 == b.1;
        if dash == Dash::Solid && (vertical || horizontal) {
            // Axis-aligned solid lines are quads: crisp and far cheaper than
            // tessellated paths (every candle wick takes this path).
            let (c, w) = self.snap_line(if vertical { a.0 } else { a.1 }, width);
            let bounds = if vertical {
                let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
                self.snapped(c - w / 2.0, y0, w, y1 - y0)
            } else {
                let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
                self.snapped(x0, c - w / 2.0, x1 - x0, w)
            };
            window.paint_quad(fill(bounds, color));
            return;
        }
        if vertical {
            let (c, _) = self.snap_line(a.0, width);
            (a.0, b.0) = (c, c);
        } else if horizontal {
            let (c, _) = self.snap_line(a.1, width);
            (a.1, b.1) = (c, c);
        }
        let mut builder = PathBuilder::stroke(px(width));
        let scaled = |d: [f32; 2]| [px(d[0] * width.max(1.0)), px(d[1] * width.max(1.0))];
        match dash {
            Dash::Solid => {}
            Dash::Guide => builder = builder.dash_array(&[px(GUIDE_DASH[0]), px(GUIDE_DASH[1])]),
            Dash::Dashed => {
                builder = builder.dash_array(&[px(DRAWING_DASH[0]), px(DRAWING_DASH[1])])
            }
            Dash::Dotted => {
                builder = builder
                    .with_style(PathStyle::Stroke(
                        StrokeOptions::default()
                            .with_line_width(width)
                            .with_line_cap(LineCap::Round),
                    ))
                    .dash_array(&scaled(DRAWING_DOT));
            }
        }
        builder.move_to(self.at(a));
        builder.line_to(self.at(b));
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }

    fn shape(
        &self,
        text: &str,
        font: &Font,
        font_size: f32,
        runs: &[(usize, Hsla)],
        window: &mut Window,
    ) -> gpui::ShapedLine {
        let runs: Vec<TextRun> = runs
            .iter()
            .map(|&(len, color)| TextRun {
                len,
                font: font.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            })
            .collect();
        window.text_system().shape_line(
            SharedString::from(text.to_owned()),
            px(font_size),
            &runs,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        pos: (f32, f32),
        text: &str,
        look: TextLook,
        align: TextAlign,
        bg: Option<Hsla>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if text.is_empty() {
            return;
        }
        let line = self.shape(text, look.font, look.size, &[(text.len(), look.color)], window);
        let line_h = TEXT_LINE_HEIGHT.max(look.size * 1.25);
        let w = f32::from(line.width());
        let mut x = match align {
            TextAlign::Left => pos.0,
            TextAlign::Center => pos.0 - w / 2.0,
            TextAlign::Right => pos.0 - w,
        };
        let mut pos = pos;
        if look.clamp {
            let pl = &self.plot;
            let pad = if bg.is_some() { LABEL_PAD_X } else { 0.0 } + TEXT_CLAMP_PAD;
            x = clamp_span(x - pad, w + 2.0 * pad, pl.x, pl.x + pl.w) + pad;
            let h = if bg.is_some() { line_h.max(LABEL_HEIGHT) } else { line_h };
            pos.1 = clamp_span(pos.1 - h / 2.0, h, pl.y, pl.y + pl.h) + h / 2.0;
        }
        if let Some(bg) = bg {
            let (bx, by) = (x - LABEL_PAD_X, pos.1 - LABEL_HEIGHT / 2.0);
            window.paint_quad(
                fill(
                    self.snapped(bx, by, w + 2.0 * LABEL_PAD_X, LABEL_HEIGHT),
                    bg,
                )
                .corner_radii(px(LABEL_RADIUS)),
            );
        }
        let _ = line.paint(
            self.at((x, pos.1 - line_h / 2.0)),
            px(line_h),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    /// Top-left legend: semibold title, then O/H/L/C/V and the change.
    fn legend(&self, lg: &Legend, plot: RectF, c: &ChartColors, window: &mut Window, cx: &mut App) {
        let line_h = px(20.);
        let y = plot.y + LEGEND_Y;
        let mut title_font = self.font.clone();
        title_font.weight = FontWeight::SEMIBOLD;
        let title = self.shape(
            &lg.title,
            &title_font,
            TITLE_FONT_SIZE,
            &[(lg.title.len(), u32_to_hsla(c.text))],
            window,
        );
        let x = plot.x + LEGEND_X;
        let _ = title.paint(
            self.at((x, y - 10.0)),
            line_h,
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );

        let muted = u32_to_hsla(c.muted_text);
        let value = u32_to_hsla(if lg.up { c.up } else { c.down });
        let mut text = String::new();
        let mut runs: Vec<(usize, Hsla)> = Vec::new();
        let mut push = |s: &str, color: Hsla| {
            text.push_str(s);
            runs.push((s.len(), color));
        };
        for (label, v) in [
            ("O ", &lg.o),
            ("  H ", &lg.h),
            ("  L ", &lg.l),
            ("  C ", &lg.c),
        ] {
            push(label, muted);
            push(v, value);
        }
        push("  ", muted);
        push(&lg.change, value);
        push("  Vol ", muted);
        push(&lg.v, muted);
        let row = self.shape(&text, &self.font, AXIS_FONT_SIZE, &runs, window);
        let x = x + f32::from(title.width()) + LEGEND_GAP;
        let _ = row.paint(
            self.at((x, y - 10.0)),
            line_h,
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
    }
}

// ---- input ----

fn register_input(
    bounds: Bounds<Pixels>,
    hitbox: &Hitbox,
    view: &gpui::WeakEntity<ChartView>,
    window: &mut Window,
) {
    let origin = bounds.origin;
    let rel = move |p: Point<Pixels>| (f32::from(p.x - origin.x), f32::from(p.y - origin.y));

    {
        let (view, hitbox) = (view.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || e.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let _ = view.update(cx, |this, cx| {
                window.focus(&this.focus, cx);
                this.dragging = true;
                let fx = this
                    .state
                    .mouse_down(rel(e.position), mods(&e.modifiers), e.click_count);
                this.apply_effects(fx, cx);
            });
        });
    }
    {
        let (view, hitbox) = (view.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || e.button != MouseButton::Right
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let _ = view.update(cx, |this, cx| {
                window.focus(&this.focus, cx);
                this.right_click(rel(e.position), cx);
            });
        });
    }
    {
        let (view, hitbox) = (view.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hovered = hitbox.is_hovered(window);
            let _ = view.update(cx, |this, cx| {
                this.hovered = hovered;
                if this.dragging || hovered {
                    let pressed = e.pressed_button == Some(MouseButton::Left);
                    if !pressed {
                        // Release was lost (e.g. outside the window); the
                        // state finishes the drag like a mouse-up.
                        this.dragging = false;
                    }
                    let fx = this
                        .state
                        .mouse_move(rel(e.position), mods(&e.modifiers), pressed);
                    this.apply_effects(fx, cx);
                } else if this.state.crosshair.is_some() {
                    this.state.mouse_left();
                    cx.notify();
                }
            });
        });
    }
    {
        let (view, hitbox) = (view.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            let hovered = hitbox.is_hovered(window);
            let _ = view.update(cx, |this, cx| {
                if this.dragging || hovered {
                    this.dragging = false;
                    let fx = this.state.mouse_up(rel(e.position), mods(&e.modifiers));
                    this.apply_effects(fx, cx);
                }
            });
        });
    }
    {
        let view = view.clone();
        window.on_mouse_event(move |_: &MouseExitEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let _ = view.update(cx, |this, cx| {
                this.hovered = false;
                this.state.mouse_left();
                cx.notify();
            });
        });
    }
    {
        let (view, hitbox) = (view.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                return;
            }
            let (dx, dy) = scroll_px(&e.delta);
            let _ = view.update(cx, |this, cx| {
                let fx = this
                    .state
                    .scroll(rel(e.position), dx, dy, mods(&e.modifiers));
                this.apply_effects(fx, cx);
            });
        });
    }
    {
        let (view, hitbox) = (view.clone(), hitbox.clone());
        window.on_mouse_event(move |e: &PinchEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            let _ = view.update(cx, |this, cx| {
                let fx = this.state.pinch(rel(e.position), e.delta);
                this.apply_effects(fx, cx);
            });
        });
    }
}

// ---- frame-time log (XVIEW_FRAME_LOG=1) ----

/// CPU time spent building the frame (prepaint) and emitting paint
/// primitives, logged every `FRAME_LOG_EVERY` frames.
#[derive(Default)]
struct FrameStats {
    frames: u32,
    build_sum: Duration,
    paint_sum: Duration,
    total_max: Duration,
    prims: usize,
}

impl FrameStats {
    fn record(&mut self, build: Duration, paint: Duration, prims: usize) {
        self.frames += 1;
        self.build_sum += build;
        self.paint_sum += paint;
        self.total_max = self.total_max.max(build + paint);
        self.prims = prims;
        if self.frames == FRAME_LOG_EVERY {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0 / f64::from(self.frames);
            eprintln!(
                "xview frame-time: build {:.3} ms + paint {:.3} ms = {:.3} ms avg, max {:.3} ms over {} frames ({} prims)",
                ms(self.build_sum),
                ms(self.paint_sum),
                ms(self.build_sum + self.paint_sum),
                self.total_max.as_secs_f64() * 1000.0,
                self.frames,
                self.prims,
            );
            *self = FrameStats::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(key: &str, ch: Option<&str>, m: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: m,
            key: key.into(),
            key_char: ch.map(Into::into),
        }
    }

    #[test]
    fn letters_open_search_but_shortcuts_do_not() {
        let none = Modifiers::default();
        assert_eq!(type_to_search_letter(&ks("e", Some("e"), none)), Some("e".into()));
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(type_to_search_letter(&ks("e", Some("E"), shift)), Some("E".into()));
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        assert_eq!(type_to_search_letter(&ks("k", None, cmd)), None);
        assert_eq!(type_to_search_letter(&ks("k", Some("k"), cmd)), None);
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(type_to_search_letter(&ks("a", Some("a"), ctrl)), None);
        assert_eq!(type_to_search_letter(&ks("1", Some("1"), none)), None);
        assert_eq!(type_to_search_letter(&ks("escape", None, none)), None);
        assert_eq!(type_to_search_letter(&ks("space", Some(" "), none)), None);
    }
}
