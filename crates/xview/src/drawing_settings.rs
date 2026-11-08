//! The drawing settings dialog (spec §2.4–2.6): a modal card with Style,
//! Text, Coordinates and Visibility tabs, and Template ▾ / Cancel / Ok.
//!
//! Opening captures `original = ChartState::begin_edit()`; the dialog edits a
//! working copy and previews every change on the chart (`preview_edit`).
//! Ok commits one undo step (`commit_edit`); Cancel, ✕ and Esc put the
//! original back (`cancel_edit`). If the drawing disappears or the selection
//! changes while open, the dialog cancels and closes itself.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    Anchor, AnchoredPositionMode, AnyElement, App, Bounds, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, SharedString, Stateful, Subscription, Window, actions, anchored, deferred, div, point,
    prelude::*, px,
};
use xv_core::TzOffset;
use xv_drawings::{
    Drawing, DrawingKind, Extend, HAlign, LineEnd, Range, StatsMode, StatsPos, Style, Tool, VAlign,
    Visibility, tool_of,
};
use xv_store::{NamedTemplate, TemplatesFile};

use crate::chart_view::ChartView;
use crate::color_picker::{ColorPicker, ColorPickerEvent};
use crate::drawing_toolbar::{LINE_STYLES, ToolbarEvent, WIDTHS, line_sample, pick_row};
use crate::form::{
    FIELD_H, FIELD_RADIUS, OnPick, RangeEnd, checkbox, format_datetime, format_field_price,
    nearest_end, parse_datetime, parse_price, range_track, select_button, select_menu,
    set_range_end, slider_value, square_button, step_price, tab_strip,
};
use crate::templates_menu::{
    TemplateHost, apply_template, name_input, template_style, templates_menu,
};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{AppTheme, u32_to_hsla};
use crate::widgets::{icon, menu_box, menu_row, popover_layer, popover_layer_at};

actions!(drawing_settings, [DialogCancel, DialogOk]);

pub const KEY_CONTEXT: &str = "DrawingSettings";

pub fn bind_keys(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", DialogCancel, ctx),
        KeyBinding::new("cmd-enter", DialogOk, ctx),
    ]);
}

const CARD_W: f32 = 400.;
const RADIUS: f32 = 16.;
const PAD: f32 = 24.;
const LABEL_W: f32 = 112.;
const ROW_H: f32 = 52.;
const SELECT_W: f32 = 186.;
const SMALL_SELECT_W: f32 = 100.;
const BODY_TEXT: f32 = 15.;
const TITLE_TEXT: f32 = 24.;
const POPOVER_GAP: f32 = 6.;
const NUM_W: f32 = 76.;
const PRICE_W: f32 = 92.;
const COORD_TEXT: f32 = 14.;
const COORD_LABEL_W: f32 = 106.;
const TEXT_MAX: usize = 2000;
const NAME_MAX: usize = 60;

const TAB_LABELS: [&str; 4] = ["Style", "Text", "Coordinates", "Visibility"];
const EXTENDS: [(Extend, &str); 4] = [
    (Extend::None, "Don't extend"),
    (Extend::Left, "Left"),
    (Extend::Right, "Right"),
    (Extend::Both, "Both"),
];
const STATS: [(StatsMode, &str); 3] = [
    (StatsMode::Hidden, "Hidden"),
    (StatsMode::Compact, "Compact"),
    (StatsMode::Values, "Values"),
];
const STATS_POS: [(StatsPos, &str); 3] = [
    (StatsPos::Left, "Left"),
    (StatsPos::Center, "Center"),
    (StatsPos::Right, "Right"),
];
const SIZES: [u8; 10] = [10, 11, 12, 14, 16, 20, 24, 28, 32, 40];
const VALIGNS: [(VAlign, &str); 3] = [
    (VAlign::Top, "Top"),
    (VAlign::Middle, "Middle"),
    (VAlign::Bottom, "Bottom"),
];
const HALIGNS: [(HAlign, &str); 3] = [
    (HAlign::Left, "Left"),
    (HAlign::Center, "Center"),
    (HAlign::Right, "Right"),
];
/// Visibility rows and their upper bounds (spec §2.4).
pub const VIS_ROWS: [(&str, u16); 5] = [
    ("Minutes", 59),
    ("Hours", 24),
    ("Days", 366),
    ("Weeks", 52),
    ("Months", 12),
];

/// The display name of a tool (the header when a drawing has no name).
pub fn tool_name(t: Tool) -> &'static str {
    match t {
        Tool::TrendLine => "Trend line",
        Tool::Ray => "Ray",
        Tool::ExtendedLine => "Extended line",
        Tool::HorizontalLine => "Horizontal line",
        Tool::VerticalLine => "Vertical line",
        Tool::Rectangle => "Rectangle",
        Tool::Cursor => "Drawing",
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Coords {
    TwoPoints,
    Price,
    Time,
}

/// Which rows a drawing kind shows (spec §2.5). Line, Text and Visibility
/// are always shown; a rectangle's line row is its border.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rows {
    pub fill: bool,
    pub ends: bool,
    pub extend: bool,
    pub middle_point: bool,
    pub price_labels: bool,
    pub stats: bool,
    pub coords: Coords,
}

pub fn rows_for_kind(tool: Tool) -> Rows {
    let two = Rows {
        fill: false,
        ends: true,
        extend: true,
        middle_point: true,
        price_labels: true,
        stats: true,
        coords: Coords::TwoPoints,
    };
    let bare = Rows {
        ends: false,
        extend: false,
        middle_point: false,
        stats: false,
        ..two
    };
    match tool {
        Tool::TrendLine | Tool::ExtendedLine | Tool::Cursor => two,
        Tool::Ray => Rows {
            extend: false,
            ..two
        },
        Tool::Rectangle => Rows {
            fill: true,
            ends: false,
            ..two
        },
        Tool::HorizontalLine => Rows {
            coords: Coords::Price,
            ..bare
        },
        Tool::VerticalLine => Rows {
            price_labels: false,
            coords: Coords::Time,
            ..bare
        },
    }
}

