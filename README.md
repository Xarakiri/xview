# XView

XView is an open-source desktop charting app for traders, written in Rust on
[GPUI](https://www.gpui.rs/). The long-term goal is a full alternative to
TradingView, with its own name, look and code. macOS only for now.

Milestone 1 (this release) is the chart core: candlesticks with volume, eleven
timeframes (1m to 1M), live data from Binance, symbol search, CSV import, and
five drawing tools (trendline, ray, horizontal line, vertical line, rectangle)
that persist across restarts.

## Roadmap

1. Chart core and basic drawings (done)
2. Full drawing toolset
3. Indicators
4. Workspace (watchlist, multi-chart layouts, saved layouts)
5. Alerts
6. User scripting (our own language or an embedded one, not Pine Script)
7. Bar replay and backtesting
8. Screener
9. Broker connections and trading from the chart

## Build

You need Rust 1.98 or newer and the Xcode Command Line Tools
(`xcode-select --install`). Shaders compile at runtime (`runtime_shaders`), so
full Xcode is not required.

```sh
cargo run -p xview --release
```

## Using it

- Drag the chart to pan. Scroll or pinch to zoom around the cursor.
- Hold Option and scroll to move the price range vertically.
- Drag the price axis to scale it; double-click the price axis to auto-fit again.
- Drag the time axis to zoom in time.
- Pick a drawing tool in the left column, then click on the chart to place it.
  Click a drawing to select it, drag its body to move it or a handle to resize.
  Delete or Backspace removes the selection, Esc cancels a drag or pending
  placement, then clears the selection, then drops the active tool. Cmd+Z undoes and Shift+Cmd+Z redoes.
- Start typing a letter with the chart focused to open symbol search. Cmd+K
  opens it too.
- Cmd+O opens a CSV file. Option+Cmd+T toggles the light and dark theme.
- The range buttons in the bottom bar (1D, 5D, 1M, 3M, 6M, YTD, 1Y, 5Y, All)
  zoom to that span ending at the latest candle.
- Click the clock in the bottom bar to switch between local time and UTC.

Panning left past the loaded data fetches older history automatically.

## Data sources

### Binance

The default source is Binance public market data (no account or API key). XView
tries `data-api.binance.vision` first and falls back to `api.binance.com` (REST)
and `stream.binance.com` (websocket). Some regions, the US among them, are
blocked by Binance: the servers answer HTTP 451. XView then shows a region
error. Use a VPN or the CSV import in that case. Other HTTP errors (403
included) show a load error with Retry.

### CSV

Open a file with Cmd+O. Columns, in order:

```
time,open,high,low,close,volume
```

- `volume` is optional (missing or empty means 0). At least 5 fields per row.
- Comma or semicolon separated (chosen per line). A header row is detected and
  skipped. A UTF-8 BOM is tolerated.
- `time` can be Unix seconds or milliseconds, RFC 3339 (`2025-11-07T14:37:00Z`,
  with or without an offset), `YYYY-MM-DD HH:MM[:SS]`, `YYYY-MM-DDTHH:MM[:SS]`
  or `YYYY-MM-DD`. Times without an offset are read as UTC.
- Rows are sorted by time. If a time repeats, the later row wins. If high is
  below low they are swapped. Invalid rows are skipped and reported.
- The timeframe is inferred from the most common gap between candles (within
  10 percent of a known timeframe, or 28 to 31 days for monthly). Larger
  timeframes are built from it by resampling. The symbol name is the file name
  in upper case.

## Where data is stored

Drawings, last symbol, last timeframe and window size are saved under
`~/Library/Application Support/XView/`. Set `XVIEW_DATA_DIR` to use another
directory.

## TradingView feed plugin

A TradingView data source is planned as a separate, opt-in plugin in its own
repository. It is not part of this repository, not enabled by default, and not
yet available. It would be unofficial, could break at any time, and may violate
TradingView's terms of service. If you use it, you do so at your own risk.

## Development

Environment variables read by the app:

| Variable | Effect |
| --- | --- |
| `XVIEW_DATA_DIR` | Data directory override |
| `XVIEW_DEMO=1` | Offline, deterministic demo data instead of Binance |
| `XVIEW_DEMO_CANDLES` | Number of demo candles (default 5000) |
| `XVIEW_DEMO_BAR_SPACING` | Initial bar spacing in px for the demo |
| `XVIEW_DEMO_AUTOPAN=1` | Auto-pan the demo chart (for perf checks) |
| `XVIEW_FRAME_LOG=1` | Log frame timing |
| `XVIEW_SCRIPT`, `XVIEW_SCRIPT_SHOT` | Scripted input for UI checks. Debug builds only |

```sh
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
