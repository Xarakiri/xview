//! Form controls for the drawing settings dialog: checkbox, select
//! (dropdown button + menu), tab strip, dual-handle range slider, plus the
//! pure parsing helpers behind the price, date-time and range fields. The
//! text fields themselves are `text_input::TextInput`.

use std::cell::Cell;
use std::rc::Rc;

use chrono::{NaiveDate, NaiveDateTime};
use gpui::{
    App, Bounds, Div, ElementId, FontWeight, Pixels, SharedString, Stateful, Window, canvas, div,
    prelude::*, px,
};
use xv_core::TzOffset;
use xv_drawings::Range;

use crate::theme::AppTheme;
use crate::widgets::{icon, menu_box, menu_row};

/// Called with the picked index of a select or tab strip.
pub type OnPick = Rc<dyn Fn(usize, &mut Window, &mut App)>;

pub const FIELD_H: f32 = 34.;
pub const FIELD_RADIUS: f32 = 8.;
const CHECK: f32 = 18.;
const KNOB: f32 = 14.;
const TRACK_H: f32 = 6.;

// ---- pure helpers ----

/// A price typed with `.` or `,` as the decimal separator.
pub fn parse_price(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    if s.is_empty() {
        return None;
    }
    s.parse::<f64>().ok().filter(|p| p.is_finite())
}

fn valid_tick(tick: f64) -> f64 {
    if tick.is_finite() && tick > 0.0 {
        tick
    } else {
        0.01
    }
}

/// A price for a field: the tick's decimals, no thousands separator.
pub fn format_field_price(p: f64, tick: f64) -> String {
    format!("{:.*}", xv_chart::decimals_of(valid_tick(tick)), p)
}

/// `p` moved `dir` ticks, on the tick grid.
pub fn step_price(p: f64, tick: f64, dir: i32) -> f64 {
    let tick = valid_tick(tick);
    ((p / tick).round() + dir as f64) * tick
}

/// `YYYY-MM-DD HH:MM` of `ms` in `tz`.
pub fn format_datetime(ms: i64, tz: TzOffset) -> String {
    let local = ms.saturating_add(tz.seconds as i64 * 1000);
    chrono::DateTime::from_timestamp_millis(local)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// The instant of `YYYY-MM-DD HH:MM` (or `YYYY-MM-DD`, as 00:00) in `tz`.
pub fn parse_datetime(s: &str, tz: TzOffset) -> Option<i64> {
    let s = s.trim();
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M")
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
        })?;
    Some(naive.and_utc().timestamp_millis() - tz.seconds as i64 * 1000)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeEnd {
    Min,
    Max,
}

/// `r` with one end set to `v`, clamped to `1..=bound` and to the other end.
pub fn set_range_end(r: Range, bound: u16, end: RangeEnd, v: i64) -> Range {
    let bound = bound.max(1);
    let (min, max) = (r.min.clamp(1, bound), r.max.clamp(1, bound));
    match end {
        RangeEnd::Min => Range {
            min: v.clamp(1, max as i64) as u16,
            max,
            ..r
        },
        RangeEnd::Max => Range {
            max: v.clamp(min as i64, bound as i64) as u16,
            min,
            ..r
        },
    }
}

/// The step-1 value at fraction `t` (0..=1) of a `1..=bound` track.
pub fn slider_value(t: f32, bound: u16) -> i64 {
    let t = t.clamp(0.0, 1.0) as f64;
    1 + (t * (bound.max(1) - 1) as f64).round() as i64
}

/// Fraction of the track where `v` sits.
pub fn slider_frac(v: u16, bound: u16) -> f32 {
    if bound <= 1 {
        return 0.0;
    }
    (v.saturating_sub(1) as f32 / (bound - 1) as f32).clamp(0.0, 1.0)
}

/// The handle a press at value `v` grabs: the nearer one (Max on a tie
/// above, so two stacked handles can still be pulled apart).
pub fn nearest_end(r: Range, v: i64) -> RangeEnd {
    let (dmin, dmax) = ((v - r.min as i64).abs(), (v - r.max as i64).abs());
    if dmin < dmax || (dmin == dmax && v < r.min as i64) {
        RangeEnd::Min
    } else {
        RangeEnd::Max
    }
}

// ---- controls ----

