//! Left drawing-tool column (spec §6.1). Re-renders only when the tool,
//! magnet, stay-in-drawing-mode, lock or hide state (or the hidden-drawing
//! count) changes, or a flyout opens or closes.

use gpui::{
    Bounds, Context, Entity, Hsla, IntoElement, PathBuilder, Pixels, Render, SharedString,
    Subscription, Window, canvas, div, point, prelude::*, px,
};
use xv_chart::MagnetMode;
use xv_drawings::Tool;

use crate::chart_view::ChartView;
use crate::theme::AppTheme;
use crate::widgets::{
    DISABLED_OPACITY, TOOLTIP_COMING_SOON, hsep, icon_button, menu_box, menu_row,
    popover_layer, tooltip,
};
use crate::workspace::{
    ClearDrawings, LEFT_TOOLBAR_WIDTH, REGION_BORDER, SelectTool, SetMagnetMode,
    ToggleHideDrawings, ToggleLockDrawings, ToggleMagnet, ToggleStayInDrawingMode,
    UnhideAllDrawings,
};

const BUTTON: f32 = 42.;
const GAP: f32 = 6.;
const CORNER: f32 = 6.;
/// Click target around a toggle button's corner triangle (opens its flyout).
const CORNER_HIT: f32 = 16.;
const SWITCH_W: f32 = 34.;
const SWITCH_H: f32 = 20.;

/// One flyout entry: icon, label and the tool (None = coming soon).
type Item = (&'static str, &'static str, Option<Tool>);

/// The tool groups with flyouts.
const GROUPS: [&[Item]; 3] = [
    &[
        ("trend_line", "Trend line", Some(Tool::TrendLine)),
        ("ray", "Ray", Some(Tool::Ray)),
        ("extended_line", "Extended line", Some(Tool::ExtendedLine)),
    ],
    &[
        (
            "horizontal_line",
            "Horizontal line",
            Some(Tool::HorizontalLine),
        ),
        ("horizontal_ray", "Horizontal ray", None),
        ("vertical_line", "Vertical line", Some(Tool::VerticalLine)),
    ],
    &[
        ("rectangle", "Rectangle", Some(Tool::Rectangle)),
        ("brush", "Brush", None),
    ],
];

/// The chart state the column shows.
#[derive(Copy, Clone, PartialEq)]
struct Snapshot {
    tool: Tool,
    magnet: bool,
    magnet_mode: MagnetMode,
    stay: bool,
    locked: bool,
    hidden: bool,
    /// Individually hidden drawings ("Show N hidden drawings").
    hidden_count: usize,
}

impl Snapshot {
    fn of(chart: &ChartView) -> Self {
        let s = &chart.state;
        Snapshot {
            tool: s.tool,
            magnet: s.magnet,
            magnet_mode: s.magnet_mode,
            stay: s.stay_in_drawing_mode,
            locked: s.drawings_locked,
            hidden: s.drawings_hidden,
            hidden_count: s.hidden_count(),
        }
    }
}

/// The open flyout.
#[derive(Copy, Clone, PartialEq, Debug)]
enum Flyout {
    Group(usize),
    Magnet,
    Eye,
}

fn group_of(tool: Tool) -> Option<usize> {
    GROUPS
        .iter()
        .position(|g| g.iter().any(|(_, _, t)| *t == Some(tool)))
}

pub struct LeftToolbar {
    theme: AppTheme,
    snap: Snapshot,
    /// Index into the group's items of the last-used tool, per group.
    last_used: [usize; 3],
    open: Option<Flyout>,
    _observe: Subscription,
}

