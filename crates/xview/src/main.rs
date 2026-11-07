mod assets;
mod bottom_bar;
mod chart_view;
mod demo;
mod left_toolbar;
mod side_panel;
mod style_bar;
mod theme;
mod top_bar;
mod widgets;
mod workspace;

use gpui::{App, Bounds, WindowBounds, WindowOptions, prelude::*, px, size};
use gpui_platform::application;

use workspace::Workspace;

fn main() {
    application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            gpui_tokio::init(cx);
            chart_view::bind_keys(cx);
            workspace::bind_keys(cx);
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
                |window, cx| cx.new(|cx| Workspace::new(window, cx)),
            )
            .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
}
