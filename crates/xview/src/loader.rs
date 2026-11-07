//! Token-guarded series loading: history pages, older pages, the live
//! subscription and its reconnect backfill, with an LRU series cache.
//!
//! Every request carries the `LoadToken` of the `open` that started it. A
//! response lands only while that token is still current, so a slow answer
//! for a symbol or timeframe the user already left never reaches the chart.
//! Live updates and older pages additionally need the token's series to be
//! the one on screen (`shown`): until a cache-miss `open` lands, the chart
//! still shows the previous series.

use std::sync::Arc;

use futures::StreamExt;
use gpui::{Context, Entity, Task};
use gpui_tokio::Tokio;
use xv_core::{Series, Symbol, Timeframe};
use xv_data::{
    CandleUpdate, DataError, DataSource, HistoryPage, LoadGuard, LoadToken, SeriesCache,
    SourceRegistry,
};
use xv_source_csv::CsvSource;

use crate::chart_view::ChartView;
use crate::workspace::Workspace;

/// Candles per history request.
pub const PAGE: usize = 1000;

/// Runs `f` only while `token` is still the current one. Returns whether it ran.
pub fn apply_if_current(guard: &LoadGuard, token: &LoadToken, f: impl FnOnce()) -> bool {
    if guard.is_current(token) {
        f();
        true
    } else {
        false
    }
}

/// What the loader is showing or about to show.
#[derive(Clone)]
struct Current {
    symbol: Symbol,
    tf: Timeframe,
    token: LoadToken,
}

pub struct Loader {
    registry: SourceRegistry,
    csv: Arc<CsvSource>,
    cache: SeriesCache,
    guard: LoadGuard,
    live: Option<Task<()>>,
    chart: Entity<ChartView>,
    current: Option<Current>,
    /// Token whose series the chart displays.
    shown: Option<LoadToken>,
    load: Option<Task<()>>,
    older: Option<Task<()>>,
    backfill: Option<Task<()>>,
}

type JoinResult<T> = Result<T, gpui_tokio::JoinError>;

fn flatten(r: JoinResult<Result<HistoryPage, DataError>>) -> Result<HistoryPage, DataError> {
    r.map_err(|e| DataError::Network(format!("request task failed: {e}")))?
}

impl Loader {
    pub fn new(registry: SourceRegistry, csv: Arc<CsvSource>, chart: Entity<ChartView>) -> Self {
        Loader {
            registry,
            csv,
            cache: SeriesCache::new(),
            guard: LoadGuard::new(),
            live: None,
            chart,
            current: None,
            shown: None,
            load: None,
            older: None,
            backfill: None,
        }
    }

    pub fn registry(&self) -> &SourceRegistry {
        &self.registry
    }

    pub fn csv(&self) -> &Arc<CsvSource> {
        &self.csv
    }

    pub fn is_current(&self, token: &LoadToken) -> bool {
        self.guard.is_current(token)
    }

    pub fn supported_timeframes(&self, symbol: &Symbol) -> Vec<Timeframe> {
        self.registry
            .get(&symbol.source)
            .map(|s| s.supported_timeframes(symbol))
            .unwrap_or_default()
    }

    /// Replaces the metadata (display name, tick size, ...) of the current
    /// symbol, e.g. once a placeholder restored from settings is resolved.
    pub fn update_symbol(&mut self, symbol: &Symbol, cx: &mut Context<Workspace>) -> bool {
        let Some(cur) = self.current.as_mut() else {
            return false;
        };
        if cur.symbol.key() != symbol.key() {
            return false;
        }
        cur.symbol = symbol.clone();
        for tf in Timeframe::ALL {
            if let Some(s) = self.cache.get_mut(&symbol.key(), tf) {
                s.symbol = symbol.clone();
            }
        }
        let sym = symbol.clone();
        self.chart.update(cx, |c, cx| {
            if c.state.series.symbol.key() == sym.key() {
                c.state.series.symbol = sym;
                cx.notify();
            }
        });
        true
    }

    /// Forgets cached series of `symbol` (a CSV file that was re-read).
    pub fn invalidate(&mut self, symbol: &Symbol) {
        self.cache.remove_symbol(&symbol.key());
    }