impl LeftToolbar {
    pub fn new(chart: Entity<ChartView>, theme: AppTheme, cx: &mut Context<Self>) -> Self {
        let snap = Snapshot::of(chart.read(cx));
        // Chart notifies on every mouse move; only re-render on a change
        // (e.g. ToolFinished returning to the cursor).
        let observe = cx.observe(&chart, |this, chart, cx| {
            let snap = Snapshot::of(chart.read(cx));
            if snap != this.snap {
                this.snap = snap;
                this.remember(snap.tool);
                cx.notify();
            }
        });
        LeftToolbar {
            theme,
            snap,
            last_used: [0; 3],
            open: None,
            _observe: observe,
        }
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn remember(&mut self, tool: Tool) {
        if let Some(g) = group_of(tool)
            && let Some(i) = GROUPS[g].iter().position(|(_, _, t)| *t == Some(tool))
        {
            self.last_used[g] = i;
        }
    }

    fn set_open(&mut self, f: Option<Flyout>, cx: &mut Context<Self>) {
        if self.open != f {
            self.open = f;
            cx.notify();
        }
    }

    fn toggle_open(&mut self, f: Flyout, cx: &mut Context<Self>) {
        let next = (self.open != Some(f)).then_some(f);
        self.set_open(next, cx);
    }

    /// Forces a flyout open (debug script screenshots).
    #[cfg(debug_assertions)]
    pub fn debug_open_flyout(&mut self, name: &str, cx: &mut Context<Self>) {
        let f = match name {
            "magnet" => Some(Flyout::Magnet),
            "eye" => Some(Flyout::Eye),
            "trend" => Some(Flyout::Group(0)),
            _ => None,
        };
        self.set_open(f, cx);
    }

    fn group_button(&self, g: usize, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let (icon_name, _, _) = GROUPS[g][self.last_used[g]];
        let active = group_of(self.snap.tool) == Some(g);
        let open = self.open == Some(Flyout::Group(g));
        let mut button = icon_button(
            SharedString::from(format!("group-{g}")),
            icon_name,
            BUTTON,
            active || open,
            true,
            t,
        )
        .relative()
        .when(!active, |b| b.child(corner(t.muted)))
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(Flyout::Group(g), cx)));
        if open {
            let menu = self.flyout(g, cx).into_any_element();
            let close = cx.listener(|this, _, _, cx| this.set_open(None, cx));
            button = button.children(popover_layer((BUTTON + 4., 0.), menu, close, window));
        }
        button
    }

    fn flyout(&self, g: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let mut menu = menu_box(t);
        for (i, &(icon_name, label, tool)) in GROUPS[g].iter().enumerate() {
            let id = SharedString::from(format!("flyout-{g}-{i}"));
            let active = tool.is_some_and(|tl| tl == self.snap.tool);
            let row = menu_row(id, Some(icon_name), label, active, tool.is_some(), t);
            menu = menu.child(match tool {
                Some(tool) => row.on_click(cx.listener(move |this, _, window, cx| {
                    this.last_used[g] = i;
                    this.set_open(None, cx);
                    cx.notify();
                    window.dispatch_action(Box::new(SelectTool(tool)), cx);
                })),
                None => row.tooltip(tooltip(TOOLTIP_COMING_SOON, t)),
            });
        }
        menu
    }

    /// A placeholder tool group (disabled, coming soon).
    fn placeholder(&self, id: &'static str, icon_name: &str, flyout: bool) -> impl IntoElement {
        let t = &self.theme;
        icon_button(id, icon_name, BUTTON, false, false, t)
            .relative()
            .when(flyout, |b| b.child(corner(t.muted)))
            .tooltip(tooltip(TOOLTIP_COMING_SOON, t))
    }

    fn toggle(
        &self,
        id: &'static str,
        icon_name: &str,
        on: bool,
        tip: &'static str,
        action: fn() -> Box<dyn gpui::Action>,
    ) -> impl IntoElement {
        let t = &self.theme;
        icon_button(id, icon_name, BUTTON, on, true, t)
            .tooltip(tooltip(tip, t))
            .on_click(move |_, window, cx| window.dispatch_action(action(), cx))
    }

    /// A toggle button whose click dispatches `action` and whose corner
    /// triangle opens `flyout`, built by `menu`.
    #[allow(clippy::too_many_arguments)]
    fn toggle_with_flyout(
        &self,
        id: &'static str,
        icon_name: &str,
        on: bool,
        tip: &'static str,
        action: fn() -> Box<dyn gpui::Action>,
        flyout: Flyout,
        menu: impl FnOnce(&Self, &mut Context<Self>) -> gpui::AnyElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let open = self.open == Some(flyout);
        let corner_hit = div()
            .id(SharedString::from(format!("{id}-corner")))
            .absolute()
            .right(px(0.))
            .bottom(px(0.))
            .size(px(CORNER_HIT))
            .child(corner(t.muted))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_open(flyout, cx);
            }));
        let mut button = icon_button(id, icon_name, BUTTON, on || open, true, t)
            .relative()
            .tooltip(tooltip(tip, t))
            .child(corner_hit)
            .on_click(move |_, window, cx| window.dispatch_action(action(), cx));
        if open {
            let menu = menu(self, cx);
            let close = cx.listener(|this, _, _, cx| this.set_open(None, cx));
            button = button.children(popover_layer((BUTTON + 4., 0.), menu, close, window));
        }
        button
    }

    fn magnet_flyout(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = &self.theme;
        let mode = self.snap.magnet_mode;
        let mut menu = menu_box(t).min_w(px(240.));
        for (id, icon_name, label, m) in [
            ("magnet-weak", "magnet_weak", "Weak magnet", MagnetMode::Weak),
            ("magnet-strong", "magnet_strong", "Strong magnet", MagnetMode::Strong),
        ] {
            let row = menu_row(id, Some(icon_name), label, false, true, t)
                .child(div().flex_1())
                .when(mode == m, |r| r.child(check(t)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_open(None, cx);
                    window.dispatch_action(Box::new(SetMagnetMode(m)), cx);
                }));
            menu = menu.child(row);
        }
        menu.child(menu_sep(t))
            .child(
                div()
                    .id("snap-indicators")
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .px(px(12.))
                    .text_size(px(14.))
                    .tooltip(tooltip(TOOLTIP_COMING_SOON, t))
                    .child(div().opacity(DISABLED_OPACITY).child("Snap to indicators"))
                    .child(div().flex_1().min_w(px(16.)))
                    .child(switch(false, false, t)),
            )
            .into_any_element()
    }

    fn eye_flyout(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = &self.theme;
        let n = self.snap.hidden_count;
        let show_label = if n == 1 {
            "Show 1 hidden drawing".to_string()
        } else {
            format!("Show {n} hidden drawings")
        };
        let show = menu_row("eye-show-hidden", Some("eye"), show_label, false, n > 0, t)
            .when(n > 0, |r| {
                r.on_click(cx.listener(|this, _, window, cx| {
                    this.set_open(None, cx);
                    window.dispatch_action(Box::new(UnhideAllDrawings), cx);
                }))
            });
        menu_box(t)
            .min_w(px(240.))
            .child(
                menu_row("eye-hide-all", Some("eye"), "Hide all drawings", false, true, t)
                    .child(div().flex_1().min_w(px(16.)))
                    .child(switch(self.snap.hidden, true, t))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ToggleHideDrawings), cx)
                    }),
            )
            .child(show)
            .into_any_element()
    }
}

