//! Floating toolbar over the selected drawing (spec §2.3): grip, templates,
//! line color, text color, width, line style, settings, alert, lock, delete
//! and More (⋯), plus the right-click context menu.
//!
//! It is its own cached entity laid over the chart: it re-renders only when
//! its snapshot (selected id, bounds, style, locked, chart size) or its own
//! state (open popover, grip drag, clipboard, templates) changes. It follows
//! the drawing while the drawing is dragged because the bounds change.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    AnchoredPositionMode, AnyElement, App, Context, Div, Entity, EventEmitter, Focusable, Hsla,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, SharedString, Stateful,
    Subscription, Window, anchored, deferred, div, point, prelude::*, px,
};
use xv_chart::{ChartKey, RectF};
use xv_drawings::{Drawing, DrawingId, LineStyle, Style, Tool, tool_of};
use xv_store::{NamedTemplate, TemplatesFile};

use crate::chart_view::{ChartView, ChartViewEvent};
use crate::color_picker::{ColorPicker, ColorPickerEvent};
use crate::context_menu::{self, MenuAction};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{AppTheme, u32_to_hsla};
use crate::widgets::{DISABLED_OPACITY, backdrop, icon, menu_box, menu_row, popover_layer, tooltip};

const HEIGHT: f32 = 52.;
const BTN: f32 = 44.;
const GRIP_W: f32 = 22.;
const WIDTH_BTN_W: f32 = 84.;
const PAD: f32 = 4.;
const BORDER: f32 = 1.;
const ICON: f32 = 24.;
/// Distance between the drawing and the bar.
const OFFSET: f32 = 8.;
/// Margin kept to the chart edges.
const MARGIN: f32 = 4.;
/// Gap between the bar and a popover under it.
const POPOVER_GAP: f32 = 6.;
const WIDTHS: [u8; 4] = [1, 2, 3, 4];
const LINE_STYLES: [(LineStyle, &str); 3] = [
    (LineStyle::Solid, "Line"),
    (LineStyle::Dashed, "Dashed line"),
    (LineStyle::Dotted, "Dotted line"),
];
const TEMPLATE_NAME_MAX: usize = 40;

/// H-lines and V-lines have no text-color button.
fn has_text_color(tool: Tool) -> bool {
    !matches!(tool, Tool::HorizontalLine | Tool::VerticalLine)
}

/// Outer width of the bar.
pub fn toolbar_width(text_color: bool) -> f32 {
    let buttons = if text_color { 9. } else { 8. };
    2. * (PAD + BORDER) + GRIP_W + WIDTH_BTN_W + buttons * BTN
}

/// Top-left of a bar of width `w` for a drawing with handle bounds `b` in a
/// chart of `size`: centered 8 px above the drawing, below it when there is
/// no room above, clamped inside the chart.
pub fn position(b: RectF, size: (f32, f32), w: f32) -> (f32, f32) {
    let x = b.x + b.w / 2.0 - w / 2.0;
    let above = b.y - OFFSET - HEIGHT;
    let y = if above >= MARGIN {
        above
    } else {
        b.y + b.h + OFFSET
    };
    clamp_pos((x, y), size, w)
}

/// Keeps a bar of width `w` at `p` inside the chart.
pub fn clamp_pos(p: (f32, f32), size: (f32, f32), w: f32) -> (f32, f32) {
    let max_x = (size.0 - w - MARGIN).max(MARGIN);
    let max_y = (size.1 - HEIGHT - MARGIN).max(MARGIN);
    (p.0.clamp(MARGIN, max_x), p.1.clamp(MARGIN, max_y))
}

/// The bar position after a grip drag by `offset`, clamped.
pub fn place(b: RectF, size: (f32, f32), w: f32, offset: (f32, f32)) -> (f32, f32) {
    let (x, y) = position(b, size, w);
    clamp_pos((x + offset.0, y + offset.1), size, w)
}

/// `d`'s style replaced by `template`, keeping the drawing's text.
pub fn apply_template(d: &mut Drawing, template: &Style) {
    let content = std::mem::take(&mut d.style.text.content);
    d.style = template.clone();
    d.style.text.content = content;
}

