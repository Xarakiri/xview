//! Symbol search modal: a minimal one-line text input over a result list.
//!
//! The input uses the shared one-line key mapping from `text_input`
//! (typing, backspace, cmd-backspace, cmd-v). Every registered source is queried 150 ms after the last edit;
//! results keep the registry order (Binance first, then CSV files).
//!
//! While open, this is the only shell element that tracks focus. The
//! workspace gives focus back to the chart when it closes.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    MouseButton, SharedString, Task, Window, div, prelude::*, px,
};
use gpui_tokio::Tokio;
use xv_core::Symbol;
use xv_data::{DataError, DataSource};

use crate::text_input::{KeyAction, caret, key_action, sanitize_paste};
use crate::theme::AppTheme;
use crate::widgets::icon;

pub const DEBOUNCE: Duration = Duration::from_millis(150);
/// Results kept from all sources together.
pub const MAX_RESULTS: usize = 50;
/// Rows on screen; ↑/↓ scroll the window to keep the selection visible.
pub const VISIBLE_ROWS: usize = 10;
const ROW_HEIGHT: f32 = 40.;
const PANEL_WIDTH: f32 = 600.;

#[derive(Clone, Debug, PartialEq)]
pub enum SearchEvent {
    Picked(Symbol),
    Dismissed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub symbol: Symbol,
    pub source: SharedString,
}

/// Moves the selection by `delta`, clamped to the list.
pub fn move_selection(selected: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (selected as isize + delta).clamp(0, len as isize - 1) as usize
}

/// The rows to draw so that `selected` stays visible.
pub fn visible_window(selected: usize, len: usize, rows: usize) -> Range<usize> {
    if len <= rows {
        return 0..len;
    }
    let start = selected.saturating_sub(rows - 1).min(len - rows);
    start..start + rows
}

/// Concatenates per-source answers in registry order. Errors only surface
/// when nothing was found anywhere.
pub fn merge_results(
    answers: Vec<(SharedString, Result<Vec<Symbol>, DataError>)>,
) -> (Vec<SearchResult>, Option<String>) {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (source, answer) in answers {
        match answer {
            Ok(symbols) => out.extend(symbols.into_iter().map(|symbol| SearchResult {
                symbol,
                source: source.clone(),
            })),
            Err(e) => errors.push(format!("{source}: {e}")),
        }
    }
    out.truncate(MAX_RESULTS);
    let error = (out.is_empty() && !errors.is_empty()).then(|| errors.join("; "));
    (out, error)
}

pub struct SymbolSearch {
    focus: FocusHandle,
    theme: AppTheme,
    sources: Vec<Arc<dyn DataSource>>,
    query: String,
    results: Vec<SearchResult>,
    selected: usize,
    searching: bool,
    error: Option<String>,
    pending: Option<Task<()>>,
}

impl EventEmitter<SearchEvent> for SymbolSearch {}

impl Focusable for SymbolSearch {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SymbolSearch {
    pub fn new(
        sources: Vec<Arc<dyn DataSource>>,
        theme: AppTheme,
        query: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = SymbolSearch {
            focus: cx.focus_handle(),
            theme,
            sources,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            searching: false,
            error: None,
            pending: None,
        };
        this.set_query(query, cx);
        this
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if query == self.query && (self.searching || !self.results.is_empty()) {
            return;
        }
        self.query = query;
        let q = self.query.trim().to_string();
        self.error = None;
        if q.is_empty() {
            self.pending = None;
            self.searching = false;
            self.results.clear();
            self.selected = 0;
            cx.notify();
            return;
        }
        self.searching = true;
        let requests: Vec<_> = self
            .sources
            .iter()
            .map(|s| (SharedString::from(s.name().to_string()), s.search_symbols(&q)))
            .collect();
        // Replacing the task cancels the previous (debouncing or running) one.
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let Ok(task) = this.update(cx, |_, cx| {
                Tokio::spawn(
                    cx,
                    futures::future::join_all(
                        requests
                            .into_iter()
                            .map(|(name, fut)| async move { (name, fut.await) }),
                    ),
                )
            }) else {
                return;
            };
            let answers = task.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                let (results, error) = merge_results(answers);
                this.results = results;
                this.error = error;
                this.selected = 0;
                this.searching = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.results.get(ix) {
            cx.emit(SearchEvent::Picked(r.symbol.clone()));
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn pick_selected(&mut self, cx: &mut Context<Self>) {
        self.pick(self.selected, cx);
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        match key_action(&self.query, &e.keystroke) {
            KeyAction::Edit(q) => self.set_query(q, cx),
            KeyAction::Move(d) => {
                self.selected = move_selection(self.selected, self.results.len(), d);
                cx.notify();
            }
            KeyAction::Pick => self.pick(self.selected, cx),
            KeyAction::Dismiss => cx.emit(SearchEvent::Dismissed),
            KeyAction::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let q = format!("{}{}", self.query, sanitize_paste(&text));
                    self.set_query(q, cx);
                }
            }
            KeyAction::None => return,
        }
        cx.stop_propagation();
    }

