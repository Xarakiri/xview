# XView Milestone 1 (Chart Core) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A macOS desktop app, XView, that shows live Binance (or CSV) candlestick charts across 11 timeframes with pan/zoom, crosshair, volume, symbol search, and persistent trendline/ray/horizontal/vertical/rectangle drawings, in the owner's reference layout.

**Architecture:** A Cargo workspace. All logic lives in GPUI-free crates that are unit-tested with plain `cargo test`: core types and resampling, data sources, drawings, persistence, and the chart engine. The chart engine (`xv_chart`) includes a `ChartState` that turns input events into state changes and a `build_frame` that turns state into drawing primitives. The `xview` binary is a thin GPUI layer: it paints primitives in a `canvas`, forwards mouse and keyboard events to `ChartState`, and renders the toolbars and panels with `div`s.

**Tech Stack:** Rust (edition 2024), GPUI + gpui_platform + gpui_tokio (git, Zed repo, pinned), tokio, reqwest (rustls), tokio-tungstenite (rustls), serde/serde_json, chrono, futures, dirs.

**Spec:** `docs/superpowers/specs/2026-11-07-xview-chart-core-design.md`

## Global Constraints

- Rust edition 2024, `rust-version = "1.98"`. macOS only; don't add Linux/Windows CI or code paths.
- GPUI: `git = "https://github.com/zed-industries/zed"`, `rev = "9a8ab54db4f9f9af4dc6efb42b304c55c6a62bd3"`, crates `gpui`, `gpui_platform`, `gpui_tokio`. No local-path dependency on a Zed checkout (a commented-out `[patch]` for local dev is fine).
- `gpui_platform` features: `["font-kit", "runtime_shaders"]`. The owner's Mac has only Command Line Tools (no Xcode.app), and `runtime_shaders` avoids the build-time `metal` compiler. This deviates from the spec's "needs full Xcode" line; the README documents it.
- License **GPL-3.0-or-later** in every crate's `license` field, with the full GPLv3 text in `LICENSE`. Every dependency must be GPLv3-compatible (MIT, Apache-2.0, BSD, MPL-2.0, ISC, Zlib are fine).
- Name **XView**. Crates are prefixed `xv_` and the binary is `xview`. The data dir is `~/Library/Application Support/XView/`, overridable by the env var `XVIEW_DATA_DIR`, which tests use.
- No TradingView name, logo, wordmark or icon files anywhere in the UI or assets. Icons are original or from Lucide/Tabler (MIT), with the license noted in `crates/xview/assets/icons/LICENSE-icons.md`.
- Nothing in `Cargo.toml` or `Cargo.lock` may reference a TradingView plugin repo.
- Timeframes (exact labels): `1m 3m 5m 15m 30m 1h 2h 4h D W M`. The top bar shows `1m 3m 5m 15m 30m 1h 4h D W M`, and 2h lives in the chevron dropdown.
- Performance: 60 fps with ~2,000 visible candles and 100k loaded. `build_frame` only touches visible candles.
- Drawings: stored as exact `time_ms` + `price`; files are versioned JSON (`version: 1`) written atomically; corrupt files are moved aside, never overwritten.

## Review Focus

1. **Websocket drops mid-session** (Wi-Fi blip, sleep/wake): the live candle must resume, and candles missed during the gap must be backfilled, not left as a hole. → Task 9 (`reconnect_backoff_sequence`) and Task 3 (`merge_range_fills_gap_and_overwrites_overlap`).
2. **Corrupt or half-written drawings/settings file at startup**: the app must start, show an empty set for that symbol, and keep the bad file as `*.corrupt-<unix>`. It must never silently replace it. → Task 10 (`corrupt_drawings_file_is_moved_aside`).
3. **Switching symbol/timeframe while a history request is in flight**: a late response for the old series must not land in the new one. → Task 7 (`stale_token_is_rejected`) and Task 15 (load path checks the token).
4. **Messy CSVs** (unsorted rows, duplicate timestamps, blank lines, a header in the middle, `NaN`, high < low): load what's valid, sort and dedupe (last wins), and report skipped line numbers. Never panic. → Task 8 (`messy_csv_is_cleaned_and_reported`).
5. **Degenerate series** (0 candles, 1 candle, all prices equal): autofit, ticks and frame building must not divide by zero or produce NaN coordinates. → Task 4 (`autofit_flat_series_pads_range`) and Task 6 (`ticks_on_zero_range_are_finite`).

---

## File Structure

```
Cargo.toml                       workspace, shared deps, [patch.crates-io] copied from Zed
LICENSE                          GPLv3 text
README.md
crates/
  xv_core/src/{lib.rs, timeframe.rs, candle.rs, symbol.rs, series.rs, resample.rs}
  xv_data/src/{lib.rs, source.rs, registry.rs, cache.rs, token.rs}
  xv_source_csv/src/{lib.rs, parse.rs, source.rs}
  xv_source_binance/src/{lib.rs, rest.rs, parse.rs, ws.rs, search.rs}
    tests/fixtures/{klines.json, exchange_info.json, ws_kline.json}
  xv_drawings/src/{lib.rs, model.rs, hit.rs, set.rs, tool.rs, file.rs}
  xv_store/src/{lib.rs, atomic.rs, settings.rs, drawings.rs}
  xv_chart/src/{lib.rs, time_index.rs, viewport.rs, ticks.rs, colors.rs, frame.rs, state.rs, projector.rs}
  xview/
    src/{main.rs, assets.rs, theme.rs, workspace.rs, chart_view.rs, top_bar.rs,
         left_toolbar.rs, style_bar.rs, side_panel.rs, bottom_bar.rs, symbol_search.rs,
         loader.rs, persistence.rs}
    assets/icons/*.svg, assets/icons/LICENSE-icons.md
```

Dependency direction: `xv_core` ← `xv_data` ← sources. `xv_core` ← `xv_drawings` ← `xv_chart`. `xv_store` depends on `xv_core` and `xv_drawings`. `xview` depends on everything.

---

### Task 1: Workspace scaffold and a GPUI window that builds

**Files:**
- Create: `Cargo.toml`, `LICENSE`, `README.md` (stub: name, one-line description, build command), `crates/xview/Cargo.toml`, `crates/xview/src/main.rs`, and empty-lib crates `xv_core`, `xv_data`, `xv_source_csv`, `xv_source_binance`, `xv_drawings`, `xv_store`, `xv_chart` (each `Cargo.toml` + `src/lib.rs`).

**Interfaces:**
- Produces: workspace `[workspace.dependencies]` entries every later task uses: `gpui`, `gpui_platform`, `gpui_tokio` (git+rev above), `tokio = { version = "1", features = ["rt-multi-thread","macros","time","sync"] }`, `futures = "0.3"`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `chrono = "0.4"`, `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls","json"] }`, `tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }`, `dirs = "5"`, `thiserror = "2"`, `anyhow = "1"`, `tempfile = "3"` (dev).

- [ ] **Step 1: Write the workspace `Cargo.toml`**: `members = ["crates/*"]`, `resolver = "3"`, `[workspace.package] edition = "2024", license = "GPL-3.0-or-later", rust-version = "1.98"`. Copy **all** `[patch.crates-io]` entries from Zed's root `Cargo.toml` at the pinned rev. Cargo does not apply a git dependency's patches, and GPUI needs at least the `async-task` fork. Delete any entry that later produces an "unused patch" warning.
- [ ] **Step 2: `main.rs`** opens one 1280×800 window titled "XView" whose root view renders a white `div` with the text "XView". Use `gpui_platform::application().run(...)` and call `gpui_tokio::init(cx)` before opening the window.
- [ ] **Step 3: Build.** Run: `cargo build -p xview`. Expected: success. If shader compilation complains about `xcrun metal`, confirm `runtime_shaders` is enabled on `gpui_platform`.
- [ ] **Step 4: Launch and look.** Run `cargo run -p xview &`, then `sleep 8; screencapture -x /tmp/xv1.png` and view the PNG. Expected: a window showing "XView". If screen capture is blocked by macOS permissions, ask the owner to confirm the window opened. Kill the process.
- [ ] **Step 5: Commit.** `git add -A && git commit -m "chore: scaffold XView workspace with GPUI window"`

---

### Task 2: Core types: Timeframe, Candle, Symbol