/// What the toolbar shows of the chart.
#[derive(Clone, PartialEq)]
struct Snap {
    id: DrawingId,
    tool: Tool,
    bounds: RectF,
    style: Style,
    locked: bool,
    /// Chart element size.
    size: (f32, f32),
}

impl Snap {
    fn of(chart: &ChartView) -> Option<Self> {
        let s = &chart.state;
        let bounds = s.selected_screen_bounds()?;
        let d = s.selected_drawing()?;
        let l = &s.layout;
        Some(Snap {
            id: d.id,
            tool: tool_of(d),
            bounds,
            style: d.style.clone(),
            locked: d.locked,
            size: (l.plot.w + l.price_axis.w, l.plot.h + l.time_axis.h),
        })
    }

    /// True when `chart` shows exactly this snapshot (no clone).
    fn matches(this: &Option<Self>, chart: &ChartView) -> bool {
        let s = &chart.state;
        let cur = s.selected_screen_bounds().zip(s.selected_drawing());
        match (this, cur) {
            (None, None) => true,
            (Some(a), Some((bounds, d))) => {
                let l = &s.layout;
                a.id == d.id
                    && a.bounds == bounds
                    && a.locked == d.locked
                    && a.style == d.style
                    && a.tool == tool_of(d)
                    && a.size == (l.plot.w + l.price_axis.w, l.plot.h + l.time_axis.h)
            }
            _ => false,
        }
    }