/// (price, time) of each anchor; `None` where the kind has no such value.
pub fn anchors(k: &DrawingKind) -> Vec<(Option<f64>, Option<i64>)> {
    match k {
        DrawingKind::TrendLine { a, b }
        | DrawingKind::Ray { a, b }
        | DrawingKind::Rectangle { a, b } => {
            vec![
                (Some(a.price), Some(a.time_ms)),
                (Some(b.price), Some(b.time_ms)),
            ]
        }
        DrawingKind::HorizontalLine { price } => vec![(Some(*price), None)],
        DrawingKind::VerticalLine { time_ms } => vec![(None, Some(*time_ms))],
    }
}

pub fn set_anchor_price(k: &mut DrawingKind, i: usize, p: f64) {
    match k {
        DrawingKind::TrendLine { a, b }
        | DrawingKind::Ray { a, b }
        | DrawingKind::Rectangle { a, b } => {
            if i == 0 {
                a.price = p
            } else {
                b.price = p
            }
        }
        DrawingKind::HorizontalLine { price } => *price = p,
        DrawingKind::VerticalLine { .. } => {}
    }
}

pub fn set_anchor_time(k: &mut DrawingKind, i: usize, t: i64) {
    match k {
        DrawingKind::TrendLine { a, b }
        | DrawingKind::Ray { a, b }
        | DrawingKind::Rectangle { a, b } => {
            if i == 0 {
                a.time_ms = t
            } else {
                b.time_ms = t
            }
        }
        DrawingKind::VerticalLine { time_ms } => *time_ms = t,
        DrawingKind::HorizontalLine { .. } => {}
    }
}

pub fn vis_range(v: &mut Visibility, i: usize) -> &mut Range {
    match i {
        0 => &mut v.minutes,
        1 => &mut v.hours,
        2 => &mut v.days,
        3 => &mut v.weeks,
        _ => &mut v.months,
    }
}

/// A rectangle's fill as shown: `None` is the border color at alpha 0x33.
pub fn fill_color(st: &Style) -> u32 {
    st.fill.unwrap_or((st.color & 0xFFFF_FF00) | 0x33)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    Style,
    Text,
    Coordinates,
    Visibility,
}

