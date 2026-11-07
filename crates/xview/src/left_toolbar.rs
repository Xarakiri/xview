//! Left drawing-tool column (spec §6.1). Re-renders only when the tool,
//! magnet, lock or hide state changes, or a flyout opens or closes.

use gpui::{
    Bounds, Context, Entity, Hsla, IntoElement, PathBuilder, Pixels, Render, SharedString,
    Subscription, Window, canvas, div, point, prelude::*, px,
};
use xv_drawings::Tool;

use crate::chart_view::ChartView;
use crate::theme::AppTheme;
use crate::widgets::{
    TOOLTIP_COMING_SOON, hsep, icon_button, menu_box, menu_row, popover_layer, tooltip,
};
use crate::workspace::{
    ClearDrawings, LEFT_TOOLBAR_WIDTH, REGION_BORDER, SelectTool, ToggleHideDrawings,
    ToggleLockDrawings, ToggleMagnet,
};

const BUTTON: f32 = 42.;
const GAP: f32 = 6.;
const CORNER: f32 = 6.;

/// One flyout entry: icon, label and the tool (None = coming soon).
type Item = (&'static str, &'static str, Option<Tool>);

/// The tool groups with flyouts.
const GROUPS: [&[Item]; 3] = [
    &[
        ("trend_line", "Trend line", Some(Tool::TrendLine)),
        ("ray", "Ray", Some(Tool::Ray)),
        ("extended_line", "Extended line", None),
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
    locked: bool,
    hidden: bool,
}

impl Snapshot {
    fn of(chart: &ChartView) -> Self {
        let s = &chart.state;
        Snapshot {
            tool: s.tool,
            magnet: s.magnet,
            locked: s.drawings_locked,
            hidden: s.drawings_hidden,
        }
    }
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
    open_group: Option<usize>,
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
            open_group: None,
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

    fn set_open(&mut self, g: Option<usize>, cx: &mut Context<Self>) {
        if self.open_group != g {
            self.open_group = g;
            cx.notify();
        }
    }

    fn group_button(&self, g: usize, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let (icon_name, _, _) = GROUPS[g][self.last_used[g]];
        let active = group_of(self.snap.tool) == Some(g);
        let open = self.open_group == Some(g);
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
        .on_click(cx.listener(move |this, _, _, cx| {
            let next = if this.open_group == Some(g) {
                None
            } else {
                Some(g)
            };
            this.set_open(next, cx);
        }));
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
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(GAP))
            .pt(px(8.))
            .w(px(LEFT_TOOLBAR_WIDTH))
            .h_full()
            .overflow_hidden()
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
            .child(
                self.toggle("magnet", "magnet", s.magnet, "Magnet: snap to OHLC", || {
                    Box::new(ToggleMagnet)
                }),
            )
            .child(
                self.toggle("lock", "lock", s.locked, "Lock all drawings", || {
                    Box::new(ToggleLockDrawings)
                }),
            )
            .child(
                self.toggle("eye", "eye", s.hidden, "Hide all drawings", || {
                    Box::new(ToggleHideDrawings)
                }),
            )
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