    fn status_text(&self) -> Option<String> {
        if !self.results.is_empty() {
            return None;
        }
        if self.query.trim().is_empty() {
            return Some("Type a symbol, e.g. BTCUSDT".into());
        }
        if self.searching {
            return Some("Searching…".into());
        }
        if let Some(e) = &self.error {
            return Some(format!("Search failed: {e}"));
        }
        Some("No symbols match".into())
    }
}

impl Render for SymbolSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let window_rows = visible_window(self.selected, self.results.len(), VISIBLE_ROWS);
        let rows = window_rows.map(|ix| {
            let r = &self.results[ix];
            let active = ix == self.selected;
            let hover = t.hover;
            div()
                .id(("search-row", ix))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .h(px(ROW_HEIGHT))
                .px(px(16.))
                .when(active, |d| d.bg(t.active_bg))
                .when(!active, |d| d.hover(move |s| s.bg(hover)))
                .child(
                    div()
                        .w(px(150.))
                        .flex_none()
                        .overflow_hidden()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(r.symbol.display.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(t.muted)
                        .child(r.symbol.description.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(r.source.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick(ix, cx)))
        });
        let query: SharedString = self.query.clone().into();
        let input = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .h(px(48.))
            .px(px(14.))
            .border_b_1()
            .border_color(t.border)
            .child(icon("zoom", 24., t.muted))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_1()
                    .text_size(px(16.))
                    .when(!self.query.is_empty(), |d| d.child(query))
                    .child(caret(20., &t))
                    .when(self.query.is_empty(), |d| {
                        d.child(div().pl(px(2.)).text_color(t.muted).child("Search"))
                    }),
            );
        let panel = div()
            .id("symbol-search")
            .track_focus(&self.focus)
            .key_context("SymbolSearch")
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .w(px(PANEL_WIDTH))
            .mt(px(72.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(14.))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px(px(16.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Symbol Search"),
            )
            .child(input)
            .child(div().flex().flex_col().py(px(4.)).children(rows))
            .children(self.status_text().map(|s| {
                div()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .text_color(t.muted)
                    .child(s)
            }));
        // Full-window backdrop: a click outside the panel closes the modal.
        div()
            .id("symbol-search-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .bg(gpui::hsla(0., 0., 0., 0.25))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Dismissed)),
            )
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};
    use xv_core::SourceId;

    fn ks(key: &str, ch: Option<&str>, m: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: m,
            key: key.into(),
            key_char: ch.map(Into::into),
        }
    }

    fn sym(id: &str) -> Symbol {
        Symbol {
            source: SourceId("binance".into()),
            id: id.into(),
            display: id.into(),
            description: String::new(),
            base: String::new(),
            quote: String::new(),
            tick_size: 0.01,
        }
    }

    #[test]
    fn keys_edit_navigate_pick_and_dismiss() {
        let none = Modifiers::default();
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        assert_eq!(key_action("et", &ks("h", Some("h"), none)), KeyAction::Edit("eth".into()));
        assert_eq!(key_action("eth", &ks("backspace", None, none)), KeyAction::Edit("et".into()));
        assert_eq!(key_action("eth", &ks("backspace", None, cmd)), KeyAction::Edit("".into()));
        assert_eq!(key_action("", &ks("backspace", None, none)), KeyAction::Edit("".into()));
        assert_eq!(key_action("x", &ks("up", None, none)), KeyAction::Move(-1));
        assert_eq!(key_action("x", &ks("down", None, none)), KeyAction::Move(1));
        assert_eq!(key_action("x", &ks("enter", None, none)), KeyAction::Pick);
        assert_eq!(key_action("x", &ks("escape", None, none)), KeyAction::Dismiss);
        assert_eq!(key_action("x", &ks("v", Some("v"), cmd)), KeyAction::Paste);
        assert_eq!(key_action("x", &ks("k", Some("k"), cmd)), KeyAction::None);
        assert_eq!(key_action("x", &ks("tab", Some("\t"), none)), KeyAction::None);
    }

    #[test]
    fn selection_and_window() {
        assert_eq!(move_selection(0, 0, 1), 0);
        assert_eq!(move_selection(0, 3, -1), 0);
        assert_eq!(move_selection(1, 3, 1), 2);
        assert_eq!(move_selection(2, 3, 1), 2);
        assert_eq!(visible_window(0, 3, 10), 0..3);
        assert_eq!(visible_window(0, 50, 10), 0..10);
        assert_eq!(visible_window(9, 50, 10), 0..10);
        assert_eq!(visible_window(10, 50, 10), 1..11);
        assert_eq!(visible_window(49, 50, 10), 40..50);
    }

    #[test]
    fn results_keep_source_order_and_hide_partial_errors() {
        let (r, err) = merge_results(vec![
            ("Binance".into(), Ok(vec![sym("ETHUSDT"), sym("ETHBTC")])),
            ("CSV".into(), Ok(vec![sym("ETH_DAILY")])),
        ]);
        assert_eq!(r[0].symbol.id, "ETHUSDT");
        assert_eq!(r[2].source, SharedString::from("CSV"));
        assert_eq!(err, None);

        let (r, err) = merge_results(vec![
            ("Binance".into(), Err(DataError::Network("down".into()))),
            ("CSV".into(), Ok(vec![sym("ETH_DAILY")])),
        ]);
        assert_eq!(r.len(), 1);
        assert_eq!(err, None);

        let (r, err) = merge_results(vec![
            ("Binance".into(), Err(DataError::Network("down".into()))),
            ("CSV".into(), Ok(vec![])),
        ]);
        assert!(r.is_empty());
        assert_eq!(err.as_deref(), Some("Binance: network: down"));
    }

    #[test]
    fn paste_keeps_first_line() {
        assert_eq!(sanitize_paste("eth\nbtc"), "eth");
        assert_eq!(sanitize_paste("e\tth"), "eth");
    }
}