const TABS: [Tab; 4] = [Tab::Style, Tab::Text, Tab::Coordinates, Tab::Visibility];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ColorTarget {
    Line,
    Fill,
    Text,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Sel {
    Extend,
    Stats,
    StatsPos,
    TextSize,
    VAlign,
    HAlign,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Pop {
    Color(ColorTarget),
    LineStyle,
    LeftEnd,
    RightEnd,
    Select(Sel),
    Template,
}

struct CoordRow {
    price: Option<Entity<TextInput>>,
    time: Option<Entity<TextInput>>,
}

struct VisRow {
    min: Entity<TextInput>,
    max: Entity<TextInput>,
    track: Rc<Cell<Bounds<Pixels>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEvent {
    /// The dialog committed or cancelled its edit; drop it.
    Closed,
}

pub struct DrawingSettings {
    chart: Entity<ChartView>,
    theme: AppTheme,
    /// The tool at open: the header, the rows and the templates.
    tool: Tool,
    original: Drawing,
    working: Drawing,
    tab: Tab,
    focus: FocusHandle,
    open: Option<Pop>,
    picker: Option<(Entity<ColorPicker>, Subscription)>,
    name_edit: Option<(Entity<TextInput>, Vec<Subscription>)>,
    template_name: Option<(Entity<TextInput>, Subscription)>,
    text_area: Entity<TextInput>,
    coords: Vec<CoordRow>,
    vis: Vec<VisRow>,
    drag: Option<(usize, RangeEnd)>,
    named: BTreeMap<String, Vec<NamedTemplate>>,
    custom_colors: Vec<u32>,
    tick: f64,
    tz: TzOffset,
    done: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for DrawingSettings {}
impl EventEmitter<ToolbarEvent> for DrawingSettings {}

impl Focusable for DrawingSettings {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DrawingSettings {
    /// `original` is what `begin_edit` returned for the selected drawing.
    pub fn new(
        chart: Entity<ChartView>,
        original: Drawing,
        theme: AppTheme,
        templates: &TemplatesFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tick, tz) = {
            let s = &chart.read(cx).state;
            (s.series.symbol.tick_size, s.tz)
        };
        let mut subs = Vec::new();
        let text_area = cx.new(|cx| {
            TextInput::new(
                original.style.text.content.clone(),
                "Add text",
                |c| !c.is_control(),
                TEXT_MAX,
                theme.clone(),
                cx,
            )
            .multiline(4)
            .with_text_size(BODY_TEXT)
        });
        subs.push(cx.subscribe(&text_area, |this, _, e: &TextInputEvent, cx| {
            if let TextInputEvent::Changed(t) = e {
                let t = t.clone();
                this.edit(|d| d.style.text.content = t, cx);
            }
        }));
        let field = |text: String,
                     accept: fn(char) -> bool,
                     max: usize,
                     stepper: bool,
                     cx: &mut Context<Self>| {
            let theme = theme.clone();
            // Coordinates need a smaller face to fit a full date and time.
            let size = if max > 3 { COORD_TEXT } else { BODY_TEXT };
            cx.new(|cx| {
                let f = TextInput::new(text, "", accept, max, theme, cx).with_text_size(size);
                if stepper { f.with_stepper() } else { f }
            })
        };
        let mut coords = Vec::new();
        for (i, (p, t)) in anchors(&original.kind).into_iter().enumerate() {
            let price = p.map(|p| {
                let f = field(
                    format_field_price(p, tick),
                    |c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'),
                    24,
                    true,
                    cx,
                );
                subs.push(cx.subscribe_in(
                    &f,
                    window,
                    move |this, f, e: &TextInputEvent, window, cx| {
                        this.on_price(i, f, e, window, cx)
                    },
                ));
                subs.push(cx.on_blur(&f.focus_handle(cx), window, |this, window, cx| {
                    this.reformat(window, cx)
                }));
                f
            });
            let time = t.map(|t| {
                let f = field(
                    format_datetime(t, tz),
                    |c| c.is_ascii_digit() || matches!(c, '-' | ':' | ' '),
                    16,
                    false,
                    cx,
                );
                subs.push(cx.subscribe_in(
                    &f,
                    window,
                    move |this, f, e: &TextInputEvent, window, cx| {
                        this.on_time(i, f, e, window, cx)
                    },
                ));
                subs.push(cx.on_blur(&f.focus_handle(cx), window, |this, window, cx| {
                    this.reformat(window, cx)
                }));
                f
            });
            coords.push(CoordRow { price, time });
        }
        let mut vis = Vec::new();
        let mut v = original.style.visibility;
        for i in 0..VIS_ROWS.len() {
            let r = *vis_range(&mut v, i);
            let mut make = |end: RangeEnd, val: u16, cx: &mut Context<Self>| {
                let f = field(val.to_string(), |c| c.is_ascii_digit(), 3, true, cx);
                subs.push(cx.subscribe_in(
                    &f,
                    window,
                    move |this, f, e: &TextInputEvent, window, cx| {
                        this.on_vis(i, end, f, e, window, cx)
                    },
                ));
                subs.push(cx.on_blur(&f.focus_handle(cx), window, |this, window, cx| {
                    this.reformat(window, cx)
                }));
                f
            };
            let min = make(RangeEnd::Min, r.min, cx);
            let max = make(RangeEnd::Max, r.max, cx);
            vis.push(VisRow {
                min,
                max,
                track: Rc::new(Cell::new(Bounds::default())),
            });
        }
        // Closes (cancelling) when the drawing goes away or another one is
        // selected, e.g. after a symbol switch.
        subs.push(cx.observe(&chart, |this, chart, cx| {
            if this.done {
                return;
            }
            let s = &chart.read(cx).state;
            let id = this.original.id;
            if s.selected != Some(id) || s.drawings.get(id).is_none() {
                this.finish(false, cx);
            }
        }));
        DrawingSettings {
            chart,
            theme,
            tool: tool_of(&original),
            working: original.clone(),
            original,
            tab: Tab::Style,
            focus: cx.focus_handle(),
            open: None,
            picker: None,
            name_edit: None,
            template_name: None,
            text_area,
            coords,
            vis,
            drag: None,
            named: templates.named.clone(),
            custom_colors: templates.custom_colors.clone(),
            tick,
            tz,
            done: false,
            _subs: subs,
        }
    }

    pub fn set_templates(&mut self, f: &TemplatesFile, cx: &mut Context<Self>) {
        self.named = f.named.clone();
        self.custom_colors = f.custom_colors.clone();
        if let Some((p, _)) = &self.picker {
            let custom = self.custom_colors.clone();
            p.update(cx, |v, cx| v.set_custom(custom, cx));
        }
        cx.notify();
    }

    /// Edits the working copy and previews it on the chart.
    fn edit(&mut self, f: impl FnOnce(&mut Drawing), cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        f(&mut self.working);
        let d = self.working.clone();
        self.chart.update(cx, |c, cx| {
            let fx = c.state.preview_edit(d);
            c.apply_effects(fx, cx);
        });
        cx.notify();
    }

    /// Ends the edit: Ok commits one undo step, otherwise the original is
    /// restored. Emits `Closed`.
    pub fn finish(&mut self, ok: bool, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        if ok {
            self.commit_name(cx);
        }
        self.done = true;
        self.picker = None;
        self.open = None;
        let original = self.original.clone();
        self.chart.update(cx, |c, cx| {
            let fx = if ok {
                c.state.commit_edit(original)
            } else {
                c.state.cancel_edit(original)
            };
            c.apply_effects(fx, cx);
        });
        cx.emit(SettingsEvent::Closed);
    }

    fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(None, window, cx);
        self.tab = tab;
        let focus = if tab == Tab::Text {
            self.text_area.focus_handle(cx)
        } else {
            self.focus.clone()
        };
        window.focus(&focus, cx);
        cx.notify();
    }

    fn set_open(&mut self, p: Option<Pop>, window: &mut Window, cx: &mut Context<Self>) {
        if self.open == p {
            return;
        }
        self.picker = None;
        self.template_name = None;
        self.open = p;
        if let Some(Pop::Color(target)) = p {
            self.start_picker(target, window, cx);
        }
        cx.notify();
    }

    fn toggle(&mut self, p: Pop, window: &mut Window, cx: &mut Context<Self>) {
        let next = (self.open != Some(p)).then_some(p);
        self.set_open(next, window, cx);
    }

    fn start_picker(&mut self, target: ColorTarget, window: &mut Window, cx: &mut Context<Self>) {
        let st = &self.working.style;
        let color = match target {
            ColorTarget::Line => st.color,
            ColorTarget::Fill => fill_color(st),
            ColorTarget::Text => st.text.color,
        };
        let (theme, custom) = (self.theme.clone(), self.custom_colors.clone());
        let view = cx.new(|cx| ColorPicker::new(color, custom, theme, cx));
        let sub = cx.subscribe_in(
            &view,
            window,
            move |this, _, e: &ColorPickerEvent, window, cx| match e {
                ColorPickerEvent::Changed(c) => {
                    let c = *c;
                    this.edit(
                        |d| match target {
                            ColorTarget::Line => d.style.color = c,
                            ColorTarget::Fill => d.style.fill = Some(c),
                            ColorTarget::Text => d.style.text.color = c,
                        },
                        cx,
                    );
                }
                ColorPickerEvent::CustomColorAdded(c) => cx.emit(ToolbarEvent::CustomColor(*c)),
                ColorPickerEvent::InputClosed => window.focus(&this.focus, cx),
            },
        );
        self.picker = Some((view, sub));
    }

    // ---- name ----

    fn start_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.name_edit.is_some() {
            return;
        }
        let text = self.title().to_string();
        let theme = self.theme.clone();
        let input = cx.new(|cx| {
            TextInput::new(
                text,
                tool_name(self.tool),
                |c| !c.is_control(),
                NAME_MAX,
                theme,
                cx,
            )
            .with_text_size(18.)
        });
        let sub = cx.subscribe_in(&input, window, |this, _, e: &TextInputEvent, window, cx| {
            if let TextInputEvent::Submit(_) = e {
                this.commit_name(cx);
                window.focus(&this.focus, cx);
            }
        });
        let blur = cx.on_blur(&input.focus_handle(cx), window, |this, _, cx| {
            this.commit_name(cx)
        });
        window.focus(&input.focus_handle(cx), cx);
        self.name_edit = Some((input, vec![sub, blur]));
        cx.notify();
    }

    /// Commits an open name field: empty (or the tool name on an unnamed
    /// drawing) is no name.
    fn commit_name(&mut self, cx: &mut Context<Self>) {
        let Some((input, _)) = self.name_edit.take() else {
            return;
        };
        let text = input.read(cx).text().trim().to_string();
        let name =
            if text.is_empty() || (self.working.name.is_none() && text == tool_name(self.tool)) {
                None
            } else {
                Some(text)
            };
        self.edit(|d| d.name = name, cx);
    }

    fn title(&self) -> SharedString {
        match &self.working.name {
            Some(n) => n.clone().into(),
            None => tool_name(self.tool).into(),
        }
    }

    // ---- fields ----

    fn on_price(
        &mut self,
        i: usize,
        f: &Entity<TextInput>,
        e: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            TextInputEvent::Changed(t) => {
                let p = parse_price(t);
                f.update(cx, |f, cx| f.set_invalid(p.is_none(), cx));
                if let Some(p) = p {
                    self.edit(|d| set_anchor_price(&mut d.kind, i, p), cx);
                }
            }
            TextInputEvent::Step(dir) => {
                let cur = parse_price(f.read(cx).text()).or(anchors(&self.working.kind)[i].0);
                if let Some(cur) = cur {
                    let p = step_price(cur, self.tick, *dir);
                    let text = format_field_price(p, self.tick);
                    f.update(cx, |f, cx| {
                        f.set_text(text, cx);
                        f.set_invalid(false, cx);
                    });
                    self.edit(|d| set_anchor_price(&mut d.kind, i, p), cx);
                }
            }
            TextInputEvent::Submit(_) => window.focus(&self.focus, cx),
            TextInputEvent::Cancel => {}
        }
    }

    fn on_time(
        &mut self,
        i: usize,
        f: &Entity<TextInput>,
        e: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            TextInputEvent::Changed(t) => {
                let ms = parse_datetime(t, self.tz);
                f.update(cx, |f, cx| f.set_invalid(ms.is_none(), cx));
                if let Some(ms) = ms {
                    self.edit(|d| set_anchor_time(&mut d.kind, i, ms), cx);
                }
            }
            // Valid text is already applied; leaving the field reformats it
            // (invalid text keeps its red border).
            TextInputEvent::Submit(_) => window.focus(&self.focus, cx),
            TextInputEvent::Step(_) | TextInputEvent::Cancel => {}
        }
    }

    fn on_vis(
        &mut self,
        i: usize,
        end: RangeEnd,
        f: &Entity<TextInput>,
        e: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            TextInputEvent::Changed(t) => match t.parse::<i64>() {
                Ok(v) => {
                    f.update(cx, |f, cx| f.set_invalid(false, cx));
                    self.set_range(i, end, v, window, cx);
                }
                Err(_) => f.update(cx, |f, cx| f.set_invalid(true, cx)),
            },
            TextInputEvent::Step(d) => {
                let r = *vis_range(&mut self.working.style.visibility, i);
                let cur = if end == RangeEnd::Min { r.min } else { r.max };
                self.set_range(i, end, cur as i64 + *d as i64, window, cx);
                let v = self.range_value(i, end);
                f.update(cx, |f, cx| {
                    f.set_text(v.to_string(), cx);
                    f.set_invalid(false, cx);
                });
            }
            TextInputEvent::Submit(_) => window.focus(&self.focus, cx),
            TextInputEvent::Cancel => {}
        }
    }

    fn range_value(&mut self, i: usize, end: RangeEnd) -> u16 {
        let r = *vis_range(&mut self.working.style.visibility, i);
        if end == RangeEnd::Min { r.min } else { r.max }
    }

    fn set_range(
        &mut self,
        i: usize,
        end: RangeEnd,
        v: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bound = VIS_ROWS[i].1;
        self.edit(
            |d| {
                let r = vis_range(&mut d.style.visibility, i);
                *r = set_range_end(*r, bound, end, v);
            },
            cx,
        );
        self.reformat(window, cx);
    }

    /// Rewrites every unfocused field from the working copy (fields with
    /// invalid text keep it, so the red border stays until fixed).
    fn reformat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let set = |f: &Entity<TextInput>, text: String, window: &Window, cx: &mut Context<Self>| {
            if !f.focus_handle(cx).is_focused(window) && !f.read(cx).is_invalid() {
                f.update(cx, |f, cx| f.set_text(text, cx));
            }
        };
        let points = anchors(&self.working.kind);
        for (row, (p, t)) in self.coords.iter().zip(points) {
            if let (Some(f), Some(p)) = (&row.price, p) {
                set(f, format_field_price(p, self.tick), window, cx);
            }
            if let (Some(f), Some(t)) = (&row.time, t) {
                set(f, format_datetime(t, self.tz), window, cx);
            }
        }
        let mut v = self.working.style.visibility;
        for (i, row) in self.vis.iter().enumerate() {
            let r = *vis_range(&mut v, i);
            for (f, val) in [(&row.min, r.min), (&row.max, r.max)] {
                if !f.focus_handle(cx).is_focused(window) {
                    f.update(cx, |f, cx| {
                        f.set_invalid(false, cx);
                        f.set_text(val.to_string(), cx);
                    });
                }
            }
        }
        let content = self.working.style.text.content.clone();
        if self.text_area.read(cx).text() != content {
            self.text_area.update(cx, |f, cx| f.set_text(content, cx));
        }
    }

    fn slider_to(
        &mut self,
        i: usize,
        x: Pixels,
        grab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = self.vis[i].track.get();
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return;
        }
        let bound = VIS_ROWS[i].1;
        let v = slider_value(f32::from(x - b.origin.x) / w, bound);
        let end = match (grab, self.drag) {
            (false, Some((_, end))) => end,
            _ => nearest_end(*vis_range(&mut self.working.style.visibility, i), v),
        };
        self.drag = Some((i, end));
        self.set_range(i, end, v, window, cx);
    }

    // ---- selects ----

    fn sel_items(&self, sel: Sel) -> Vec<(SharedString, bool)> {
        let st = &self.working.style;
        fn items<T: PartialEq + Copy>(list: &[(T, &str)], cur: T) -> Vec<(SharedString, bool)> {
            list.iter()
                .map(|(v, l)| (SharedString::from(*l), *v == cur))
                .collect()
        }
        match sel {
            Sel::Extend => items(&EXTENDS, st.extend),
            Sel::Stats => items(&STATS, st.stats),
            Sel::StatsPos => items(&STATS_POS, st.stats_pos),
            Sel::VAlign => items(&VALIGNS, st.text.valign),
            Sel::HAlign => items(&HALIGNS, st.text.halign),
            Sel::TextSize => SIZES
                .iter()
                .map(|s| (SharedString::from(s.to_string()), *s == st.text.size))
                .collect(),
        }
    }

    fn sel_label(&self, sel: Sel) -> SharedString {
        let items = self.sel_items(sel);
        items
            .iter()
            .find(|(_, a)| *a)
            .map(|(l, _)| l.clone())
            .unwrap_or_else(|| self.working.style.text.size.to_string().into())
    }

    fn sel_pick(&mut self, sel: Sel, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(None, window, cx);
        self.edit(
            |d| {
                let st = &mut d.style;
                match sel {
                    Sel::Extend => st.extend = EXTENDS[i].0,
                    Sel::Stats => st.stats = STATS[i].0,
                    Sel::StatsPos => st.stats_pos = STATS_POS[i].0,
                    Sel::TextSize => st.text.size = SIZES[i],
                    Sel::VAlign => st.text.valign = VALIGNS[i].0,
                    Sel::HAlign => st.text.halign = HALIGNS[i].0,
                }
            },
            cx,
        );
    }

    // ---- rendering ----

    /// Adds popover `p` to button `b`: toggles on click, and shows `menu`
    /// under it (or above, `up`) while open.
    fn popover(
        &self,
        b: Stateful<Div>,
        p: Pop,
        up: bool,
        menu: impl FnOnce(&Self, &mut Context<Self>) -> AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let b = b.on_click(cx.listener(move |this, _, window, cx| this.toggle(p, window, cx)));
        if self.open != Some(p) {
            return b;
        }
        let menu = menu(self, cx);
        let close = cx.listener(|this, _, window, cx| this.set_open(None, window, cx));
        if up {
            b.children(popover_layer_at(
                (0., -POPOVER_GAP),
                Anchor::BottomLeft,
                menu,
                close,
                window,
            ))
        } else {
            b.children(popover_layer(
                (0., FIELD_H + POPOVER_GAP),
                menu,
                close,
                window,
            ))
        }
    }

    fn select(
        &self,
        id: &'static str,
        sel: Sel,
        w: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self.open == Some(Pop::Select(sel));
        let b = select_button(id, self.sel_label(sel), open, w, &self.theme);
        self.popover(
            b,
            Pop::Select(sel),
            false,
            move |this, cx| {
                let weak = cx.weak_entity();
                let pick: OnPick = Rc::new(move |i, window, cx| {
                    weak.update(cx, |this, cx| this.sel_pick(sel, i, window, cx))
                        .ok();
                });
                select_menu(id, &this.sel_items(sel), w, pick, &this.theme).into_any_element()
            },
            window,
            cx,
        )
    }

    fn picker_menu(&self) -> AnyElement {
        match &self.picker {
            Some((p, _)) => p.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn swatch(&self, color: u32, size: f32) -> Div {
        div()
            .flex_none()
            .size(px(size))
            .rounded(px(4.))
            .border_1()
            .border_color(self.theme.border)
            .bg(gpui::white())
            .child(div().size_full().rounded(px(3.)).bg(u32_to_hsla(color)))
    }

    fn label_row(&self, label: &'static str) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_H))
            .child(div().flex_none().w(px(LABEL_W)).child(label))
    }

    fn check_row(
        &self,
        id: &'static str,
        checked: bool,
        label: &'static str,
        toggle: fn(&mut Style),
        cx: &mut Context<Self>,
    ) -> Div {
        div().flex().items_center().h(px(ROW_H)).child(
            checkbox(id, checked, label, &self.theme).on_click(
                cx.listener(move |this, _, _, cx| this.edit(|d| toggle(&mut d.style), cx)),
            ),
        )
    }

    fn line_style_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let st = &self.working.style;
        let mut menu = menu_box(t).min_w(px(170.)).py(px(4.));
        for (i, (ls, label)) in LINE_STYLES.into_iter().enumerate() {
            let active = ls == st.line_style;
            let fg = if active { t.bg } else { t.text };
            menu = menu.child(
                pick_row(SharedString::from(format!("dlg-line-style-{i}")), active, t)
                    .child(line_sample(ls, 1.5, fg))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.edit(|d| d.style.line_style = ls, cx)
                    })),
            );
        }
        menu = menu.child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(t.border));
        for w in WIDTHS {
            let active = w == st.width;
            let fg = if active { t.bg } else { t.text };
            menu = menu.child(
                pick_row(SharedString::from(format!("dlg-width-{w}")), active, t)
                    .child(div().w(px(22.)).h(px(w as f32)).bg(fg))
                    .child(format!("{w}px"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.edit(|d| d.style.width = w, cx)),
                    ),
            );
        }
        menu.into_any_element()
    }

    fn ends_menu(&self, left: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let st = &self.working.style;
        let cur = if left { st.left_end } else { st.right_end };
        let mut menu = menu_box(t).min_w(px(150.)).py(px(4.));
        for (i, end) in [LineEnd::Normal, LineEnd::Arrow].into_iter().enumerate() {
            let label = if end == LineEnd::Normal {
                "Normal"
            } else {
                "Arrow"
            };
            menu = menu.child(
                menu_row(
                    SharedString::from(format!("dlg-end-{left}-{i}")),
                    Some(end_icon(left, end)),
                    label,
                    end == cur,
                    true,
                    t,
                )
                .text_size(px(15.))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_open(None, window, cx);
                    this.edit(
                        |d| {
                            if left {
                                d.style.left_end = end
                            } else {
                                d.style.right_end = end
                            }
                        },
                        cx,
                    );
                })),
            );
        }
        menu.into_any_element()
    }

    fn line_row(&self, rows: Rows, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let st = &self.working.style;
        let hover = t.hover;
        let zone = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .h_full()
                .rounded(px(FIELD_RADIUS - 1.))
                .hover(move |s| s.bg(hover))
                .cursor_pointer()
        };
        let color = self.popover(
            zone("dlg-line-color")
                .pl(px(4.))
                .pr(px(4.))
                .child(self.swatch(st.color | 0xFF, 26.)),
            Pop::Color(ColorTarget::Line),
            false,
            |this, _| this.picker_menu(),
            window,
            cx,
        );
        let sample = self.popover(
            zone("dlg-line-style").w(px(46.)).child(line_sample(
                st.line_style,
                st.width as f32,
                t.text,
            )),
            Pop::LineStyle,
            false,
            |this, cx| this.line_style_menu(cx),
            window,
            cx,
        );
        let mut row = self
            .label_row(if rows.fill { "Border" } else { "Line" })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(FIELD_H))
                    .rounded(px(FIELD_RADIUS))
                    .border_1()
                    .border_color(t.border)
                    .child(color)
                    .child(sample),
            );
        if rows.ends {
            for left in [true, false] {
                let (id, p, end) = if left {
                    ("dlg-left-end", Pop::LeftEnd, st.left_end)
                } else {
                    ("dlg-right-end", Pop::RightEnd, st.right_end)
                };
                let b = square_button(id, self.open == Some(p), &t).child(icon(
                    end_icon(left, end),
                    26.,
                    t.text,
                ));
                row = row.child(div().ml(px(8.)).child(self.popover(
                    b,
                    p,
                    false,
                    move |this, cx| this.ends_menu(left, cx),
                    window,
                    cx,
                )));
            }
        }
        row
    }

    fn style_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let rows = rows_for_kind(self.tool);
        let st = self.working.style.clone();
        let mut body = div()
            .flex()
            .flex_col()
            .child(self.line_row(rows, window, cx));
        if rows.fill {
            let b = square_button("dlg-fill", false, &t).child(self.swatch(fill_color(&st), 24.));
            body = body.child(self.label_row("Fill").child(self.popover(
                b,
                Pop::Color(ColorTarget::Fill),
                false,
                |this, _| this.picker_menu(),
                window,
                cx,
            )));
        }
        if rows.extend {
            body = body.child(self.label_row("Extend").child(self.select(
                "dlg-extend",
                Sel::Extend,
                SELECT_W,
                window,
                cx,
            )));
        }
        if rows.middle_point {
            body = body.child(self.check_row(
                "dlg-middle",
                st.middle_point,
                "Middle point",
                |s| s.middle_point = !s.middle_point,
                cx,
            ));
        }
        if rows.price_labels {
            body = body.child(self.check_row(
                "dlg-labels",
                st.price_labels,
                "Price labels",
                |s| s.price_labels = !s.price_labels,
                cx,
            ));
        }
        if rows.stats {
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(36.))
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child("INFO"),
                )
                .child(self.label_row("Stats").child(self.select(
                    "dlg-stats",
                    Sel::Stats,
                    SELECT_W,
                    window,
                    cx,
                )))
                .child(self.label_row("Stats position").child(self.select(
                    "dlg-stats-pos",
                    Sel::StatsPos,
                    SELECT_W,
                    window,
                    cx,
                )))
                .child(self.check_row(
                    "dlg-always",
                    st.always_stats,
                    "Always show stats",
                    |s| s.always_stats = !s.always_stats,
                    cx,
                ));
        }
        body
    }

    fn text_tab(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let txt = self.working.style.text.clone();
        let color = self.popover(
            square_button("dlg-text-color", false, &t).child(self.swatch(txt.color | 0xFF, 24.)),
            Pop::Color(ColorTarget::Text),
            false,
            |this, _| this.picker_menu(),
            window,
            cx,
        );
        let bold = square_button("dlg-bold", txt.bold, &t)
            .text_size(px(17.))
            .font_weight(FontWeight::BOLD)
            .child("B")
            .on_click(cx.listener(|this, _, _, cx| {
                this.edit(|d| d.style.text.bold = !d.style.text.bold, cx)
            }));
        let italic = square_button("dlg-italic", txt.italic, &t)
            .text_size(px(17.))
            .italic()
            .child("I")
            .on_click(cx.listener(|this, _, _, cx| {
                this.edit(|d| d.style.text.italic = !d.style.text.italic, cx)
            }));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(ROW_H))
                    .child(color)
                    .child(self.select("dlg-size", Sel::TextSize, SMALL_SELECT_W, window, cx))
                    .child(bold)
                    .child(italic),
            )
            .child(div().mt(px(8.)).child(self.text_area.clone()))
            .child(
                self.label_row("Text alignment")
                    .mt(px(8.))
                    .gap(px(8.))
                    .child(self.select("dlg-valign", Sel::VAlign, SMALL_SELECT_W, window, cx))
                    .child(self.select("dlg-halign", Sel::HAlign, SMALL_SELECT_W, window, cx)),
            )
    }

    fn coords_tab(&self) -> Div {
        let rows = rows_for_kind(self.tool);
        let what = match rows.coords {
            Coords::TwoPoints => "price, time",
            Coords::Price => "price",
            Coords::Time => "time",
        };
        let mut body = div().flex().flex_col();
        for (i, row) in self.coords.iter().enumerate() {
            let label = SharedString::from(format!("#{} ({what})", i + 1));
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .h(px(ROW_H))
                    .child(
                        div()
                            .flex_none()
                            .w(px(COORD_LABEL_W))
                            .whitespace_nowrap()
                            .child(label),
                    )
                    .children(
                        row.price
                            .clone()
                            .map(|f| div().flex_none().w(px(PRICE_W)).child(f)),
                    )
                    .children(row.time.clone().map(|f| div().flex_1().min_w_0().child(f))),
            );
        }
        body
    }

    fn vis_tab(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let mut body = div().flex().flex_col();
        let mut v = self.working.style.visibility;
        for (i, row) in self.vis.iter().enumerate() {
            let r = *vis_range(&mut v, i);
            let (label, bound) = VIS_ROWS[i];
            let track = range_track(
                SharedString::from(format!("vis-track-{i}")),
                r,
                bound,
                row.track.clone(),
                &t,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.focus(&this.focus, cx);
                    this.slider_to(i, e.position.x, true, window, cx);
                }),
            );
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(ROW_H))
                    .child(
                        div().flex_none().w(px(104.)).child(
                            checkbox(SharedString::from(format!("vis-on-{i}")), r.on, label, &t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.edit(
                                        |d| {
                                            let r = vis_range(&mut d.style.visibility, i);
                                            r.on = !r.on;
                                        },
                                        cx,
                                    )
                                })),
                        ),
                    )
                    .child(div().flex_none().w(px(NUM_W)).child(row.min.clone()))
                    .child(track)
                    .child(div().flex_none().w(px(NUM_W)).child(row.max.clone())),
            );
        }
        body
    }

    /// Full-window layer that moves a range handle while dragged.
    fn drag_layer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let vp = window.viewport_size();
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .id("vis-drag")
                        .w(vp.width)
                        .h(vp.height)
                        .occlude()
                        .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                            match this.drag {
                                Some((i, _)) if e.pressed_button == Some(MouseButton::Left) => {
                                    this.slider_to(i, e.position.x, false, window, cx)
                                }
                                _ => {
                                    this.drag = None;
                                    cx.notify();
                                }
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.drag = None;
                                cx.notify();
                            }),
                        ),
                ),
        )
        .priority(3)
        .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let hover = t.hover;
        let title: AnyElement = match &self.name_edit {
            Some((input, _)) => div().w(px(260.)).child(input.clone()).into_any_element(),
            None => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(
                    div()
                        .text_size(px(TITLE_TEXT))
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(self.title()),
                )
                .child(
                    div()
                        .id("dlg-rename")
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(32.))
                        .rounded(px(6.))
                        .hover(move |s| s.bg(hover))
                        .cursor_pointer()
                        .child(icon("pencil", 26., t.text))
                        .on_click(cx.listener(|this, _, window, cx| this.start_name(window, cx))),
                )
                .into_any_element(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(40.))
            .child(div().flex_1().min_w_0().child(title))
            .child(
                div()
                    .id("dlg-close")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .mr(px(-6.))
                    .rounded(px(6.))
                    .hover(move |s| s.bg(hover))
                    .cursor_pointer()
                    .child(icon("close", 30., t.text))
                    .on_click(cx.listener(|this, _, _, cx| this.finish(false, cx))),
            )
    }

    fn footer(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = self.theme.clone();
        let hover = t.hover;
        let open = self.open == Some(Pop::Template);
        let template = self.popover(
            select_button("dlg-template", "Template", open, 112., &t),
            Pop::Template,
            true,
            |this, cx| {
                let named = this.named.get(this.tool.key()).cloned().unwrap_or_default();
                let input = this.template_name.as_ref().map(|(i, _)| i.clone());
                templates_menu(this.tool, &named, input.as_ref(), &this.theme, cx)
                    .into_any_element()
            },
            window,
            cx,
        );
        let button = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .h(px(FIELD_H))
                .px(px(14.))
                .rounded(px(FIELD_RADIUS))
                .cursor_pointer()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .px(px(PAD))
            .py(px(18.))
            .border_t_1()
            .border_color(t.border)
            .child(template)
            .child(div().flex_1())
            .child(
                button("dlg-cancel")
                    .border_1()
                    .border_color(t.text)
                    .hover(move |s| s.bg(hover))
                    .child("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.finish(false, cx))),
            )
            .child(
                button("dlg-ok")
                    .px(px(16.))
                    .bg(t.text)
                    .text_color(t.bg)
                    .child("Ok")
                    .on_click(cx.listener(|this, _, _, cx| this.finish(true, cx))),
            )
    }

    // ---- debug script hooks ----

    #[cfg(debug_assertions)]
    pub fn debug_tab(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let tab = match name {
            "text" => Tab::Text,
            "coordinates" => Tab::Coordinates,
            "visibility" => Tab::Visibility,
            _ => Tab::Style,
        };
        self.set_tab(tab, window, cx);
    }

    /// `color:RRGGBBAA`, `text:STR` (`\n` for newlines), `name:STR`,
    /// `price1:STR` / `time1:STR` (typed into anchor 1's field),
    /// `vis:ROW:MIN-MAX`, `pop:line_color|line_style|left_end|extend|size|template|fill`.
    #[cfg(debug_assertions)]
    pub fn debug_set(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (k, v) = arg.split_once(':').unwrap_or((arg, ""));
        match k {
            "color" => {
                if let Ok(c) = u32::from_str_radix(v, 16) {
                    self.edit(|d| d.style.color = c, cx);
                }
            }
            "text" => {
                let text = v.replace("\\n", "\n");
                self.text_area
                    .update(cx, |f, cx| f.set_text(text.clone(), cx));
                self.edit(|d| d.style.text.content = text, cx);
            }
            "name" => {
                self.start_name(window, cx);
                if let Some((input, _)) = &self.name_edit {
                    input.update(cx, |f, cx| f.set_text(v.to_string(), cx));
                }
                self.commit_name(cx);
            }
            "price1" | "time1" | "price2" | "time2" => {
                let i = if k.ends_with('1') { 0 } else { 1 };
                let Some(row) = self.coords.get(i) else {
                    return;
                };
                let f = if k.starts_with("price") {
                    row.price.clone()
                } else {
                    row.time.clone()
                };
                let Some(f) = f else { return };
                f.update(cx, |f, cx| f.set_text(v.to_string(), cx));
                let e = TextInputEvent::Changed(v.to_string());
                if k.starts_with("price") {
                    self.on_price(i, &f, &e, window, cx);
                } else {
                    self.on_time(i, &f, &e, window, cx);
                }
            }
            "vis" => {
                let mut it = v.splitn(2, ':');
                let (row, range) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                let Some(i) = VIS_ROWS
                    .iter()
                    .position(|(l, _)| l.eq_ignore_ascii_case(row))
                else {
                    return;
                };
                if let Some((a, b)) = range.split_once('-')
                    && let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>())
                {
                    self.set_range(i, RangeEnd::Max, b, window, cx);
                    self.set_range(i, RangeEnd::Min, a, window, cx);
                }
            }
            "pop" => {
                let p = match v {
                    "line_color" => Pop::Color(ColorTarget::Line),
                    "fill" => Pop::Color(ColorTarget::Fill),
                    "text_color" => Pop::Color(ColorTarget::Text),
                    "line_style" => Pop::LineStyle,
                    "left_end" => Pop::LeftEnd,
                    "extend" => Pop::Select(Sel::Extend),
                    "size" => Pop::Select(Sel::TextSize),
                    "template" => Pop::Template,
                    _ => return self.set_open(None, window, cx),
                };
                self.set_open(Some(p), window, cx);
            }
            _ => eprintln!("xview-script: unknown dlg setting {arg}"),
        }
    }
}

