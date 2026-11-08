//! Root view: lays out the top bar, tool column, chart, bottom bar, side
//! panel and rail, and handles the workspace actions. It also owns the data
//! loader, the symbol search modal, the error banner and persistence.
//!
//! Rendering cost: the toolbars and panels are separate entities embedded as
//! cached views, so a chart notify (every mouse move) re-renders only the
//! chart and this light layout shell; the drawing toolbar over the chart is
//! cached too and re-renders only when its snapshot changes. Each cached view
//! notifies itself only when what it shows changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Action, App, Context, Entity, FontWeight, Focusable, KeyBinding, PathPromptOptions,
    PromptLevel, SharedString, StyleRefinement, Subscription, Task, Window, actions,
    div, prelude::*, px,
};
use gpui_tokio::Tokio;
use xv_chart::{ChartEffect, MagnetMode};
use xv_core::{Series, SourceId, Symbol, SymbolKey, Timeframe, TzOffset};
use xv_data::{DataError, HistoryPage, LoadGuard, LoadToken, SourceRegistry};
use xv_drawings::{Drawing, DrawingId, Tool};
use xv_source_binance::BINANCE_SOURCE_ID;
use xv_source_csv::{CSV_SOURCE_ID, CsvError, CsvLoadReport, CsvSource};
use xv_store::{
    LoadedDrawings, LoadedTemplates, MagnetSetting, Settings, TemplatesFile, ThemeMode, WindowRect,
};

use crate::bottom_bar::BottomBar;
use crate::chart_view::{ChartView, ChartViewEvent, CopyDrawing, PasteDrawing};
use crate::demo;
use crate::drawing_settings::{DrawingSettings, SettingsEvent};
use crate::drawing_toolbar::{DrawingToolbar, ToolbarEvent};
use crate::left_toolbar::LeftToolbar;
use crate::loader::{BackfillOutcome, Loader};
use crate::persistence::Persistence;
use crate::symbol_search::{SearchEvent, SymbolSearch};
use crate::side_panel::SidePanel;
use crate::theme::AppTheme;
use crate::top_bar::TopBar;

pub const TOP_BAR_HEIGHT: f32 = 56.;
pub const LEFT_TOOLBAR_WIDTH: f32 = 52.;
pub const BOTTOM_BAR_HEIGHT: f32 = 34.;
pub const SIDE_PANEL_WIDTH: f32 = 300.;
pub const RAIL_WIDTH: f32 = 44.;
/// Thick panel-gray borders between the shell regions.
pub const REGION_BORDER: f32 = 4.;
pub const BANNER_HEIGHT: f32 = 36.;
/// Smallest window content size: leaves a plot of at least ~420 × 400 px
/// beside the fixed tool column, side panel and price axis, with a banner up
/// (see the `min_window_keeps_a_usable_plot` test).
pub const MIN_WINDOW_WIDTH: f32 = 900.;
pub const MIN_WINDOW_HEIGHT: f32 = 560.;
const MAX_BANNERS: usize = 5;
/// Window moves and resizes are saved this long after the last change.
const BOUNDS_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
pub const DEFAULT_SYMBOL_ID: &str = "BTCUSDT";
pub const DEFAULT_TIMEFRAME: Timeframe = Timeframe::H1;
pub const REGION_BLOCKED_TEXT: &str =
    "Binance isn't available from your network (HTTP 451). Open a CSV file instead: File → Open CSV…";

// The workspace actions (the brief's `WorkspaceAction` set). The data-carrying
// ones are `no_json`: they are dispatched from buttons, never from keymaps.
actions!(
    workspace,
    [
        ToggleHideDrawings,
        ToggleLockDrawings,
        ToggleMagnet,
        ToggleStayInDrawingMode,
        UnhideAllDrawings,
        ClearDrawings,
        OpenSymbolSearch,
        OpenCsv,
        ToggleTheme,
        ToggleClockUtc,
        Quit,
    ]
);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct SelectTimeframe(pub Timeframe);

#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct SelectTool(pub Tool);

/// Picks the magnet mode and turns the magnet on.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct SetMagnetMode(pub MagnetMode);

/// Every drawing tool whose last-used style is kept in `templates.json`.
pub const STYLED_TOOLS: [Tool; 6] = [
    Tool::TrendLine,
    Tool::ExtendedLine,
    Tool::Ray,
    Tool::HorizontalLine,
    Tool::VerticalLine,
    Tool::Rectangle,
];

/// The tool with `Tool::key() == key`, if it is a drawing tool.
pub fn tool_from_key(key: &str) -> Option<Tool> {
    STYLED_TOOLS.iter().copied().find(|t| t.key() == key)
}

/// `tool_styles` from a templates file's `last_used`, skipping unknown keys.
pub fn tool_styles_from(
    f: &TemplatesFile,
) -> std::collections::HashMap<Tool, xv_drawings::Style> {
    f.last_used
        .iter()
        .filter_map(|(k, st)| tool_from_key(k).map(|t| (t, st.clone())))
        .collect()
}

/// Writes `styles` into `f.last_used`, keeping keys of unknown tools.
pub fn store_tool_styles(
    f: &mut TemplatesFile,
    styles: &std::collections::HashMap<Tool, xv_drawings::Style>,
) {
    for t in STYLED_TOOLS {
        match styles.get(&t) {
            Some(st) => f.last_used.insert(t.key().to_string(), st.clone()),
            None => f.last_used.remove(t.key()),
        };
    }
}

pub fn magnet_mode_of(s: MagnetSetting) -> MagnetMode {
    match s {
        MagnetSetting::Weak => MagnetMode::Weak,
        MagnetSetting::Strong => MagnetMode::Strong,
    }
}

pub fn magnet_setting_of(m: MagnetMode) -> MagnetSetting {
    match m {
        MagnetMode::Weak => MagnetSetting::Weak,
        MagnetMode::Strong => MagnetSetting::Strong,
    }
}