/// A check mark for the current choice in a menu.
fn check(t: &AppTheme) -> impl IntoElement {
    div().text_color(t.accent).text_size(px(15.)).child("✓")
}

/// A full-width separator inside a menu.
fn menu_sep(t: &AppTheme) -> impl IntoElement {
    div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(t.border)
}

/// An on/off switch (display only; the row handles the click).
fn switch(on: bool, enabled: bool, t: &AppTheme) -> impl IntoElement {
    let knob = SWITCH_H - 4.;
    div()
        .flex_none()
        .flex()
        .items_center()
        .w(px(SWITCH_W))
        .h(px(SWITCH_H))
        .px(px(2.))
        .rounded(px(SWITCH_H / 2.))
        .bg(if on { t.accent } else { t.border })
        .when(on, |d| d.justify_end())
        .when(!enabled, |d| d.opacity(DISABLED_OPACITY))
        .child(div().size(px(knob)).rounded(px(knob / 2.)).bg(t.bg))
}

/// The small triangle in a group button's bottom-right corner.
fn corner(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let mut p = PathBuilder::fill();
            p.move_to(point(b.right(), b.top()));
            p.line_to(point(b.right(), b.bottom()));
            p.line_to(point(b.left(), b.bottom()));
            p.close();
            if let Ok(path) = p.build() {
                window.paint_path(path, color);
            }
        },
    )
    .absolute()
    .right(px(3.))
    .bottom(px(3.))
    .size(px(CORNER))
}