**Files:**
- Create: `crates/xv_core/src/{timeframe.rs, candle.rs, symbol.rs}`, re-exported from `lib.rs`
- Test: inline `#[cfg(test)]` modules

**Interfaces:**
- Produces:
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)] pub enum Timeframe { M1, M3, M5, M15, M30, H1, H2, H4, D1, W1, Mo1 }`
  - `Timeframe::ALL: [Timeframe; 11]`, `Timeframe::TOP_BAR: [Timeframe; 10]` (all except H2)
  - `fn label(self) -> &'static str`, returning `"1m","3m","5m","15m","30m","1h","2h","4h","D","W","M"`
  - `fn nominal_ms(self) -> i64` (Mo1 = 30 days)
  - `fn bucket_start(self, ts_ms: i64, tz: TzOffset) -> i64` and `fn next_bucket(self, bucket_start_ms: i64, tz: TzOffset) -> i64`. Intraday buckets align to multiples since the epoch, shifted by tz. D1 aligns to local midnight. W1 aligns to Monday 00:00 local (Binance convention). Mo1 aligns to the 1st of the calendar month at 00:00 local.
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)] pub struct TzOffset { pub seconds: i32 }` with `TzOffset::UTC` and `fn local_now() -> TzOffset` (from `chrono::Local`)
  - `#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Candle { pub open_time_ms: i64, pub open: f64, pub high: f64, pub low: f64, pub close: f64, pub volume: f64 }`
  - `#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] pub struct SourceId(pub String)`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Symbol { pub source: SourceId, pub id: String, pub display: String, pub description: String, pub base: String, pub quote: String, pub tick_size: f64 }` with `fn key(&self) -> SymbolKey`
  - `#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] pub struct SymbolKey { pub source: SourceId, pub id: String }`

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn labels_match_spec() {
    let l: Vec<_> = Timeframe::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(l, ["1m","3m","5m","15m","30m","1h","2h","4h","D","W","M"]);
    assert!(!Timeframe::TOP_BAR.contains(&Timeframe::H2));
}
#[test] fn bucket_start_intraday_and_daily() {
    let t = 1_762_526_220_000; // 2025-11-07T14:37:00Z
    assert_eq!(Timeframe::H1.bucket_start(t, TzOffset::UTC), 1_762_524_000_000); // 14:00
    assert_eq!(Timeframe::D1.bucket_start(t, TzOffset::UTC), 1_762_473_600_000); // 00:00
    assert_eq!(Timeframe::D1.bucket_start(t, TzOffset { seconds: 5 * 3600 }), 1_762_455_600_000); // 00:00 UTC+5 = 19:00Z prev day
}
#[test] fn weekly_starts_monday_monthly_starts_first() {
    let t = 1_762_526_220_000; // Fri 2025-11-07
    assert_eq!(Timeframe::W1.bucket_start(t, TzOffset::UTC), 1_762_128_000_000); // Mon 2025-11-03
    let m = Timeframe::Mo1.bucket_start(t, TzOffset::UTC);
    assert_eq!(m, 1_761_955_200_000); // 2025-11-01
    assert_eq!(Timeframe::Mo1.next_bucket(m, TzOffset::UTC), 1_764_547_200_000); // 2025-12-01
}
```

- [ ] **Step 2: Run** `cargo test -p xv_core`. Expected: FAIL (types missing).
- [ ] **Step 3: Implement** the types above. Use `chrono` for W1 and Mo1 calendar math. Plain integer math is fine for intraday and D1.
- [ ] **Step 4: Run** `cargo test -p xv_core`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): timeframe, candle, symbol types`

---

### Task 3: Series and resampling

**Files:**
- Create: `crates/xv_core/src/{series.rs, resample.rs}`

**Interfaces:**
- Consumes: Task 2 types.
- Produces:
  - `#[derive(Clone, Debug)] pub struct Series { pub symbol: Symbol, pub timeframe: Timeframe, pub has_older: bool, candles: Vec<Candle> }`
  - `fn new(symbol, timeframe, candles: Vec<Candle>, has_older: bool) -> Series` (sorts, dedupes by `open_time_ms`, last wins)
  - `fn candles(&self) -> &[Candle]`, `fn len(&self) -> usize`, `fn last(&self) -> Option<&Candle>`
  - `fn prepend_older(&mut self, older: Vec<Candle>, has_more: bool) -> usize` (returns the number of candles actually added; drops overlaps)
  - `fn apply_update(&mut self, c: Candle) -> UpdateKind`, with `pub enum UpdateKind { Replaced, Appended, Ignored }` (Ignored when older than the last candle)
  - `fn merge_range(&mut self, cs: Vec<Candle>)`: inserts or overwrites by timestamp, keeping the order (used for reconnect backfill)
  - `pub fn resample(candles: &[Candle], to: Timeframe, tz: TzOffset) -> Vec<Candle>`: OHLC = first open, max high, min low, last close, summed volume; bucket key = `to.bucket_start(open_time_ms, tz)`

- [ ] **Step 1: Write failing tests**: `prepend_older_drops_overlap_and_counts` (series 10..20, prepend 5..12 → returns 5, len 15); `apply_update_replaces_last_and_appends_next` (same ts → Replaced with new close; next ts → Appended; older ts → Ignored); `merge_range_fills_gap_and_overwrites_overlap` (series [0,1,2,6,7] in minutes, merge [2',3,4,5] → [0,1,2',3,4,5,6,7] where 2' has the new close); `resample_1m_to_5m` (10 one-minute candles from 00:00 with open=i, high=i+1, low=i-1, close=i+0.5, vol=1 → 2 candles: first {open 0, high 5, low -1, close 4.5, vol 5}); `resample_respects_tz_for_daily` (candles at 18:00Z and 20:00Z with tz UTC+5 land in different D1 buckets).
- [ ] **Step 2: Run** `cargo test -p xv_core`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_core`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): series merging and resampling`

---

### Task 4: Time↔index mapping and Viewport

**Files:**
- Create: `crates/xv_chart/src/{time_index.rs, viewport.rs}`

**Interfaces:**
- Consumes: `Candle`, `Timeframe`, `TzOffset`.
- Produces:
  - `pub fn time_to_index(candles: &[Candle], tf: Timeframe, ts_ms: i64) -> f64` and `pub fn index_to_time(candles: &[Candle], tf: Timeframe, idx: f64) -> i64`. Spec anchor rule: for bar `i` with start `t_i`, `end_i = min(tf.next_bucket(t_i, UTC), t_{i+1})`. If `t_i ≤ ts < end_i`, the index is `i + (ts−t_i)/(end_i−t_i)`. If `end_i ≤ ts < t_{i+1}` (a gap), the index is `i+1`. Before the first bar: `(ts−t_0)/nominal`. After the last bar's start: `(n−1) + (ts−t_last)/nominal`. Empty candles: `ts / nominal`, so the mapping stays defined. `index_to_time` is the exact inverse on non-gap times.
  - `#[derive(Copy, Clone, Debug, PartialEq)] pub struct PriceRange { pub min: f64, pub max: f64 }`
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum PriceScale { Linear }` (log/percent arrive later)
  - `#[derive(Clone, Debug)] pub struct Viewport { pub width: f32, pub height: f32, pub bar_spacing: f32, pub right_index: f64, pub price: PriceRange, pub auto_scale: bool, pub scale: PriceScale }`. `width`/`height` are the plot area; `right_index` is the fractional bar index at the plot's right edge.
  - Constants: `DEFAULT_BAR_SPACING = 8.0`, `MIN_BAR_SPACING = 0.5`, `MAX_BAR_SPACING = 60.0`, `RIGHT_MARGIN_BARS = 5.0`, `AUTOFIT_PAD = 0.1` (10% top and bottom), `VOLUME_FRACTION = 0.2`
  - Methods: `x_of_index(&self, f64) -> f32` (`width − (right_index − idx)·bar_spacing`), `index_of_x(&self, f32) -> f64`, `y_of_price(&self, f64) -> f32`, `price_of_y(&self, f32) -> f64`, `visible_range(&self, len: usize) -> Range<usize>` (clamped, one extra bar each side), `pan_pixels(&mut self, dx: f32)`, `zoom_at(&mut self, factor: f32, anchor_x: f32)` (index under `anchor_x` stays fixed; spacing clamped), `scale_price(&mut self, factor: f32)` (around the center; sets `auto_scale = false`), `scroll_price(&mut self, dy_px: f32)` (sets `auto_scale = false`), `autofit(&mut self, candles: &[Candle])` (min low / max high of visible candles plus padding; sets `auto_scale = true`), `scroll_to_latest(&mut self, len: usize)` (`right_index = len−1 + RIGHT_MARGIN_BARS`), `is_following(&self, len: usize) -> bool` (last bar within the plot)

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn fractional_index_inside_bar() { // 1h bars at 13:00,14:00,15:00
    let c = hourly(&[13, 14, 15]);
    let ts = at(14, 37);
    assert!((time_to_index(&c, Timeframe::H1, ts) - (1.0 + 37.0 / 60.0)).abs() < 1e-9);
}
#[test] fn gap_maps_to_next_bar_start() { // bars Fri 23:00 (1h), next bar Mon 00:00
    let c = vec![bar(fri(23)), bar(mon(0))];
    assert_eq!(time_to_index(&c, Timeframe::H1, sat(12)), 1.0);
}
#[test] fn extrapolates_both_sides_and_round_trips() {
    let c = hourly(&[13, 14, 15]);
    assert!((time_to_index(&c, Timeframe::H1, at(17, 30)) - 4.5).abs() < 1e-9);
    assert!((time_to_index(&c, Timeframe::H1, at(11, 0)) + 2.0).abs() < 1e-9);
    for ts in [at(13, 5), at(14, 59), at(20, 0), at(9, 15)] {
        assert_eq!(index_to_time(&c, Timeframe::H1, time_to_index(&c, Timeframe::H1, ts)), ts);
    }
}
#[test] fn zoom_keeps_index_under_cursor() {
    let mut v = vp(800.0, 400.0); let before = v.index_of_x(300.0);
    v.zoom_at(1.5, 300.0);
    assert!((v.index_of_x(300.0) - before).abs() < 1e-6);
    v.zoom_at(1000.0, 300.0); assert_eq!(v.bar_spacing, MAX_BAR_SPACING);
}
#[test] fn autofit_flat_series_pads_range() {
    let mut v = vp(800.0, 400.0);
    v.autofit(&[flat(100.0); 3]); assert!(v.price.max > v.price.min && v.price.min.is_finite());
    v.autofit(&[flat(0.0)]); assert!(v.price.max > v.price.min);
    v.autofit(&[]); assert!(v.price.max > v.price.min); // keeps previous or default 0..1
}
```
Helpers (`hourly`, `bar`, `at`, `fri`/`sat`/`mon`, `vp`, `flat`) are local to the test module. A flat series pads by ±1% of the price, or ±1.0 when the price is 0.

- [ ] **Step 2: Run** `cargo test -p xv_chart`. Expected: FAIL.
- [ ] **Step 3: Implement.** `time_to_index` uses binary search on `open_time_ms`.
- [ ] **Step 4: Run** `cargo test -p xv_chart`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(chart): time-index mapping and viewport math`