/// Fits the last `span_ms` (or everything for `None`) into the chart.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = workspace, no_json)]
pub struct ShowSpan(pub Option<i64>);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-k", OpenSymbolSearch, None),
        KeyBinding::new("cmd-o", OpenCsv, None),
        KeyBinding::new("alt-cmd-t", ToggleTheme, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

/// What `main` hands the workspace: the data directory, loaded settings and
/// the registered sources.
pub struct Boot {
    pub dir: PathBuf,
    pub settings: Settings,
    pub registry: SourceRegistry,
    pub csv: Arc<CsvSource>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BannerKind {
    /// The current series failed to load; offers Retry.
    Load,
    /// Anything else (older pages, saves, CSV reports, startup notes).
    Info,
}

#[derive(Clone, Debug)]
struct Banner {
    text: SharedString,
    kind: BannerKind,
}

pub struct Workspace {
    chart: Entity<ChartView>,
    pub(crate) loader: Loader,
    pub(crate) persistence: Persistence,
    theme: AppTheme,
    /// The series on screen, or being opened. After a failed open this is
    /// put back to what the chart shows.
    symbol: Symbol,
    timeframe: Timeframe,
    /// What the load banner's Retry opens: the failed request.
    retry_target: Option<(Symbol, Timeframe)>,
    /// A symbol restored from settings whose metadata isn't resolved yet;
    /// its tick size is guessed from the first page until then.
    unresolved: Option<SymbolKey>,
    /// Clock and chart times in UTC instead of the local offset.
    use_utc: bool,
    /// Pending banners; the newest is shown, dismissing reveals the previous.
    banners: Vec<Banner>,
    /// Bumped by every symbol/timeframe switch and CSV open, so a CSV that
    /// finishes parsing after the user moved on is dropped.
    nav: LoadGuard,
    search: Option<(Entity<SymbolSearch>, Subscription)>,
    /// Symbol whose drawings the chart holds (that of the series on screen).
    drawings_key: Option<SymbolKey>,
    window_rect: Option<WindowRect>,
    /// Last settings save error shown, so bounds changes don't repeat it.
    settings_error: Option<String>,
    /// Drawing templates (last-used styles), saved to `templates.json`.
    templates: TemplatesFile,
    /// In-app clipboard (cmd-c / cmd-v), for the current symbol only.
    clipboard: Option<Drawing>,
    /// The open drawing settings dialog.
    settings: Option<(Entity<DrawingSettings>, Vec<Subscription>)>,
    bounds_save: Option<Task<()>>,
    resolve: Option<Task<()>>,
    top_bar: Entity<TopBar>,
    left_toolbar: Entity<LeftToolbar>,
    drawing_toolbar: Entity<DrawingToolbar>,
    side_panel: Entity<SidePanel>,
    bottom_bar: Entity<BottomBar>,
    _subscriptions: Vec<Subscription>,
}

/// A Binance symbol known only by its id (restored from settings); the
/// metadata is filled in by `resolve_symbol`.
pub fn placeholder_symbol(key: &SymbolKey) -> Symbol {
    Symbol {
        source: key.source.clone(),
        id: key.id.clone(),
        display: key.id.clone(),
        description: String::new(),
        base: String::new(),
        quote: String::new(),
        tick_size: 0.01,
    }
}

pub fn default_symbol() -> Symbol {
    placeholder_symbol(&SymbolKey {
        source: SourceId(BINANCE_SOURCE_ID.into()),
        id: DEFAULT_SYMBOL_ID.into(),
    })
}

/// `tf` if supported, else the next coarser supported one, else the
/// coarsest. Empty `supported` keeps `tf`.
pub fn clamp_timeframe(tf: Timeframe, supported: &[Timeframe]) -> Timeframe {
    if supported.is_empty() || supported.contains(&tf) {
        return tf;
    }
    supported
        .iter()
        .copied()
        .filter(|s| *s > tf)
        .min()
        .or_else(|| supported.iter().copied().max())
        .unwrap_or(tf)
}

/// The symbol to start on.
#[derive(Debug, PartialEq)]
pub struct StartPoint {
    pub symbol: Symbol,
    pub timeframe: Timeframe,
    /// The symbol is a placeholder whose metadata should be looked up.
    pub resolve: bool,
    pub banner: Option<String>,
}

/// Restores the last symbol and timeframe from `settings`, reloading a CSV
/// file. A missing CSV file or unknown source falls back to the default.
pub fn start_point(settings: &Settings, registry: &SourceRegistry, csv: &CsvSource) -> StartPoint {
    let fallback = |banner: Option<String>| {
        let symbol = match registry.default_source() {
            Some(src) if src.id().0 == demo::DEMO_SOURCE_ID => demo::symbol(),
            _ => default_symbol(),
        };
        let resolve = symbol.source.0 == BINANCE_SOURCE_ID;
        let timeframe = if banner.is_some() {
            DEFAULT_TIMEFRAME
        } else {
            settings.last_timeframe
        };
        StartPoint {
            symbol,
            timeframe,
            resolve,
            banner,
        }
    };
    let Some(key) = &settings.last_symbol else {
        return fallback(None);
    };
    if registry.default_source().is_some_and(|s| s.id().0 == demo::DEMO_SOURCE_ID)
        && key.source.0 != demo::DEMO_SOURCE_ID
    {
        return fallback(None);
    }
    if key.source.0 == CSV_SOURCE_ID {
        return match csv.load_file_report(Path::new(&key.id)) {
            Ok(report) => {
                let supported = registry
                    .get(&key.source)
                    .map(|s| s.supported_timeframes(&report.symbol))
                    .unwrap_or_default();
                StartPoint {
                    timeframe: clamp_timeframe(settings.last_timeframe, &supported),
                    banner: skipped_banner(&report),
                    symbol: report.symbol,
                    resolve: false,
                }
            }
            Err(e) => fallback(Some(format!(
                "The last CSV file {} could not be opened ({}); showing {DEFAULT_SYMBOL_ID} instead.",
                key.id,
                csv_error_text(&e)
            ))),
        };
    }
    match registry.get(&key.source) {
        Some(_) if key.source.0 == demo::DEMO_SOURCE_ID => StartPoint {
            symbol: demo::symbol(),
            timeframe: settings.last_timeframe,
            resolve: false,
            banner: None,
        },
        Some(_) => StartPoint {
            symbol: placeholder_symbol(key),
            timeframe: settings.last_timeframe,
            resolve: key.source.0 == BINANCE_SOURCE_ID,
            banner: None,
        },
        None => fallback(Some(format!(
            "The last symbol's data source \"{}\" is not available; showing {DEFAULT_SYMBOL_ID} instead.",
            key.source.0
        ))),
    }
}

/// A short name for a symbol key: the file name for CSV files, else the id.
fn symbol_label(key: &SymbolKey) -> String {
    if key.source.0 == CSV_SOURCE_ID
        && let Some(name) = Path::new(&key.id).file_name()
    {
        return name.to_string_lossy().into_owned();
    }
    key.id.clone()
}

fn csv_error_text(e: &CsvError) -> String {
    match e {
        CsvError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => "file not found".into(),
        other => other.to_string(),
    }
}

/// "Loaded N rows, skipped M (lines 6, 8, …)" when lines were skipped.
pub fn skipped_banner(r: &CsvLoadReport) -> Option<String> {
    const SHOWN: usize = 5;
    if r.skipped.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = r.skipped.iter().take(SHOWN).map(|(l, _)| l.to_string()).collect();
    if r.skipped.len() > SHOWN {
        lines.push("…".into());
    }
    Some(format!(
        "Loaded {} rows, skipped {} (lines {})",
        r.rows,
        r.skipped.len(),
        lines.join(", ")
    ))
}

/// Text for a failed current-series load.
pub fn load_error_text(symbol: &Symbol, tf: Timeframe, e: &DataError) -> String {
    match e {
        DataError::RegionBlocked { .. } => REGION_BLOCKED_TEXT.into(),
        e => format!("Couldn't load {} {}: {e}", symbol.display, tf.label()),
    }
}

pub fn corrupt_templates_text(path: &Path, moved: bool) -> String {
    if moved {
        format!(
            "Drawing templates were unreadable; the file was kept at {}",
            path.display()
        )
    } else {
        format!(
            "Drawing templates are unreadable and the file could not be moved aside ({}). \
             Last-used styles won't be saved until that file is fixed or removed.",
            path.display()
        )
    }
}

pub fn corrupt_drawings_text(symbol: &Symbol, path: &Path, moved: bool) -> String {
    if moved {
        format!(
            "Drawings for {} were unreadable; the file was kept at {}",
            symbol.display,
            path.display()
        )
    } else {
        format!(
            "Drawings for {} are unreadable and the file could not be moved aside ({}). \
             New drawings for this symbol won't be saved until that file is fixed or removed.",
            symbol.display,
            path.display()
        )
    }
}

/// Whether a saved window rect is on a display, with enough of it visible
/// (100 × 50 px) to grab the title bar.
pub fn rect_visible(r: &WindowRect, displays: &[WindowRect]) -> bool {
    const MIN_W: f32 = 100.;
    const MIN_H: f32 = 50.;
    if !(r.w.is_finite() && r.h.is_finite() && r.x.is_finite() && r.y.is_finite()) {
        return false;
    }
    if r.w < 400. || r.h < 300. {
        return false;
    }
    displays.iter().any(|d| {
        let w = (r.x + r.w).min(d.x + d.w) - r.x.max(d.x);
        let h = (r.y + r.h).min(d.y + d.h) - r.y.max(d.y);
        w >= MIN_W && h >= MIN_H
    })
}

pub fn rect_of(b: gpui::Bounds<gpui::Pixels>) -> WindowRect {
    WindowRect {
        x: f32::from(b.origin.x),
        y: f32::from(b.origin.y),
        w: f32::from(b.size.width),
        h: f32::from(b.size.height),
    }
}

/// The rect to restore the window with: the frame's top-left (what macOS
/// positions by) and the content size (what `WindowOptions` sizes by).
/// Maximized and fullscreen windows save their restore bounds.
fn window_rect(window: &Window) -> WindowRect {
    match window.window_bounds() {
        gpui::WindowBounds::Windowed(frame) => {
            let content = window.viewport_size();
            WindowRect {
                w: f32::from(content.width),
                h: f32::from(content.height),
                ..rect_of(frame)
            }
        }
        other => rect_of(other.get_bounds()),
    }
}

impl Workspace {
    pub fn new(boot: Boot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let Boot {
            dir,
            settings,
            registry,
            csv,
        } = boot;
        let theme = AppTheme::for_mode(settings.theme);
        let use_utc = settings.use_utc;
        let start = start_point(&settings, &registry, &csv);
        let tz = tz_for(use_utc);
        let chart = cx.new(|cx| {
            let empty = Series::new(start.symbol.clone(), start.timeframe, Vec::new(), false);
            let mut view = ChartView::new(empty, "", theme.chart.clone(), window, cx);
            view.state.tz = tz;
            if let Some(spacing) = demo::bar_spacing() {
                view.state.vp.bar_spacing = spacing;
            }
            view.state.magnet = settings.magnet_on;
            view.state.magnet_mode = magnet_mode_of(settings.magnet_mode);
            view.state.stay_in_drawing_mode = settings.stay_in_drawing_mode;
            view
        });
        let persistence = Persistence::new(dir);
        let mut startup_banners = Vec::new();
        let templates = match persistence.load_templates() {
            LoadedTemplates::Ok(f) => f,
            LoadedTemplates::Missing => TemplatesFile::default(),
            LoadedTemplates::Corrupt { path, moved } => {
                startup_banners.push(corrupt_templates_text(&path, moved));
                TemplatesFile::default()
            }
        };
        let styles = tool_styles_from(&templates);
        chart.update(cx, |c, _| c.state.tool_styles = styles);
        let events = cx.subscribe_in(&chart, window, |this, _, e: &ChartViewEvent, window, cx| {
            match e {
                ChartViewEvent::NeedOlder(before) => this.loader.load_older(*before, cx),
                ChartViewEvent::DrawingsChanged => this.schedule_drawings_save(cx),
                ChartViewEvent::TypeToSearch(letter) => {
                    this.open_search(letter.clone(), window, cx)
                }
                ChartViewEvent::ToolStylesChanged => this.schedule_templates_save(cx),
                ChartViewEvent::ContextMenu { position, .. } => {
                    let position = *position;
                    this.drawing_toolbar.update(cx, |tb, cx| {
                        tb.open_context_menu(position, window, cx)
                    });
                }
                ChartViewEvent::OpenSettings(id) => this.open_drawing_settings(*id, window, cx),
                ChartViewEvent::ToolFinished
                | ChartViewEvent::SelectionChanged
                | ChartViewEvent::DismissPopover => {}
            }
        });
        if demo::autopan() {
            demo::start_autopan(chart.clone(), cx);
        }
        window.focus(&chart.focus_handle(cx), cx);

        let quit = cx.on_app_quit(|this, cx| {
            this.flush_all(cx);
            async {}
        });
        let bounds = cx.observe_window_bounds(window, |this, window, cx| {
            this.window_rect = Some(window_rect(window));
            this.bounds_save = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(BOUNDS_SAVE_DEBOUNCE).await;
                this.update(cx, |this, cx| this.save_settings(cx)).ok();
            }));
        });
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| this.flush_all(cx)).ok();
            true
        });

        let top_bar = cx.new(|_| TopBar::new(theme.clone(), &start.symbol, start.timeframe));
        let left_toolbar = cx.new(|cx| LeftToolbar::new(chart.clone(), theme.clone(), cx));
        let drawing_toolbar = cx.new(|cx| {
            DrawingToolbar::new(chart.clone(), theme.clone(), &templates, window, cx)
        });
        let toolbar_events =
            cx.subscribe(&drawing_toolbar, |this, _, e: &ToolbarEvent, cx| {
                this.on_toolbar_event(e, cx)
            });
        let side_panel = cx.new(|cx| SidePanel::new(chart.clone(), theme.clone(), cx));
        let bottom_bar = cx.new(|cx| BottomBar::new(chart.clone(), theme.clone(), use_utc, cx));

        let mut this = Workspace {
            loader: Loader::new(registry, csv, chart.clone()),
            persistence,
            chart,
            theme,
            symbol: start.symbol.clone(),
            timeframe: start.timeframe,
            retry_target: None,
            unresolved: start.resolve.then(|| start.symbol.key()),
            use_utc,
            banners: Vec::new(),
            nav: LoadGuard::new(),
            search: None,
            drawings_key: None,
            window_rect: Some(window_rect(window)),
            settings_error: None,
            templates,
            clipboard: None,
            settings: None,
            bounds_save: None,
            resolve: None,
            top_bar,
            left_toolbar,
            drawing_toolbar,
            side_panel,
            bottom_bar,
            _subscriptions: vec![events, quit, bounds, toolbar_events],
        };
        // Before opening, so a corrupt-drawings banner lands on top.
        for text in startup_banners.into_iter().chain(start.banner) {
            this.show_banner(text, BannerKind::Info, cx);
        }
        this.open_symbol(start.symbol.clone(), start.timeframe, cx);
        if start.resolve {
            this.resolve_symbol(start.symbol.key(), cx);
        }
        #[cfg(debug_assertions)]
        if let Some(script) = crate::debug_script::script() {
            crate::debug_script::start(script, window, cx);
        }
        this
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

    // ---- loading ----

    /// Switches the chart to `symbol` at `tf` through the loader.
    fn open_symbol(&mut self, symbol: Symbol, tf: Timeframe, cx: &mut Context<Self>) {
        self.nav.next();
        self.symbol = symbol.clone();
        self.timeframe = tf;
        self.retry_target = None;
        let supported = self.loader.supported_timeframes(&symbol);
        self.top_bar.update(cx, |bar, cx| {
            bar.set_symbol_timeframe(&symbol, tf, cx);
            bar.set_supported(supported, cx);
        });
        let before = self.banners.len();
        self.banners.retain(|b| b.kind != BannerKind::Load);
        if self.banners.len() != before {
            cx.notify();
        }
        if let Err(e) = self.loader.open(symbol, tf, cx) {
            self.open_failed(&e, cx);
        }
        // A cache hit is already on screen.
        self.sync_drawings(cx);
        self.save_settings(cx);
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        let (symbol, tf) = self
            .retry_target
            .take()
            .unwrap_or_else(|| (self.symbol.clone(), self.timeframe));
        self.open_symbol(symbol, tf, cx);
    }

    pub(crate) fn on_history(
        &mut self,
        token: LoadToken,
        result: Result<HistoryPage, DataError>,
        cx: &mut Context<Self>,
    ) {
        if !self.loader.is_current(&token) {
            return;
        }
        match result {
            Ok(page) => {
                let empty = page.candles.is_empty();
                let tick = (self.unresolved.as_ref() == Some(&self.symbol.key()) && !empty)
                    .then(|| xv_core::infer_tick_size(&page.candles));
                if self.loader.land(token, page, cx) {
                    self.sync_drawings(cx);
                    if let Some(tick) = tick {
                        // Until (or unless) the metadata lookup answers.
                        let symbol = Symbol {
                            tick_size: tick,
                            ..self.symbol.clone()
                        };
                        self.apply_symbol_metadata(symbol, cx);
                    }
                    if empty {
                        let text = format!(
                            "No data for {} at {}",
                            self.symbol.display,
                            self.timeframe.label()
                        );
                        self.show_banner(text, BannerKind::Info, cx);
                    }
                }
            }
            Err(e) => {
                self.loader.open_failed(token);
                self.open_failed(&e, cx);
            }
        }
    }

    /// The open of `self.symbol`/`self.timeframe` failed: shows the error
    /// (Retry opens that request again) and puts the top bar back to the
    /// series the chart still shows.
    fn open_failed(&mut self, e: &DataError, cx: &mut Context<Self>) {
        self.show_load_error(e, cx);
        self.retry_target = Some((self.symbol.clone(), self.timeframe));
        let (shown, tf) = {
            let s = &self.chart.read(cx).state.series;
            (s.symbol.clone(), s.timeframe)
        };
        if shown.key() != self.symbol.key() || tf != self.timeframe {
            self.symbol = shown.clone();
            self.timeframe = tf;
            let supported = self.loader.supported_timeframes(&shown);
            self.top_bar.update(cx, |bar, cx| {
                bar.set_symbol_timeframe(&shown, tf, cx);
                bar.set_supported(supported, cx);
            });
            self.save_settings(cx);
        }
    }

    pub(crate) fn on_older(
        &mut self,
        token: LoadToken,
        result: Result<HistoryPage, DataError>,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = self.loader.finish_older(token, result, cx) {
            let text = match e {
                DataError::RegionBlocked { .. } => REGION_BLOCKED_TEXT.to_string(),
                e => format!("Couldn't load older history: {e}"),
            };
            self.show_banner(text, BannerKind::Info, cx);
        }
    }

    pub(crate) fn on_backfill(
        &mut self,
        token: LoadToken,
        result: Result<HistoryPage, DataError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(page) => match self.loader.on_backfill_page(token, page, cx) {
                BackfillOutcome::Handled => {}
                BackfillOutcome::Reopen(symbol, tf) => {
                    // Too many bars missed to page through: load afresh.
                    self.open_symbol(symbol, tf, cx);
                }
            },
            Err(e) => {
                if self.loader.backfill_failed(token) {
                    let text = format!("Couldn't fill the gap after reconnecting: {e}");
                    self.show_banner(text, BannerKind::Info, cx);
                }
            }
        }
    }

    fn show_load_error(&mut self, e: &DataError, cx: &mut Context<Self>) {
        let text = load_error_text(&self.symbol, self.timeframe, e);
        let kind = match e {
            DataError::RegionBlocked { .. } => BannerKind::Info,
            _ => BannerKind::Load,
        };
        self.show_banner(text, kind, cx);
    }

    /// Looks up the full metadata (tick size, description) of a symbol
    /// restored from settings by its id.
    fn resolve_symbol(&mut self, key: SymbolKey, cx: &mut Context<Self>) {
        let Some(src) = self.loader.registry().get(&key.source) else {
            return;
        };
        let task = Tokio::spawn(cx, src.search_symbols(&key.id));
        self.resolve = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(found)) = task.await else { return };
            let Some(symbol) = found.into_iter().find(|s| s.key() == key) else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.unresolved.as_ref() == Some(&key) {
                    this.unresolved = None;
                }
                this.apply_symbol_metadata(symbol, cx);
            })
            .ok();
        }));
    }

    /// Replaces the metadata of the current symbol (same key).
    fn apply_symbol_metadata(&mut self, symbol: Symbol, cx: &mut Context<Self>) {
        if self.loader.update_symbol(&symbol, cx) && symbol.key() == self.symbol.key() {
            self.symbol = symbol.clone();
            let tf = self.timeframe;
            self.top_bar
                .update(cx, |bar, cx| bar.set_symbol_timeframe(&symbol, tf, cx));
        }
    }

    fn select_timeframe(&mut self, tf: Timeframe, cx: &mut Context<Self>) {
        if tf == self.timeframe {
            return;
        }
        let supported = self.loader.supported_timeframes(&self.symbol);
        if !supported.is_empty() && !supported.contains(&tf) {
            return;
        }
        self.open_symbol(self.symbol.clone(), tf, cx);
    }

    fn pick_symbol(&mut self, symbol: Symbol, cx: &mut Context<Self>) {
        let supported = self.loader.supported_timeframes(&symbol);
        let tf = clamp_timeframe(self.timeframe, &supported);
        self.open_symbol(symbol, tf, cx);
    }

    fn open_csv(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let csv = self.loader.csv().clone();
        let token = self.nav.next();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let p = path.clone();
            let result = cx
                .background_spawn(async move { csv.load_file_report(&p) })
                .await;
            this.update(cx, |this, cx| this.on_csv_loaded(token, &path, result, cx))
                .ok();
        })
        .detach();
    }

    fn on_csv_loaded(
        &mut self,
        token: LoadToken,
        path: &Path,
        result: Result<CsvLoadReport, CsvError>,
        cx: &mut Context<Self>,
    ) {
        if !self.nav.is_current(&token) {
            // The user picked another symbol or file while this one parsed.
            return;
        }
        match result {
            Ok(report) => {
                // A re-read file may have changed.
                self.loader.invalidate(&report.symbol);
                let symbol = report.symbol.clone();
                self.pick_symbol(symbol, cx);
                if let Some(text) = skipped_banner(&report) {
                    self.show_banner(text, BannerKind::Info, cx);
                }
            }
            Err(e) => {
                let text = format!("Couldn't open {}: {}", path.display(), csv_error_text(&e));
                self.show_banner(text, BannerKind::Info, cx);
            }
        }
    }

    // ---- symbol search ----

    fn open_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((search, _)) = &self.search {
            window.focus(&search.focus_handle(cx), cx);
            return;
        }
        let sources = self.loader.registry().all();
        let theme = self.theme.clone();
        let search = cx.new(|cx| SymbolSearch::new(sources, theme, query, cx));
        let sub = cx.subscribe_in(&search, window, |this, _, e: &SearchEvent, window, cx| {
            match e {
                SearchEvent::Picked(symbol) => {
                    let symbol = symbol.clone();
                    this.close_search(window, cx);
                    this.pick_symbol(symbol, cx);
                }
                SearchEvent::Dismissed => this.close_search(window, cx),
            }
        });
        window.focus(&search.focus_handle(cx), cx);
        self.search = Some((search, sub));
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_some() {
            window.focus(&self.chart.focus_handle(cx), cx);
            cx.notify();
        }
    }

    // ---- banner ----

    fn show_banner(&mut self, text: impl Into<SharedString>, kind: BannerKind, cx: &mut Context<Self>) {
        let text = text.into();
        eprintln!("xview: {text}");
        self.banners.retain(|b| b.text != text);
        self.banners.push(Banner { text, kind });
        if self.banners.len() > MAX_BANNERS {
            self.banners.remove(0);
        }
        cx.notify();
    }

    fn dismiss_banner(&mut self, cx: &mut Context<Self>) {
        if self.banners.pop().is_some() {
            cx.notify();
        }
    }

    // ---- persistence ----

    /// Makes the chart hold the drawings of the symbol it shows, flushing
    /// the previous symbol's edits first.
    fn sync_drawings(&mut self, cx: &mut Context<Self>) {
        let symbol = self.chart.read(cx).state.series.symbol.clone();
        let key = symbol.key();
        if self.drawings_key.as_ref() == Some(&key) {
            return;
        }
        // An open settings dialog cancels, and an open color picker commits
        // its session, into the old symbol's set while that set is still
        // loaded; the result is queued under the old key before the flush
        // (the DrawingsChanged events would only arrive after the key has
        // moved on).
        let closed = self.close_drawing_settings(cx);
        if self.drawing_toolbar.update(cx, |tb, cx| tb.close_popover(cx)) || closed {
            self.schedule_drawings_save(cx);
        }
        self.flush_drawings(cx);
        let ds = match self.persistence.load_drawings(&key) {
            LoadedDrawings::Ok(ds) => ds,
            LoadedDrawings::Missing => Vec::new(),
            LoadedDrawings::Corrupt { path, moved } => {
                let text = corrupt_drawings_text(&symbol, &path, moved);
                self.show_banner(text, BannerKind::Info, cx);
                Vec::new()
            }
        };
        self.chart.update(cx, |c, cx| {
            c.state.set_drawings(ds);
            cx.notify();
        });
        self.drawings_key = Some(key);
        // Copy and paste work within one symbol.
        self.clipboard = None;
        self.drawing_toolbar
            .update(cx, |tb, cx| tb.set_has_clipboard(false, cx));
    }

    fn schedule_drawings_save(&mut self, cx: &mut Context<Self>) {
        // An open settings dialog's preview is not committed; closing the
        // dialog queues the final state.
        if self.settings.is_some() {
            return;
        }
        let Some(key) = self.drawings_key.clone() else {
            return;
        };
        let ds = self.chart.read(cx).state.drawings.all().to_vec();
        self.persistence.schedule_drawings_save(key, ds, cx);
    }

    pub(crate) fn flush_drawings(&mut self, cx: &mut Context<Self>) {
        if let Err(errors) = self.persistence.flush() {
            self.report_save_errors(errors, cx);
        }
    }

    /// Shows failed drawings saves in the banner.
    pub(crate) fn report_save_errors(
        &mut self,
        errors: Vec<crate::persistence::SaveError>,
        cx: &mut Context<Self>,
    ) {
        if !errors.is_empty() {
            let parts: Vec<String> = errors
                .iter()
                .map(|e| {
                    let path = xv_store::drawings_path(self.persistence.dir(), &e.key);
                    let name = symbol_label(&e.key);
                    if e.error.kind() == std::io::ErrorKind::InvalidData {
                        format!(
                            "{name}: the existing file {} is unreadable and was kept; fix or remove it to save again",
                            path.display()
                        )
                    } else {
                        format!("{name}: {}", e.error)
                    }
                })
                .collect();
            let text = format!("Couldn't save drawings for {}", parts.join("; "));
            self.show_banner(text, BannerKind::Info, cx);
        }
    }

    fn schedule_templates_save(&mut self, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        store_tool_styles(&mut self.templates, &self.chart.read(cx).state.tool_styles);
        self.persistence
            .schedule_templates_save(self.templates.clone(), cx);
    }

    /// Shows a failed templates save in the banner.
    pub(crate) fn report_templates_error(&mut self, e: std::io::Error, cx: &mut Context<Self>) {
        let text = if e.kind() == std::io::ErrorKind::InvalidData {
            format!(
                "Couldn't save drawing templates: {} is unreadable and was kept; fix or remove it to save again",
                xv_store::templates_path(self.persistence.dir()).display()
            )
        } else {
            format!("Couldn't save drawing templates: {e}")
        };
        self.show_banner(text, BannerKind::Info, cx);
    }

    fn copy_drawing(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.chart.read(cx).state.selected_drawing().cloned() {
            self.clipboard = Some(d);
            self.drawing_toolbar
                .update(cx, |tb, cx| tb.set_has_clipboard(true, cx));
        }
    }

    fn on_toolbar_event(&mut self, e: &ToolbarEvent, cx: &mut Context<Self>) {
        match e {
            ToolbarEvent::Copy => return self.copy_drawing(cx),
            ToolbarEvent::Paste => return self.paste_drawing(cx),
            ToolbarEvent::SaveTemplate { tool, name, style } => {
                let list = self.templates.named.entry(tool.key().to_string()).or_default();
                let t = xv_store::NamedTemplate {
                    name: name.clone(),
                    style: style.clone(),
                };
                match list.iter_mut().find(|t| t.name == *name) {
                    Some(old) => *old = t,
                    None => list.push(t),
                }
            }
            ToolbarEvent::DeleteTemplate { tool, name } => {
                if let Some(list) = self.templates.named.get_mut(tool.key()) {
                    list.retain(|t| t.name != *name);
                    if list.is_empty() {
                        self.templates.named.remove(tool.key());
                    }
                }
            }
            ToolbarEvent::CustomColor(c) => xv_store::push_custom_color(&mut self.templates, *c),
        }
        let f = self.templates.clone();
        self.drawing_toolbar
            .update(cx, |tb, cx| tb.set_templates(&f, cx));
        if let Some((d, _)) = &self.settings {
            d.update(cx, |d, cx| d.set_templates(&f, cx));
        }
        self.schedule_templates_save(cx);
    }

    // ---- drawing settings dialog ----

    /// Opens the settings dialog for drawing `id` (it must be the selected
    /// one). Every open path ends here: double-click, the toolbar's Settings
    /// button, More → Settings… and the context menu's Settings….
    fn open_drawing_settings(&mut self, id: DrawingId, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        self.drawing_toolbar.update(cx, |tb, cx| tb.close_popover(cx));
        let original = self.chart.update(cx, |c, _| {
            (c.state.selected == Some(id))
                .then(|| c.state.begin_edit())
                .flatten()
        });
        let Some(original) = original else { return };
        let (chart, theme) = (self.chart.clone(), self.theme.clone());
        let templates = self.templates.clone();
        let dialog = cx.new(|cx| DrawingSettings::new(chart, original, theme, &templates, window, cx));
        let closed = cx.subscribe_in(&dialog, window, |this, _, _: &SettingsEvent, window, cx| {
            this.drop_drawing_settings(cx);
            window.focus(&this.chart.focus_handle(cx), cx);
        });
        let templates = cx.subscribe(&dialog, |this, _, e: &ToolbarEvent, cx| this.on_toolbar_event(e, cx));
        window.focus(&dialog.focus_handle(cx), cx);
        self.chart.update(cx, |c, _| c.modal_open = true);
        self.settings = Some((dialog, vec![closed, templates]));
        cx.notify();
    }

    fn drop_drawing_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.chart.update(cx, |c, _| c.modal_open = false);
            // Saves were held while the dialog was open: queue the
            // committed (Ok) or restored (Cancel) state now.
            self.schedule_drawings_save(cx);
            self.schedule_templates_save(cx);
            cx.notify();
        }
    }

    /// Cancels and closes an open settings dialog. Returns whether one was
    /// open. Focus goes back to the chart on the next render.
    fn close_drawing_settings(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((d, _)) = &self.settings else {
            return false;
        };
        d.update(cx, |d, cx| d.finish(false, cx));
        self.drop_drawing_settings(cx);
        true
    }

    fn paste_drawing(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.clipboard.clone() {
            self.with_chart(cx, |s| s.paste(&d));
        }
    }

    fn set_magnet(&mut self, on: bool, mode: Option<MagnetMode>, cx: &mut Context<Self>) {
        self.with_chart(cx, |s| {
            s.magnet = on;
            if let Some(m) = mode {
                s.magnet_mode = m;
            }
            Vec::new()
        });
        self.save_settings(cx);
    }

    fn set_stay_in_drawing_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        self.with_chart(cx, |s| {
            s.stay_in_drawing_mode = on;
            Vec::new()
        });
        self.save_settings(cx);
    }

    fn current_settings(&self, cx: &App) -> Settings {
        let s = &self.chart.read(cx).state;
        Settings {
            version: 1,
            last_symbol: Some(self.symbol.key()),
            last_timeframe: self.timeframe,
            window: self.window_rect,
            theme: self.theme.mode,
            use_utc: self.use_utc,
            magnet_on: s.magnet,
            magnet_mode: magnet_setting_of(s.magnet_mode),
            stay_in_drawing_mode: s.stay_in_drawing_mode,
        }
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        match self.persistence.save_settings(&self.current_settings(cx)) {
            Ok(()) => self.settings_error = None,
            Err(e) => {
                let text = if e.kind() == std::io::ErrorKind::InvalidData {
                    format!(
                        "Couldn't save settings: {} is unreadable and was kept; fix or remove it",
                        self.persistence.dir().join("settings.json").display()
                    )
                } else {
                    format!("Couldn't save settings: {e}")
                };
                if self.settings_error.as_ref() != Some(&text) {
                    self.settings_error = Some(text.clone());
                    self.show_banner(text, BannerKind::Info, cx);
                }
            }
        }
    }

    /// Writes everything pending: on quit and window close.
    fn flush_all(&mut self, cx: &mut Context<Self>) {
        self.bounds_save = None;
        // An open settings dialog's preview is not a committed edit: put
        // the original back (closing queues the restored state).
        self.close_drawing_settings(cx);
        self.flush_drawings(cx);
        if let Err(e) = self.persistence.flush_templates() {
            self.report_templates_error(e, cx);
        }
        self.save_settings(cx);
    }

    fn toggle_clock_utc(&mut self, cx: &mut Context<Self>) {
        self.use_utc = !self.use_utc;
        let tz = tz_for(self.use_utc);
        // Sources bucket days in UTC, so only the labels change.
        self.chart.update(cx, |chart, cx| {
            chart.state.tz = tz;
            cx.notify();
        });
        let use_utc = self.use_utc;
        self.bottom_bar
            .update(cx, |bar, cx| bar.set_use_utc(use_utc, cx));
        self.save_settings(cx);
    }

    fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.theme = AppTheme::for_mode(match self.theme.mode {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        });
        let t = self.theme.clone();
        self.chart
            .update(cx, |c, cx| c.set_colors(t.chart.clone(), cx));
        self.top_bar.update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.left_toolbar
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.drawing_toolbar
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.side_panel
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        self.bottom_bar
            .update(cx, |v, cx| v.set_theme(t.clone(), cx));
        if let Some((search, _)) = &self.search {
            search.update(cx, |v, cx| v.set_theme(t.clone(), cx));
        }
        cx.notify();
        self.save_settings(cx);
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

    fn banner_strip(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let b = self.banners.last()?;
        let more = self.banners.len() - 1;
        let t = &self.theme;
        let bg = crate::theme::u32_to_hsla(match t.mode {
            ThemeMode::Light => 0xFFF1D6FF,
            ThemeMode::Dark => 0x3B3020FF,
        });
        let button = |id: &'static str, label: &'static str| {
            let hover = t.active_bg;
            div()
                .id(id)
                .flex_none()
                .px(px(10.))
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .hover(move |s| s.bg(hover))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .flex_none()
                .w_full()
                .h(px(BANNER_HEIGHT))
                .pl(px(12.))
                .pr(px(6.))
                .bg(bg)
                .text_size(px(13.))
                .child(
                    div()
                        .id("banner-text")
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .tooltip(crate::widgets::tooltip(b.text.clone(), t))
                        .child(b.text.clone()),
                )
                .when(more > 0, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_color(t.muted)
                            .child(format!("+{more} more")),
                    )
                })
                .when(b.kind == BannerKind::Load, |d| {
                    d.child(
                        button("banner-retry", "Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx))),
                    )
                })
                .child(
                    button("banner-dismiss", "✕")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_banner(cx))),
                ),
        )
    }
}

