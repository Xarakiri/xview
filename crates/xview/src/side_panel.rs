//! Right side: the watchlist/details panel and the icon rail. Placeholders
//! in milestone 1 (spec §6.1), except the details card, which shows the
//! current symbol and its live last price from the chart series.

use gpui::{
    Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use xv_chart::format_price;

use crate::chart_view::ChartView;
use crate::theme::{AppTheme, u32_to_hsla};
use crate::widgets::{DISABLED_OPACITY, TOOLTIP_COMING_SOON, icon, icon_button, tooltip};
use crate::workspace::{RAIL_WIDTH, REGION_BORDER, SIDE_PANEL_WIDTH};

const RAIL_ICONS: [&str; 6] = ["list", "bell", "chat", "bulb", "calendar", "help"];

/// What the details card shows.
#[derive(Clone, PartialEq)]
struct Details {
    symbol: SharedString,
    description: SharedString,
    /// Last close and the change from the previous close.
    last: Option<(f64, f64)>,
    tick_size: f64,
}

impl Details {
    fn of(chart: &ChartView) -> Self {
        let s = &chart.state.series;
        let cs = s.candles();
        let last = cs.last().map(|c| {
            let prev = if cs.len() >= 2 {
                cs[cs.len() - 2].close
            } else {
                c.open
            };
            (c.close, c.close - prev)
        });
        Details {
            symbol: s.symbol.display.clone().into(),
            description: s.symbol.description.clone().into(),
            last,
            tick_size: s.symbol.tick_size,
        }
    }
}

pub struct SidePanel {
    theme: AppTheme,
    details: Details,
    _observe: Subscription,
}

impl SidePanel {
    pub fn new(chart: Entity<ChartView>, theme: AppTheme, cx: &mut Context<Self>) -> Self {
        let details = Details::of(chart.read(cx));
        let observe = cx.observe(&chart, |this, chart, cx| {
            let d = Details::of(chart.read(cx));
            if d != this.details {
                this.details = d;
                cx.notify();
            }
        });
        SidePanel {
            theme,
            details,
            _observe: observe,
        }
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn header(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(48.))
            .px(px(12.))
            .gap(px(4.))
            .child(
                div()
                    .flex_1()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Watchlist"),
            )
            .child(
                icon_button("wl-add", "plus", 32., false, false, t)
                    .tooltip(tooltip(TOOLTIP_COMING_SOON, t)),
            )
            .child(
                icon_button("wl-more", "more", 32., false, false, t)
                    .tooltip(tooltip(TOOLTIP_COMING_SOON, t)),
            )
    }

    fn details_card(&self) -> impl IntoElement {
        let t = &self.theme;
        let d = &self.details;
        let (price, change) = match d.last {
            Some((close, ch)) => {
                let up = ch >= 0.0;
                let color = u32_to_hsla(if up { t.chart.up } else { t.chart.down });
                let pct = if close - ch != 0.0 {
                    ch / (close - ch) * 100.0
                } else {
                    0.0
                };
                let sign = if up { "+" } else { "-" };
                (
                    format_price(close, d.tick_size),
                    Some((
                        format!(
                            "{sign}{} ({sign}{:.2}%)",
                            format_price(ch.abs(), d.tick_size),
                            pct.abs()
                        ),
                        color,
                    )),
                )
            }
            None => ("-".to_string(), None),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.))
            .p(px(16.))
            .border_t(px(REGION_BORDER))
            .border_color(t.panel_bg)
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::BOLD)
                    .child(d.symbol.clone()),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child(d.description.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(8.))
                    .mt(px(10.))
                    .child(
                        div()
                            .text_size(px(26.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(price),
                    )
                    .when_some(change, |row, (text, color)| {
                        row.child(
                            div()
                                .pb(px(4.))
                                .text_size(px(14.))
                                .text_color(color)
                                .child(text),
                        )
                    }),
            )
    }

    fn rail(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .pt(px(8.))
            .w(px(RAIL_WIDTH + REGION_BORDER))
            .h_full()
            .border_l(px(REGION_BORDER))
            .border_color(t.panel_bg)
            .children(RAIL_ICONS.iter().map(|&name| {
                div()
                    .id(SharedString::from(format!("rail-{name}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(36.))
                    .rounded(px(6.))
                    .opacity(DISABLED_OPACITY)
                    .tooltip(tooltip(TOOLTIP_COMING_SOON, t))
                    .child(icon(name, 28., t.text))
            }))
    }
}

impl Render for SidePanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        div()
            .flex()
            .flex_row()
            .h_full()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(SIDE_PANEL_WIDTH + REGION_BORDER))
                    .h_full()
                    .border_l(px(REGION_BORDER))
                    .border_color(t.panel_bg)
                    .child(self.header())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .px(px(24.))
                            .text_size(px(14.))
                            .text_color(t.muted)
                            .child("Watchlist arrives in a later version"),
                    )
                    .child(self.details_card()),
            )
            .child(self.rail())
    }
}
