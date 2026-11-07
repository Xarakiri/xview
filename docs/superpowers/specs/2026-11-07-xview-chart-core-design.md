# XView — Milestone 1: Chart Core — Design

Date: 2026-11-07
Status: ready for owner review

## 1. Intent

XView is an open-source desktop charting platform for traders, written in Rust on
GPUI (the GPU UI framework behind the Zed editor). The long-term goal is a full
alternative to TradingView. It aims for feature parity in what traders can do, but has
its own name, visual identity and code. It must not copy TradingView's branding,
look, assets or Pine Script language.

That goal is reached through a roadmap of separately shippable milestones. **This
document covers Milestone 1 only.** Its bar: *a trader who mainly uses TradingView to
look at candlestick charts across timeframes and draw lines and zones on them can
switch to XView for that job and not miss anything.*

### Roadmap (for context; each later milestone gets its own design)

1. **Chart core + basic drawings** ← this document
2. Full drawing toolset (Fibonacci, text, channels, etc.)
3. Indicators (overlays + separate panes, settings)
4. Workspace (watchlist, multi-chart layouts, saved layouts)
5. Alerts
6. User scripting (our own language or an embedded one such as Lua/Rhai, not Pine Script)
7. Bar replay and backtesting
8. Screener
9. Broker connections / trading from the chart

Milestone 1 must not paint the later ones into a corner (see §8).

## 2. Goals (Milestone 1)

- Candlestick chart with volume bars, light theme by default (dark optional), XView's own palette.
- Timeframes: 1m, 3m, 5m, 15m, 30m, 1h, 2h, 4h, 1D, 1W, 1M, switchable from a
  button row in the top bar.
- Smooth pan (drag), zoom (scroll wheel / trackpad pinch, and dragging the time axis),
  vertical scale (drag the price axis; double-click to return to auto-fit).
- Price auto-fits to visible candles by default.
- Crosshair with price and time labels on the axes; legend top-left shows symbol,
  timeframe and O/H/L/C/V of the hovered candle (or latest when not hovering).
- Panning left past loaded data fetches older history automatically.
- The newest candle updates live.
- Symbol search: type "eth", get matching symbols, pick one.
- Data sources are pluggable. Built in: **Binance public market data** (default) and
  **CSV import**. A **TradingView feed** source exists as a separate, opt-in plugin
  (§6.3).
- Drawing tools: **trendline, ray, horizontal line, vertical line, rectangle**. Each
  can be created, selected, moved, resized via handles, recolored, and deleted.
- Drawings are anchored to (time, price), not pixels, so they stay put through pan,
  zoom and timeframe changes. Drawings belong to a symbol and show on every
  timeframe of it.
- Persistence: drawings, last symbol, last timeframe and window size survive restarts.

## 3. Non-goals (Milestone 1)

- Indicators of any kind (Milestone 3). Volume is the only sub-series.
- Watchlists, multiple charts, layouts, alerts, scripting, replay, screener, trading.
- Chart types other than candles (bars, line, Heikin-Ashi come later).
- Log / percentage price scale (later; the scale abstraction must allow it, §8).
- Linux and Windows builds. macOS only. Keep code free of needless Mac-only
  assumptions, but don't build or test elsewhere.
- Dynamic (runtime-loaded) plugins. Plugins are compile-time crates in M1.
- Accounts, cloud sync, any XView server.

## 4. Anti-goals (failure even if it "works")

- Any stutter when panning or zooming on a normal Mac, or a noticeable delay
  switching timeframes once data is cached.
- Drawings drifting off their price/time anchors at any zoom, scale or timeframe.
- Losing a user's drawings (crash, restart, bad save).
- Looking like a TradingView knockoff (their logo, name, color scheme, icons).
- Shipping the TradingView scraper inside the main repo or enabling it by default.

## 5. Constraints

- **Language/UI:** Rust (stable, edition 2024), GPUI.
- **GPUI dependency:** git dependency on `https://github.com/zed-industries/zed`,
  crates `gpui` and `gpui_platform`, **pinned to a specific commit**. The owner's
  local checkout is at `9a8ab54db4` (2026-11-06); start there. `gpui_platform`
  is not on crates.io, so a git dep is required. Do not require contributors to
  have a local Zed checkout. A local `[patch]` override for development is fine.
