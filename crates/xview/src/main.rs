mod assets;
mod bottom_bar;
mod chart_view;
mod color_picker;
mod context_menu;
#[cfg(debug_assertions)]
mod debug_script;
mod demo;
mod drawing_toolbar;
mod left_toolbar;
mod loader;
mod persistence;
mod side_panel;
mod symbol_search;
mod text_input;
mod theme;
mod top_bar;
mod widgets;
mod workspace;

use std::sync::Arc;

use gpui::{
    App, Bounds, Menu, MenuItem, WindowBounds, WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;
use xv_data::SourceRegistry;
use xv_source_binance::BinanceSource;
use xv_source_csv::CsvSource;
use xv_store::{data_dir, load_settings};

use workspace::{
    Boot, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, OpenCsv, Quit, ToggleTheme, Workspace, rect_of,
    rect_visible,
};

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("XView").items([MenuItem::action("Quit XView", Quit)]),
        Menu::new("File").items([MenuItem::action("Open CSV…", OpenCsv)]),
        Menu::new("View").items([MenuItem::action("Toggle Theme", ToggleTheme)]),
    ]);
}

fn main() {
    application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            gpui_tokio::init(cx);
            chart_view::bind_keys(cx);
            workspace::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            set_menus(cx);

            let dir = data_dir();
            let settings = load_settings(&dir);
            let csv = Arc::new(CsvSource::new());
            let mut registry = SourceRegistry::new();
            if demo::enabled() {
                registry.add(Arc::new(demo::DemoSource::new()));
            }
            // Binance first: the registry's default source.
            registry.add(Arc::new(BinanceSource::new()));
            registry.add(csv.clone());

            let displays: Vec<_> = cx.displays().iter().map(|d| rect_of(d.bounds())).collect();
            let bounds = settings
                .window
                .filter(|r| rect_visible(r, &displays))
                .map(|r| Bounds {
                    origin: point(px(r.x), px(r.y)),
                    size: size(px(r.w.max(MIN_WINDOW_WIDTH)), px(r.h.max(MIN_WINDOW_HEIGHT))),
                })
                .unwrap_or_else(|| Bounds::centered(None, size(px(1280.), px(800.)), cx));
            let boot = Boot {
                dir,
                settings,
                registry,
                csv,
            };
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("XView".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(boot, window, cx)),
            )
            .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
}
