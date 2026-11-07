mod chart_view;
mod demo;
mod theme;

use gpui::{
    App, Bounds, Context, Entity, Focusable, Subscription, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use gpui_platform::application;

use chart_view::{ChartView, ChartViewEvent};
use theme::AppTheme;

/// Temporary root: only the demo chart (the app shell arrives in Task 14).
struct DemoRoot {
    chart: Entity<ChartView>,
    theme: AppTheme,
    _events: Subscription,
}

impl DemoRoot {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let theme = AppTheme::light();
        let series = demo::series(demo::candle_count());
        let chart = cx.new(|cx| {
            let mut view = ChartView::new(series, "Demo", theme.chart.clone(), window, cx);
            if let Some(spacing) = demo::bar_spacing() {
                view.state.vp.bar_spacing = spacing;
            }
            view
        });
        let events = cx.subscribe(&chart, |_, _, e: &ChartViewEvent, _| {
            eprintln!("xview demo: chart event {e:?}");
        });
        if demo::autopan() {
            demo::start_autopan(chart.clone(), cx);
        }
        window.focus(&chart.focus_handle(cx), cx);
        DemoRoot {
            chart,
            theme,
            _events: events,
        }
    }
}

impl Render for DemoRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(self.theme.bg)
            .text_color(self.theme.text)
            .child(self.chart.clone())
    }
}

fn main() {
    application().run(|cx: &mut App| {
        gpui_tokio::init(cx);
        chart_view::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("XView".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| DemoRoot::new(window, cx)),
        )
        .unwrap();
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