    fn width(&self) -> f32 {
        toolbar_width(has_text_color(self.tool))
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Popover {
    Templates,
    LineColor,
    TextColor,
    Width,
    LineStyle,
    More,
    /// Right-click menu at a window position.
    Context(Point<Pixels>),
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum ColorTarget {
    Line,
    Text,
}

/// An open color picker: one undo step from `original` until it closes.
struct PickerSession {
    view: Entity<ColorPicker>,
    original: Drawing,
    _sub: Subscription,
}

#[derive(Copy, Clone)]
struct GripDrag {
    start: Point<Pixels>,
    offset: (f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolbarEvent {
    SaveTemplate { tool: Tool, name: String, style: Style },
    DeleteTemplate { tool: Tool, name: String },
    CustomColor(u32),
    Copy,
    Paste,
}

pub struct DrawingToolbar {
    chart: Entity<ChartView>,
    theme: AppTheme,
    snap: Option<Snap>,
    has_clipboard: bool,
    named: BTreeMap<String, Vec<NamedTemplate>>,
    custom_colors: Vec<u32>,
    open: Option<Popover>,
    picker: Option<PickerSession>,
    name_input: Option<(Entity<TextInput>, Subscription)>,
    /// Grip-drag offset from the computed position, for `offset_id`.
    offset: (f32, f32),
    grip: Option<GripDrag>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ToolbarEvent> for DrawingToolbar {}

impl DrawingToolbar {
    pub fn new(
        chart: Entity<ChartView>,
        theme: AppTheme,
        templates: &TemplatesFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Chart notifies on every mouse move; only re-render on a change.
        let observe = cx.observe(&chart, |this, chart, cx| {
            // Compared in place; only a change clones the style.
            if Snap::matches(&this.snap, chart.read(cx)) {
                return;
            }
            let snap = Snap::of(chart.read(cx));
            let (old, new) = (this.snap.as_ref().map(|s| s.id), snap.as_ref().map(|s| s.id));
            this.snap = snap;
            if old != new {
                // A different (or no) selection: the dragged offset resets
                // and any popover closes (a picker commits its edit).
                this.offset = (0.0, 0.0);
                this.grip = None;
                this.set_open(None, None, cx);
            }
            cx.notify();
        });
        let events = cx.subscribe_in(&chart, window, |this, _, e: &ChartViewEvent, window, cx| {
            if *e == ChartViewEvent::DismissPopover {
                this.set_open(None, Some(window), cx);
            }
        });
        let snap = Snap::of(chart.read(cx));
        DrawingToolbar {
            chart,
            theme,
            snap,
            has_clipboard: false,
            named: templates.named.clone(),
            custom_colors: templates.custom_colors.clone(),
            open: None,
            picker: None,
            name_input: None,
            offset: (0.0, 0.0),
            grip: None,
            _subs: vec![observe, events],
        }
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub fn set_has_clipboard(&mut self, has: bool, cx: &mut Context<Self>) {
        if has != self.has_clipboard {
            self.has_clipboard = has;
            cx.notify();
        }
    }

    pub fn set_templates(&mut self, f: &TemplatesFile, cx: &mut Context<Self>) {
        self.named = f.named.clone();
        self.custom_colors = f.custom_colors.clone();
        if let Some(p) = &self.picker {
            let custom = self.custom_colors.clone();
            p.view.update(cx, |v, cx| v.set_custom(custom, cx));
        }
        cx.notify();
    }

    /// Closes any popover; an open color picker commits its session. Returns
    /// whether a picker session was committed.
    pub fn close_popover(&mut self, cx: &mut Context<Self>) -> bool {
        let had_picker = self.picker.is_some();
        self.set_open(None, None, cx);
        had_picker
    }

    /// Opens the right-click menu at `position` (window coordinates).
    pub fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_open(Some(Popover::Context(position)), Some(window), cx);
    }

    /// Opens `p` (closing any other popover), or closes with `None`. A picker
    /// session commits its edit as one undo step when it closes. With a
    /// window, focus goes back to the chart (a popover field may have had it).
    fn set_open(&mut self, p: Option<Popover>, window: Option<&mut Window>, cx: &mut Context<Self>) {
        if self.open == p {
            return;
        }
        self.end_picker(cx);
        self.name_input = None;
        self.open = p;
        if let Some(window) = window {
            let focus = self.chart.focus_handle(cx);
            window.focus(&focus, cx);
            match p {
                Some(Popover::LineColor) => self.start_picker(ColorTarget::Line, window, cx),
                Some(Popover::TextColor) => self.start_picker(ColorTarget::Text, window, cx),
                _ => {}
            }
        }
        let open = self.open.is_some();
        self.chart.update(cx, |c, _| c.popover_open = open);
        cx.notify();
    }

    fn toggle(&mut self, p: Popover, window: &mut Window, cx: &mut Context<Self>) {
        let next = (self.open != Some(p)).then_some(p);
        self.set_open(next, Some(window), cx);
    }

    fn start_picker(&mut self, target: ColorTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(original) = self.chart.update(cx, |c, _| c.state.begin_edit()) else {
            self.open = None;
            return;
        };
        let color = match target {
            ColorTarget::Line => original.style.color,
            ColorTarget::Text => original.style.text.color,
        };
        let (theme, custom) = (self.theme.clone(), self.custom_colors.clone());
        let view = cx.new(|cx| ColorPicker::new(color, custom, theme, cx));
        let sub = cx.subscribe_in(&view, window, move |this, _, e: &ColorPickerEvent, window, cx| {
            match e {
                ColorPickerEvent::Changed(c) => {
                    let c = *c;
                    this.chart.update(cx, |chart, cx| {
                        let Some(mut d) = chart.state.selected_drawing().cloned() else {
                            return;
                        };
                        match target {
                            ColorTarget::Line => d.style.color = c,
                            ColorTarget::Text => d.style.text.color = c,
                        }
                        let fx = chart.state.preview_edit(d);
                        chart.apply_effects(fx, cx);
                    });
                }
                ColorPickerEvent::CustomColorAdded(c) => cx.emit(ToolbarEvent::CustomColor(*c)),
                ColorPickerEvent::InputClosed => {
                    let focus = this.chart.focus_handle(cx);
                    window.focus(&focus, cx);
                }
            }
        });
        self.picker = Some(PickerSession {
            view,
            original,
            _sub: sub,
        });
    }

    fn end_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.picker.take() {
            self.chart.update(cx, |c, cx| {
                let fx = c.state.commit_edit(p.original);
                c.apply_effects(fx, cx);
            });
        }
    }

    /// Runs `f` on the chart state and routes its effects.
    fn with_chart(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut xv_chart::ChartState) -> Vec<xv_chart::ChartEffect>,
    ) {
        self.chart.update(cx, |c, cx| {
            let fx = f(&mut c.state);
            c.apply_effects(fx, cx);
        });
    }

    fn open_settings(&self, cx: &mut App) {
        if let Some(id) = self.snap.as_ref().map(|s| s.id) {
            self.chart
                .update(cx, |_, cx| cx.emit(ChartViewEvent::OpenSettings(id)));
        }
    }

    fn run_menu(&mut self, a: MenuAction, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(None, Some(window), cx);
        match a {
            MenuAction::Clone => self.with_chart(cx, |s| s.clone_selected()),
            MenuAction::Copy => cx.emit(ToolbarEvent::Copy),
            MenuAction::Paste => cx.emit(ToolbarEvent::Paste),
            MenuAction::Lock => self.with_chart(cx, |s| s.set_selected_locked(true)),
            MenuAction::Unlock => self.with_chart(cx, |s| s.set_selected_locked(false)),
            MenuAction::Hide => self.with_chart(cx, |s| s.hide_selected()),
            MenuAction::BringToFront => self.with_chart(cx, |s| s.bring_selected_to_front()),
            MenuAction::SendToBack => self.with_chart(cx, |s| s.send_selected_to_back()),
            MenuAction::Settings => self.open_settings(cx),
            MenuAction::Delete => self.with_chart(cx, |s| s.key(ChartKey::Delete)),
        }
    }

    fn open_name_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let input = cx.new(|cx| {
            TextInput::new("", "Template name", |c| !c.is_control(), TEMPLATE_NAME_MAX, theme, cx)
        });
        let sub = cx.subscribe_in(&input, window, |this, _, e: &TextInputEvent, window, cx| {
            match e {
                // Re-render for the Save button's enabled state.
                TextInputEvent::Changed(_) => cx.notify(),
                TextInputEvent::Submit(name) => this.save_template(name.clone(), window, cx),
                TextInputEvent::Cancel => {
                    this.name_input = None;
                    let focus = this.chart.focus_handle(cx);
                    window.focus(&focus, cx);
                    cx.notify();
                }
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        self.name_input = Some((input, sub));
        cx.notify();
    }

    fn save_template(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        let Some(s) = &self.snap else { return };
        if name.is_empty() {
            return;
        }
        let mut style = s.style.clone();
        // Templates carry the look, not the words.
        style.text.content.clear();
        cx.emit(ToolbarEvent::SaveTemplate {
            tool: s.tool,
            name,
            style,
        });
        self.set_open(None, Some(window), cx);
    }

    fn apply_style(&mut self, style: Style, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(None, Some(window), cx);
        self.with_chart(cx, |s| s.update_selected(|d| apply_template(d, &style)));
    }

    // ---- rendering ----

    fn button(&self, id: &'static str, open: bool, enabled: bool, w: f32) -> Stateful<Div> {
        let t = &self.theme;
        let hover = t.hover;
        div()
            .id(id)
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .justify_center()
            .w(px(w))
            .h(px(BTN))
            .rounded(px(6.))
            .text_color(t.text)
            .when(open, |d| d.bg(t.active_bg))
            .when(enabled && !open, |d| d.hover(move |s| s.bg(hover)))
            .when(!enabled, |d| d.opacity(DISABLED_OPACITY))
    }

    /// A button that toggles popover `p`, built by `menu` when open.
    #[allow(clippy::too_many_arguments)]
    fn popover_button(
        &self,
        id: &'static str,
        tip: &'static str,
        w: f32,
        p: Popover,
        content: impl IntoElement,
        menu: impl FnOnce(&Self, &mut Window, &mut Context<Self>) -> AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let open = self.open == Some(p);
        let mut b = self
            .button(id, open, true, w)
            .relative()
            .child(content)
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(p, window, cx)));
        if open {
            let menu = menu(self, window, cx);
            let close = cx.listener(|this, _, window, cx| this.set_open(None, Some(window), cx));
            b = b.children(popover_layer((0., BTN + PAD + BORDER + POPOVER_GAP), menu, close, window));
        } else {
            b = b.tooltip(tooltip(tip, &self.theme));
        }
        b
    }

    fn color_content(&self, icon_name: &str, color: u32) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .child(icon(icon_name, ICON, t.text))
            .child(
                div()
                    .w(px(22.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(u32_to_hsla(color)),
            )
    }

    fn picker_menu(&self) -> AnyElement {
        match &self.picker {
            Some(p) => p.view.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn templates_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let _ = window;
        let t = &self.theme;
        let Some(s) = &self.snap else {
            return div().into_any_element();
        };
        let tool = s.tool;
        let mut menu = menu_box(t).min_w(px(260.)).child(
            menu_row("tpl-save", None, "Save Drawing Template As…", false, true, t)
                .text_size(px(15.))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.name_input.is_none() {
                        this.open_name_input(window, cx);
                    }
                })),
        );
        if let Some((input, _)) = &self.name_input {
            let input = input.clone();
            let save_input = input.clone();
            let can_save = !input.read(cx).text().trim().is_empty();
            menu = menu.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(4.))
                    .child(div().flex_1().child(input))
                    .child(
                        small_button("tpl-name-save", "Save", true, t)
                            .when(!can_save, |b| b.opacity(DISABLED_OPACITY))
                            .when(can_save, |b| {
                                b.on_click(cx.listener(move |this, _, window, cx| {
                                    let name = save_input.read(cx).text().to_string();
                                    this.save_template(name, window, cx);
                                }))
                            }),
                    )
                    .child(small_button("tpl-name-cancel", "Cancel", false, t).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.name_input = None;
                            let focus = this.chart.focus_handle(cx);
                            window.focus(&focus, cx);
                            cx.notify();
                        }),
                    )),
            );
        }
        menu = menu.child(
            menu_row("tpl-default", None, "Apply Default Drawing Template", false, true, t)
                .text_size(px(15.))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.apply_style(tool.default_style(), window, cx)
                })),
        );
        let named = self.named.get(tool.key()).cloned().unwrap_or_default();
        if !named.is_empty() {
            menu = menu.child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(t.border));
        }
        for (i, nt) in named.into_iter().enumerate() {
            let group = SharedString::from(format!("tpl-row-{i}"));
            let name = nt.name.clone();
            let style = nt.style.clone();
            let hover = t.hover;
            menu = menu.child(
                menu_row(group.clone(), None, nt.name.clone(), false, true, t)
                    .group(group.clone())
                    .text_size(px(15.))
                    .child(div().flex_1().min_w(px(16.)))
                    .child(
                        div()
                            .id(SharedString::from(format!("tpl-del-{i}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded(px(4.))
                            .text_color(t.muted)
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .hover(move |s| s.bg(hover))
                            .child("✕")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(ToolbarEvent::DeleteTemplate {
                                    tool,
                                    name: name.clone(),
                                });
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_style(style.clone(), window, cx)
                    })),
            );
        }
        menu.into_any_element()
    }

    fn width_menu(&self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let cur = self.snap.as_ref().map_or(0, |s| s.style.width);
        let mut menu = menu_box(t).min_w(px(100.)).py(px(4.));
        for w in WIDTHS {
            let active = w == cur;
            let fg = if active { t.bg } else { t.text };
            menu = menu.child(
                pick_row(SharedString::from(format!("width-{w}")), active, t)
                    .child(div().w(px(22.)).h(px(w as f32)).bg(fg))
                    .child(format!("{w}px"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_open(None, Some(window), cx);
                        this.with_chart(cx, |s| s.update_selected(|d| d.style.width = w));
                    })),
            );
        }
        menu.into_any_element()
    }

    fn line_style_menu(&self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = &self.theme;
        let cur = self.snap.as_ref().map(|s| s.style.line_style);
        let mut menu = menu_box(t).min_w(px(170.)).py(px(4.));
        for (i, (ls, label)) in LINE_STYLES.into_iter().enumerate() {
            let active = Some(ls) == cur;
            let fg = if active { t.bg } else { t.text };
            menu = menu.child(
                pick_row(SharedString::from(format!("line-style-{i}")), active, t)
                    .child(line_sample(ls, 1.5, fg))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_open(None, Some(window), cx);
                        this.with_chart(cx, |s| s.update_selected(|d| d.style.line_style = ls));
                    })),
            );
        }
        menu.into_any_element()
    }

    fn menu_items(&self, context: bool, cx: &mut Context<Self>) -> Div {
        let locked = self.snap.as_ref().is_some_and(|s| s.locked);
        let items = context_menu::items(context, self.has_clipboard, locked);
        let weak = cx.weak_entity();
        let pick: context_menu::OnPick = Rc::new(move |a, window, cx| {
            weak.update(cx, |this, cx| this.run_menu(a, window, cx)).ok();
        });
        context_menu::render(if context { "ctx" } else { "more" }, &items, &self.theme, pick)
    }

    fn bar(&self, s: &Snap, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let w = s.width();
        let (x, y) = place(s.bounds, s.size, w, self.offset);
        let st = &s.style;
        let lock_icon = if s.locked { "lock" } else { "lock_open" };
        let grip = div()
            .id("tb-grip")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(GRIP_W))
            .h(px(BTN))
            .cursor_grab()
            .child(icon("grip", 22., t.muted))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.set_open(None, Some(window), cx);
                    this.grip = Some(GripDrag {
                        start: e.position,
                        offset: this.offset,
                    });
                    cx.notify();
                }),
            );
        let width_content = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.))
            .text_size(px(15.))
            .child(div().w(px(18.)).h(px(st.width as f32)).bg(t.text))
            .child(format!("{}px", st.width));
        div()
            .id("drawing-toolbar")
            .absolute()
            .left(px(x))
            .top(px(y))
            .flex()
            .flex_row()
            .items_center()
            .w(px(w))
            .h(px(HEIGHT))
            .px(px(PAD))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .shadow_lg()
            // Clicks here must not reach the chart (that would deselect).
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(grip)
            .child(self.popover_button(
                "tb-templates",
                "Templates",
                BTN,
                Popover::Templates,
                icon("templates", ICON, t.text),
                Self::templates_menu,
                window,
                cx,
            ))
            .child(self.popover_button(
                "tb-line-color",
                "Line color",
                BTN,
                Popover::LineColor,
                self.color_content("pencil", st.color),
                |this, _, _| this.picker_menu(),
                window,
                cx,
            ))
            .when(has_text_color(s.tool), |d| {
                d.child(self.popover_button(
                    "tb-text-color",
                    "Text color",
                    BTN,
                    Popover::TextColor,
                    self.color_content("text_color", st.text.color),
                    |this, _, _| this.picker_menu(),
                    window,
                    cx,
                ))
            })
            .child(self.popover_button(
                "tb-width",
                "Line width",
                WIDTH_BTN_W,
                Popover::Width,
                width_content,
                Self::width_menu,
                window,
                cx,
            ))
            .child(self.popover_button(
                "tb-line-style",
                "Line style",
                BTN,
                Popover::LineStyle,
                line_sample(st.line_style, 1.5, t.text),
                Self::line_style_menu,
                window,
                cx,
            ))
            .child(
                self.button("tb-settings", false, true, BTN)
                    .tooltip(tooltip("Settings", &t))
                    .child(icon("settings_hex", ICON, t.text))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_open(None, Some(window), cx);
                        this.open_settings(cx);
                    })),
            )
            .child(
                self.button("tb-alert", false, false, BTN)
                    .tooltip(tooltip("Alerts (coming soon)", &t))
                    .child(icon("alarm_plus", ICON, t.text)),
            )
            .child(
                self.button("tb-lock", false, true, BTN)
                    .tooltip(tooltip(if s.locked { "Unlock" } else { "Lock" }, &t))
                    .child(icon(lock_icon, ICON, t.text))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_open(None, Some(window), cx);
                        let locked = this.snap.as_ref().is_some_and(|s| s.locked);
                        this.with_chart(cx, |s| s.set_selected_locked(!locked));
                    })),
            )
            .child(
                self.button("tb-delete", false, true, BTN)
                    .tooltip(tooltip("Remove", &t))
                    .child(icon("trash", ICON, t.text))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_open(None, Some(window), cx);
                        this.with_chart(cx, |s| s.key(ChartKey::Delete));
                    })),
            )
            .child(self.popover_button(
                "tb-more",
                "More",
                BTN,
                Popover::More,
                icon("more", ICON, t.text),
                |this, _, cx| this.menu_items(false, cx).into_any_element(),
                window,
                cx,
            ))
    }

    /// Full-window layer that moves the bar while the grip is dragged.
    fn grip_layer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let vp = window.viewport_size();
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .id("toolbar-grip-drag")
                        .w(vp.width)
                        .h(vp.height)
                        .occlude()
                        .cursor_grabbing()
                        .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                            let (Some(g), Some(s)) = (this.grip, &this.snap) else {
                                return;
                            };
                            if e.pressed_button != Some(MouseButton::Left) {
                                this.grip = None;
                                cx.notify();
                                return;
                            }
                            let w = s.width();
                            let base = position(s.bounds, s.size, w);
                            let d = e.position - g.start;
                            let want = (
                                base.0 + g.offset.0 + f32::from(d.x),
                                base.1 + g.offset.1 + f32::from(d.y),
                            );
                            let got = clamp_pos(want, s.size, w);
                            // Store the effective offset, so dragging back
                            // from an edge responds at once.
                            this.offset = (got.0 - base.0, got.1 - base.1);
                            cx.notify();
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.grip = None;
                                cx.notify();
                            }),
                        ),
                ),
        )
        .priority(3)
        .into_any_element()
    }

    fn context_layer(
        &self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> [AnyElement; 2] {
        let close = cx.listener(|this, _, window, cx| this.set_open(None, Some(window), cx));
        let menu = self.menu_items(true, cx);
        [
            backdrop(close, window),
            deferred(
                anchored()
                    .position_mode(AnchoredPositionMode::Window)
                    .position(position)
                    // Clamped inside the window.
                    .snap_to_window_with_margin(px(8.))
                    .child(div().occlude().child(menu)),
            )
            .priority(2)
            .into_any_element(),
        ]
    }

    // ---- debug script hooks ----

    /// Forces a popover open (debug script screenshots).
    #[cfg(debug_assertions)]
    pub fn debug_open_popover(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let p = match name {
            "templates" | "save_as" => Some(Popover::Templates),
            "line_color" | "hex" => Some(Popover::LineColor),
            "text_color" => Some(Popover::TextColor),
            "width" => Some(Popover::Width),
            "line_style" => Some(Popover::LineStyle),
            "more" => Some(Popover::More),
            "context" => self.snap.as_ref().map(|s| {
                let origin = self.chart.read(cx).origin();
                let c = (s.bounds.x + s.bounds.w / 2.0, s.bounds.y + s.bounds.h / 2.0);
                Popover::Context(origin + point(px(c.0), px(c.1)))
            }),
            _ => None,
        };
        self.set_open(p, Some(window), cx);
        match name {
            "save_as" => self.open_name_input(window, cx),
            "hex" => {
                if let Some(p) = &self.picker {
                    p.view.update(cx, |v, cx| v.debug_open_hex(window, cx));
                }
            }
            _ => {}
        }
    }

    /// Picks a color in the open picker, as a swatch click would.
    #[cfg(debug_assertions)]
    pub fn debug_pick_color(&mut self, color: u32, cx: &mut Context<Self>) {
        if let Some(p) = &self.picker {
            p.view.update(cx, |v, cx| v.debug_set_color(color, cx));
        }
    }

    /// Saves the current style as a named template.
    #[cfg(debug_assertions)]
    pub fn debug_save_template(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.save_template(name.to_string(), window, cx);
    }
}