fn end_icon(left: bool, end: LineEnd) -> &'static str {
    match (left, end) {
        (true, LineEnd::Normal) => "line_start_normal",
        (true, LineEnd::Arrow) => "line_start_arrow",
        (false, LineEnd::Normal) => "line_end_normal",
        (false, LineEnd::Arrow) => "line_end_arrow",
    }
}

impl TemplateHost for DrawingSettings {
    fn open_template_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.template_name.is_some() {
            return;
        }
        let input = name_input(&self.theme, cx);
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, e: &TextInputEvent, window, cx| match e {
                TextInputEvent::Changed(_) => cx.notify(),
                TextInputEvent::Submit(name) => this.save_template(name.clone(), window, cx),
                TextInputEvent::Cancel | TextInputEvent::Step(_) => {}
            },
        );
        window.focus(&input.focus_handle(cx), cx);
        self.template_name = Some((input, sub));
        cx.notify();
    }

    fn save_template(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        cx.emit(ToolbarEvent::SaveTemplate {
            tool: self.tool,
            name,
            style: template_style(&self.working.style),
        });
        self.set_open(None, window, cx);
        window.focus(&self.focus, cx);
    }

    fn cancel_template_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.template_name = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn apply_template_style(&mut self, style: Style, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(None, window, cx);
        self.edit(|d| apply_template(d, &style), cx);
        self.reformat(window, cx);
    }

    fn delete_template(&mut self, name: String, cx: &mut Context<Self>) {
        cx.emit(ToolbarEvent::DeleteTemplate {
            tool: self.tool,
            name,
        });
    }
}

