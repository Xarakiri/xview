//! Bottom range bar: span presets, go-to-date (placeholder) and the live
//! clock. Only the clock changes every second, and this view is cached, so
//! the tick re-renders just this bar.

use std::time::Duration;

use chrono::{Datelike, TimeZone, Utc};
use gpui::{Context, IntoElement, Render, SharedString, Task, Window, div, prelude::*, px};
use xv_core::TzOffset;

use crate::theme::AppTheme;
use crate::widgets::{icon_button, tooltip, vsep};
use crate::workspace::{BOTTOM_BAR_HEIGHT, ShowSpan, ToggleClockUtc, tz_for};

const DAY_MS: i64 = 86_400_000;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Range {
    Days(i64),
    YearToDate,
    All,
}

pub const RANGES: [(&str, Range); 9] = [
    ("1D", Range::Days(1)),
    ("5D", Range::Days(5)),
    ("1M", Range::Days(30)),
    ("3M", Range::Days(91)),
    ("6M", Range::Days(182)),
    ("YTD", Range::YearToDate),
    ("1Y", Range::Days(365)),
    ("5Y", Range::Days(5 * 365 + 1)),
    ("All", Range::All),
];

/// The span for `ShowSpan`. YTD runs from Jan 1 00:00 in `tz` of the
/// current year up to `now_ms`.
pub fn span_ms(r: Range, now_ms: i64, tz: TzOffset) -> Option<i64> {
    match r {
        Range::Days(d) => Some(d * DAY_MS),
        Range::All => None,
        Range::YearToDate => {
            let off = tz.seconds as i64 * 1000;
            let local = Utc.timestamp_millis_opt(now_ms + off).single()?;
            let jan1 = Utc
                .with_ymd_and_hms(local.year(), 1, 1, 0, 0, 0)
                .single()?
                .timestamp_millis()
                - off;
            Some((now_ms - jan1).max(DAY_MS))
        }
    }
}

/// `HH:MM:SS UTC±H[:MM]`, or `HH:MM:SS UTC` for a zero offset.
pub fn format_clock(now_ms: i64, tz: TzOffset) -> String {
    let local = now_ms + tz.seconds as i64 * 1000;
    let t = Utc.timestamp_millis_opt(local).single().unwrap_or_default();
    let secs = tz.seconds;
    let zone = if secs == 0 {
        "UTC".to_string()
    } else {
        let sign = if secs > 0 { '+' } else { '-' };
        let (h, m) = (secs.abs() / 3600, secs.abs() % 3600 / 60);
        if m == 0 {
            format!("UTC{sign}{h}")
        } else {
            format!("UTC{sign}{h}:{m:02}")
        }
    };
    format!("{} {zone}", t.format("%H:%M:%S"))
}

pub struct BottomBar {
    theme: AppTheme,
    use_utc: bool,
    _tick: Task<()>,
}

impl BottomBar {
    pub fn new(theme: AppTheme, use_utc: bool, cx: &mut Context<Self>) -> Self {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                // Wake just after the next whole second.
                let ms = 1000 - Utc::now().timestamp_subsec_millis() as u64 % 1000;
                cx.background_executor()
                    .timer(Duration::from_millis(ms + 5))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        BottomBar {
            theme,
            use_utc,
            _tick: tick,
        }
    }

    pub fn set_theme(&mut self, theme: AppTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    pub fn set_use_utc(&mut self, use_utc: bool, cx: &mut Context<Self>) {
        self.use_utc = use_utc;
        cx.notify();
    }
}

impl Render for BottomBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let hover = t.hover;
        let tz = tz_for(self.use_utc);
        let clock = format_clock(Utc::now().timestamp_millis(), tz);
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.))
            .w_full()
            .h(px(BOTTOM_BAR_HEIGHT))
            .px(px(8.))
            .border_t_1()
            .border_color(t.border)
            .bg(t.bg)
            .text_color(t.text)
            .text_size(px(13.))
            .children(RANGES.iter().map(|&(label, range)| {
                div()
                    .id(SharedString::from(format!("range-{label}")))
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(4.))
                    .hover(move |s| s.bg(hover))
                    .child(label)
                    .on_click(move |_, window, cx| {
                        let span =
                            span_ms(range, Utc::now().timestamp_millis(), TzOffset::local_now());
                        window.dispatch_action(Box::new(ShowSpan(span)), cx)
                    })
            }))
            .child(vsep(18., t))
            .child(
                icon_button("goto-date", "calendar", 28., false, false, t)
                    .tooltip(tooltip("Go to date (coming soon)", t)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("clock")
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(4.))
                    .hover(move |s| s.bg(hover))
                    .tooltip(tooltip(
                        if self.use_utc {
                            "Show local time"
                        } else {
                            "Show UTC"
                        },
                        t,
                    ))
                    .child(clock)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleClockUtc), cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-11-07 12:34:56 UTC.
    const NOW: i64 = 1_794_054_896_000;

    #[test]
    fn clock_formats_offsets() {
        assert_eq!(format_clock(NOW, TzOffset::UTC), "12:34:56 UTC");
        assert_eq!(
            format_clock(NOW, TzOffset { seconds: 3 * 3600 }),
            "15:34:56 UTC+3"
        );
        assert_eq!(
            format_clock(NOW, TzOffset { seconds: -5 * 3600 }),
            "07:34:56 UTC-5"
        );
        assert_eq!(
            format_clock(
                NOW,
                TzOffset {
                    seconds: 5 * 3600 + 1800
                }
            ),
            "18:04:56 UTC+5:30"
        );
    }

    #[test]
    fn spans() {
        assert_eq!(
            span_ms(Range::Days(5), NOW, TzOffset::UTC),
            Some(5 * DAY_MS)
        );
        assert_eq!(span_ms(Range::All, NOW, TzOffset::UTC), None);
        let jan1 = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(
            span_ms(Range::YearToDate, NOW, TzOffset::UTC),
            Some(NOW - jan1)
        );
        let tz = TzOffset { seconds: 3600 };
        assert_eq!(
            span_ms(Range::YearToDate, NOW, tz),
            Some(NOW - (jan1 - 3_600_000))
        );
    }
}