/// Debug script hooks (`XVIEW_SCRIPT`); not compiled into release builds.
#[cfg(debug_assertions)]
impl Workspace {
    // ---- debug script hooks (XVIEW_SCRIPT) ----

    pub(crate) fn debug_select_timeframe(&mut self, tf: Timeframe, cx: &mut Context<Self>) {
        self.select_timeframe(tf, cx);
    }

    pub(crate) fn debug_open_search(&mut self, q: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_search(q, window, cx);
    }

    pub(crate) fn debug_open_csv(&mut self, path: &Path, cx: &mut Context<Self>) {
        let token = self.nav.next();
        let result = self.loader.csv().load_file_report(path);
        self.on_csv_loaded(token, path, result, cx);
    }

    pub(crate) fn debug_pick_search(&mut self, cx: &mut Context<Self>) {
        if let Some((search, _)) = &self.search {
            search.update(cx, |s, cx| s.pick_selected(cx));
        }
    }

    pub(crate) fn debug_set_magnet(&mut self, on: bool, mode: Option<MagnetMode>, cx: &mut Context<Self>) {
        self.set_magnet(on, mode, cx);
    }

    pub(crate) fn debug_set_stay(&mut self, on: bool, cx: &mut Context<Self>) {
        self.set_stay_in_drawing_mode(on, cx);
    }

