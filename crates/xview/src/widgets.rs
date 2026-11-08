//! Small shared UI helpers for the app shell: icons, tooltips, separators and
//! click-away popover layers. XView does not use Zed's `ui` crate.

use gpui::{
    Anchor, AnchoredPositionMode, AnyElement, AnyView, App, Context, Div, Hsla, MouseButton,
    MouseDownEvent, Render, SharedString, Stateful, Svg, Window, anchored, deferred, div, point,
    prelude::*, px, svg,
};

use crate::assets::icon_path;
use crate::theme::AppTheme;

/// Opacity of disabled controls.
pub const DISABLED_OPACITY: f32 = 0.4;
pub const TOOLTIP_COMING_SOON: &str = "Coming soon";

pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(icon_path(name))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A 1 px vertical separator of the given height.
pub fn vsep(height: f32, theme: &AppTheme) -> Div {
    div()
        .flex_none()
        .w(px(1.))
        .h(px(height))
        .mx(px(6.))
        .bg(theme.border)
}

/// A horizontal divider for the tool column.
pub fn hsep(width: f32, theme: &AppTheme) -> Div {
    div()
        .flex_none()
        .w(px(width))
        .h(px(1.))
        .my(px(4.))
        .bg(theme.border)
}

struct TooltipView {
    text: SharedString,
    bg: Hsla,
    fg: Hsla,
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(self.bg)
            .text_color(self.fg)
            .text_size(px(12.))
            .child(self.text.clone())
    }
}

/// A tooltip builder for `.tooltip(...)`.
pub fn tooltip(
    text: impl Into<SharedString>,
    theme: &AppTheme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    // Tooltips are inverted: text color as background.
    let (bg, fg) = (theme.text, theme.bg);
    move |_, cx| {
        let text = text.clone();
        cx.new(|_| TooltipView { text, bg, fg }).into()
    }
}

/// A square icon button. `active` gets the active background; disabled
/// buttons are dimmed and only show their tooltip.
pub fn icon_button(
    id: impl Into<gpui::ElementId>,
    name: &str,
    size: f32,
    active: bool,
    enabled: bool,
    theme: &AppTheme,
) -> Stateful<Div> {
    let hover = theme.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(6.))
        .when(active, |d| d.bg(theme.active_bg))
        .when(enabled && !active, |d| d.hover(move |s| s.bg(hover)))
        .when(!enabled, |d| d.opacity(DISABLED_OPACITY))
        .child(icon(name, 28., theme.text))
}

/// A popover menu box.
pub fn menu_box(theme: &AppTheme) -> Div {
    div()
        .flex()
        .flex_col()
        .py(px(6.))
        .min_w(px(180.))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg)
        .text_color(theme.text)
        .shadow_lg()
}

/// One row of a popover menu.
pub fn menu_row(
    id: impl Into<gpui::ElementId>,
    icon_name: Option<&str>,
    label: impl Into<SharedString>,
    active: bool,
    enabled: bool,
    theme: &AppTheme,
) -> Stateful<Div> {
    let hover = theme.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(36.))
        .px(px(12.))
        .text_size(px(14.))
        .when(active, |d| d.bg(theme.active_bg))
        .when(enabled && !active, |d| d.hover(move |s| s.bg(hover)))
        .when(!enabled, |d| d.opacity(DISABLED_OPACITY))
        .when_some(icon_name, |d, n| d.child(icon(n, 28., theme.text)))
        .child(label.into())
}

/// A window-sized transparent layer under a popover that closes it on any
/// left or right click outside, and keeps those clicks from the chart.
pub fn backdrop(
    on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    window: &Window,
) -> AnyElement {
    let vp = window.viewport_size();
    let on_close = std::rc::Rc::new(on_close);
    let on_right = on_close.clone();
    deferred(
        anchored()
            .position_mode(AnchoredPositionMode::Window)
            .position(point(px(0.), px(0.)))
            .child(
                div()
                    .id("popover-backdrop")
                    .w(vp.width)
                    .h(vp.height)
                    .occlude()
                    .on_mouse_down(MouseButton::Left, move |e, w, cx| on_close(e, w, cx))
                    .on_mouse_down(MouseButton::Right, move |e, w, cx| on_right(e, w, cx)),
            ),
    )
    .priority(1)
    .into_any_element()
}

/// Places `menu` at `offset` from the origin of the element this is added
/// to (which should be `relative()`), in a floating layer, above a
/// window-sized transparent backdrop that closes the menu on any click.
pub fn popover_layer(
    offset: (f32, f32),
    menu: impl IntoElement,
    on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    window: &Window,
) -> [AnyElement; 2] {
    popover_layer_at(offset, Anchor::TopLeft, menu, on_close, window)
}

/// `popover_layer` with the menu's `anchor` corner at `offset` (e.g.
/// `BottomLeft` to open upwards).
pub fn popover_layer_at(
    offset: (f32, f32),
    anchor: Anchor,
    menu: impl IntoElement,
    on_close: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    window: &Window,
) -> [AnyElement; 2] {
    let backdrop = backdrop(on_close, window);
    let menu = div()
        .absolute()
        .left(px(offset.0))
        .top(px(offset.1))
        .child(
            deferred(
                anchored()
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(div().occlude().child(menu)),
            )
            .priority(2),
        )
        .into_any_element();
    [backdrop, menu]
}