---

### Task 5: Drawings model, hit-testing, undo set, placement, file format

**Files:**
- Create: `crates/xv_drawings/src/{model.rs, hit.rs, set.rs, tool.rs, file.rs}`

**Interfaces:**
- Consumes: `serde`.
- Produces:
  - `#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Anchor { pub time_ms: i64, pub price: f64 }`
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)] pub struct DrawingId(pub u64)`
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(tag = "type", rename_all = "snake_case")] pub enum DrawingKind { TrendLine { a: Anchor, b: Anchor }, Ray { a: Anchor, b: Anchor }, HorizontalLine { price: f64 }, VerticalLine { time_ms: i64 }, Rectangle { a: Anchor, b: Anchor } }`
  - `#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Style { pub color: u32, pub width: u8 }` (`color` is 0xRRGGBBAA; `width` is 1..=4); `pub const PALETTE: [u32; 8]` (XView's own 8 colors; first = default blue `0x2962FFFF` is fine, pick the rest); `Style::default()` = `PALETTE[0]`, width 2
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Drawing { pub id: DrawingId, pub kind: DrawingKind, pub style: Style }`
  - `pub trait Projector { fn to_screen(&self, a: Anchor) -> (f32, f32); fn to_anchor(&self, p: (f32, f32)) -> Anchor; fn width(&self) -> f32; fn height(&self) -> f32; }`
  - `pub enum Hit { Body, Handle(usize) }`; `pub const HIT_TOLERANCE: f32 = 6.0`
  - `pub fn hit_test(ds: &[Drawing], p: (f32, f32), proj: &dyn Projector) -> Option<(DrawingId, Hit)>` (handles before body, last drawing wins; a rectangle body = inside or on its border; a ray only to the right of `a` along a→b)
  - `pub fn handles(d: &Drawing, proj: &dyn Projector) -> Vec<(f32, f32)>` (horizontal line: one handle at x = width/2; vertical: y = height/2)
  - `impl Drawing { pub fn translated(&self, dx: f32, dy: f32, proj: &dyn Projector) -> Drawing; pub fn with_handle(&self, idx: usize, to: Anchor) -> Drawing }`
  - `pub struct DrawingSet` with `new(Vec<Drawing>)`, `all(&self) -> &[Drawing]`, `get(id)`, `add(kind, style) -> DrawingId`, `replace(Drawing)`, `remove(id)`, `clear()`, `undo() -> bool`, `redo() -> bool`, and `begin_gesture()` / `end_gesture()` so a whole drag is one undo step. Undo depth is 200; the next id is max existing + 1.
  - `#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Tool { Cursor, TrendLine, Ray, HorizontalLine, VerticalLine, Rectangle }`; `pub enum PlaceResult { Pending(Anchor), Done(DrawingKind) }`; `pub fn place_click(tool: Tool, first: Option<Anchor>, at: Anchor) -> Option<PlaceResult>` (None for Cursor; horizontal/vertical finish on the first click; the others need two)
  - `#[derive(Serialize, Deserialize)] pub struct DrawingsFile { pub version: u32, pub symbol: String, pub drawings: Vec<Drawing> }`; `pub const FILE_VERSION: u32 = 1`; `pub fn to_json(&DrawingsFile) -> String` (pretty); `pub fn from_json(&str) -> Result<DrawingsFile, DrawingsError>`; `#[derive(thiserror::Error, Debug)] pub enum DrawingsError { #[error("parse: {0}")] Parse(String), #[error("unsupported version {0}")] UnsupportedVersion(u32) }`

- [ ] **Step 1: Write failing tests** using a test `Projector` where x = time_ms/1000 and y = 1000 − price:
  - `hit_trendline_body_and_handle`: line (0,100)→(100,0) in screen space; point (50,50) → Body; (1,99) → Handle(0); (50,70) → None.
  - `ray_hits_only_forward`: ray a=(100,500) b=(200,500); (400,500) → Body; (50,500) → None.
  - `rectangle_hits_inside`: corners (100,100),(200,200); (150,150) → Body.
  - `topmost_wins`: two overlapping lines → the id of the later one.
  - `undo_redo_gesture`: add, then a gesture with 3 `replace` calls, then `undo` restores the post-add state in one step; `redo` reapplies.
  - `place_click_flow`: TrendLine first click → Pending; second → Done(TrendLine{a,b}); HorizontalLine one click → Done.
  - `file_round_trip_and_version_guard`: to_json → from_json equal; `{"version":2,...}` → `UnsupportedVersion(2)`; garbage → `Parse`.
- [ ] **Step 2: Run** `cargo test -p xv_drawings`. Expected: FAIL.
- [ ] **Step 3: Implement.** Distance to a segment uses the standard point-to-segment projection.
- [ ] **Step 4: Run** `cargo test -p xv_drawings`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(drawings): model, hit-testing, undo, placement, file format`

---

### Task 6: Axis ticks, formatting, chart colors, and frame building

**Files:**
- Create: `crates/xv_chart/src/{ticks.rs, colors.rs, projector.rs, frame.rs}`

**Interfaces:**
- Consumes: Viewport and time_index (Task 4), `Series` (Task 3), drawings (Task 5).
- Produces:
  - `pub fn price_ticks(r: PriceRange, height: f32, tick_size: f64) -> Vec<f64>`: steps from {1, 2, 2.5, 5}×10^k, aiming for a 60–90 px spacing, with step ≥ tick_size, always finite.
  - `pub fn format_price(p: f64, tick_size: f64) -> String`: decimals = decimals of tick_size, `,` thousands separator (e.g. `4,092.0` for tick 0.1, `1.33470` for tick 0.00001).
  - `pub struct TimeTick { pub index: usize, pub label: String, pub major: bool }`; `pub fn time_ticks(candles: &[Candle], tf: Timeframe, vp: &Viewport, tz: TzOffset) -> Vec<TimeTick>`: about one label per 90 px. A label is `HH:MM` intraday, the day number at a day change (major), `Mon` at a month change (major), and `YYYY` at a year change (major).
  - `pub fn format_crosshair_time(ts_ms: i64, tf: Timeframe, tz: TzOffset) -> String`: `"Mon 09 Nov '26  13:40"` intraday, `"Mon 09 Nov '26"` for D/W/M.
  - `#[derive(Clone, Debug)] pub struct ChartColors { pub background, grid, text, muted_text, up, down, up_volume, down_volume, crosshair, axis_label_bg, axis_label_text, last_price_up, last_price_down, selection: u32 }` with `fn light() -> Self` and `fn dark() -> Self` (XView's own palette, not TradingView's exact hex values).
  - `pub struct ChartProjector<'a> { pub candles: &'a [Candle], pub tf: Timeframe, pub vp: &'a Viewport }` implementing `xv_drawings::Projector`.
  - `#[derive(Copy, Clone, Debug, PartialEq)] pub struct RectF { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }`
  - `pub struct ChartLayout { pub plot: RectF, pub price_axis: RectF, pub time_axis: RectF }`; `ChartLayout::new(width: f32, height: f32)` with `PRICE_AXIS_WIDTH = 72.0` and `TIME_AXIS_HEIGHT = 28.0`
  - `pub enum TextAlign { Left, Center, Right }`
  - `#[derive(Clone, Debug, PartialEq)] pub enum Prim { Rect { r: RectF, color: u32 }, StrokeRect { r: RectF, color: u32, width: f32 }, Line { a: (f32, f32), b: (f32, f32), color: u32, width: f32, dashed: bool }, Text { pos: (f32, f32), text: String, color: u32, align: TextAlign, bg: Option<u32> }, Handle { center: (f32, f32), color: u32 } }`. Coordinates are relative to the chart element origin.
  - `pub struct Legend { pub title: String, pub o: String, pub h: String, pub l: String, pub c: String, pub v: String, pub change: String, pub up: bool }` (title e.g. `"BTCUSDT · 1h · Binance"`; values come from the candle under the crosshair, else the last candle; `change` = close vs previous close, e.g. `+1.1 (+0.03%)`)
  - `pub struct Frame { pub plot: Vec<Prim>, pub price_axis: Vec<Prim>, pub time_axis: Vec<Prim>, pub legend: Legend }`. The painter clips `plot` to `layout.plot`.
  - `pub struct FrameInput<'a> { pub series: &'a Series, pub vp: &'a Viewport, pub layout: &'a ChartLayout, pub crosshair: Option<(f32, f32)>, pub drawings: &'a [Drawing], pub drawings_hidden: bool, pub selected: Option<DrawingId>, pub colors: &'a ChartColors, pub tz: TzOffset, pub source_name: &'a str }`; `pub fn build_frame(i: &FrameInput) -> Frame`
  - Frame contents, in order: grid lines at the ticks, then volume bars (bottom `VOLUME_FRACTION` of the plot, scaled to the max visible volume), then candles (wick line + body rect with a minimum height of 1 px; when `bar_spacing < 2.0`, a single high–low line per candle), then drawings (ray clipped at the plot's right edge; horizontal line full width plus a price label in `price_axis`; rectangle fill = style color at 20% alpha plus border), then selection handles, then the dashed last-price line plus its colored label in `price_axis`, then the crosshair (dashed lines plus price and time labels).

- [ ] **Step 1: Write failing tests**: `price_ticks_are_nice` (range 4030..4160, height 800 → steps of 10.0, all within range); `ticks_on_zero_range_are_finite` (min==max → non-empty, all finite); `format_price_cases` (`format_price(4092.0, 0.1) == "4,092.0"`, `format_price(1.3347, 0.00001) == "1.33470"`, `format_price(67412.5, 0.01) == "67,412.50"`); `crosshair_time_format` (2026-11-09 13:40 UTC, M5 → `"Mon 09 Nov '26  13:40"`); `frame_only_touches_visible` (100k-candle series, viewport showing ~200 → the candle body count in `plot` ≤ 205); `frame_has_no_nan` (every coordinate in the frame is finite for 0-, 1- and flat-candle series); `ray_is_clipped_to_plot` (the ray's end x == `layout.plot.w`).
- [ ] **Step 2: Run** `cargo test -p xv_chart`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_chart`. Expected: PASS.
- [ ] **Step 5: Perf check.** Add `#[test] #[ignore] fn build_frame_perf()`: 100k candles, ~2,000 visible, 1,000 iterations; assert the average is < 2 ms. Run `cargo test -p xv_chart --release -- --ignored build_frame_perf`. Expected: PASS.
- [ ] **Step 6: Commit** `feat(chart): ticks, colors, frame builder`

---

### Task 7: Data source trait, registry, cache, load tokens

**Files:**
- Create: `crates/xv_data/src/{source.rs, registry.rs, cache.rs, token.rs}`

**Interfaces:**
- Consumes: `xv_core`.
- Produces:
  - `#[derive(thiserror::Error, Debug, Clone, PartialEq)] pub enum DataError { #[error("{host} is not available from your network (HTTP {status})")] RegionBlocked { host: String, status: u16 }, #[error("network: {0}")] Network(String), #[error("not found: {0}")] NotFound(String), #[error("unsupported: {0}")] Unsupported(String), #[error("parse: {0}")] Parse(String) }`
  - `pub struct HistoryPage { pub candles: Vec<Candle>, pub has_more: bool }`
  - `#[derive(Clone, Debug, PartialEq)] pub enum CandleUpdate { Candle(Candle), Disconnected, Reconnected }`
  - `pub trait DataSource: Send + Sync + 'static { fn id(&self) -> SourceId; fn name(&self) -> &str; fn search_symbols(&self, query: &str) -> BoxFuture<'static, Result<Vec<Symbol>, DataError>>; fn supported_timeframes(&self, symbol: &Symbol) -> Vec<Timeframe>; fn history(&self, symbol: &Symbol, tf: Timeframe, end_before_ms: Option<i64>, limit: usize) -> BoxFuture<'static, Result<HistoryPage, DataError>>; fn subscribe(&self, symbol: &Symbol, tf: Timeframe) -> Option<BoxStream<'static, CandleUpdate>>; }` (futures types from `futures::future`/`futures::stream`). Futures may require a tokio runtime; the app runs them via `gpui_tokio::Tokio::spawn`.
  - `pub struct SourceRegistry` with `new()`, `add(Arc<dyn DataSource>)`, `get(&SourceId) -> Option<Arc<dyn DataSource>>`, `all() -> Vec<Arc<dyn DataSource>>`, `default_source() -> Option<Arc<dyn DataSource>>` (the first added)
  - `pub struct SeriesCache` (LRU, capacity 32) with `get(&SymbolKey, Timeframe) -> Option<&Series>`, `get_mut(...)`, `insert(Series)`
  - `pub struct LoadToken(u64)` and `pub struct LoadGuard` with `next(&mut self) -> LoadToken` and `is_current(&self, &LoadToken) -> bool`

- [ ] **Step 1: Write failing tests**: `registry_default_is_first`; `cache_evicts_least_recent` (insert 33 → the first is gone, and the most recently `get` one stays); `stale_token_is_rejected` (t1 = next, t2 = next → `!is_current(t1)`, `is_current(t2)`); `fake_source_satisfies_trait` (an in-test `FakeSource` returning fixed candles, polled with `futures::executor::block_on`).
- [ ] **Step 2: Run** `cargo test -p xv_data`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_data`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(data): source trait, registry, cache, load tokens`

---

### Task 8: CSV source

**Files:**
- Create: `crates/xv_source_csv/src/{parse.rs, source.rs}`

**Interfaces:**
- Consumes: `DataSource` (Task 7), `resample` (Task 3).
- Produces:
  - `pub struct ParsedCsv { pub candles: Vec<Candle>, pub skipped: Vec<(usize, String)> }` (1-based line numbers)
  - `pub fn parse_csv(text: &str) -> Result<ParsedCsv, CsvError>`: columns `time,open,high,low,close[,volume]`, comma- or semicolon-separated, header optional (skipped anywhere if non-numeric), blank lines ignored. A row is rejected when fields are missing or a value is non-finite. When high < low, the two are swapped *(plan's call: the data was probably exported with the columns swapped)*. Rows are sorted and deduped (last wins). The result is `Err(CsvError::NoRows)` when nothing is valid.
  - `pub fn parse_time(s: &str) -> Option<i64>`: an integer < 100_000_000_000 is seconds (×1000), else ms. RFC 3339 with an offset is honored; `YYYY-MM-DD HH:MM[:SS]` and `YYYY-MM-DDTHH:MM[:SS]` with no zone are UTC; `YYYY-MM-DD` alone is UTC midnight.
  - `pub fn infer_timeframe(candles: &[Candle]) -> Option<Timeframe>`: the most common gap matched to the nearest `nominal_ms` within 10%, where any gap of 28–31 days → Mo1. Needs ≥ 2 candles.
  - `pub const CSV_SOURCE_ID: &str = "csv"`
  - `pub struct CsvSource` with `new()` and `load_file(&self, path: &Path) -> Result<Symbol, CsvError>`. The Symbol's `id` = the absolute path, `display` = the file stem uppercased, `tick_size` inferred from the max decimals seen (capped at 8).
  - `impl DataSource for CsvSource`: `name()` = "CSV". `supported_timeframes` = all `≥ base`. `history` resamples from the base (cached per tf) and returns the up-to-`limit` candles before `end_before`. `subscribe` → `None`. `search_symbols` matches loaded files by display name. An unloaded id → `NotFound`.
  - `#[derive(thiserror::Error, Debug)] pub enum CsvError { #[error("io: {0}")] Io(#[from] std::io::Error), #[error("no valid rows")] NoRows, #[error("cannot infer timeframe")] UnknownTimeframe }`

- [ ] **Step 1: Write failing tests**: `parse_time_variants` (`"1762526220"` → 1762526220000; `"1762526220000"` same; `"2025-11-07T14:37:00Z"` same; `"2025-11-07 14:37"` same; `"2025-11-07T19:37:00+05:00"` same; `"2025-11-07"` → midnight); `messy_csv_is_cleaned_and_reported` (input with a header, a blank line, a duplicate ts, out-of-order rows, a `NaN` close on line 6 and a short row on line 8 → sorted unique candles, `skipped` lines == [6, 8], dup keeps the later row); `infer_hourly_with_weekend_gaps` (hourly bars Mon–Fri with weekend gaps → H1); `history_resamples_and_pages` (2,000 1m rows → `history(M5, None, 100)` returns 100 candles ending at the last bucket with `has_more == true`; `supported_timeframes` excludes nothing below M1 and includes Mo1).
- [ ] **Step 2: Run** `cargo test -p xv_source_csv`. Expected: FAIL.
- [ ] **Step 3: Implement.** Plain `str::split` parsing is enough; no csv crate needed.
- [ ] **Step 4: Run** `cargo test -p xv_source_csv`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(csv): CSV data source with resampling`

---

### Task 9: Binance source

**Files:**
- Create: `crates/xv_source_binance/src/{rest.rs, parse.rs, ws.rs, search.rs}`, `crates/xv_source_binance/tests/fixtures/{klines.json, exchange_info.json, ws_kline.json}` (small hand-trimmed real responses: 3 klines; exchangeInfo with 3 symbols, one with `status: "BREAK"`; one ws kline message)

**Interfaces:**
- Consumes: `DataSource` (Task 7).
- Produces:
  - `pub const BINANCE_SOURCE_ID: &str = "binance"`
  - `pub const REST_HOSTS: [&str; 2] = ["https://data-api.binance.vision", "https://api.binance.com"]`
  - `pub const WS_HOSTS: [&str; 2] = ["wss://data-stream.binance.vision", "wss://stream.binance.com:9443"]`
  - `pub fn interval(tf: Timeframe) -> &'static str`, returning `"1m","3m","5m","15m","30m","1h","2h","4h","1d","1w","1M"`
  - `pub fn parse_klines(json: &str) -> Result<Vec<Candle>, DataError>` (array of arrays; prices come as strings)
  - `pub fn parse_exchange_info(json: &str) -> Result<Vec<Symbol>, DataError>` (only `status == "TRADING"`; `tick_size` from the `PRICE_FILTER` `tickSize`; `display` = the symbol, `description` = `"{base} / {quote}"`)
  - `pub fn parse_ws_kline(msg: &str) -> Result<Candle, DataError>` (from the `k` object: `t, o, h, l, c, v`)
  - `pub fn map_status(status: u16, host: &str) -> Option<DataError>` (451 or 403 → `RegionBlocked`; other non-2xx → `Network`)
  - `pub fn rank_search(symbols: &[Symbol], q: &str) -> Vec<Symbol>`: case-insensitive; tiers: exact id, then id prefix, then base prefix, then id contains; within a tier, quote priority `USDT, USDC, FDUSD, BTC`, then others, then alphabetical; at most 50
  - `pub fn backoff_secs(attempt: u32) -> u64`: 1, 2, 4, 8, 16, then 30 forever
  - `pub struct BinanceSource` with `new() -> Self` (reqwest client with a 10 s timeout and `User-Agent: XView/<version>`). It implements `DataSource`. `name()` = "Binance". `history` → `GET {host}/api/v3/klines?symbol=&interval=&limit=min(limit,1000)[&endTime=end_before-1]`, trying hosts in order; it moves to the next host on a connect error or `RegionBlocked` and returns the last error if all fail. `has_more = returned == requested`. `search_symbols` fetches `/api/v3/exchangeInfo` once, caches it in a `tokio::sync::OnceCell`, then calls `rank_search`. `supported_timeframes` = `Timeframe::ALL`. `subscribe` → a stream from `{ws_host}/ws/{lowercase symbol}@kline_{interval}`; on close or error it yields `Disconnected`, waits `backoff_secs(n)`, reconnects (falling back across `WS_HOSTS`), yields `Reconnected`, and resets the attempt count after a message arrives.

- [ ] **Step 1: Write failing tests**: `parses_klines_fixture` (3 candles; assert the exact OHLCV and `open_time_ms` of the first fixture row); `exchange_info_filters_non_trading_and_reads_tick` (2 symbols, BTCUSDT `tick_size == 0.01`); `parses_ws_kline`; `status_451_is_region_blocked` (`map_status(451, "data-api.binance.vision") == Some(RegionBlocked{..status: 451})`); `search_ranking` (symbols (id, base, quote): `ETHBTC/ETH/BTC`, `ETHUSDT/ETH/USDT`, `SETHUSDT/SETH/USDT`, `BTCUSDT/BTC/USDT`, `WBETHETH/WBETH/ETH`; `rank_search(syms, "eth")` ids == `["ETHUSDT", "ETHBTC", "SETHUSDT", "WBETHETH"]`); `reconnect_backoff_sequence` (`[1,2,4,8,16,30,30]`); `interval_strings`.
- [ ] **Step 2: Run** `cargo test -p xv_source_binance`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_source_binance`. Expected: PASS.
- [ ] **Step 5: Live smoke test** as `#[tokio::test] #[ignore] async fn live_btcusdt_history()`, which fetches 10 1h candles and gets one ws update within 70 s. Run `cargo test -p xv_source_binance -- --ignored live_btcusdt_history`. Expected: PASS (needs network).
- [ ] **Step 6: Commit** `feat(binance): REST + websocket data source with host fallback`

---

### Task 10: Persistence store

**Files:**
- Create: `crates/xv_store/src/{atomic.rs, settings.rs, drawings.rs}`

**Interfaces:**
- Consumes: `Timeframe`, `SymbolKey`, `TzOffset`; `xv_drawings::{Drawing, DrawingsFile, to_json, from_json, FILE_VERSION}`.
- Produces:
  - `pub fn data_dir() -> PathBuf`: `$XVIEW_DATA_DIR` if set, else `dirs::data_dir()/XView`; created if missing.
  - `pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>` (temp file in the same dir, `sync_all`, `rename`)
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Settings { pub version: u32, pub last_symbol: Option<SymbolKey>, pub last_timeframe: Timeframe, pub window: Option<WindowRect>, pub theme: ThemeMode, pub use_utc: bool }`. Default: `version 1`, `None`, `H1`, `None`, `Light`, `false`. All fields use `#[serde(default)]` so new fields don't break old files.
  - `#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct WindowRect { pub x: f32, pub y: f32, pub w: f32, pub h: f32 }`; `#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)] pub enum ThemeMode { #[default] Light, Dark }`
  - `pub fn load_settings(dir: &Path) -> Settings` (missing → default; corrupt → moved to `settings.json.corrupt-<unix>`, default returned)
  - `pub fn save_settings(dir: &Path, s: &Settings) -> io::Result<()>`
  - `pub fn drawings_path(dir: &Path, key: &SymbolKey) -> PathBuf`: `dir/drawings/<source>/<sanitized id>-<16-hex FNV-1a-64 of id>.json`, where sanitize keeps `[A-Za-z0-9._-]`, maps everything else to `_`, and truncates to 64 chars.
  - `pub enum LoadedDrawings { Ok(Vec<Drawing>), Missing, Corrupt { moved_to: PathBuf } }`; `pub fn load_drawings(dir: &Path, key: &SymbolKey) -> LoadedDrawings`. A parse or unsupported-version failure moves the file to `<name>.corrupt-<unix>`.
  - `pub fn save_drawings(dir: &Path, key: &SymbolKey, ds: &[Drawing]) -> io::Result<()>`

- [ ] **Step 1: Write failing tests** (each uses a `tempfile::TempDir`, not the env var): `settings_round_trip`; `settings_missing_fields_use_defaults` (`{"version":1}` → default fields); `corrupt_settings_moved_aside`; `drawings_round_trip`; `corrupt_drawings_file_is_moved_aside` (write `"{garbage"` → `Corrupt{moved_to}` exists with the garbage content, and the original path no longer exists); `paths_are_distinct_for_similar_ids` (`"/a/b.csv"` vs `"/a_b.csv"` → different paths); `atomic_write_leaves_no_temp_files`.
- [ ] **Step 2: Run** `cargo test -p xv_store`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_store`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(store): atomic settings and drawings persistence`

---

### Task 11: ChartState: navigation input

**Files:**
- Create: `crates/xv_chart/src/state.rs`

**Interfaces:**
- Consumes: Viewport, ChartLayout, build_frame, Series, DrawingSet, Tool.
- Produces:
  - `#[derive(Copy, Clone, Debug, Default)] pub struct Mods { pub shift: bool, pub cmd: bool, pub alt: bool }`
  - `#[derive(Clone, Debug, PartialEq)] pub enum ChartEffect { NeedOlder { end_before_ms: i64 }, DrawingsChanged, ToolFinished, SelectionChanged }`
  - `pub enum ChartKey { Delete, Escape, Undo, Redo }`
  - `pub const OLDER_THRESHOLD_BARS: f64 = 50.0`
  - `pub struct ChartState { pub series: Series, pub vp: Viewport, pub layout: ChartLayout, pub crosshair: Option<(f32, f32)>, pub drawings: DrawingSet, pub drawings_hidden: bool, pub drawings_locked: bool, pub magnet: bool, pub tool: Tool, pub selected: Option<DrawingId>, pub loading_older: bool, pub tz: TzOffset, /* private drag state */ }`
  - `fn new(series: Series, width: f32, height: f32) -> Self` (scrolls to latest, autofits)
  - `fn set_size(&mut self, w: f32, h: f32)`, `fn set_series(&mut self, s: Series)` (resets the viewport to the latest; keeps `bar_spacing`), `fn set_drawings(&mut self, ds: Vec<Drawing>)`
  - `fn mouse_down(&mut self, p: (f32, f32), m: Mods, click_count: usize) -> Vec<ChartEffect>`, `fn mouse_move(&mut self, p, m, pressed: bool) -> Vec<ChartEffect>`, `fn mouse_up(&mut self, p, m) -> Vec<ChartEffect>`, `fn mouse_left(&mut self)` (clears the crosshair)
  - `fn scroll(&mut self, p, dx: f32, dy: f32, m: Mods) -> Vec<ChartEffect>`, `fn pinch(&mut self, p, delta: f32) -> Vec<ChartEffect>`
  - `fn apply_update(&mut self, c: Candle)` (if it was following before the update, it keeps following; autofit if `auto_scale`)
  - `fn prepend_older(&mut self, page_candles: Vec<Candle>, has_more: bool)` (`right_index += added`, so the view doesn't jump; clears `loading_older`)
  - `fn merge_backfill(&mut self, cs: Vec<Candle>)`
  - `fn show_span(&mut self, span_ms: Option<i64>)` (bottom range bar: fit the last `span_ms` of data, or all data for `None`, by setting `bar_spacing` and `right_index`)
  - `fn frame(&self, colors: &ChartColors, source_name: &str) -> Frame`
- Behavior (this task covers navigation only; drawing gestures are Task 12):
  - Plot drag pans. Price-axis drag vertically scales (`factor = exp(dy * 0.005)`). Price-axis double-click → autofit. Time-axis drag horizontally zooms anchored at the plot's right edge.
  - Scroll: `|dx| > |dy|` → pan by dx. Otherwise with `alt` held → `scroll_price(dy)`. Otherwise zoom at the cursor with `factor = exp(-dy * 0.002)`.
  - Pinch → `zoom_at(1 + delta, x)`.
  - After any navigation: if `auto_scale`, autofit. If the left visible index < `OLDER_THRESHOLD_BARS`, `series.has_older` is set, and nothing is loading → set `loading_older` and emit `NeedOlder { end_before_ms: first candle ts }` once.

- [ ] **Step 1: Write failing tests**: `drag_in_plot_pans`; `vertical_scroll_zooms_at_cursor` (index under the cursor unchanged); `horizontal_scroll_pans`; `alt_scroll_moves_price_and_disables_autoscale`; `price_axis_double_click_restores_autofit`; `need_older_emitted_once_near_left_edge` (pan far left → exactly one `NeedOlder` across repeated moves until `prepend_older` is called); `prepend_keeps_view_stable` (the time under x=400 is unchanged after prepending 500 candles); `live_update_follows_when_at_latest` (at latest + appended candle → still following; scrolled back → `right_index` unchanged); `show_span_fits_last_day` (5m series, span 1 day → about 288 bars visible, ±2).
- [ ] **Step 2: Run** `cargo test -p xv_chart state`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_chart`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(chart): navigation state machine`

---

### Task 12: ChartState: drawing gestures

**Files:**
- Modify: `crates/xv_chart/src/state.rs`
- Create: `crates/xv_chart/src/snap.rs`

**Interfaces:**
- Consumes: Task 5 (`place_click`, `hit_test`, `translated`, `with_handle`, `DrawingSet` gestures), Task 11.
- Produces:
  - `pub fn snap_anchor(candles: &[Candle], tf: Timeframe, vp: &Viewport, p: (f32, f32)) -> Anchor`: nearest bar by x, then the nearest of O/H/L/C by y; `time_ms` = that bar's open time.
  - `ChartState::set_tool(&mut self, t: Tool)`, `set_selected_style(&mut self, s: Style) -> Vec<ChartEffect>`, `clear_drawings(&mut self) -> Vec<ChartEffect>`, `key(&mut self, k: ChartKey) -> Vec<ChartEffect>`, `selected_screen_bounds(&self) -> Option<RectF>` (for positioning the style bar)
- Behavior:
  - With a drawing tool active, a plot click calls `place_click`; the anchor is `snap_anchor` if `m.cmd || self.magnet`, else the projector anchor. Click-drag-release also places a 2-point drawing (press = a, release = b, when moved > 3 px).
  - On Done → add with `Style::default()`, select it, emit `DrawingsChanged`. Unless `m.shift` is held → tool = Cursor and emit `ToolFinished`.
  - With Cursor: mouse-down on a hit (unless locked) → select it and begin a gesture. Body drag → `translated` from the drag-start snapshot (not cumulative, so there's no drift). Handle drag → `with_handle`. Mouse-up → end the gesture and emit `DrawingsChanged` if anything moved. A miss → deselect and pan.
  - Keys: Delete removes the selected drawing. Escape cancels a pending placement, else deselects. Undo/Redo call the `DrawingSet` methods. Each emits `DrawingsChanged` when the set changed.
  - Hidden drawings are neither hit-tested nor rendered. Locked drawings are rendered but not hit-tested.

- [ ] **Step 1: Write failing tests**: `two_click_trendline` (tool TrendLine, click, click → 1 drawing, tool back to Cursor, `ToolFinished` emitted); `shift_keeps_tool`; `drag_places_rectangle`; `cmd_snaps_to_high` (click near a candle's high with cmd → anchor price == high exactly); `drag_moves_without_drift` (drag the body 100 px right in 10 steps, then back 100 px in 10 steps → anchors equal the originals within 1 ms / 1e-9 price); `handle_drag_resizes_one_end`; `delete_and_undo_restore`; `escape_cancels_pending`; `locked_drawings_not_selectable`; `drawings_stay_put_across_timeframes` (a trendline drawn on M1 at 14:37 → `set_series(H1 resample)` → `ChartProjector::to_screen(a)` x equals `vp.x_of_index(time_to_index(..., a.time_ms))`, with the fractional position 37/60 inside the 14:00 bar).
- [ ] **Step 2: Run** `cargo test -p xv_chart`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p xv_chart`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(chart): drawing placement, selection, editing, undo`

---

### Task 13: GPUI chart view (painting and input forwarding)

**Files:**
- Create: `crates/xview/src/{theme.rs, chart_view.rs}`
- Modify: `crates/xview/src/main.rs` (temporarily show a `ChartView` fed by a CSV fixture or generated candles)

**Interfaces:**
- Consumes: `ChartState`, `Frame`, `Prim`, `ChartColors`, `Mods`, `ChartKey`, `ChartEffect`.
- Produces:
  - `pub struct AppTheme { pub mode: ThemeMode, pub chart: ChartColors, pub bg: Hsla, pub panel_bg: Hsla, pub border: Hsla, pub text: Hsla, pub muted: Hsla, pub hover: Hsla, pub active_bg: Hsla, pub accent: Hsla, pub badge: Hsla }` with `fn light()` / `fn dark()`; light values: white bg, `#F0F3FA`-ish panel gray, `#E0E3EB`-ish border (XView's own picks); a `u32 → Hsla` helper.
  - `pub struct ChartView { pub state: ChartState, pub source_name: SharedString, focus: FocusHandle }` (a GPUI entity) with `pub enum ChartViewEvent { NeedOlder(i64), DrawingsChanged, ToolFinished, SelectionChanged }` emitted via `cx.emit` (`impl EventEmitter<ChartViewEvent>`).
  - Render: a `canvas` filling its parent. Prepaint stores the bounds and calls `state.set_size`. Paint fills the background, paints `frame.plot` inside `window.with_content_mask(plot bounds)`, then the axes, then the legend (`Text` prims are shaped with `window.text_system().shape_line` at 12 px; `bg` = a rounded label quad behind the text; dashed lines use `PathBuilder::stroke(px(w)).dash_array(&[px(4.), px(4.)])`). The legend draws as GPUI text at the top-left: the title in 14 px semibold, then `O… H… L… C…` and the change colored up/down.
  - Input via `window.on_mouse_event` in paint (MouseDown/Move/Up, Scroll, Pinch) with positions converted to element-relative coordinates. `ScrollDelta::Lines` is multiplied by 20 px. `click_count` comes from `MouseDownEvent`. Keyboard actions `chart::Delete`, `chart::Cancel`, `chart::Undo`, `chart::Redo` are bound to `backspace`/`delete`, `escape`, `cmd-z`, `cmd-shift-z` in the `ChartView` key context. Every effect from `ChartState` → `cx.emit` + `cx.notify()`.

- [ ] **Step 1: Wire a demo.** `main.rs` builds a `Series` from 5,000 generated random-walk 1m candles and shows only a `ChartView`.
- [ ] **Step 2: Build** `cargo build -p xview`. Expected: success.
- [ ] **Step 3: Visual check.** Run the app, `screencapture -x /tmp/xv13.png`, and view it. Expected: candles, volume at the bottom ~20%, a right price axis with `,`-separated labels, a time axis, a dashed last-price line with its label, light theme. Then drag, scroll and pinch by hand (ask the owner if needed) and confirm smooth motion with no tearing between the plot and axes.
- [ ] **Step 4: Perf check.** Generate 100,000 candles, zoom out to ~2,000 visible, and pan. Confirm smooth motion. Add a debug-only frame-time log (`XVIEW_FRAME_LOG=1` prints the paint duration every 60 frames) and verify < 8 ms.
- [ ] **Step 5: Commit** `feat(app): GPUI chart view painting and input`

---

### Task 14: App shell: top bar, left toolbar, side panel, bottom bar, style bar, icons

**Files:**
- Create: `crates/xview/src/{assets.rs, top_bar.rs, left_toolbar.rs, style_bar.rs, side_panel.rs, bottom_bar.rs, workspace.rs}`, `crates/xview/assets/icons/*.svg`, `crates/xview/assets/icons/LICENSE-icons.md`
- Modify: `crates/xview/src/main.rs` (root view = `Workspace`)

**Interfaces:**
- Consumes: `ChartView`, `AppTheme`, `Timeframe::{TOP_BAR, ALL}`, `Tool`, `PALETTE`.
- Produces:
  - `pub struct Assets` implementing `gpui::AssetSource`, serving `icons/<name>.svg` from `include_bytes!` (a `match` on the name; no extra crate).
  - Icon names (the files must exist): `avatar_placeholder`, `candles`, `bars_style`, `plus_circle`, `chevron_down`, `crosshair`, `trend_line`, `horizontal_lines`, `patterns`, `fibonacci`, `brush_shapes`, `text`, `emoji`, `measure`, `zoom`, `magnet`, `lock`, `eye`, `trash`, `ray`, `extended_line`, `horizontal_line`, `horizontal_ray`, `vertical_line`, `rectangle`, `calendar`, `settings`. All 28×28 viewBox, stroke 1.5, round caps and joins, `currentColor`, no fills except dots.
  - `pub struct Workspace` (the root GPUI view) owns `chart: Entity<ChartView>`, `theme: AppTheme`, the current `Symbol` and `Timeframe`, and later (Task 15) the loader. `pub enum WorkspaceAction` is implemented as GPUI actions: `SelectTimeframe(Timeframe)`, `SelectTool(Tool)`, `ToggleHideDrawings`, `ToggleLockDrawings`, `ClearDrawings`, `OpenSymbolSearch`, `OpenCsv`, `ToggleTheme`, `ShowSpan(Option<i64>)`.
  - Layout (flex): top bar 56 px tall with a 4 px bottom border in panel gray; below it a row: left toolbar 52 px wide with a 4 px right border, then the chart area (flex-1, a column: chart view flex-1, bottom bar 34 px), then the side panel 300 px, then the rail 44 px.
  - **Top bar** (spec §6.1, in this exact order): avatar 38 px circle (purple, letter "X") with a red badge showing `11` *(placeholder count; wired later)* → symbol pill (40 px tall, radius 20, panel-gray bg, bold 16 px symbol, a white 32 px circle on the right with the `candles` icon; click → `OpenSymbolSearch`) → `plus_circle` button (disabled styling, tooltip "Compare (coming soon)") → separator (1×26 px) → timeframe text buttons for `TOP_BAR` (16 px text, 38 px tall, radius 6, active = `active_bg`) → `chevron_down` opening a popover listing `Timeframe::ALL` → separator → `bars_style` button (tooltip "Chart settings (coming soon)") → `candles` button shown active → `chevron_down` opening a chart-type popover with only "Candles" enabled.
  - **Left toolbar** (spec §6.1, in order): `crosshair` (Tool::Cursor), then `trend_line` group (flyout: Trend line ✓, Ray ✓, Extended line disabled), then `horizontal_lines` group (flyout: Horizontal line ✓, Horizontal ray disabled, Vertical line ✓), then `patterns` (disabled), then `fibonacci` (disabled), then `brush_shapes` group (flyout: Rectangle ✓, Brush disabled), then `text` (disabled), then `emoji` (disabled), then a divider, then `measure` (disabled), then `zoom` (disabled), then a divider, then `magnet` (toggles `ChartState::magnet`, i.e. always snap to OHLC), then `lock` (ToggleLockDrawings), then `eye` (ToggleHideDrawings), then a divider, then `trash` (ClearDrawings after a `window.prompt` confirm "Delete all drawings on {symbol}?"). Buttons are 42×42 with radius 6; the active one gets `active_bg`; group buttons show a small corner triangle and open their flyout on click; a group button shows the icon of the last-used tool in the group. Disabled items are 40% opacity with tooltip "Coming soon".
  - **Style bar**: when `selected_screen_bounds()` is Some, a floating row 8 px above the drawing (clamped inside the chart) with 8 color swatches from `PALETTE`, width buttons 1–4 and a trash button. It calls `set_selected_style` / `key(Delete)`.
  - **Side panel** (placeholder): header "Watchlist" with greyed `+` and `⋯`, empty-state text "Watchlist arrives in a later version", and a details card below with the current symbol, description and last price (live from the chart series, since it's free). **Rail**: 6 greyed icon buttons with tooltip "Coming soon".
  - **Bottom bar**: text buttons `1D 5D 1M 3M 6M YTD 1Y 5Y All` → `ShowSpan` (YTD = from Jan 1 local; All = None), then a separator, the `calendar` icon (disabled, "Go to date (coming soon)"), and on the right a live clock `HH:MM:SS UTC±H` (updates every second; it shows `UTC` when `use_utc`; click toggles UTC and persists).

- [ ] **Step 1: Write the icons** (original drawings, or Lucide/Tabler adaptations, recorded in `LICENSE-icons.md`).
- [ ] **Step 2: Implement the layout and actions** against the demo series from Task 13.
- [ ] **Step 3: Build** `cargo build -p xview`. Expected: success.
- [ ] **Step 4: Visual check against the reference.** Screenshot the app and compare with `.superpowers/brainstorm/*/content/toolbars.html` (the approved mockup). Same order, sizes and spacing; the active timeframe and tool are highlighted; flyouts open; disabled items are dimmed with tooltips. Fix mismatches, then show the owner the screenshot.
- [ ] **Step 5: Commit** `feat(app): top bar, tool column, style bar, side panel, bottom bar`

---

### Task 15: Data wiring, symbol search, CSV open, persistence

**Files:**
- Create: `crates/xview/src/{loader.rs, symbol_search.rs, persistence.rs}`
- Modify: `crates/xview/src/{workspace.rs, main.rs}`

**Interfaces:**
- Consumes: `SourceRegistry`, `SeriesCache`, `LoadGuard` (Task 7), `BinanceSource` (Task 9), `CsvSource` (Task 8), `xv_store` (Task 10), `ChartViewEvent` (Task 13).
- Produces:
  - `loader.rs`: `pub struct Loader { registry: SourceRegistry, csv: Arc<CsvSource>, cache: SeriesCache, guard: LoadGuard, live: Option<Task<()>> }` with:
    - `fn open(&mut self, symbol: Symbol, tf: Timeframe, cx: &mut Context<Workspace>)`: increments the token. On a cache hit it shows the series immediately; otherwise `history(.., None, 1000)` runs via `gpui_tokio::Tokio::spawn`. On completion, if the token is still current it calls `ChartView::set_series` and stores the series in the cache. It drops the old live task and starts `subscribe`, which forwards `Candle` → `apply_update` (and the cache) and `Reconnected` → `history(.., None, 1000)` → `merge_backfill`, all guarded by the token.
    - `fn load_older(&mut self, end_before_ms: i64, cx)`: same token guard → `prepend_older`.
    - Errors: `RegionBlocked` → `Workspace.banner = Some("Binance isn't available from your network (HTTP 451). Open a CSV file instead: File → Open CSV…")`. Other errors → a banner with the error text and a Retry button. A banner is a 36 px strip above the chart, dismissible.
  - `symbol_search.rs`: `pub struct SymbolSearch` (modal, opened by `OpenSymbolSearch`, or by typing any letter while the chart is focused): a text input plus a result list (symbol, description, source name). It queries every registered source with a 150 ms debounce. ↑/↓/Enter/Esc work; picking → `Loader::open(symbol, current tf)`.
  - `persistence.rs`: `pub struct Persistence { dir: PathBuf, pending: Option<Task<()>> }` with `fn schedule_drawings_save(&mut self, key: SymbolKey, ds: Vec<Drawing>, cx)` (500 ms debounce; a newer call replaces the pending one), `fn flush(&mut self)` (synchronous save, called on symbol switch and on quit), and `fn save_settings(&self, s: &Settings)`.
  - On `DrawingsChanged` → `schedule_drawings_save`. On symbol change → `flush`, then `load_drawings` for the new key (`Corrupt` → a banner "Drawings for {symbol} were unreadable; the file was kept at {path}"). Timeframe changes keep the same drawings (keyed per symbol).
  - Startup (`main.rs`): `data_dir()`, `load_settings`. Register `BinanceSource` first (default), then `CsvSource`. Restore window bounds (if on a visible display), the theme, and the last symbol + timeframe (default `binance:BTCUSDT` + `H1`). If the last symbol is a CSV whose file is missing → fall back to the default with a banner. Save settings on symbol/timeframe/theme/UTC change and on window bounds change (debounced) and on quit.
  - Menus: `XView → Quit (cmd-q)`, `File → Open CSV… (cmd-o)` (uses `cx.prompt_for_paths`, then `CsvSource::load_file` → `Loader::open`; CSV parse errors and skipped lines → a banner "Loaded N rows, skipped M (lines 6, 8, …)"), `View → Toggle Theme`.
  - Timeframe buttons the current source doesn't support (CSV below base) render disabled.

- [ ] **Step 1: Unit-test the guard path.** In `loader.rs`, add `#[cfg(test)] fn apply_if_current(guard: &LoadGuard, token: &LoadToken, f: impl FnOnce())` and test `late_response_is_dropped`: two opens in a row, complete the first after the second → only the second's series lands. Run `cargo test -p xview loader`. Expected: PASS after implementing.
- [ ] **Step 2: Implement** the loader, search, persistence, startup and menus.
- [ ] **Step 3: Build and run** `cargo run -p xview`. Expected: BTCUSDT 1h from Binance, with the last candle ticking live within a minute.
- [ ] **Step 4: Manual acceptance pass** (screenshot each, or ask the owner):
  1. Switch through all 11 timeframes; returning to a cached one is instant.
  2. Pan far left: older history loads with no visible jump.
  3. Search "eth" → ETHUSDT is first; pick it; the chart switches.
  4. Draw one of each tool type; restart the app; all are present on the same symbol, also on another timeframe, at the same price/time.
  5. Open a CSV of daily stock data with weekend gaps: D/W/M work, buttons below D are disabled, a drawing over a weekend sits at Monday's bar.
  6. Turn Wi-Fi off for 30 s and back on: the live candle resumes and the gap is backfilled.
  7. Corrupt a drawings JSON by hand and restart: the app starts, shows the banner, and the `.corrupt-*` file exists.
- [ ] **Step 5: Commit** `feat(app): live data loading, symbol search, CSV open, persistence`

---

### Task 16: README and final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Write the README** with these sections:
  - What XView is and the roadmap (from spec §1).
  - Build: Rust ≥ 1.98 and Xcode Command Line Tools, then `cargo run -p xview --release`. Note that shaders compile at runtime (`runtime_shaders`), so full Xcode isn't needed.
  - Data sources: Binance public data (region note about HTTP 451), CSV format (columns and time formats from Task 8).
  - Where data is stored (`~/Library/Application Support/XView/`).
  - A "TradingView feed plugin" section: separate repo, opt-in, unofficial, may break, may violate TradingView's terms, used at your own risk, not yet available.
  - License: GPL-3.0-or-later.
- [ ] **Step 2: Full test run.** `cargo test --workspace`. Expected: all pass. Then `cargo test --workspace --release -- --ignored`. Expected: the perf test and the live Binance test pass (network needed).
- [ ] **Step 3: Lint.** `cargo clippy --workspace -- -D warnings`. Expected: no warnings.
- [ ] **Step 4: Check for forbidden references.** `grep -ri "tradingview" crates/ Cargo.toml Cargo.lock`. Expected: no matches outside comments that explain the plugin boundary. The README is excluded from this check.
- [ ] **Step 5: Commit** `docs: README for milestone 1`