impl Render for LeftToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let s = self.snap;
        let cursor_active = s.tool == Tool::Cursor;
        div()
            // Scrolls vertically so every tool stays reachable in short windows.
            .id("left-toolbar")
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(GAP))
            .pt(px(8.))
            .pb(px(8.))
            .w(px(LEFT_TOOLBAR_WIDTH))
            .h_full()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .border_r(px(REGION_BORDER))
            .border_color(t.panel_bg)
            .bg(t.bg)
            .text_color(t.text)
            .child(
                icon_button("cursor", "crosshair", BUTTON, cursor_active, true, &t)
                    .tooltip(tooltip("Crosshair", &t))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(SelectTool(Tool::Cursor)), cx)
                    }),
            )
            .child(self.group_button(0, window, cx))
            .child(self.group_button(1, window, cx))
            .child(self.placeholder("patterns", "patterns", true))
            .child(self.placeholder("fibonacci", "fibonacci", true))
            .child(self.group_button(2, window, cx))
            .child(self.placeholder("text", "text", true))
            .child(self.placeholder("emoji", "emoji", true))
            .child(hsep(30., &t))
            .child(self.placeholder("measure", "measure", false))
            .child(self.placeholder("zoom", "zoom", false))
            .child(hsep(30., &t))
            .child(self.toggle_with_flyout(
                "magnet",
                match s.magnet_mode {
                    MagnetMode::Weak => "magnet_weak",
                    MagnetMode::Strong => "magnet_strong",
                },
                s.magnet,
                match s.magnet_mode {
                    MagnetMode::Weak => "Weak magnet",
                    MagnetMode::Strong => "Strong magnet",
                },
                || Box::new(ToggleMagnet),
                Flyout::Magnet,
                Self::magnet_flyout,
                window,
                cx,
            ))
            .child(self.toggle(
                "stay",
                "pencil_lock",
                s.stay,
                "Stay in drawing mode",
                || Box::new(ToggleStayInDrawingMode),
            ))
            .child(
                self.toggle("lock", "lock", s.locked, "Lock all drawings", || {
                    Box::new(ToggleLockDrawings)
                }),
            )
            .child(self.toggle_with_flyout(
                "eye",
                "eye",
                s.hidden,
                "Hide all drawings",
                || Box::new(ToggleHideDrawings),
                Flyout::Eye,
                Self::eye_flyout,
                window,
                cx,
            ))
            .child(hsep(30., &t))
            .child(
                self.toggle("trash", "trash", false, "Remove all drawings", || {
                    Box::new(ClearDrawings)
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drawing_tool_is_in_one_group() {
        for tool in [
            Tool::TrendLine,
            Tool::ExtendedLine,
            Tool::Ray,
            Tool::HorizontalLine,
            Tool::VerticalLine,
            Tool::Rectangle,
        ] {
            assert!(group_of(tool).is_some(), "{tool:?}");
        }
        assert_eq!(group_of(Tool::Cursor), None);
    }
}