/// A row of the width / line-style menus; the current one is inverted.
fn pick_row(id: SharedString, active: bool, t: &AppTheme) -> Stateful<Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(14.))
        .h(px(32.))
        .px(px(14.))
        .text_size(px(15.))
        .when(active, |d| d.bg(t.text).text_color(t.bg))
        .when(!active, |d| d.hover(move |s| s.bg(hover)))
}

/// A short horizontal sample of a line style.
fn line_sample(ls: LineStyle, h: f32, color: Hsla) -> Div {
    let seg = |w: f32| div().flex_none().w(px(w)).h(px(h)).bg(color);
    let row = div().flex().flex_row().items_center().w(px(22.));
    match ls {
        LineStyle::Solid => row.child(seg(22.)),
        LineStyle::Dashed => row.gap(px(3.)).child(seg(5.)).child(seg(5.)).child(seg(5.)),
        LineStyle::Dotted => row
            .gap(px(2.5))
            .children((0..5).map(|_| div().flex_none().size(px(2.)).rounded(px(1.)).bg(color))),
    }
}

fn small_button(id: &'static str, label: &'static str, primary: bool, t: &AppTheme) -> Stateful<Div> {
    let hover = t.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(13.))
        .when(primary, |d| d.bg(t.text).text_color(t.bg))
        .when(!primary, |d| {
            d.border_1()
                .border_color(t.border)
                .hover(move |s| s.bg(hover))
        })
        .child(label)
}

