use crate::{PriceRange, Viewport};
use chrono::{Datelike, NaiveDate, Timelike};
use std::collections::BTreeSet;
use xv_core::{Candle, Timeframe, TzOffset};

const DAY_MS: i64 = 86_400_000;
const MIN_PRICE_SPACING_PX: f64 = 60.0;
const TIME_LABEL_SPACING_PX: f32 = 90.0;
const MAX_PRICE_TICKS: i64 = 500;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Smallest "nice" step (1, 2, 2.5 or 5 times a power of ten) that is at least
/// `min_step` and, when `tick_size` is known, a whole multiple of it.
fn nice_step(min_step: f64, tick_size: f64) -> f64 {
    let start = min_step.log10().floor() as i32 - 1;
    for exp in start..start + 40 {
        let base = 10f64.powi(exp);
        for m in [1.0, 2.0, 2.5, 5.0] {
            let step = m * base;
            if step < min_step * (1.0 - 1e-9) {
                continue;
            }
            if tick_size > 0.0 {
                let r = step / tick_size;
                if (r - r.round()).abs() > 1e-6 {
                    continue;
                }
            }
            return step;
        }
    }
    min_step
}

/// Price levels for the grid and price axis, roughly 60-120 px apart.
pub fn price_ticks(r: PriceRange, height: f32, tick_size: f64) -> Vec<f64> {
    let span = r.max - r.min;
    let valid = r.min.is_finite() && r.max.is_finite() && span > 0.0 && height > 0.0;
    if !valid {
        return vec![if r.min.is_finite() { r.min } else { 0.0 }];
    }
    let tick = if tick_size.is_finite() && tick_size > 0.0 {
        tick_size
    } else {
        0.0
    };
    let raw = (span * MIN_PRICE_SPACING_PX / height as f64).max(tick);
    let step = nice_step(raw, tick);
    let first = (r.min / step).ceil() as i64;
    let last = (r.max / step).floor() as i64;
    if last < first || last - first > MAX_PRICE_TICKS {
        return vec![r.min];
    }
    (first..=last)
        .map(|k| k as f64 * step)
        .filter(|v| *v >= r.min && *v <= r.max)
        .collect()
}

/// Number of decimals needed to represent multiples of `tick_size`.
fn decimals_of(tick_size: f64) -> usize {
    if !tick_size.is_finite() || tick_size <= 0.0 {
        return 2;
    }
    for d in 0..=12 {
        let s = tick_size * 10f64.powi(d);
        if s >= 0.5 && (s - s.round()).abs() < 1e-6 * s {
            return d as usize;
        }
    }
    12
}

/// Price with the tick size's decimals and a `,` thousands separator.
pub fn format_price(p: f64, tick_size: f64) -> String {
    if !p.is_finite() {
        return "-".into();
    }
    let s = format!("{:.*}", decimals_of(tick_size), p.abs());
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
    };
    let mut out = String::with_capacity(s.len() + int.len() / 3 + 1);
    let nonzero = s.bytes().any(|b| (b'1'..=b'9').contains(&b));
    if p < 0.0 && nonzero {
        out.push('-');
    }
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeTick {
    pub index: usize,
    pub label: String,
    pub major: bool,
}

/// Time zone the sources bucket daily, weekly and monthly bars in. Binance
/// and CSV files both start a day at UTC midnight (and a month on the 1st,
/// UTC), so those bars are labelled with their date there: in the display
/// zone a day bar would show the previous date west of UTC.
const DAILY_BUCKET_TZ: TzOffset = TzOffset::UTC;