    fn source(&self, symbol: &Symbol) -> Result<Arc<dyn DataSource>, DataError> {
        self.registry
            .get(&symbol.source)
            .ok_or_else(|| DataError::NotFound(format!("data source \"{}\"", symbol.source.0)))
    }

    /// Starts showing `symbol` at `tf`: instantly from the cache, else after
    /// the first history page arrives. Restarts the live subscription.
    pub fn open(
        &mut self,
        symbol: Symbol,
        tf: Timeframe,
        cx: &mut Context<Workspace>,
    ) -> Result<(), DataError> {
        let token = self.guard.next();
        self.live = None;
        self.load = None;
        self.older = None;
        self.backfill = None;
        self.current = Some(Current {
            symbol: symbol.clone(),
            tf,
            token,
        });
        let src = self.source(&symbol)?;
        let cached = self.cache.get(&symbol.key(), tf).cloned();
        let hit = cached.is_some();
        if let Some(series) = cached {
            self.show(token, series, cx);
        } else {
            let task = Tokio::spawn(cx, src.history(&symbol, tf, None, PAGE));
            self.load = Some(cx.spawn(async move |ws, cx| {
                let result = flatten(task.await);
                ws.update(cx, |ws, cx| ws.on_history(token, result, cx)).ok();
            }));
        }
        self.start_live(&src, &symbol, tf, token, hit, cx);
        Ok(())
    }

    /// Lands the first page of `token`'s open. Returns false when stale.
    pub fn land(
        &mut self,
        token: LoadToken,
        page: HistoryPage,
        cx: &mut Context<Workspace>,
    ) -> bool {
        let Some(cur) = self.current.clone() else {
            return false;
        };
        let mut landed = false;
        apply_if_current(&self.guard, &token, || landed = true);
        if !landed {
            return false;
        }
        let series = Series::new(cur.symbol, cur.tf, page.candles, page.has_more);
        self.show(token, series, cx);
        true
    }

    fn show(&mut self, token: LoadToken, series: Series, cx: &mut Context<Workspace>) {
        self.cache.insert(series.clone());
        self.shown = Some(token);
        let name = self
            .registry
            .get(&series.symbol.source)
            .map(|s| s.name().to_string())
            .unwrap_or_default();
        self.chart.update(cx, |c, cx| {
            c.set_source_name(name, cx);
            c.set_series(series, cx);
        });
    }

