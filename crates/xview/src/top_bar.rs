//! Top bar (spec §6.1): avatar, symbol pill, compare, timeframes, chart
//! style and chart type.

use gpui::{
    Context, FontWeight, Hsla, IntoElement, MouseButton, Render, SharedString, Window, div,
    prelude::*, px,
};
use xv_core::{Symbol, Timeframe};

use crate::theme::{AppTheme, u32_to_hsla};
use crate::widgets::{
    DISABLED_OPACITY, TOOLTIP_COMING_SOON, icon, icon_button, menu_box, menu_row, popover_layer, tooltip, vsep,
};
use crate::workspace::{OpenSymbolSearch, REGION_BORDER, SelectTimeframe, TOP_BAR_HEIGHT};

/// Avatar background (XView's own pick).
const AVATAR_BG: u32 = 0x8A5CA8FF;
/// Placeholder notification count (wired in a later milestone).
const BADGE_COUNT: &str = "11";

#[derive(Copy, Clone, PartialEq)]
enum Menu {
    Timeframes,
    ChartType,
}

pub struct TopBar {
    theme: AppTheme,
    symbol: SharedString,
    timeframe: Timeframe,
    /// Timeframes the current source offers; the others render disabled.
    supported: Vec<Timeframe>,
    menu: Option<Menu>,
}

fn long_name(tf: Timeframe) -> &'static str {
    match tf {
        Timeframe::M1 => "1 minute",
        Timeframe::M3 => "3 minutes",
        Timeframe::M5 => "5 minutes",
        Timeframe::M15 => "15 minutes",
        Timeframe::M30 => "30 minutes",
        Timeframe::H1 => "1 hour",
        Timeframe::H2 => "2 hours",
        Timeframe::H4 => "4 hours",
        Timeframe::D1 => "1 day",
        Timeframe::W1 => "1 week",
        Timeframe::Mo1 => "1 month",
    }
}

impl TopBar {
    pub fn new(theme: AppTheme, symbol: &Symbol, timeframe: Timeframe) -> Self {
        TopBar {
            theme,
            symbol: symbol.display.clone().into(),
            timeframe,
            supported: Timeframe::ALL.to_vec(),
            menu: None,
        }
    }