/// The zone a bar of `tf` is labelled in: the display zone `tz` for intraday
/// bars, the bucket zone for D/W/M bars.
fn label_tz(tf: Timeframe, tz: TzOffset) -> TzOffset {
    if tf.nominal_ms() < DAY_MS {
        tz
    } else {
        DAILY_BUCKET_TZ
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

fn date_of(local_ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(local_ms)
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

/// Time axis labels: about one per 90 px. Day, month and year changes are
/// major and take priority over regular `HH:MM` (or day number) labels.
pub fn time_ticks(candles: &[Candle], tf: Timeframe, vp: &Viewport, tz: TzOffset) -> Vec<TimeTick> {
    let range = vp.visible_range(candles.len());
    if range.is_empty() || vp.bar_spacing <= 0.0 {
        return Vec::new();
    }
    let tz_ms = label_tz(tf, tz).seconds as i64 * 1000;
    let min_gap = (TIME_LABEL_SPACING_PX / vp.bar_spacing).max(1.0) as f64;
    let bars_per_label = (TIME_LABEL_SPACING_PX / vp.bar_spacing).ceil().max(1.0) as i64;
    let intraday = tf.nominal_ms() < DAY_MS;
    // Regular intraday labels sit on round clock times.
    let clock_step = if intraday {
        let need = bars_per_label * tf.nominal_ms();
        const STEPS_MIN: [i64; 10] = [1, 5, 15, 30, 60, 120, 180, 360, 720, 1440];
        STEPS_MIN.iter().map(|m| m * 60_000).find(|s| *s >= need)
    } else {
        None
    };
    // Bars open on UTC multiples of the bar length, so with an offset that
    // isn't a multiple of it (+5:30, +5:45, or -5h on 2h bars) no bar sits on
    // a round local time. Labels then sit on round local times shifted by
    // the part of the offset the bars can't express: 05:30, 11:30, ... at
    // +5:30. Offsets the bars can express get round local times as before.
    let clock_shift = clock_step.map_or(0, |step| tz_ms.rem_euclid(gcd(tf.nominal_ms(), step)));

    // Candidates: (priority, tick). Year > month > day changes > regular labels.
    let mut cands: Vec<(u8, TimeTick)> = Vec::new();
    let mut prev_date =
        (range.start > 0).then(|| date_of(candles[range.start - 1].open_time_ms + tz_ms));
    for i in range.clone() {
        let local = candles[i].open_time_ms + tz_ms;
        let date = date_of(local);
        let mut tick = None;
        if let Some(prev) = prev_date.filter(|p| *p != date) {
            let (prio, label) = if date.year() != prev.year() {
                (3, date.year().to_string())
            } else if date.month() != prev.month() {
                (2, MONTHS[date.month0() as usize].to_string())
            } else {
                (1, date.day().to_string())
            };
            // On daily and longer bars every bar changes the day, so only
            // year/month changes are major there.
            if prio > 1 || intraday {
                tick = Some((
                    prio,
                    TimeTick {
                        index: i,
                        label,
                        major: true,
                    },
                ));
            } else {
                tick = Some((
                    0,
                    TimeTick {
                        index: i,
                        label,
                        major: false,
                    },
                ));
            }
        } else if let Some(step) = clock_step
            && (local - clock_shift).rem_euclid(DAY_MS) % step == 0
        {
            {
                let t = chrono::DateTime::from_timestamp_millis(local).unwrap_or_default();
                let label = format!("{:02}:{:02}", t.hour(), t.minute());
                tick = Some((
                    0,
                    TimeTick {
                        index: i,
                        label,
                        major: false,
                    },
                ));
            }
        }
        prev_date = Some(date);
        if let Some(t) = tick {
            cands.push(t);
        }
    }

    // Highest priority first (stable, so left to right within a priority);
    // drop labels that crowd an already accepted one.
    cands.sort_by_key(|(p, _)| std::cmp::Reverse(*p));
    let mut taken: BTreeSet<usize> = BTreeSet::new();
    let mut kept: Vec<TimeTick> = Vec::new();
    let gap = min_gap.ceil() as usize;
    for (_, t) in cands {
        let crowded = taken
            .range(t.index.saturating_sub(gap - 1)..t.index + gap)
            .next()
            .is_some();
        if !crowded && (0.0..=vp.width).contains(&vp.x_of_index(t.index as f64)) {
            taken.insert(t.index);
            kept.push(t);
        }
    }
    kept.sort_by_key(|t| t.index);
    kept
}

/// `"Mon 09 Nov '26  13:40"` for intraday timeframes, in `tz`; the bar's
/// date `"Mon 09 Nov '26"` for D/W/M bars (in the zone they are bucketed in).
pub fn format_crosshair_time(ts_ms: i64, tf: Timeframe, tz: TzOffset) -> String {
    let local = ts_ms.saturating_add(label_tz(tf, tz).seconds as i64 * 1000);
    let dt = chrono::DateTime::from_timestamp_millis(local).unwrap_or_default();
    let date = format!(
        "{} {:02} {} '{:02}",
        WEEKDAYS[dt.weekday().num_days_from_monday() as usize],
        dt.day(),
        MONTHS[dt.month0() as usize],
        dt.year().rem_euclid(100)
    );
    if tf.nominal_ms() < DAY_MS {
        format!("{date}  {:02}:{:02}", dt.hour(), dt.minute())
    } else {
        date
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;
    const DAY: i64 = 24 * H;

    fn bar(t: i64) -> Candle {
        Candle {
            open_time_ms: t,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume: 0.0,
        }
    }

    #[test]
    fn price_ticks_are_nice() {
        let r = PriceRange {
            min: 4030.0,
            max: 4160.0,
        };
        let t = price_ticks(r, 800.0, 0.1);
        assert!(t.len() >= 2);
        for w in t.windows(2) {
            assert!((w[1] - w[0] - 10.0).abs() < 1e-9, "{t:?}");
        }
        assert!(t.iter().all(|v| *v >= r.min && *v <= r.max));
    }

    #[test]
    fn ticks_respect_tick_size() {
        let t = price_ticks(
            PriceRange {
                min: 0.0,
                max: 0.003,
            },
            800.0,
            0.001,
        );
        assert!(t.windows(2).all(|w| w[1] - w[0] >= 0.001 - 1e-12));
    }

    #[test]
    fn ticks_on_zero_range_are_finite() {
        for (min, max) in [(5.0, 5.0), (0.0, 0.0), (f64::NAN, f64::NAN)] {
            let t = price_ticks(PriceRange { min, max }, 600.0, 0.01);
            assert!(!t.is_empty() && t.iter().all(|v| v.is_finite()));
        }
        assert!(!price_ticks(PriceRange { min: 0.0, max: 1.0 }, 0.0, 0.01).is_empty());
    }

    #[test]
    fn format_price_cases() {
        assert_eq!(format_price(4092.0, 0.1), "4,092.0");
        assert_eq!(format_price(1.3347, 0.00001), "1.33470");
        assert_eq!(format_price(67412.5, 0.01), "67,412.50");
        assert_eq!(format_price(-1234567.0, 1.0), "-1,234,567");
        assert_eq!(format_price(-0.001, 0.01), "0.00");
        assert_eq!(format_price(f64::NAN, 0.01), "-");
        assert_eq!(format_price(0.00001234, 1e-8), "0.00001234");
        assert_eq!(format_price(0.000000015, 5e-9), "0.000000015");
        assert_eq!(format_price(10.25, 0.25), "10.25");
        assert_eq!(format_price(1234.0, 5.0), "1,234");
    }

    #[test]
    fn price_ticks_for_tiny_tick_size() {
        let t = price_ticks(
            PriceRange {
                min: 0.0000120,
                max: 0.0000130,
            },
            600.0,
            1e-8,
        );
        assert!(!t.is_empty() && t.iter().all(|v| v.is_finite()));
        assert!(t.len() < 3 || t.windows(2).all(|w| w[1] - w[0] >= 1e-8 * (1.0 - 1e-6)));
    }

    #[test]
    fn crosshair_time_format() {
        // 2026-11-09 13:40 UTC (a Monday)
        let ts = chrono_ts(2026, 11, 9, 13, 40);
        assert_eq!(
            format_crosshair_time(ts, Timeframe::M5, TzOffset::UTC),
            "Mon 09 Nov '26  13:40"
        );
        assert_eq!(
            format_crosshair_time(ts, Timeframe::D1, TzOffset::UTC),
            "Mon 09 Nov '26"
        );
        let plus3 = TzOffset { seconds: 3 * 3600 };
        assert_eq!(
            format_crosshair_time(ts, Timeframe::H1, plus3),
            "Mon 09 Nov '26  16:40"
        );
    }

    fn chrono_ts(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn time_ticks_intraday_mark_day_changes() {
        let t0 = chrono_ts(2026, 11, 9, 0, 0);
        let cs: Vec<Candle> = (0..72).map(|i| bar(t0 + i * H)).collect();
        let mut vp = Viewport::new(720.0, 400.0); // 10 px per bar
        vp.bar_spacing = 10.0;
        vp.right_index = 72.0;
        let ticks = time_ticks(&cs, Timeframe::H1, &vp, TzOffset::UTC);
        assert!(ticks.len() >= 3 && ticks.len() <= 9, "{ticks:?}");
        let major: Vec<_> = ticks.iter().filter(|t| t.major).collect();
        assert!(
            major.iter().any(|t| t.index == 24 && t.label == "10"),
            "{ticks:?}"
        );
        assert!(major.iter().any(|t| t.index == 48 && t.label == "11"));
        assert!(ticks.iter().any(|t| !t.major && t.label.contains(':')));
        // spacing between labels is at least ~90px
        for w in ticks.windows(2) {
            assert!((w[1].index - w[0].index) as f32 * 10.0 >= 85.0, "{ticks:?}");
        }
    }

    #[test]
    fn time_ticks_month_and_year_changes() {
        let t0 = chrono_ts(2025, 11, 1, 0, 0);
        let cs: Vec<Candle> = (0..120).map(|i| bar(t0 + i * DAY)).collect();
        let mut vp = Viewport::new(1200.0, 400.0);
        vp.bar_spacing = 10.0;
        vp.right_index = 120.0;
        let ticks = time_ticks(&cs, Timeframe::D1, &vp, TzOffset::UTC);
        let labels: Vec<&str> = ticks
            .iter()
            .filter(|t| t.major)
            .map(|t| t.label.as_str())
            .collect();
        assert!(labels.contains(&"Dec"), "{ticks:?}");
        assert!(labels.contains(&"2026"), "{ticks:?}");
        assert!(labels.contains(&"Feb"));
    }

    #[test]
    fn time_ticks_empty_and_bounded() {
        let vp = Viewport::new(800.0, 400.0);
        assert!(time_ticks(&[], Timeframe::H1, &vp, TzOffset::UTC).is_empty());
    }

    const MINUS5: TzOffset = TzOffset { seconds: -18_000 };
    const PLUS530: TzOffset = TzOffset { seconds: 19_800 };
    const PLUS545: TzOffset = TzOffset { seconds: 20_700 };

    fn month_bars(y0: i32, n: u32) -> Vec<Candle> {
        (0..n)
            .map(|k| bar(chrono_ts(y0 + (k / 12) as i32, k % 12 + 1, 1, 0, 0)))
            .collect()
    }

    #[test]
    fn daily_and_monthly_crosshair_shows_the_bars_date_in_any_zone() {
        // A D bar opens at UTC midnight: west of UTC that instant is the
        // previous evening, but the bar is still Mon 09 Nov.
        let day = chrono_ts(2026, 11, 9, 0, 0);
        let month = chrono_ts(2026, 11, 1, 0, 0);
        for tz in [MINUS5, PLUS530, PLUS545, TzOffset::UTC] {
            assert_eq!(format_crosshair_time(day, Timeframe::D1, tz), "Mon 09 Nov '26");
            assert_eq!(format_crosshair_time(day, Timeframe::W1, tz), "Mon 09 Nov '26");
            assert_eq!(format_crosshair_time(month, Timeframe::Mo1, tz), "Sun 01 Nov '26");
        }
        // Intraday stays in the display zone.
        assert_eq!(format_crosshair_time(day, Timeframe::H1, MINUS5), "Sun 08 Nov '26  19:00");
        assert_eq!(format_crosshair_time(day, Timeframe::H1, PLUS530), "Mon 09 Nov '26  05:30");
        assert_eq!(format_crosshair_time(day, Timeframe::H1, PLUS545), "Mon 09 Nov '26  05:45");
    }

    #[test]
    fn daily_axis_labels_months_on_the_first_west_of_utc() {
        let t0 = chrono_ts(2025, 11, 1, 0, 0);
        let cs: Vec<Candle> = (0..120).map(|i| bar(t0 + i * DAY)).collect();
        let mut vp = Viewport::new(1200.0, 400.0);
        vp.bar_spacing = 10.0;
        vp.right_index = 120.0;
        for tz in [MINUS5, PLUS530, PLUS545] {
            let ticks = time_ticks(&cs, Timeframe::D1, &vp, tz);
            let at = |label: &str| ticks.iter().find(|t| t.major && t.label == label).map(|t| t.index);
            // 2025-12-01 is bar 30, 2026-01-01 bar 61, 2026-02-01 bar 92.
            assert_eq!(at("Dec"), Some(30), "{tz:?} {ticks:?}");
            assert_eq!(at("2026"), Some(61), "{tz:?} {ticks:?}");
            assert_eq!(at("Feb"), Some(92), "{tz:?} {ticks:?}");
            // Day-number labels name each bar's own date.
            for t in ticks.iter().filter(|t| !t.major) {
                let d = date_of(cs[t.index].open_time_ms);
                assert_eq!(t.label, d.day().to_string(), "{tz:?} {t:?}");
            }
        }
    }

    #[test]
    fn monthly_axis_labels_years_on_january_west_of_utc() {
        let cs = month_bars(2024, 36);
        let mut vp = Viewport::new(1080.0, 400.0);
        vp.bar_spacing = 30.0;
        vp.right_index = 36.0;
        for tz in [MINUS5, PLUS530, PLUS545] {
            let ticks = time_ticks(&cs, Timeframe::Mo1, &vp, tz);
            let years: Vec<_> = ticks
                .iter()
                .filter(|t| t.major && t.label.len() == 4)
                .map(|t| (t.index, t.label.as_str()))
                .collect();
            assert_eq!(years, [(12, "2025"), (24, "2026")], "{tz:?} {ticks:?}");
            // Month labels name each bar's own month.
            for t in ticks.iter().filter(|t| t.label.len() == 3) {
                let m = date_of(cs[t.index].open_time_ms).month0() as usize;
                assert_eq!(t.label, MONTHS[m], "{tz:?} {t:?}");
            }
        }
    }

    #[test]
    fn hourly_axis_has_clock_labels_with_any_offset() {
        let t0 = chrono_ts(2026, 11, 9, 0, 0);
        let cs: Vec<Candle> = (0..72).map(|i| bar(t0 + i * H)).collect();
        let mut vp = Viewport::new(720.0, 400.0);
        vp.bar_spacing = 10.0;
        vp.right_index = 72.0;
        for (tz, minute) in [(MINUS5, ":00"), (PLUS530, ":30"), (PLUS545, ":45"), (TzOffset::UTC, ":00")] {
            let ticks = time_ticks(&cs, Timeframe::H1, &vp, tz);
            let clock: Vec<_> = ticks.iter().filter(|t| t.label.contains(':')).collect();
            assert!(clock.len() >= 2, "{tz:?} {ticks:?}");
            assert!(clock.iter().all(|t| t.label.ends_with(minute)), "{tz:?} {ticks:?}");
            // Day changes follow the display zone: the first bar of a local day.
            let local_day = |i: usize| date_of(cs[i].open_time_ms + tz.seconds as i64 * 1000);
            for t in ticks.iter().filter(|t| t.major) {
                assert_ne!(local_day(t.index), local_day(t.index - 1), "{tz:?} {t:?}");
            }
            assert!(ticks.iter().any(|t| t.major), "{tz:?} {ticks:?}");
        }
    }

    #[test]
    fn two_hour_bars_get_labels_with_an_odd_hour_offset() {
        let t0 = chrono_ts(2026, 11, 9, 0, 0);
        let cs: Vec<Candle> = (0..60).map(|i| bar(t0 + i * 2 * H)).collect();
        // 30 px per bar: a label every 6 h. At -5h the bars open at odd
        // local hours, so none is on 00/06/12/18.
        let mut vp = Viewport::new(1800.0, 400.0);
        vp.bar_spacing = 30.0;
        vp.right_index = 60.0;
        let ticks = time_ticks(&cs, Timeframe::H2, &vp, MINUS5);
        let clock: Vec<_> = ticks.iter().filter(|t| t.label.contains(':')).collect();
        assert!(clock.len() >= 4, "{ticks:?}");
        assert!(clock.iter().all(|t| ["01:00", "07:00", "13:00", "19:00"].contains(&t.label.as_str())), "{ticks:?}");
    }
}
