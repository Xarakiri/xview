//! Temporary demo data for the chart view (replaced by real loading in Task 15).
//!
//! Environment knobs:
//! - `XVIEW_DEMO_CANDLES=N`: number of generated 1m candles (default 5,000).
//! - `XVIEW_DEMO_BAR_SPACING=PX`: initial bar spacing (e.g. 0.6 shows ~2,000
//!   bars in a 1280 px window).
//! - `XVIEW_DEMO_AUTOPAN=1`: pans back and forth every 16 ms, to exercise the
//!   paint path without hand input (pair with `XVIEW_FRAME_LOG=1`).

use std::time::Duration;

use gpui::{Context, Entity};
use xv_chart::Mods;
use xv_core::{Candle, Series, SourceId, Symbol, Timeframe};

use crate::chart_view::ChartView;

pub const DEFAULT_CANDLES: usize = 5_000;

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.trim().parse().ok()
}

pub fn candle_count() -> usize {
    env_parse("XVIEW_DEMO_CANDLES").unwrap_or(DEFAULT_CANDLES)
}

pub fn bar_spacing() -> Option<f32> {
    env_parse::<f32>("XVIEW_DEMO_BAR_SPACING").filter(|v| v.is_finite() && *v > 0.0)
}

pub fn autopan() -> bool {
    std::env::var_os("XVIEW_DEMO_AUTOPAN").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Deterministic xorshift64* generator in `[0, 1)`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// `n` random-walk 1m candles ending at a fixed time, from a fixed seed.
pub fn candles(n: usize) -> Vec<Candle> {
    const MINUTE: i64 = 60_000;
    // 2026-11-06 00:00 UTC.
    const END_MS: i64 = 1_793_923_200_000;
    let mut rng = Rng(0x5856_6965_7733_1300);
    let mut price = 30_000.0f64;
    let start = END_MS - n as i64 * MINUTE;
    (0..n)
        .map(|k| {
            let open = price;
            let mut hi = open;
            let mut lo = open;
            for _ in 0..4 {
                price *= 1.0 + (rng.next() - 0.5) * 0.002;
                hi = hi.max(price);
                lo = lo.min(price);
            }
            hi *= 1.0 + rng.next() * 0.0005;
            lo *= 1.0 - rng.next() * 0.0005;
            let round = |v: f64| (v * 100.0).round() / 100.0;
            Candle {
                open_time_ms: start + k as i64 * MINUTE,
                open: round(open),
                high: round(hi),
                low: round(lo),
                close: round(price),
                volume: 5.0 + rng.next() * 95.0,
            }
        })
        .collect()
}

pub fn series(n: usize) -> Series {
    let symbol = Symbol {
        source: SourceId("demo".into()),
        id: "DEMO".into(),
        display: "DEMOUSD".into(),
        description: "Generated random walk".into(),
        base: "DEMO".into(),
        quote: "USD".into(),
        tick_size: 0.01,
    };
    Series::new(symbol, Timeframe::M1, candles(n), false)
}

/// Pans 300 frames one way, then back, forever.
pub fn start_autopan<T: 'static>(chart: Entity<ChartView>, cx: &mut Context<T>) {
    let chart = chart.downgrade();
    cx.spawn(async move |_, cx| {
        let mut step = 0u32;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let dx = if (step / 300).is_multiple_of(2) {
                8.0
            } else {
                -8.0
            };
            step = step.wrapping_add(1);
            let alive = chart.update(cx, |v, cx| {
                let fx = v.state.scroll((400.0, 300.0), dx, 0.0, Mods::default());
                v.apply_effects(fx, cx);
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candles_are_deterministic_and_valid() {
        let a = candles(500);
        assert_eq!(a, candles(500));
        assert_eq!(a.len(), 500);
        for w in a.windows(2) {
            assert_eq!(w[1].open_time_ms - w[0].open_time_ms, 60_000);
        }
        for c in &a {
            assert!(c.low <= c.open.min(c.close) && c.high >= c.open.max(c.close));
            assert!(c.volume > 0.0);
        }
    }
}
