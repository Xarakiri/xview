//! Root view: lays out the top bar, tool column, chart, bottom bar, side
//! panel and rail, and handles the workspace actions.
//!
//! Rendering cost: the toolbars and panels are separate entities embedded as
//! cached views, so a chart notify (every mouse move) re-renders only the
//! chart, the style bar and this light layout shell. Each cached view
//! notifies itself only when what it shows changes.

use gpui::{
    Action, App, Context, Entity, Focusable, KeyBinding, PromptLevel, StyleRefinement,
    Subscription, Window, actions, div, prelude::*, px,
};
use xv_chart::ChartEffect;
use xv_core::{Series, Symbol, Timeframe, TzOffset, resample};
use xv_drawings::Tool;

use crate::bottom_bar::BottomBar;
use crate::chart_view::{ChartView, ChartViewEvent};
use crate::demo;
use crate::left_toolbar::LeftToolbar;
use crate::side_panel::SidePanel;
use crate::style_bar::StyleBar;
use crate::theme::AppTheme;
use crate::top_bar::TopBar;

pub const TOP_BAR_HEIGHT: f32 = 56.;
pub const LEFT_TOOLBAR_WIDTH: f32 = 52.;
pub const BOTTOM_BAR_HEIGHT: f32 = 34.;
pub const SIDE_PANEL_WIDTH: f32 = 300.;
pub const RAIL_WIDTH: f32 = 44.;
/// Thick panel-gray borders between the shell regions.
pub const REGION_BORDER: f32 = 4.;

// The workspace actions (the brief's `WorkspaceAction` set). The data-carrying
// ones are `no_json`: they are dispatched from buttons, never from keymaps.
actions!(
    workspace,
    [
        ToggleHideDrawings,
        ToggleLockDrawings,
        ToggleMagnet,
        ClearDrawings,
        OpenSymbolSearch,
        OpenCsv,
        ToggleTheme,
        ToggleClockUtc,
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct SelectTimeframe(pub Timeframe);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct SelectTool(pub Tool);

/// Fits the last `span_ms` (or everything for `None`) into the chart.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct ShowSpan(pub Option<i64>);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", OpenSymbolSearch, None),
        KeyBinding::new("cmd-o", OpenCsv, None),
        KeyBinding::new("alt-cmd-t", ToggleTheme, None),
    ]);
}

pub struct Workspace {
    chart: Entity<ChartView>,
    theme: AppTheme,
    symbol: Symbol,
    timeframe: Timeframe,
    /// Clock and chart times in UTC instead of the local offset.
    use_utc: bool,
    top_bar: Entity<TopBar>,
    left_toolbar: Entity<LeftToolbar>,
    style_bar: Entity<StyleBar>,
    side_panel: Entity<SidePanel>,
    bottom_bar: Entity<BottomBar>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = AppTheme::light();
        let use_utc = false;
        let tz = tz_for(use_utc);
        // TODO(Task 15): replace the demo series with the loader.
        let series = demo_series(Timeframe::M1, tz);
        let symbol = series.symbol.clone();
        let timeframe = series.timeframe;
        let chart = cx.new(|cx| {
            let mut view = ChartView::new(series, "XView", theme.chart.clone(), window, cx);
            view.state.tz = tz;
            if let Some(spacing) = demo::bar_spacing() {
                view.state.vp.bar_spacing = spacing;
            }
            view
        });
        let events = cx.subscribe(&chart, |_, _, e: &ChartViewEvent, _| match e {
            // TODO(Task 15): fetch older history / save drawings.
            ChartViewEvent::NeedOlder(before) => eprintln!("xview: need older before {before}"),
            ChartViewEvent::DrawingsChanged
            | ChartViewEvent::ToolFinished
            | ChartViewEvent::SelectionChanged => {}
        });
        if demo::autopan() {
            demo::start_autopan(chart.clone(), cx);
        }
        window.focus(&chart.focus_handle(cx), cx);

        let top_bar = cx.new(|_| TopBar::new(theme.clone(), &symbol, timeframe));
        let left_toolbar = cx.new(|cx| LeftToolbar::new(chart.clone(), theme.clone(), cx));
        let style_bar = cx.new(|_| StyleBar::new(chart.clone(), theme.clone()));
        let side_panel = cx.new(|cx| SidePanel::new(chart.clone(), theme.clone(), cx));
        let bottom_bar = cx.new(|cx| BottomBar::new(chart.clone(), theme.clone(), use_utc, cx));