    pub(crate) fn debug_open_flyout(&mut self, name: &str, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.left_toolbar
            .update(cx, |tb, cx| tb.debug_open_flyout(&name, cx));
    }

    pub(crate) fn debug_open_popover(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.drawing_toolbar
            .update(cx, |tb, cx| tb.debug_open_popover(&name, window, cx));
    }

    pub(crate) fn debug_pick_color(&mut self, color: u32, cx: &mut Context<Self>) {
        self.drawing_toolbar
            .update(cx, |tb, cx| tb.debug_pick_color(color, cx));
    }

    pub(crate) fn debug_save_template(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.drawing_toolbar
            .update(cx, |tb, cx| tb.debug_save_template(&name, window, cx));
    }

    pub(crate) fn debug_open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.chart.read(cx).state.selected {
            self.open_drawing_settings(id, window, cx);
        }
    }

    pub(crate) fn debug_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut DrawingSettings, &mut Window, &mut Context<DrawingSettings>),
    ) {
        match &self.settings {
            Some((d, _)) => d.update(cx, |d, cx| f(d, window, cx)),
            None => eprintln!("xview-script: no settings dialog open"),
        }
    }

    pub(crate) fn debug_chart(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut ChartView, &mut Context<ChartView>),
    ) {
        self.chart.update(cx, f);
    }
}