impl Render for DrawingSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let (origin, size) = {
            let c = self.chart.read(cx);
            let l = &c.state.layout;
            (
                c.origin(),
                (l.plot.w + l.price_axis.w, l.plot.h + l.time_axis.h),
            )
        };
        let tab = TABS.iter().position(|x| *x == self.tab).unwrap_or(0);
        let weak = cx.weak_entity();
        let pick_tab: OnPick = Rc::new(move |i, window, cx| {
            weak.update(cx, |this, cx| this.set_tab(TABS[i], window, cx))
                .ok();
        });
        let body = match self.tab {
            Tab::Style => self.style_tab(window, cx),
            Tab::Text => self.text_tab(window, cx),
            Tab::Coordinates => self.coords_tab(),
            Tab::Visibility => self.vis_tab(cx),
        };
        let card = div()
            .id("drawing-settings")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &DialogCancel, window, cx| {
                // Esc closes an open popover first.
                if this.open.is_some() {
                    this.set_open(None, window, cx);
                    window.focus(&this.focus, cx);
                } else {
                    this.finish(false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &DialogOk, _, cx| this.finish(true, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .flex()
            .flex_col()
            .w(px(CARD_W))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .shadow_xl()
            .text_size(px(BODY_TEXT))
            .text_color(t.text)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px(px(PAD))
                    .pt(px(18.))
                    .child(self.header(cx))
                    .child(
                        div()
                            .mt(px(18.))
                            .child(tab_strip(&TAB_LABELS, tab, pick_tab, &t)),
                    ),
            )
            .child(div().px(px(PAD)).pt(px(14.)).pb(px(20.)).child(body))
            .child(self.footer(window, cx));
        let dim = if t.mode == xv_store::ThemeMode::Dark {
            gpui::hsla(0., 0., 0., 0.45)
        } else {
            gpui::hsla(0., 0., 0., 0.12)
        };
        div()
            .id("drawing-settings-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .bg(dim)
            // A backdrop click does nothing: only Cancel, ✕ or Esc close.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .absolute()
                    .left(origin.x)
                    .top(origin.y)
                    .w(px(size.0))
                    .h(px(size.1))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(card),
            )
            .when(self.drag.is_some(), |d| {
                d.child(self.drag_layer(window, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_drawings::Anchor;

    #[test]
    fn rows_for_kind() {
        let trend = super::rows_for_kind(Tool::TrendLine);
        assert!(
            trend.ends && trend.extend && trend.middle_point && trend.price_labels && trend.stats
        );
        assert!(!trend.fill);
        assert_eq!(trend.coords, Coords::TwoPoints);
        assert_eq!(super::rows_for_kind(Tool::ExtendedLine), trend);
        let ray = super::rows_for_kind(Tool::Ray);
        assert!(ray.ends && !ray.extend && ray.stats);
        let rect = super::rows_for_kind(Tool::Rectangle);
        assert!(
            rect.fill
                && rect.extend
                && !rect.ends
                && rect.middle_point
                && rect.stats
                && rect.price_labels
        );
        let h = super::rows_for_kind(Tool::HorizontalLine);
        assert!(h.price_labels && !h.ends && !h.extend && !h.middle_point && !h.stats && !h.fill);
        assert_eq!(h.coords, Coords::Price);
        let v = super::rows_for_kind(Tool::VerticalLine);
        assert!(!v.price_labels && !v.ends && !v.extend && !v.stats);
        assert_eq!(v.coords, Coords::Time);
    }

    #[test]
    fn anchors_get_and_set() {
        let a = Anchor {
            time_ms: 10,
            price: 1.0,
        };
        let b = Anchor {
            time_ms: 20,
            price: 2.0,
        };
        let mut k = DrawingKind::Rectangle { a, b };
        assert_eq!(
            anchors(&k),
            vec![(Some(1.0), Some(10)), (Some(2.0), Some(20))]
        );
        set_anchor_price(&mut k, 1, 5.0);
        set_anchor_time(&mut k, 0, 7);
        assert_eq!(
            anchors(&k),
            vec![(Some(1.0), Some(7)), (Some(5.0), Some(20))]
        );
        let mut h = DrawingKind::HorizontalLine { price: 3.0 };
        set_anchor_price(&mut h, 0, 4.0);
        set_anchor_time(&mut h, 0, 99);
        assert_eq!(anchors(&h), vec![(Some(4.0), None)]);
        let mut v = DrawingKind::VerticalLine { time_ms: 5 };
        set_anchor_time(&mut v, 0, 6);
        assert_eq!(anchors(&v), vec![(None, Some(6))]);
    }

    #[test]
    fn tool_names_and_fill() {
        assert_eq!(tool_name(Tool::TrendLine), "Trend line");
        assert_eq!(tool_name(Tool::HorizontalLine), "Horizontal line");
        let st = Style {
            color: 0x112233FF,
            ..Style::default()
        };
        assert_eq!(fill_color(&st), 0x11223333);
        assert_eq!(
            fill_color(&Style {
                fill: Some(0xAABBCC80),
                ..st
            }),
            0xAABBCC80
        );
        // Visibility rows follow the model's bounds.
        let mut vis = Visibility::default();
        for (i, (_, bound)) in VIS_ROWS.iter().enumerate() {
            assert_eq!(vis_range(&mut vis, i).max, *bound);
        }
    }
}