/// A checkbox with a label; the caller adds `on_click`.
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: bool,
    label: impl Into<SharedString>,
    t: &AppTheme,
) -> Stateful<Div> {
    let mut b = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(CHECK))
        .rounded(px(4.));
    b = if checked {
        b.bg(t.text).child(icon("check", 18., t.bg))
    } else {
        b.border_1().border_color(t.text)
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .cursor_pointer()
        .child(b)
        .child(label.into())
}

/// A dropdown button: a rounded border, the current value and a chevron.
pub fn select_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    open: bool,
    w: f32,
    t: &AppTheme,
) -> Stateful<Div> {
    let hover = t.hover;
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .flex_row()
        .items_center()
        .w(px(w))
        .h(px(FIELD_H))
        .pl(px(12.))
        .pr(px(6.))
        .rounded(px(FIELD_RADIUS))
        .border_1()
        .border_color(t.border)
        .when(open, |d| d.bg(t.hover))
        .when(!open, |d| d.hover(move |s| s.bg(hover)))
        .cursor_pointer()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .child(label.into()),
        )
        .child(icon("chevron_down", 22., t.text))
}

/// A bordered square button (swatches, end styles, B / I toggles).
pub fn square_button(id: impl Into<ElementId>, active: bool, t: &AppTheme) -> Stateful<Div> {
    let hover = t.hover;
    div()
        .id(id)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(FIELD_H))
        .rounded(px(FIELD_RADIUS))
        .border_1()
        .border_color(t.border)
        .when(active, |d| d.bg(t.active_bg))
        .when(!active, |d| d.hover(move |s| s.bg(hover)))
        .cursor_pointer()
}

/// A select's menu; `on_pick` gets the index.
pub fn select_menu(
    id: &'static str,
    items: &[(SharedString, bool)],
    min_w: f32,
    on_pick: OnPick,
    t: &AppTheme,
) -> Div {
    let mut menu = menu_box(t).min_w(px(min_w)).py(px(4.));
    for (i, (label, active)) in items.iter().enumerate() {
        let pick = on_pick.clone();
        menu = menu.child(
            menu_row(
                SharedString::from(format!("{id}-{i}")),
                None,
                label.clone(),
                *active,
                true,
                t,
            )
            .text_size(px(15.))
            .on_click(move |_, window, cx| pick(i, window, cx)),
        );
    }
    menu
}

/// The tab strip: semibold labels over a light track, the active one
/// underlined.
pub fn tab_strip(labels: &[&'static str], active: usize, on_pick: OnPick, t: &AppTheme) -> Div {
    let mut row = div().flex().flex_row().gap(px(26.));
    for (i, label) in labels.iter().enumerate() {
        let pick = on_pick.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("tab-{i}")))
                .relative()
                .flex()
                .flex_col()
                .pb(px(10.))
                .cursor_pointer()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .when(i != active, |d| d.text_color(t.text.opacity(0.85)))
                .child(*label)
                .when(i == active, |d| {
                    d.child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0()
                            .h(px(3.))
                            .rounded(px(1.5))
                            .bg(t.text),
                    )
                })
                .on_click(move |_, window, cx| pick(i, window, cx)),
        );
    }
    div()
        .relative()
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(3.))
                .rounded(px(1.5))
                .bg(t.hover),
        )
        .child(row)
}