- **Platform:** macOS only. Building needs full Xcode (Metal), which goes in the README.
- **Rendering:** the chart is drawn in a GPUI `canvas` element using
  `window.paint_quad`, `window.paint_path` (fill and stroke), text shaping, and
  `window.with_content_mask` for clipping plot vs axes. GPUI's examples
  `painting.rs` and `paths_bench.rs` are the reference.
- **Performance targets:** 60 fps while panning/zooming with up to ~2,000 visible
  candles; 100k candles loaded in memory without degradation; cold start to a drawn
  chart (cached data) under ~1 s.
- **License:** GPL (owner's decision, 2026-11-07), specifically **GPL-3.0-or-later**
  *(version is my call: GPUI is Apache-2.0, which is compatible with GPLv3 but not
  GPLv2; Zed itself uses GPL-3.0)*. All dependencies must be GPLv3-compatible.
- **Brand:** project name **XView** (owner's decision, 2026-11-07). Repo folder
  `xview`, crates prefixed `xv_`, app support dir `~/Library/Application Support/XView/`.

## 6. Design decisions

### 6.1 Screen layout (owner's reference screenshots, 2026-11-07)

Overall arrangement follows the owner's reference: top bar, left drawing-tool column,
chart with legend and right price axis, time axis, bottom range bar. A right-side
watchlist/details panel and icon rail are part of the target layout. In M1 they are
**placeholders** (owner's decision, 2026-11-07): the panel and rail render in place,
with static empty states. Making them work is Milestone 4.

**Top bar** (left to right, matching the owner's reference 1:1 in order, sizes and
spacing):

1. Round avatar / app-menu button with a red notification-count badge.
2. Symbol pill: rounded light-gray capsule with the bold symbol (e.g. `GBPUSD`); click
   opens symbol search.
3. Circled "+" button (compare / add symbol; disabled until a later milestone).
4. Thin vertical separator.
5. Timeframe buttons as plain text: `1m 3m 5m 15m 30m 1h 4h D W M`. The active one
   gets a rounded light-gray background. A chevron (`⌄`) opens the full timeframe
   list (adds 2h and any others).
6. Separator.
7. Bar-chart-style icon button (chart settings/style), then the candle icon button,
   shown as selected (gray rounded bg), followed by a chevron for the chart-type
   dropdown (only Candles enabled in M1).

**Left toolbar** (matching the owner's reference 1:1 in order, sizes and spacing;
~48 px wide column, ~40 px square buttons, active button has a rounded light-gray
background, thin-line icons):

1. Crosshair / cursor (default selected)
2. Trend line group (flyout: trendline, ray, extended line)
3. Horizontal lines group (flyout: horizontal line, horizontal ray, vertical line)
4. Patterns group (XABCD-style)
5. Fibonacci group
6. Brush group
7. Text group
8. Emoji / icons group
9. Divider, then: measure, zoom, divider, magnet, lock drawings, hide drawings,
   divider, delete drawings (with confirm)
10. Rectangle sits in the shapes position of this column *(my call: the reference's
    group 6 "brush" flyout also holds shapes, so rectangle lives there)*.

Groups whose tools arrive in later milestones (patterns, Fibonacci, brush, text,
emoji) are shown in place but disabled, with a "coming soon" tooltip *(my call: keeps
the column 1:1 with the reference while M1 ships only lines and rectangles)*.

**Icons and branding:** the arrangement, order, sizing and line style match the
reference. The icon artwork itself is XView's own: thin 1.5 px stroke, round caps,
drawn for XView or taken from a permissively licensed set (Lucide / Tabler, MIT). No
TradingView logo, icon files or wordmark are copied.

- **Theme:** light theme by default (as in the reference), dark theme as an option.
  Colors come from a single theme struct.
- Volume sits at the bottom ~20% of the main pane, sharing its time axis, with its own
  hidden scale.

### 6.2 Data model and sources

- Core types: `Symbol { source, id, display, base, quote, tick_size }`,
  `Timeframe` (enum of the 11 above), `Candle { open_time_ms, o, h, l, c, v }`
  (f64 is fine), `Series` (sorted, contiguous candles for one symbol+timeframe, plus
  "has older history" flag).
- A `DataSource` trait (async), roughly:
  - `search_symbols(query) -> Vec<Symbol>`
  - `supported_timeframes(symbol) -> Vec<Timeframe>`
  - `history(symbol, timeframe, end_before, limit) -> Vec<Candle>`
  - `subscribe(symbol, timeframe) -> stream of candle updates` (update-in-place of
    the last candle or append new one)
- Sources are registered in a `SourceRegistry`. Symbols carry their source id so the
  app can mix sources later.
- **Binance (default):** REST `klines` for history (paginate backwards with
  `endTime`), `exchangeInfo` for the symbol list (cache it), websocket kline stream
  for live updates. Spot market only in M1. No API key.
  *Endpoint (my call):* REST via `data-api.binance.vision` (Binance's public
  market-data-only host), falling back to `api.binance.com`. Websocket via
  `data-stream.binance.vision`, falling back to `stream.binance.com`. Binance blocks
  some regions (e.g. US IPs get HTTP 451). On a block, the app shows a clear message
  that suggests CSV import, and does not just render an empty chart. Both hosts were
  reachable from the owner's network on 2026-11-07. A Binance.US source is a possible
  later plugin.
- **CSV:** File → Open CSV. Expected columns `time,open,high,low,close[,volume]`,
  header optional, time as unix seconds, unix ms, or ISO-8601. The base timeframe is
  inferred from the most common gap. Higher timeframes are produced by resampling;
  lower-than-base timeframe buttons are disabled. No live updates. Shows as a symbol
  named after the file.
- An in-memory cache per (symbol, timeframe) makes switching back instant. On-disk
  candle caching is the builder's call (nice to have, not required).

### 6.3 TradingView feed plugin

- Lives in a **separate repository** (e.g. `xview-tradingview`), not in the main
  repo. It is a Rust crate implementing `DataSource` by speaking the same websocket
  protocol the community `Mathieu2301/TradingView-API` (Node.js) library uses. That
  library is the protocol reference only; we do not run Node.
- The main app exposes it through an off-by-default cargo feature
  (`--features tradingview`) that pulls the plugin crate by git. The README states
  plainly that it is unofficial, may break at any time, and may violate TradingView's
  terms, so it's used at the user's own risk.
- **Not required for Milestone 1 completion.** M1 requires that the `DataSource` trait
  be stable enough that this plugin can be written against it without changing core
  code. Building the plugin and adding the cargo feature wiring are both follow-up
  tasks. M1's manifest and lockfile must not reference the plugin repo at all
  *(my call: an optional git dependency still lands in `Cargo.lock` and would break
  builds if the repo doesn't exist yet)*.

### 6.4 Chart engine

- A pure, GPUI-free `Viewport` / coordinate module: visible time range (expressed as
  a bar index offset + bar spacing in px), price range, plot size → functions mapping
  (time, price) ↔ (x, y). Unit-tested heavily; drawings and rendering both depend on
  it.
- X axis is bar-index based (like most trading charts: no gaps for weekends or
  missing bars). Drawings store real timestamps and are converted via the series'
  time↔index mapping, including extrapolation into the future (right of the last
  candle) and the past (beyond loaded data) so anchors outside loaded data still
  place correctly.
- Rendering per frame: grid, candles (wick + body; bodies at least 1px; at very high
  zoom-out draw bodies as lines), volume, drawings, crosshair, axes with
  "nice" tick labels (prices rounded to tick size, times formatted per timeframe), and
  a last-price line with label on the price axis. Only visible candles are processed.
- **Anchor rule** *(my call)*: drawings store exact timestamps (ms), never bar
  timestamps. A timestamp maps to a *fractional* bar index: 14:37 on a 1h chart sits
  37/60 of the way through the 14:00 bar. Beyond the loaded or last bar it
  extrapolates at the timeframe's nominal duration. Inside a gap (weekend or session
  break in CSV data) it maps to the start of the next existing bar. The reverse
  mapping (placing a point) uses the same rule, so a placed point round-trips exactly
  on the timeframe it was drawn on.
- **Time zone** *(my call)*: candle boundaries come from the source (Binance uses UTC).
  CSV resampling uses UTC in M1, but the resampling API takes a time zone parameter so
  sessions and exchange zones can be added without a rewrite. Axis labels and the
  legend show time in the Mac's local time zone, with a UTC toggle in the settings
  file.
- **Trackpad / wheel** *(my call)*: two-finger horizontal swipe pans, vertical swipe or
  mouse wheel zooms around the cursor, pinch zooms, ⌥+vertical swipe scrolls price.
- Zoom anchors at the cursor (scroll) or at the right edge (time-axis drag). The right
  edge keeps a small margin of empty space. When the latest candle is in view, new
  candles keep the chart scrolled to "now".

### 6.5 Drawings

- Types: trendline (2 points), ray (2 points, extends right infinitely), horizontal
  line (price only, full width, price label on axis), vertical line (time only),
  rectangle (2 corners, semi-transparent fill + border).
- Interaction: pick a tool, then click-click (or click-drag) to place it. The tool
  returns to cursor after one drawing (*my call; shift-click keeps the tool*). Click a
  drawing to select it and show handles. Drag the body to move it, drag handles to
  resize. Delete/Backspace removes it, Esc deselects or cancels. A small floating
  style bar on selection offers color (fixed palette of ~8) and line width (1–4).
- Hit-testing in pixel space with a few px tolerance; topmost (latest) wins.
- Magnet/snap to OHLC: holding ⌘ snaps to the nearest candle's O/H/L/C *(my call;
  cheap and expected by traders)*.
- Persistence: drawings saved per symbol in the app support directory
  (`~/Library/Application Support/XView/`), written atomically (temp file +
  rename) after each change, debounced. Format is versioned JSON. Settings (last
  symbol, timeframe, window bounds) saved the same way.
- Undo/redo for drawing operations (⌘Z / ⇧⌘Z) *(my call; losing a drawing to a
  mis-click is a version of the "losing drawings" anti-goal)*.

## 7. Suggested repository shape (builder may adjust)

Cargo workspace:

- `crates/xview` — the app binary (window, top bar, toolbar, wiring)
- `crates/xv_core` — Symbol, Timeframe, Candle, Series, resampling. No GPUI.
- `crates/xv_data` — `DataSource` trait, registry, cache
- `crates/xv_source_binance`, `crates/xv_source_csv`
- `crates/xv_chart` — Viewport math (no GPUI) + the GPUI chart element
- `crates/xv_drawings` — drawing model, hit-testing, serialization (no GPUI); the
  chart crate renders them
- `crates/xv_store` — persistence

GPUI-free crates are unit-tested with plain `cargo test`. GPUI interaction can use
GPUI's `test-support` feature where practical.

## 8. Future-proofing to keep in mind (don't build, don't block)

- Panes: the chart should be one pane in a vertical pane stack, so Milestone 3 can add
  indicator panes sharing the time axis. M1 has one pane (price + volume).
- Price scale as a trait/enum so log and percent scales slot in.
- Drawing types as an extensible enum/trait plus versioned serialization, so M2 adds
  tools without migrations breaking old files.
- Multiple chart instances: no global "current chart" singletons. M4 adds layouts.
- Series computed from candles should be easy to add (indicators, M3).

## 9. Left to the builder

- Exact crate boundaries, async runtime glue (e.g. `gpui_tokio` vs smol), HTTP and
  websocket crates.
- Whether to use the `gpui-component` library for top bar, search and buttons, or
  build them with plain GPUI `div`s.
- Exact color palette and icon set (must be original or permissively licensed).
- On-disk candle cache.
- Keyboard shortcuts beyond those listed.

## 10. Done means

- `cargo run` on a Mac with Xcode opens XView showing BTCUSDT 1h from Binance with
  live updates.
- All goals in §2 work. No anti-goal in §4 is observable.
- Viewport math, resampling, CSV parsing, drawing hit-testing and serialization have
  unit tests.
- README explains building, the data sources, and the TradingView plugin disclaimer.
