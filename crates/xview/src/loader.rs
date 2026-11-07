//! Token-guarded series loading: history pages, older pages, the live
//! subscription and its reconnect backfill, with an LRU series cache.
//!
//! Every request carries the `LoadToken` of the `open` that started it. A
//! first page lands only while that token is still current, so a slow answer
//! for a symbol or timeframe the user already left never reaches the chart.
//! Live updates, backfills and older pages apply to the series on screen
//! (`shown`): until a cache-miss `open` lands, the chart still shows the
//! previous series and keeps receiving its live updates, so a failed open
//! leaves a live chart, not a frozen one.

use std::sync::Arc;

use futures::StreamExt;
use gpui::{Context, Entity, Task};
use gpui_tokio::Tokio;
use xv_core::{Candle, Series, Symbol, Timeframe};
use xv_data::{
    CandleUpdate, DataError, DataSource, HistoryPage, LoadGuard, LoadToken, SeriesCache,
    SourceRegistry,
};
use xv_source_csv::CsvSource;

use crate::chart_view::ChartView;
use crate::workspace::Workspace;

/// Candles per history request.
pub const PAGE: usize = 1000;
/// Candles re-fetched right after a cache-miss open lands, to correct bars
/// that closed while the first page was in flight (live updates are dropped
/// until the series is on screen).
pub const LANDING_BACKFILL: usize = 10;
/// Most pages one backfill fetches while looking for the series' last
/// candle. A longer gap is filled by reloading the series instead.
pub const MAX_BACKFILL_PAGES: usize = 5;

/// Next step of a backfill after it fetched page number `pages` (1-based).
#[derive(Debug, PartialEq)]
pub enum BackfillStep {
    /// The pages reach the series' last candle (or nothing older exists):
    /// merge everything fetched.
    Merge,
    /// The oldest page fetched still starts after the last candle: fetch the
    /// page before `end_before_ms`.
    Older { end_before_ms: i64 },
    /// Still no overlap after `MAX_BACKFILL_PAGES`: drop the cached series and
    /// load it afresh.
    Reload,
}

/// Decides how a backfill continues. `anchor_ms` is the open time of the
/// series' last candle when the backfill started (later live candles don't
/// count: they lie after the gap). `page` is the oldest page fetched so far.
pub fn backfill_step(anchor_ms: Option<i64>, page: &HistoryPage, pages: usize) -> BackfillStep {
    let first = page.candles.iter().map(|c| c.open_time_ms).min();
    let (Some(anchor), Some(first)) = (anchor_ms, first) else {
        return BackfillStep::Merge;
    };
    if first <= anchor || !page.has_more {
        BackfillStep::Merge
    } else if pages >= MAX_BACKFILL_PAGES {
        BackfillStep::Reload
    } else {
        BackfillStep::Older { end_before_ms: first }
    }
}

/// What the workspace must do after a backfill page.
#[derive(Debug, PartialEq)]
pub enum BackfillOutcome {
    /// Merged, still fetching, or stale: nothing.
    Handled,
    /// The gap is too long to page through: reopen this series (its cache
    /// entry is already gone, so it loads like a cache miss).
    Reopen(Symbol, Timeframe),
}

/// A backfill in progress.
struct Backfill {
    target: Current,
    /// Open time of the last candle before the gap.
    anchor_ms: Option<i64>,
    pages: usize,
    /// Newer pages fetched so far, merged together once the gap is covered.
    fetched: Vec<Candle>,
}

/// Runs `f` only while `token` is still the current one. Returns whether it ran.
pub fn apply_if_current(guard: &LoadGuard, token: &LoadToken, f: impl FnOnce()) -> bool {
    if guard.is_current(token) {
        f();
        true
    } else {
        false
    }
}

/// Token bookkeeping of the loader, free of GPUI so the real decision path
/// is unit-testable.
#[derive(Debug, Default)]
pub struct LoadState {
    guard: LoadGuard,
    /// Token whose series the chart displays.
    shown: Option<LoadToken>,
    /// Token of the older-page request in flight, if any. While one is in
    /// flight the chart's `loading_older` flag is set.
    older: Option<LoadToken>,
}