impl Render for DrawingToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = self.snap.clone();
        let mut root = div().size_full();
        if let Some(s) = &snap {
            root = root.child(self.bar(s, window, cx));
        }
        if self.grip.is_some() {
            root = root.child(self.grip_layer(window, cx));
        }
        if let (Some(Popover::Context(p)), Some(_)) = (self.open, &snap) {
            root = root.children(self.context_layer(p, window, cx));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }

    const W: f32 = 500.;

    #[test]
    fn toolbar_position_flip_clamp() {
        // Above, centered.
        let (x, y) = position(r(400., 200., 100., 50.), (1000., 600.), W);
        assert_eq!(y, 200. - OFFSET - HEIGHT);
        assert!((x + W / 2. - 450.).abs() < 1e-3);
        // Flips below when there is no room above.
        let (_, y) = position(r(400., 10., 100., 50.), (1000., 600.), W);
        assert_eq!(y, 60. + OFFSET);
        // Clamped inside the chart.
        let (x, _) = position(r(0., 300., 0., 0.), (1000., 600.), W);
        assert_eq!(x, MARGIN);
        let (x, _) = position(r(990., 300., 0., 0.), (1000., 600.), W);
        assert_eq!(x, 1000. - W - MARGIN);
        // Tall drawing filling the chart: below would overflow, so clamp.
        let (_, y) = position(r(100., 0., 10., 600.), (1000., 600.), W);
        assert_eq!(y, 600. - HEIGHT - MARGIN);
        // A grip offset moves the bar, still clamped.
        let b = r(400., 200., 100., 50.);
        let (x0, y0) = position(b, (1000., 600.), W);
        assert_eq!(place(b, (1000., 600.), W, (10., -20.)), (x0 + 10., y0 - 20.));
        assert_eq!(place(b, (1000., 600.), W, (-5000., 5000.)), (MARGIN, 600. - HEIGHT - MARGIN));
    }

    #[test]
    fn width_drops_text_color_for_h_and_v_lines() {
        assert!(has_text_color(Tool::TrendLine) && has_text_color(Tool::Rectangle));
        assert!(!has_text_color(Tool::HorizontalLine) && !has_text_color(Tool::VerticalLine));
        assert_eq!(toolbar_width(true) - toolbar_width(false), BTN);
    }

    #[test]
    fn template_keeps_text_content() {
        use xv_drawings::{Anchor, DrawingKind};
        let a = Anchor {
            time_ms: 0,
            price: 1.0,
        };
        let mut d = Drawing {
            id: DrawingId(1),
            kind: DrawingKind::TrendLine { a, b: a },
            style: Style::default(),
            name: None,
            locked: false,
            hidden: false,
        };
        d.style.text.content = "keep me".into();
        let mut tpl = Style {
            width: 4,
            color: 0x112233FF,
            ..Style::default()
        };
        tpl.text.content = "template words".into();
        apply_template(&mut d, &tpl);
        assert_eq!(d.style.width, 4);
        assert_eq!(d.style.color, 0x112233FF);
        assert_eq!(d.style.text.content, "keep me");
    }
}
