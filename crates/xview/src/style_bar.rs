//! Floating style bar over the selected drawing: color swatches, line
//! widths and delete. Rendered inside the chart's container, so the chart
//! element's origin is this view's origin.

use gpui::{
    Context, Entity, IntoElement, MouseButton, Render, SharedString, Window, div, prelude::*, px,
};
use xv_chart::{ChartKey, RectF};
use xv_drawings::{PALETTE, Style};

use crate::chart_view::ChartView;
use crate::theme::{AppTheme, u32_to_hsla};
use crate::widgets::{icon, tooltip};

const HEIGHT: f32 = 36.;
const PAD: f32 = 6.;
const SWATCH: f32 = 20.;
const SWATCH_GAP: f32 = 3.;
const CELL: f32 = 26.;
const SEP_W: f32 = 9.;
/// Distance between the drawing and the bar.
const OFFSET: f32 = 8.;
/// Margin kept to the chart edges.
const MARGIN: f32 = 4.;
const WIDTHS: [u8; 4] = [1, 2, 3, 4];

pub const WIDTH: f32 = 2. * PAD
    + PALETTE.len() as f32 * SWATCH
    // Gaps between the bar's children: swatches, sep, widths, sep, delete.
    + (PALETTE.len() + 3) as f32 * SWATCH_GAP
    + SEP_W
    + WIDTHS.len() as f32 * CELL
    + SEP_W
    + CELL;

/// Top-left of the bar for a drawing with handle bounds `b` in a chart of
/// `size`: centered 8 px above the drawing, below it when there is no room
/// above, clamped inside the chart.
pub fn position(b: RectF, size: (f32, f32)) -> (f32, f32) {
    let max_x = (size.0 - WIDTH - MARGIN).max(MARGIN);
    let x = (b.x + b.w / 2.0 - WIDTH / 2.0).clamp(MARGIN, max_x);
    let above = b.y - OFFSET - HEIGHT;
    let y = if above >= MARGIN {
        above
    } else {
        b.y + b.h + OFFSET
    };
    let max_y = (size.1 - HEIGHT - MARGIN).max(MARGIN);
    (x, y.clamp(MARGIN, max_y))
}

pub struct StyleBar {
    chart: Entity<ChartView>,
    theme: AppTheme,
}

impl StyleBar {
    pub fn new(chart: Entity<ChartView>, theme: AppTheme) -> Self {
        StyleBar { chart, theme }
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn set_style(&self, s: Style, cx: &mut gpui::App) {
        self.chart.update(cx, |c, cx| {
            let fx = c.state.set_selected_style(s);
            c.apply_effects(fx, cx);
        });
    }
}

fn vsep(t: &AppTheme) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(SEP_W))
        .h(px(20.))
        .flex()
        .justify_center()
        .child(div().w(px(1.)).h_full().bg(t.border))
}

impl Render for StyleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Re-rendered with the workspace on every chart notify, so it follows
        // the drawing while it is dragged.
        let chart = self.chart.read(cx);
        let st = &chart.state;
        let selected = st.selected_screen_bounds().zip(
            st.selected
                .and_then(|id| st.drawings.get(id))
                .map(|d| d.style),
        );
        let Some((bounds, style)) = selected else {
            return div().into_any_element();
        };
        let l = &st.layout;
        let size = (l.plot.w + l.price_axis.w, l.plot.h + l.time_axis.h);
        let (x, y) = position(bounds, size);
        let t = self.theme.clone();
        let hover = t.hover;

        let swatches = PALETTE.iter().enumerate().map(|(i, &c)| {
            let active = style.color == c;
            div()
                .id(SharedString::from(format!("swatch-{i}")))
                .flex_none()
                .size(px(SWATCH))
                .rounded(px(4.))
                .bg(u32_to_hsla(c))
                .border_2()
                .border_color(if active { t.text } else { u32_to_hsla(c) })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_style(Style { color: c, ..style }, cx)
                }))
        });
        let widths = WIDTHS.iter().map(|&w| {
            let active = style.width == w;
            div()
                .id(SharedString::from(format!("width-{w}")))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(CELL))
                .rounded(px(4.))
                .when(active, |d| d.bg(t.active_bg))
                .when(!active, |d| d.hover(move |s| s.bg(hover)))
                .tooltip(tooltip(format!("{w} px"), &t))
                .child(div().w(px(14.)).h(px(w as f32)).bg(t.text))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_style(Style { width: w, ..style }, cx)
                }))
        });
        let chart = self.chart.clone();
        div()
            .id("style-bar")
            .absolute()
            .left(px(x))
            .top(px(y))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SWATCH_GAP))
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .px(px(PAD))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .shadow_md()
            // Clicks here must not reach the chart (that would deselect).
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(swatches)
            .child(vsep(&t))
            .child(div().flex().gap(px(0.)).children(widths))
            .child(vsep(&t))
            .child(
                div()
                    .id("style-delete")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(CELL))
                    .rounded(px(4.))
                    .hover(move |s| s.bg(hover))
                    .tooltip(tooltip("Delete drawing", &t))
                    .child(icon("trash", 22., t.text))
                    .on_click(move |_, _, cx| {
                        chart.update(cx, |c, cx| {
                            let fx = c.state.key(ChartKey::Delete);
                            c.apply_effects(fx, cx);
                        })
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> RectF {
        RectF { x, y, w, h }
    }

    #[test]
    fn sits_above_centered() {
        let (x, y) = position(r(400., 200., 100., 50.), (1000., 600.));
        assert_eq!(y, 200. - OFFSET - HEIGHT);
        assert!((x + WIDTH / 2. - 450.).abs() < 1e-3);
    }

    #[test]
    fn flips_below_when_no_room_above() {
        let (_, y) = position(r(400., 10., 100., 50.), (1000., 600.));
        assert_eq!(y, 60. + OFFSET);
    }

    #[test]
    fn clamps_inside_chart() {
        let (x, _) = position(r(0., 300., 0., 0.), (1000., 600.));
        assert_eq!(x, MARGIN);
        let (x, _) = position(r(990., 300., 0., 0.), (1000., 600.));
        assert_eq!(x, 1000. - WIDTH - MARGIN);
        // Tall drawing filling the chart: below would overflow, so clamp.
        let (_, y) = position(r(100., 0., 10., 600.), (1000., 600.));
        assert_eq!(y, 600. - HEIGHT - MARGIN);
    }
}