/// What to do with a finished older-page request.
#[derive(Debug, PartialEq)]
pub enum OlderOutcome {
    Apply,
    Failed,
    /// Abandoned by an open (which cleared the chart's loading flag), or for
    /// a series no longer on screen (whose replacement reset the flag).
    Stale,
}

impl LoadState {
    /// Starts an open: issues a new token. The bool says an older request was
    /// in flight and has been abandoned, so the caller must clear the chart's
    /// loading flag (the new open may fail and leave the old series up).
    pub fn begin_open(&mut self) -> (LoadToken, bool) {
        let token = self.guard.next();
        (token, self.older.take().is_some())
    }

    pub fn is_current(&self, token: &LoadToken) -> bool {
        self.guard.is_current(token)
    }

    /// The first page of `token` arrived: lands only while current.
    pub fn land(&mut self, token: LoadToken) -> bool {
        let mut landed = false;
        apply_if_current(&self.guard, &token, || landed = true);
        if landed {
            self.shown = Some(token);
        }
        landed
    }

    /// The series of `token` was put on screen synchronously (cache hit).
    pub fn mark_shown(&mut self, token: LoadToken) {
        self.shown = Some(token);
    }

    /// Live updates, backfills and older pages apply only to the series on
    /// screen. While a newer open is pending that is still the previous one.
    pub fn accepts(&self, token: &LoadToken) -> bool {
        self.shown == Some(*token)
    }

    /// Registers an older request for `token`; false when it must be refused
    /// (the chart still shows a previous series).
    pub fn begin_older(&mut self, token: LoadToken) -> bool {
        if self.accepts(&token) {
            self.older = Some(token);
            true
        } else {
            false
        }
    }

    pub fn finish_older(&mut self, token: LoadToken, ok: bool) -> OlderOutcome {
        let registered = self.older == Some(token);
        if registered {
            self.older = None;
        }
        if !registered || !self.accepts(&token) {
            return OlderOutcome::Stale;
        }
        if ok {
            OlderOutcome::Apply
        } else {
            OlderOutcome::Failed
        }
    }
}