    fn start_live(
        &mut self,
        src: &Arc<dyn DataSource>,
        symbol: &Symbol,
        tf: Timeframe,
        token: LoadToken,
        cache_hit: bool,
        cx: &mut Context<Workspace>,
    ) {
        // Sources such as Binance start their stream task on the ambient
        // tokio runtime.
        let stream = {
            let _rt = Tokio::handle(cx).enter();
            src.subscribe(symbol, tf)
        };
        let Some(mut stream) = stream else { return };
        if cache_hit {
            // The cached copy missed the candles since it was last shown.
            self.request_backfill(token, cx);
        }
        self.live = Some(cx.spawn(async move |ws, cx| {
            while let Some(update) = stream.next().await {
                let alive = ws.update(cx, |ws, cx| ws.loader.on_live(token, update, cx));
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    fn on_live(&mut self, token: LoadToken, update: CandleUpdate, cx: &mut Context<Workspace>) {
        if !self.guard.is_current(&token) || self.shown != Some(token) {
            return;
        }
        match update {
            CandleUpdate::Candle(c) => {
                if let Some(cur) = &self.current
                    && let Some(s) = self.cache.get_mut(&cur.symbol.key(), cur.tf)
                {
                    s.apply_update(c);
                }
                self.chart.update(cx, |v, cx| v.apply_update(c, cx));
            }
            CandleUpdate::Reconnected => self.request_backfill(token, cx),
            CandleUpdate::Disconnected => {}
        }
    }

    /// Fetches the latest page and merges it, filling any gap.
    fn request_backfill(&mut self, token: LoadToken, cx: &mut Context<Workspace>) {
        let Some(cur) = self.current.clone() else {
            return;
        };
        let Ok(src) = self.source(&cur.symbol) else {
            return;
        };
        let task = Tokio::spawn(cx, src.history(&cur.symbol, cur.tf, None, PAGE));
        self.backfill = Some(cx.spawn(async move |ws, cx| {
            let result = flatten(task.await);
            ws.update(cx, |ws, cx| ws.on_backfill(token, result, cx))
                .ok();
        }));
    }

    /// Merges a backfill page. Returns false when stale or not on screen.
    pub fn merge_backfill(
        &mut self,
        token: LoadToken,
        page: HistoryPage,
        cx: &mut Context<Workspace>,
    ) -> bool {
        if !self.guard.is_current(&token) || self.shown != Some(token) {
            return false;
        }
        let Some(cur) = &self.current else {
            return false;
        };
        if let Some(s) = self.cache.get_mut(&cur.symbol.key(), cur.tf) {
            s.merge_range(page.candles.clone());
        }
        self.chart
            .update(cx, |v, cx| v.merge_backfill(page.candles, cx));
        true
    }

    /// Requests the page before `end_before_ms` for the series on screen.
    pub fn load_older(&mut self, end_before_ms: i64, cx: &mut Context<Workspace>) {
        let Some(cur) = self.current.clone() else {
            return;
        };
        let src = match self.source(&cur.symbol) {
            Ok(src) if self.shown == Some(cur.token) => src,
            // The chart still shows a previous series (or nothing loadable):
            // clear its loading flag so a later pan can ask again.
            _ => {
                self.chart.update(cx, |v, cx| v.older_failed(cx));
                return;
            }
        };
        let token = cur.token;
        let task = Tokio::spawn(
            cx,
            src.history(&cur.symbol, cur.tf, Some(end_before_ms), PAGE),
        );
        self.older = Some(cx.spawn(async move |ws, cx| {
            let result = flatten(task.await);
            ws.update(cx, |ws, cx| ws.on_older(token, result, cx)).ok();
        }));
    }

    /// Lands an older page. Errors are returned for the banner after the
    /// chart's loading flag was cleared.
    pub fn finish_older(
        &mut self,
        token: LoadToken,
        result: Result<HistoryPage, DataError>,
        cx: &mut Context<Workspace>,
    ) -> Result<(), DataError> {
        if !self.guard.is_current(&token) {
            // Stale. If its series is still on screen (the newer open has not
            // landed yet), nothing else will reset the flag.
            if self.shown == Some(token) {
                self.chart.update(cx, |v, cx| v.older_failed(cx));
            }
            return Ok(());
        }
        let cur = self.current.clone().expect("current token implies an open");
        match result {
            Ok(page) => {
                if let Some(s) = self.cache.get_mut(&cur.symbol.key(), cur.tf) {
                    s.prepend_older(page.candles.clone(), page.has_more);
                }
                self.chart
                    .update(cx, |v, cx| v.prepend_older(page.candles, page.has_more, cx));
                Ok(())
            }
            Err(e) => {
                self.chart.update(cx, |v, cx| v.older_failed(cx));
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The loader's completion path in miniature: each open takes a token,
    /// and a response lands only through `apply_if_current`.
    #[test]
    fn late_response_is_dropped() {
        let mut guard = LoadGuard::new();
        let shown: RefCell<Option<&str>> = RefCell::new(None);
        let first = guard.next();
        let second = guard.next();
        // The second open's response arrives first...
        assert!(apply_if_current(&guard, &second, || {
            *shown.borrow_mut() = Some("second")
        }));
        // ...then the first one, late: it must not replace the series.
        assert!(!apply_if_current(&guard, &first, || {
            *shown.borrow_mut() = Some("first")
        }));
        assert_eq!(*shown.borrow(), Some("second"));
    }

    #[test]
    fn late_response_is_dropped_even_if_it_arrives_last_without_newer_data() {
        let mut guard = LoadGuard::new();
        let mut shown = None;
        let first = guard.next();
        let _second = guard.next();
        apply_if_current(&guard, &first, || shown = Some("first"));
        assert_eq!(shown, None);
    }
}