/// A dual-handle track for `r` on `1..=bound`. `cell` receives the track
/// bounds each paint; the caller handles presses and drags.
pub fn range_track(
    id: impl Into<ElementId>,
    r: Range,
    bound: u16,
    cell: Rc<Cell<Bounds<Pixels>>>,
    t: &AppTheme,
) -> Stateful<Div> {
    let (a, b) = (
        slider_frac(r.min, bound),
        slider_frac(r.max.max(r.min), bound),
    );
    let knob = |f: f32| {
        div()
            .absolute()
            .top(px(0.))
            .left(gpui::relative(f))
            .ml(px(-KNOB / 2.))
            .size(px(KNOB))
            .rounded(px(KNOB / 2.))
            .border_2()
            .border_color(t.text)
            .bg(gpui::white())
    };
    let fill = if r.on { t.text } else { t.muted };
    div()
        .id(id)
        .relative()
        .flex_1()
        .h(px(KNOB))
        .mx(px(KNOB / 2. + 2.))
        .cursor_pointer()
        .child(
            canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px((KNOB - TRACK_H) / 2.))
                .h(px(TRACK_H))
                .rounded(px(TRACK_H / 2.))
                .bg(t.border),
        )
        .child(
            div()
                .absolute()
                .left(gpui::relative(a))
                .w(gpui::relative(b - a))
                .top(px((KNOB - TRACK_H - 2.) / 2.))
                .h(px(TRACK_H + 2.))
                .bg(fill),
        )
        .child(knob(a))
        .child(knob(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_parse_format_roundtrip_with_tz() {
        // 2026-11-07 13:40 UTC.
        let ms = 1_794_058_800_000;
        assert_eq!(format_datetime(ms, TzOffset::UTC), "2026-11-07 13:40");
        let plus3 = TzOffset { seconds: 3 * 3600 };
        assert_eq!(format_datetime(ms, plus3), "2026-11-07 16:40");
        assert_eq!(parse_datetime("2026-11-07 16:40", plus3), Some(ms));
        assert_eq!(parse_datetime(&format_datetime(ms, plus3), plus3), Some(ms));
        let minus5 = TzOffset { seconds: -5 * 3600 };
        assert_eq!(
            parse_datetime(&format_datetime(ms, minus5), minus5),
            Some(ms)
        );
        // A date alone is local midnight.
        assert_eq!(
            parse_datetime("2026-11-07", plus3),
            Some(1_794_009_600_000 - 3 * 3_600_000)
        );
        assert_eq!(parse_datetime(" 2026-11-07 16:40 ", plus3), Some(ms));
        for bad in [
            "",
            "2026-13-01",
            "2026-11-07 25:00",
            "07.11.2026",
            "2026-11-07 16",
        ] {
            assert_eq!(parse_datetime(bad, plus3), None, "{bad}");
        }
    }

    #[test]
    fn price_step_uses_tick() {
        assert!((step_price(4345.5, 0.5, 1) - 4346.0).abs() < 1e-9);
        assert!((step_price(4345.5, 0.5, -1) - 4345.0).abs() < 1e-9);
        // Off-grid prices snap to the grid, then step.
        assert!((step_price(1.234, 0.01, 1) - 1.24).abs() < 1e-9);
        assert_eq!(format_field_price(step_price(0.1, 0.1, 2), 0.1), "0.3");
        assert_eq!(format_field_price(4345.5, 0.5), "4345.5");
        assert_eq!(format_field_price(64000.0, 0.01), "64000.00");
        // A bad tick falls back to 0.01.
        assert!((step_price(1.0, 0.0, 1) - 1.01).abs() < 1e-9);
        assert_eq!(parse_price("4345,5"), Some(4345.5));
        assert_eq!(parse_price(" 12.25 "), Some(12.25));
        assert_eq!(parse_price(""), None);
        assert_eq!(parse_price("1.2.3"), None);
        assert_eq!(parse_price("inf"), None);
    }

    #[test]
    fn range_slider_clamps() {
        let r = Range {
            on: true,
            min: 5,
            max: 30,
        };
        assert_eq!(
            set_range_end(r, 59, RangeEnd::Min, 10),
            Range { min: 10, ..r }
        );
        // Min stops at max; max stops at min; both stay in 1..=bound.
        assert_eq!(set_range_end(r, 59, RangeEnd::Min, 45).min, 30);
        assert_eq!(set_range_end(r, 59, RangeEnd::Min, -3).min, 1);
        assert_eq!(set_range_end(r, 59, RangeEnd::Max, 2).max, 5);
        assert_eq!(set_range_end(r, 59, RangeEnd::Max, 99).max, 59);
        assert!(set_range_end(r, 59, RangeEnd::Max, 99).on);
        // Track positions at step 1.
        assert_eq!(slider_value(0.0, 59), 1);
        assert_eq!(slider_value(1.0, 59), 59);
        assert_eq!(slider_value(0.5, 59), 30);
        assert_eq!(slider_value(2.0, 24), 24);
        assert_eq!(slider_frac(1, 12), 0.0);
        assert_eq!(slider_frac(12, 12), 1.0);
        assert_eq!(nearest_end(r, 6), RangeEnd::Min);
        assert_eq!(nearest_end(r, 29), RangeEnd::Max);
        let stacked = Range {
            on: true,
            min: 7,
            max: 7,
        };
        assert_eq!(nearest_end(stacked, 3), RangeEnd::Min);
        assert_eq!(nearest_end(stacked, 9), RangeEnd::Max);
    }
}