/// A series the loader shows or is about to show, with its open's token.
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
    state: LoadState,
    /// Live stream of the series on screen.
    live: Option<Task<()>>,
    /// Live stream of a pending open; replaces `live` once it lands.
    pending_live: Option<Task<()>>,
    chart: Entity<ChartView>,
    /// The latest open (requested).
    current: Option<Current>,
    /// The series on screen; differs from `current` while an open is pending
    /// and after one failed.
    shown: Option<Current>,
    /// First-page request of a pending open; `Some` exactly while one is in
    /// flight (cleared when it lands or fails).
    load: Option<Task<()>>,
    older: Option<Task<()>>,
    backfill: Option<Task<()>>,
    backfill_state: Option<Backfill>,
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
            state: LoadState::default(),
            live: None,
            pending_live: None,
            chart,
            current: None,
            shown: None,
            load: None,
            older: None,
            backfill: None,
            backfill_state: None,
        }
    }

    pub fn registry(&self) -> &SourceRegistry {
        &self.registry
    }

    pub fn csv(&self) -> &Arc<CsvSource> {
        &self.csv
    }

    pub fn is_current(&self, token: &LoadToken) -> bool {
        self.state.is_current(token)
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
        if let Some(shown) = self.shown.as_mut()
            && shown.symbol.key() == symbol.key()
        {
            shown.symbol = symbol.clone();
        }
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
    /// the first history page arrives. The series on screen keeps its live
    /// stream until then, and for good if this open fails.
    pub fn open(
        &mut self,
        symbol: Symbol,
        tf: Timeframe,
        cx: &mut Context<Workspace>,
    ) -> Result<(), DataError> {
        let (token, older_dropped) = self.state.begin_open();
        self.pending_live = None;
        self.load = None;
        self.older = None;
        if older_dropped {
            // Otherwise `loading_older` sticks if this open fails and the
            // previous series stays on screen.
            self.chart.update(cx, |v, cx| v.older_failed(cx));
        }
        self.current = Some(Current {
            symbol: symbol.clone(),
            tf,
            token,
        });
        let src = self.source(&symbol)?;
        let cached = self.cache.get(&symbol.key(), tf).cloned();
        self.pending_live = self.start_live(&src, &symbol, tf, token, cx);
        if let Some(series) = cached {
            self.state.mark_shown(token);
            self.make_shown();
            self.show(series, cx);
            if self.live.is_some() {
                // The cached copy missed the candles since it was last shown.
                self.request_backfill(token, PAGE, cx);
            }
        } else {
            let task = Tokio::spawn(cx, src.history(&symbol, tf, None, PAGE));
            self.load = Some(cx.spawn(async move |ws, cx| {
                let result = flatten(task.await);
                ws.update(cx, |ws, cx| ws.on_history(token, result, cx)).ok();
            }));
        }
        Ok(())
    }

    /// The current open is now on screen: its live stream replaces that of
    /// the previous series.
    fn make_shown(&mut self) {
        self.shown = self.current.clone();
        self.live = self.pending_live.take();
    }

    /// The current open failed: drops its live stream. The previous series
    /// stays on screen with its own.
    pub fn open_failed(&mut self, token: LoadToken) {
        if self.state.is_current(&token) {
            self.pending_live = None;
            self.load = None;
        }
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
        if !self.state.land(token) {
            return false;
        }
        self.load = None;
        let series = Series::new(cur.symbol, cur.tf, page.candles, page.has_more);
        self.make_shown();
        self.show(series, cx);
        if self.live.is_some() {
            self.request_backfill(token, LANDING_BACKFILL, cx);
        }
        true
    }

    fn show(&mut self, series: Series, cx: &mut Context<Workspace>) {
        self.cache.insert(series.clone());
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

    /// Subscribes to `symbol`'s live candles, delivered under `token`.
    fn start_live(
        &mut self,
        src: &Arc<dyn DataSource>,
        symbol: &Symbol,
        tf: Timeframe,
        token: LoadToken,
        cx: &mut Context<Workspace>,
    ) -> Option<Task<()>> {
        // Sources such as Binance start their stream task on the ambient
        // tokio runtime.
        let mut stream = {
            let _rt = Tokio::handle(cx).enter();
            src.subscribe(symbol, tf)
        }?;
        Some(cx.spawn(async move |ws, cx| {
            while let Some(update) = stream.next().await {
                let alive = ws.update(cx, |ws, cx| ws.loader.on_live(token, update, cx));
                if alive.is_err() {
                    break;
                }
            }
        }))
    }

    fn on_live(&mut self, token: LoadToken, update: CandleUpdate, cx: &mut Context<Workspace>) {
        if !self.state.accepts(&token) {
            return;
        }
        match update {
            CandleUpdate::Candle(c) => {
                if let Some(cur) = &self.shown
                    && let Some(s) = self.cache.get_mut(&cur.symbol.key(), cur.tf)
                {
                    s.apply_update(c);
                }
                self.chart.update(cx, |v, cx| v.apply_update(c, cx));
            }
            CandleUpdate::Reconnected => self.request_backfill(token, PAGE, cx),
            CandleUpdate::Disconnected => {}
        }
    }

    /// Fetches the latest `limit` candles of the series on screen and merges
    /// them, paging back until they reach its last candle so no hole is left.
    fn request_backfill(&mut self, token: LoadToken, limit: usize, cx: &mut Context<Workspace>) {
        let Some(target) = self.shown.clone().filter(|s| s.token == token) else {
            return;
        };
        let anchor_ms = self.chart.read(cx).state.series.last().map(|c| c.open_time_ms);
        self.backfill_state = Some(Backfill {
            target,
            anchor_ms,
            pages: 0,
            fetched: Vec::new(),
        });
        self.fetch_backfill(None, limit, cx);
    }

    fn fetch_backfill(&mut self, end_before_ms: Option<i64>, limit: usize, cx: &mut Context<Workspace>) {
        let Some(b) = &self.backfill_state else {
            return;
        };
        let Ok(src) = self.source(&b.target.symbol) else {
            self.backfill_state = None;
            return;
        };
        let token = b.target.token;
        let task = Tokio::spawn(cx, src.history(&b.target.symbol, b.target.tf, end_before_ms, limit));
        self.backfill = Some(cx.spawn(async move |ws, cx| {
            let result = flatten(task.await);
            ws.update(cx, |ws, cx| ws.on_backfill(token, result, cx))
                .ok();
        }));
    }

    /// Whether a backfill response for `token` should be reported (it is for
    /// the series on screen). Also ends that backfill.
    pub fn backfill_failed(&mut self, token: LoadToken) -> bool {
        let ours = self
            .backfill_state
            .as_ref()
            .is_some_and(|b| b.target.token == token);
        if ours {
            self.backfill_state = None;
        }
        ours && self.state.accepts(&token)
    }

    /// Takes one backfill page: merges, fetches the next older page, or asks
    /// for a reload when the gap is too long.
    pub fn on_backfill_page(
        &mut self,
        token: LoadToken,
        page: HistoryPage,
        cx: &mut Context<Workspace>,
    ) -> BackfillOutcome {
        let ours = self
            .backfill_state
            .as_ref()
            .is_some_and(|b| b.target.token == token);
        if !ours || !self.state.accepts(&token) {
            return BackfillOutcome::Handled;
        }
        let b = self.backfill_state.as_mut().expect("checked above");
        b.pages += 1;
        match backfill_step(b.anchor_ms, &page, b.pages) {
            BackfillStep::Merge => {
                let mut b = self.backfill_state.take().expect("checked above");
                b.fetched.extend(page.candles);
                self.merge_backfill(&b.target, b.fetched, cx);
                BackfillOutcome::Handled
            }
            BackfillStep::Older { end_before_ms } => {
                b.fetched.extend(page.candles);
                self.fetch_backfill(Some(end_before_ms), PAGE, cx);
                BackfillOutcome::Handled
            }
            BackfillStep::Reload => {
                let mut b = self.backfill_state.take().expect("checked above");
                let t = b.target;
                self.cache.remove(&t.symbol.key(), t.tf);
                if self.load.is_none() {
                    // No other open pending (this one is current, or a newer
                    // one failed and left this series on screen).
                    BackfillOutcome::Reopen(t.symbol, t.tf)
                } else {
                    // A newer open is pending and takes over the chart once it
                    // lands; don't supersede it. Merge the newest pages
                    // meanwhile; the next visit of this series reloads it.
                    b.fetched.extend(page.candles);
                    self.merge_backfill(&t, b.fetched, cx);
                    BackfillOutcome::Handled
                }
            }
        }
    }

    /// Merges backfilled candles into `target` (the series on screen).
    fn merge_backfill(&mut self, target: &Current, candles: Vec<Candle>, cx: &mut Context<Workspace>) {
        if let Some(s) = self.cache.get_mut(&target.symbol.key(), target.tf) {
            s.merge_range(candles.clone());
        }
        self.chart
            .update(cx, |v, cx| v.merge_backfill(candles, cx));
    }

    /// Requests the page before `end_before_ms` for the series on screen.
    pub fn load_older(&mut self, end_before_ms: i64, cx: &mut Context<Workspace>) {
        let Some(cur) = self.shown.clone() else {
            self.chart.update(cx, |v, cx| v.older_failed(cx));
            return;
        };
        let src = match self.source(&cur.symbol) {
            Ok(src) if self.state.begin_older(cur.token) => src,
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
        match self.state.finish_older(token, result.is_ok()) {
            OlderOutcome::Stale => return Ok(()),
            OlderOutcome::Apply | OlderOutcome::Failed => {}
        }
        let cur = self.shown.clone().expect("an accepted token is on screen");
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
    fn open_abandoning_an_older_request_reports_it() {
        let mut st = LoadState::default();
        let (a, dropped) = st.begin_open();
        assert!(!dropped);
        assert!(st.land(a));
        assert!(st.begin_older(a));
        // A new open while the older page is in flight: the caller must clear
        // the chart's loading flag, since this open may fail.
        let (b, dropped) = st.begin_open();
        assert!(dropped);
        // The abandoned response, if it still arrived, is stale (the caller
        // already cleared the flag), though its series is still on screen.
        assert!(st.accepts(&a));
        assert_eq!(st.finish_older(a, true), OlderOutcome::Stale);
        // Once b lands, nothing about a touches the chart.
        assert!(st.land(b));
        assert_eq!(st.finish_older(a, false), OlderOutcome::Stale);
        let (_c, dropped) = st.begin_open();
        assert!(!dropped);
    }

    #[test]
    fn older_requests_need_the_current_series_on_screen() {
        let mut st = LoadState::default();
        let (a, _) = st.begin_open();
        assert!(st.land(a));
        let (b, _) = st.begin_open();
        // b is pending: the chart shows a's series, so b's older is refused
        // while a's series keeps taking live updates and older pages.
        assert!(!st.begin_older(b));
        assert!(!st.accepts(&b));
        assert!(st.accepts(&a));
        assert!(st.begin_older(a));
        assert_eq!(st.finish_older(a, true), OlderOutcome::Apply);
        st.mark_shown(b);
        assert!(!st.accepts(&a));
        assert!(st.begin_older(b));
        assert_eq!(st.finish_older(b, true), OlderOutcome::Apply);
        assert!(st.begin_older(b));
        assert_eq!(st.finish_older(b, false), OlderOutcome::Failed);
    }

    #[test]
    fn failed_open_keeps_the_shown_series_live() {
        let mut st = LoadState::default();
        let (a, _) = st.begin_open();
        assert!(st.land(a));
        // b's first page fails: nothing lands, a stays on screen and live.
        let (b, _) = st.begin_open();
        assert!(st.is_current(&b));
        assert!(st.accepts(&a));
        assert!(!st.accepts(&b));
        // A retry of b that lands takes over.
        let (b2, _) = st.begin_open();
        assert!(st.land(b2));
        assert!(!st.accepts(&a));
        assert!(st.accepts(&b2));
    }

    fn page(mins: std::ops::Range<i64>, has_more: bool) -> HistoryPage {
        let candles = mins
            .map(|m| Candle {
                open_time_ms: m * 60_000,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
            })
            .collect();
        HistoryPage { candles, has_more }
    }

    #[test]
    fn backfill_merges_once_a_page_reaches_the_last_candle() {
        let anchor = Some(100 * 60_000);
        // The latest page starts at or before the last candle: merge.
        assert_eq!(backfill_step(anchor, &page(100..1100, true), 1), BackfillStep::Merge);
        assert_eq!(backfill_step(anchor, &page(50..1050, true), 1), BackfillStep::Merge);
        // An empty series or an empty page: nothing to page for.
        assert_eq!(backfill_step(None, &page(500..1500, true), 1), BackfillStep::Merge);
        assert_eq!(backfill_step(anchor, &page(0..0, true), 1), BackfillStep::Merge);
        // No older data exists at the source: the page is all there is.
        assert_eq!(backfill_step(anchor, &page(500..800, false), 1), BackfillStep::Merge);
    }

    #[test]
    fn backfill_pages_back_from_the_oldest_page() {
        let anchor = Some(100 * 60_000);
        // 1500 bars were missed: the latest page (1601..2601) misses 101..1600.
        assert_eq!(
            backfill_step(anchor, &page(1601..2601, true), 1),
            BackfillStep::Older { end_before_ms: 1601 * 60_000 }
        );
        // The next page (601..1601) still doesn't reach; ask before its start.
        assert_eq!(
            backfill_step(anchor, &page(601..1601, true), 2),
            BackfillStep::Older { end_before_ms: 601 * 60_000 }
        );
        // The third (-399..601) overlaps: merge all three.
        assert_eq!(backfill_step(anchor, &page(-399..601, true), 3), BackfillStep::Merge);
        // Adjacent but not overlapping (starts one bar after the last candle)
        // still pages: the last candle's final values are in the next page.
        assert_eq!(
            backfill_step(anchor, &page(101..1101, true), 1),
            BackfillStep::Older { end_before_ms: 101 * 60_000 }
        );
    }

    #[test]
    fn backfill_gives_up_after_the_page_cap() {
        let anchor = Some(0);
        let far = page(100_000..101_000, true);
        for n in 1..MAX_BACKFILL_PAGES {
            assert!(matches!(backfill_step(anchor, &far, n), BackfillStep::Older { .. }));
        }
        assert_eq!(backfill_step(anchor, &far, MAX_BACKFILL_PAGES), BackfillStep::Reload);
        // Overlap on the last allowed page still merges.
        assert_eq!(
            backfill_step(anchor, &page(-10..990, true), MAX_BACKFILL_PAGES),
            BackfillStep::Merge
        );
    }

    #[test]
    fn late_first_page_does_not_land() {
        let mut st = LoadState::default();
        let (a, _) = st.begin_open();
        let (b, _) = st.begin_open();
        assert!(st.land(b));
        assert!(!st.land(a));
        assert!(st.accepts(&b));
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