pub fn tz_for(use_utc: bool) -> TzOffset {
    if use_utc {
        TzOffset::UTC
    } else {
        TzOffset::local_now()
    }
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
        let banner = self.banner_strip(cx);
        let t = &self.theme;
        let chart_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .children(banner)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.chart.clone())
                    .child(
                        self.drawing_toolbar.clone().cached(
                            StyleRefinement::default()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                        ),
                    ),
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
                let on = !this.chart.read(cx).state.magnet;
                this.set_magnet(on, None, cx);
            }))
            .on_action(cx.listener(|this, a: &SetMagnetMode, _, cx| {
                this.set_magnet(true, Some(a.0), cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleStayInDrawingMode, _, cx| {
                let on = !this.chart.read(cx).state.stay_in_drawing_mode;
                this.set_stay_in_drawing_mode(on, cx);
            }))
            .on_action(cx.listener(|this, _: &UnhideAllDrawings, _, cx| {
                this.with_chart(cx, |s| s.unhide_all())
            }))
            // cmd-c / cmd-v do nothing while a toolbar popover or the
            // settings dialog is open.
            .on_action(cx.listener(|this, _: &CopyDrawing, _, cx| {
                if !this.chart.read(cx).input_blocked() {
                    this.copy_drawing(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PasteDrawing, _, cx| {
                if !this.chart.read(cx).input_blocked() {
                    this.paste_drawing(cx)
                }
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
            // The settings dialog is modal: no search, CSV or theme switch.
            .on_action(cx.listener(|this, _: &OpenSymbolSearch, window, cx| {
                if this.settings.is_none() {
                    this.open_search(String::new(), window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &OpenCsv, _, cx| {
                if this.settings.is_none() {
                    this.open_csv(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleTheme, _, cx| {
                if this.settings.is_none() {
                    this.toggle_theme(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleClockUtc, _, cx| this.toggle_clock_utc(cx)))
            .relative()
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
            .children(self.search.as_ref().map(|(search, _)| search.clone()))
            .children(self.settings.as_ref().map(|(d, _)| d.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use xv_source_binance::BinanceSource;

    fn registry(csv: &Arc<CsvSource>) -> SourceRegistry {
        let mut r = SourceRegistry::new();
        r.add(Arc::new(BinanceSource::new()));
        r.add(csv.clone());
        r
    }

    fn daily_csv() -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        writeln!(f, "date,open,high,low,close,volume").unwrap();
        // Mon 2026-11-02 .. Fri 2026-11-06, Mon 2026-11-09 (weekend gap).
        for d in ["2026-11-02", "2026-11-03", "2026-11-04", "2026-11-05", "2026-11-06", "2026-11-09"] {
            writeln!(f, "{d},10,11,9,10.5,100").unwrap();
        }
        writeln!(f, "2026-11-10,10,11").unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn clamp_timeframe_prefers_next_coarser() {
        let daily = [Timeframe::D1, Timeframe::W1, Timeframe::Mo1];
        assert_eq!(clamp_timeframe(Timeframe::H1, &daily), Timeframe::D1);
        assert_eq!(clamp_timeframe(Timeframe::W1, &daily), Timeframe::W1);
        assert_eq!(clamp_timeframe(Timeframe::H4, &[Timeframe::M1]), Timeframe::M1);
        assert_eq!(clamp_timeframe(Timeframe::H4, &[]), Timeframe::H4);
    }

    #[test]
    fn start_point_defaults_to_btcusdt_h1() {
        let csv = Arc::new(CsvSource::new());
        let p = start_point(&Settings::default(), &registry(&csv), &csv);
        assert_eq!(p.symbol.key(), default_symbol().key());
        assert_eq!(p.symbol.source.0, "binance");
        assert_eq!(p.timeframe, Timeframe::H1);
        assert!(p.resolve);
        assert_eq!(p.banner, None);
    }

    #[test]
    fn start_point_restores_binance_symbol_and_timeframe() {
        let csv = Arc::new(CsvSource::new());
        let s = Settings {
            last_symbol: Some(SymbolKey {
                source: SourceId("binance".into()),
                id: "ETHUSDT".into(),
            }),
            last_timeframe: Timeframe::M15,
            ..Settings::default()
        };
        let p = start_point(&s, &registry(&csv), &csv);
        assert_eq!(p.symbol.id, "ETHUSDT");
        assert_eq!(p.timeframe, Timeframe::M15);
        assert!(p.resolve);
    }

    #[test]
    fn start_point_reloads_csv_and_clamps_timeframe() {
        let f = daily_csv();
        let csv = Arc::new(CsvSource::new());
        let id = f.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let s = Settings {
            last_symbol: Some(SymbolKey {
                source: SourceId(CSV_SOURCE_ID.into()),
                id: id.clone(),
            }),
            last_timeframe: Timeframe::H1,
            ..Settings::default()
        };
        let p = start_point(&s, &registry(&csv), &csv);
        assert_eq!(p.symbol.id, id);
        assert_eq!(p.timeframe, Timeframe::D1);
        assert!(!p.resolve);
        assert_eq!(p.banner.as_deref(), Some("Loaded 6 rows, skipped 1 (lines 8)"));
    }

    #[test]
    fn start_point_missing_csv_falls_back_with_banner() {
        let csv = Arc::new(CsvSource::new());
        let s = Settings {
            last_symbol: Some(SymbolKey {
                source: SourceId(CSV_SOURCE_ID.into()),
                id: "/nonexistent/xview/gone.csv".into(),
            }),
            last_timeframe: Timeframe::D1,
            ..Settings::default()
        };
        let p = start_point(&s, &registry(&csv), &csv);
        assert_eq!(p.symbol.key(), default_symbol().key());
        assert_eq!(p.timeframe, Timeframe::H1);
        let banner = p.banner.unwrap();
        assert!(banner.contains("/nonexistent/xview/gone.csv"), "{banner}");
        assert!(banner.contains("file not found"), "{banner}");
    }

    #[test]
    fn skipped_banner_lists_first_lines() {
        let sym = default_symbol();
        let r = |n: usize| CsvLoadReport {
            symbol: sym.clone(),
            rows: 100,
            skipped: (0..n).map(|i| (6 + 2 * i, "bad".into())).collect(),
        };
        assert_eq!(skipped_banner(&r(0)), None);
        assert_eq!(
            skipped_banner(&r(2)).unwrap(),
            "Loaded 100 rows, skipped 2 (lines 6, 8)"
        );
        assert_eq!(
            skipped_banner(&r(7)).unwrap(),
            "Loaded 100 rows, skipped 7 (lines 6, 8, 10, 12, 14, …)"
        );
    }

    #[test]
    fn region_blocked_has_the_csv_hint() {
        let e = DataError::RegionBlocked {
            host: "api.binance.com".into(),
            status: 451,
        };
        assert_eq!(
            load_error_text(&default_symbol(), Timeframe::H1, &e),
            REGION_BLOCKED_TEXT
        );
        let e = DataError::Network("timeout".into());
        assert_eq!(
            load_error_text(&default_symbol(), Timeframe::H1, &e),
            "Couldn't load BTCUSDT 1h: network: timeout"
        );
    }

    #[test]
    fn corrupt_text_names_the_kept_file() {
        let p = Path::new("/d/drawings/binance/BTCUSDT-1.json.corrupt-1");
        assert_eq!(
            corrupt_drawings_text(&default_symbol(), p, true),
            "Drawings for BTCUSDT were unreadable; the file was kept at /d/drawings/binance/BTCUSDT-1.json.corrupt-1"
        );
        assert!(corrupt_drawings_text(&default_symbol(), p, false).contains("could not be moved aside"));
    }

    #[test]
    fn min_window_keeps_a_usable_plot() {
        let plot_w = MIN_WINDOW_WIDTH
            - LEFT_TOOLBAR_WIDTH
            - (SIDE_PANEL_WIDTH + RAIL_WIDTH + 2. * REGION_BORDER)
            - xv_chart::PRICE_AXIS_WIDTH;
        let plot_h = MIN_WINDOW_HEIGHT
            - (TOP_BAR_HEIGHT + REGION_BORDER)
            - BANNER_HEIGHT
            - BOTTOM_BAR_HEIGHT
            - xv_chart::TIME_AXIS_HEIGHT;
        assert!(plot_w >= 300., "{plot_w}");
        assert!(plot_h >= 300., "{plot_h}");
    }

    #[test]
    fn last_used_styles_round_trip_and_keep_unknown_keys() {
        let mut f = TemplatesFile::default();
        let st = |w: u8| xv_drawings::Style {
            width: w,
            ..xv_drawings::Style::default()
        };
        f.last_used.insert("trend_line".into(), st(3));
        f.last_used.insert("ray".into(), st(1));
        f.last_used.insert("fib_retracement".into(), st(4));
        let mut styles = tool_styles_from(&f);
        assert_eq!(styles.len(), 2);
        assert_eq!(styles[&Tool::TrendLine], st(3));
        styles.remove(&Tool::Ray);
        styles.insert(Tool::Rectangle, st(2));
        store_tool_styles(&mut f, &styles);
        let keys: Vec<&str> = f.last_used.keys().map(String::as_str).collect();
        assert_eq!(keys, ["fib_retracement", "rectangle", "trend_line"]);
        for t in STYLED_TOOLS {
            assert_eq!(tool_from_key(t.key()), Some(t));
        }
        assert_eq!(tool_from_key("cursor"), None);
    }

    #[test]
    fn magnet_setting_maps_both_ways() {
        for m in [MagnetMode::Weak, MagnetMode::Strong] {
            assert_eq!(magnet_mode_of(magnet_setting_of(m)), m);
        }
    }

    #[test]
    fn window_rect_must_be_on_a_display() {
        let display = [WindowRect {
            x: 0.,
            y: 0.,
            w: 1920.,
            h: 1080.,
        }];
        let r = |x: f32, y: f32| WindowRect {
            x,
            y,
            w: 1280.,
            h: 800.,
        };
        assert!(rect_visible(&r(100., 100.), &display));
        assert!(rect_visible(&r(1800., 1000.), &display));
        assert!(!rect_visible(&r(1900., 100.), &display));
        assert!(!rect_visible(&r(-3000., 100.), &display));
        assert!(!rect_visible(&WindowRect { w: 10., ..r(0., 0.) }, &display));
        assert!(!rect_visible(&WindowRect { x: f32::NAN, ..r(0., 0.) }, &display));
    }
}