    pub fn set_supported(&mut self, supported: Vec<Timeframe>, cx: &mut Context<Self>) {
        if supported != self.supported {
            self.supported = supported;
            cx.notify();
        }
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub fn set_symbol_timeframe(&mut self, symbol: &Symbol, tf: Timeframe, cx: &mut Context<Self>) {
        let name: SharedString = symbol.display.clone().into();
        if name != self.symbol || tf != self.timeframe {
            self.symbol = name;
            self.timeframe = tf;
            cx.notify();
        }
    }

    fn toggle_menu(&mut self, m: Menu, cx: &mut Context<Self>) {
        self.menu = if self.menu == Some(m) { None } else { Some(m) };
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn avatar(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(38.))
            .mr(px(6.))
            .rounded_full()
            .bg(u32_to_hsla(AVATAR_BG))
            .text_color(gpui::white())
            .text_size(px(18.))
            .font_weight(FontWeight::SEMIBOLD)
            .child("X")
            .child(
                div()
                    .absolute()
                    .top(px(-6.))
                    .right(px(-8.))
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(10.))
                    .border_2()
                    .border_color(t.bg)
                    .bg(t.badge)
                    .text_color(gpui::white())
                    .text_size(px(11.))
                    .line_height(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(BADGE_COUNT),
            )
    }

    fn symbol_pill(&self) -> impl IntoElement {
        let t = &self.theme;
        div()
            .id("symbol-pill")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(14.))
            .h(px(40.))
            .pl(px(22.))
            .pr(px(4.))
            .rounded(px(20.))
            .bg(t.panel_bg)
            .hover(|s| s.bg(t.active_bg))
            .text_size(px(16.))
            .font_weight(FontWeight::BOLD)
            .child(self.symbol.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(32.))
                    .rounded_full()
                    .bg(t.bg)
                    .child(icon("candles", 22., t.text)),
            )
            .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSymbolSearch), cx))
    }

    fn timeframe_button(&self, tf: Timeframe) -> impl IntoElement {
        let t = &self.theme;
        let active = tf == self.timeframe;
        let enabled = self.supported.contains(&tf);
        let hover = t.hover;
        let button = div()
            .id(SharedString::from(format!("tf-{}", tf.label())))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(px(38.))
            .min_w(px(38.))
            .px(px(7.))
            .rounded(px(6.))
            .text_size(px(16.))
            .when(active, |d| d.bg(t.active_bg))
            .child(tf.label());
        if !enabled {
            return button
                .opacity(DISABLED_OPACITY)
                .tooltip(tooltip("Not available for this symbol", t));
        }
        button
            .when(!active, |d| d.hover(move |s| s.bg(hover)))
            .on_click(move |_, window, cx| {
                window.dispatch_action(Box::new(SelectTimeframe(tf)), cx)
            })
    }

    fn chevron(
        &self,
        id: &'static str,
        menu: Menu,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        let open = self.menu == Some(menu);
        let hover = t.hover;
        div()
            .id(id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(px(38.))
            .min_w(px(30.))
            .rounded(px(6.))
            .when(open, |d| d.bg(t.active_bg))
            .when(!open, |d| d.hover(move |s| s.bg(hover)))
            .child(icon("chevron_down", 18., t.text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.toggle_menu(menu, cx)),
            )
    }

    fn timeframe_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let mut menu = menu_box(t);
        for tf in Timeframe::ALL {
            let enabled = self.supported.contains(&tf);
            let row = menu_row(
                SharedString::from(format!("tf-menu-{}", tf.label())),
                None,
                long_name(tf),
                tf == self.timeframe,
                enabled,
                t,
            );
            menu = menu.child(if enabled {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    this.close_menu(cx);
                    window.dispatch_action(Box::new(SelectTimeframe(tf)), cx);
                }))
            } else {
                row
            });
        }
        menu
    }

    fn chart_type_menu(&self) -> impl IntoElement {
        let t = &self.theme;
        let mut menu = menu_box(t).child(menu_row(
            "type-candles",
            Some("candles"),
            "Candles",
            true,
            true,
            t,
        ));
        for (id, icon_name, label) in [
            ("type-bars", "bars_style", "Bars"),
            ("type-line", "trend_line", "Line"),
            ("type-ha", "candles", "Heikin Ashi"),
        ] {
            menu = menu.child(
                menu_row(id, Some(icon_name), label, false, false, t)
                    .tooltip(tooltip(TOOLTIP_COMING_SOON, t)),
            );
        }
        menu
    }
}

impl Render for TopBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let close = cx.listener(|this, _, _, cx| this.close_menu(cx));
        let mut tf_chevron = self.chevron("tf-chevron", Menu::Timeframes, cx);
        let mut type_chevron = self.chevron("type-chevron", Menu::ChartType, cx);
        match self.menu {
            Some(Menu::Timeframes) => {
                tf_chevron = tf_chevron.children(popover_layer(
                    (0., 42.),
                    self.timeframe_menu(cx),
                    close,
                    window,
                ));
            }
            Some(Menu::ChartType) => {
                type_chevron = type_chevron.children(popover_layer(
                    (0., 42.),
                    self.chart_type_menu(),
                    close,
                    window,
                ));
            }
            None => {}
        }
        let border: Hsla = t.panel_bg;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .w_full()
            .h(px(TOP_BAR_HEIGHT + REGION_BORDER))
            .px(px(10.))
            .border_b(px(REGION_BORDER))
            .border_color(border)
            .bg(t.bg)
            .text_color(t.text)
            .child(self.avatar())
            .child(self.symbol_pill())
            .child(
                icon_button("compare", "plus_circle", 38., false, false, &t)
                    .tooltip(tooltip("Compare (coming soon)", &t)),
            )
            .child(vsep(26., &t))
            .children(Timeframe::TOP_BAR.map(|tf| self.timeframe_button(tf)))
            .child(tf_chevron)
            .child(vsep(26., &t))
            .child(
                icon_button("chart-settings", "bars_style", 38., false, true, &t)
                    .tooltip(tooltip("Chart settings (coming soon)", &t)),
            )
            .child(icon_button("chart-type", "candles", 38., true, true, &t))
            .child(type_chevron)
    }
}