        Workspace {
            chart,
            theme,
            symbol,
            timeframe,
            use_utc,
            top_bar,
            left_toolbar,
            style_bar,
            side_panel,
            bottom_bar,
            _subscriptions: vec![events],
        }
    }

    /// Runs `f` on the chart state and routes the returned effects.
    fn with_chart(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut xv_chart::ChartState) -> Vec<ChartEffect>,
    ) {
        self.chart.update(cx, |chart, cx| {
            let fx = f(&mut chart.state);
            chart.apply_effects(fx, cx);
        });
    }

    fn select_timeframe(&mut self, tf: Timeframe, cx: &mut Context<Self>) {
        if tf == self.timeframe {
            return;
        }
        self.timeframe = tf;
        self.reload_series(cx);
        let (symbol, tf) = (self.symbol.clone(), self.timeframe);
        self.top_bar
            .update(cx, |bar, cx| bar.set_symbol_timeframe(&symbol, tf, cx));
        self.on_settings_changed();
    }

    /// TODO(Task 15): load through the data registry instead of the demo.
    fn reload_series(&mut self, cx: &mut Context<Self>) {
        let tz = tz_for(self.use_utc);
        let series = demo_series(self.timeframe, tz);
        self.chart
            .update(cx, |chart, cx| chart.set_series(series, cx));
    }

    fn toggle_clock_utc(&mut self, cx: &mut Context<Self>) {
        self.use_utc = !self.use_utc;
        let tz = tz_for(self.use_utc);
        self.chart.update(cx, |chart, cx| {
            chart.state.tz = tz;
            cx.notify();
        });
        // Day and longer buckets start at local midnight, so they change.
        if matches!(
            self.timeframe,
            Timeframe::D1 | Timeframe::W1 | Timeframe::Mo1
        ) {
            self.reload_series(cx);
        }
        let use_utc = self.use_utc;
        self.bottom_bar
            .update(cx, |bar, cx| bar.set_use_utc(use_utc, cx));
        self.on_settings_changed();
    }

    /// Hook for persisting `use_utc`, the symbol, the timeframe and the theme.
    /// TODO(Task 15): write them through `xv_store` settings.
    fn on_settings_changed(&self) {}

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.theme = AppTheme::for_mode(match self.theme.mode {
            xv_store::ThemeMode::Light => xv_store::ThemeMode::Dark,
            xv_store::ThemeMode::Dark => xv_store::ThemeMode::Light,
        });
        let t = self.theme.clone();
        self.chart
            .update(cx, |c, cx| c.set_colors(t.chart.clone(), cx));
        self.top_bar.update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.left_toolbar
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.style_bar
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.side_panel
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.bottom_bar
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        cx.notify();
        self.on_settings_changed();
    }

    fn clear_drawings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart.read(cx).state.drawings.all().is_empty() {
            return;
        }
        let message = format!("Delete all drawings on {}?", self.symbol.display);
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("This can be undone with Cmd-Z."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |this, cx| {
                    this.with_chart(cx, |s| s.clear_drawings());
                });
            }
        })
        .detach();
    }
}

pub fn tz_for(use_utc: bool) -> TzOffset {
    if use_utc {
        TzOffset::UTC
    } else {
        TzOffset::local_now()
    }
}

fn demo_series(tf: Timeframe, tz: TzOffset) -> Series {
    let base = demo::series(demo::candle_count());
    if tf == Timeframe::M1 {
        return base;
    }
    let candles = resample(base.candles(), tf, tz);
    Series::new(base.symbol.clone(), tf, candles, false)
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The chart owns keyboard focus: its key bindings (Delete, Esc, undo)
        // need it, and actions dispatched from the toolbars bubble from it to
        // the handlers below. No shell element tracks focus, so shell clicks
        // never take it; this only restores it if nothing is focused.
        if window.focused(cx).is_none() {
            window.focus(&self.chart.focus_handle(cx), cx);
        }
        let t = &self.theme;
        let chart_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.chart.clone())
                    .child(self.style_bar.clone()),
            )
            .child(
                self.bottom_bar.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(BOTTOM_BAR_HEIGHT))
                        .flex_none(),
                ),
            );
        div()
            .key_context("Workspace")
            .on_action(
                cx.listener(|this, a: &SelectTimeframe, _, cx| this.select_timeframe(a.0, cx)),
            )
            .on_action(cx.listener(|this, a: &SelectTool, _, cx| {
                let tool = a.0;
                this.with_chart(cx, |s| {
                    s.set_tool(tool);
                    Vec::new()
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleHideDrawings, _, cx| {
                this.with_chart(cx, |s| s.set_hidden(!s.drawings_hidden))
            }))
            .on_action(cx.listener(|this, _: &ToggleLockDrawings, _, cx| {
                this.with_chart(cx, |s| s.set_locked(!s.drawings_locked))
            }))
            .on_action(cx.listener(|this, _: &ToggleMagnet, _, cx| {
                this.with_chart(cx, |s| {
                    s.magnet = !s.magnet;
                    Vec::new()
                })
            }))
            .on_action(
                cx.listener(|this, _: &ClearDrawings, window, cx| this.clear_drawings(window, cx)),
            )
            .on_action(cx.listener(|this, a: &ShowSpan, _, cx| {
                let span = a.0;
                this.with_chart(cx, |s| {
                    s.show_span(span);
                    s.check_need_older()
                })
            }))
            .on_action(cx.listener(|_, _: &OpenSymbolSearch, _, _| {
                // TODO(Task 15): open the symbol search popover.
                eprintln!("xview: OpenSymbolSearch (arrives in Task 15)");
            }))
            .on_action(cx.listener(|_, _: &OpenCsv, _, _| {
                // TODO(Task 15): open the CSV import dialog.
                eprintln!("xview: OpenCsv (arrives in Task 15)");
            }))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| this.toggle_theme(cx)))
            .on_action(cx.listener(|this, _: &ToggleClockUtc, _, cx| this.toggle_clock_utc(cx)))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .child(
                self.top_bar.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(TOP_BAR_HEIGHT + REGION_BORDER))
                        .flex_none(),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        self.left_toolbar.clone().cached(
                            StyleRefinement::default()
                                .w(px(LEFT_TOOLBAR_WIDTH))
                                .h_full()
                                .flex_none(),
                        ),
                    )
                    .child(chart_area)
                    .child(
                        self.side_panel.clone().cached(
                            StyleRefinement::default()
                                .w(px(SIDE_PANEL_WIDTH + RAIL_WIDTH + 2. * REGION_BORDER))
                                .h_full()
                                .flex_none(),
                        ),
                    ),
            )
    }
}
