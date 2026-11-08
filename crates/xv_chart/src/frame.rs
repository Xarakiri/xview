//! Converts chart state into drawing primitives, free of any GPUI types.
//!
//! Coordinate conventions:
//! - Every `Prim` coordinate is relative to the chart element origin (the top
//!   left of the whole chart, axes included). Plot prims are already offset by
//!   `layout.plot.{x,y}`, so the painter only needs to clip `Frame::plot` to
//!   `layout.plot`.
//! - `ChartProjector` and `Viewport` work in plot-local pixels (origin at the
//!   plot's top left); `Viewport::{width,height}` must equal the plot size.
//! - `FrameInput::crosshair` is in chart element coordinates.
//! - `Prim::Text::pos` is the anchor point: x per `align`, y is the vertical
//!   center of the text line.
//! - Colors are `0xRRGGBBAA`.

use crate::{
    ChartColors, ChartProjector, PriceRange, VOLUME_FRACTION, Viewport, format_crosshair_time,
    format_price, index_to_time, price_ticks, time_ticks, time_to_index,
};
use xv_core::{Series, Timeframe, TzOffset};
use xv_drawings::{
    Anchor, Drawing, DrawingId, DrawingKind, Extend, HAlign, LineEnd, LineStyle, Projector,
    StatsMode, StatsPos, TextStyle, VAlign, handles,
};

pub const PRICE_AXIS_WIDTH: f32 = 72.0;
pub const TIME_AXIS_HEIGHT: f32 = 28.0;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ChartLayout {
    pub plot: RectF,
    pub price_axis: RectF,
    pub time_axis: RectF,
}

impl ChartLayout {
    pub fn new(width: f32, height: f32) -> Self {
        let pw = (width - PRICE_AXIS_WIDTH).max(0.0);
        let ph = (height - TIME_AXIS_HEIGHT).max(0.0);
        ChartLayout {
            plot: RectF {
                x: 0.0,
                y: 0.0,
                w: pw,
                h: ph,
            },
            price_axis: RectF {
                x: pw,
                y: 0.0,
                w: width.clamp(0.0, PRICE_AXIS_WIDTH),
                h: ph,
            },
            time_axis: RectF {
                x: 0.0,
                y: ph,
                w: pw,
                h: height.clamp(0.0, TIME_AXIS_HEIGHT),
            },
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Dash {
    Solid,
    Dashed,
    Dotted,
}

impl From<LineStyle> for Dash {
    fn from(s: LineStyle) -> Self {
        match s {
            LineStyle::Solid => Dash::Solid,
            LineStyle::Dashed => Dash::Dashed,
            LineStyle::Dotted => Dash::Dotted,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Rect {
        r: RectF,
        color: u32,
    },
    StrokeRect {
        r: RectF,
        color: u32,
        width: f32,
    },
    Line {
        a: (f32, f32),
        b: (f32, f32),
        color: u32,
        width: f32,
        dash: Dash,
    },
    Text {
        pos: (f32, f32),
        text: String,
        color: u32,
        align: TextAlign,
        bg: Option<u32>,
        /// Font size in px.
        size: f32,
        bold: bool,
        italic: bool,
    },
    /// A selection handle; `filled: false` (a locked drawing) is drawn
    /// without the background disc.
    Handle {
        center: (f32, f32),
        color: u32,
        filled: bool,
    },
    /// A filled polygon (arrowheads, the middle-point dot).
    Polygon {
        pts: Vec<(f32, f32)>,
        color: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Legend {
    pub title: String,
    pub o: String,
    pub h: String,
    pub l: String,
    pub c: String,
    pub v: String,
    pub change: String,
    pub up: bool,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub plot: Vec<Prim>,
    pub price_axis: Vec<Prim>,
    pub time_axis: Vec<Prim>,
    pub legend: Legend,
}

pub struct FrameInput<'a> {
    pub series: &'a Series,
    pub vp: &'a Viewport,
    pub layout: &'a ChartLayout,
    pub crosshair: Option<(f32, f32)>,
    pub drawings: &'a [Drawing],
    pub drawings_hidden: bool,
    pub selected: Option<DrawingId>,
    pub colors: &'a ChartColors,
    pub tz: TzOffset,
    pub source_name: &'a str,
}

impl Prim {
    fn is_finite(&self) -> bool {
        match self {
            Prim::Rect { r, .. } => [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()),
            Prim::StrokeRect { r, width, .. } => {
                [r.x, r.y, r.w, r.h, *width].iter().all(|v| v.is_finite())
            }
            Prim::Line { a, b, width, .. } => {
                [a.0, a.1, b.0, b.1, *width].iter().all(|v| v.is_finite())
            }
            Prim::Text { pos, .. } => pos.0.is_finite() && pos.1.is_finite(),
            Prim::Handle { center, .. } => center.0.is_finite() && center.1.is_finite(),
            Prim::Polygon { pts, .. } => pts.iter().all(|p| p.0.is_finite() && p.1.is_finite()),
        }
    }

    fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Prim::Rect { r, .. } | Prim::StrokeRect { r, .. } => {
                r.x += dx;
                r.y += dy;
            }
            Prim::Line { a, b, .. } => {
                *a = (a.0 + dx, a.1 + dy);
                *b = (b.0 + dx, b.1 + dy);
            }
            Prim::Text { pos, .. } => *pos = (pos.0 + dx, pos.1 + dy),
            Prim::Handle { center, .. } => *center = (center.0 + dx, center.1 + dy),
            Prim::Polygon { pts, .. } => pts
                .iter_mut()
                .for_each(|p| *p = (p.0 + dx, p.1 + dy)),
        }
    }
}

/// Collects primitives, dropping any with a non-finite coordinate so garbage
/// input (NaN prices, empty ranges) can never reach the painter.
struct Out(Vec<Prim>);

impl Out {
    fn push(&mut self, p: Prim) {
        if p.is_finite() {
            self.0.push(p);
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), color: u32, width: f32, dash: Dash) {
        self.push(Prim::Line {
            a,
            b,
            color,
            width,
            dash,
        });
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        self.push(Prim::Rect {
            r: RectF { x, y, w, h },
            color,
        });
    }

    fn text(
        &mut self,
        pos: (f32, f32),
        text: String,
        color: u32,
        align: TextAlign,
        bg: Option<u32>,
    ) {
        self.push(Prim::Text {
            pos,
            text,
            color,
            align,
            bg,
            size: DEFAULT_TEXT_SIZE,
            bold: false,
            italic: false,
        });
    }
}

/// Font size of axis labels, stats and other built-in text.
pub const DEFAULT_TEXT_SIZE: f32 = 12.0;
const BODY_FRACTION: f32 = 0.8;
const THIN_BAR_SPACING: f32 = 2.0;
const AXIS_LABEL_PAD: f32 = 8.0;
/// Height of a boxed axis label (last price, crosshair), centered on its y.
/// Plain tick labels within this distance of one are hidden under it.
pub const AXIS_LABEL_HEIGHT: f32 = 18.0;

/// Whether a tick label at `y` would collide with a boxed label at one of
/// `boxed` (all plot coordinates).
fn hidden_by_boxed_label(y: f32, boxed: &[f32]) -> bool {
    boxed.iter().any(|b| (y - b).abs() < AXIS_LABEL_HEIGHT)
}

fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xFFFF_FF00) | alpha as u32
}

fn sane_viewport(vp: &Viewport) -> Viewport {
    let mut vp = vp.clone();
    let span = vp.price.max - vp.price.min;
    if !(span.is_finite() && span > 0.0) {
        let mid = if vp.price.min.is_finite() {
            vp.price.min
        } else {
            0.0
        };
        vp.price = PriceRange {
            min: mid - 0.5,
            max: mid + 0.5,
        };
    }
    if !(vp.bar_spacing.is_finite() && vp.bar_spacing > 0.0) {
        vp.bar_spacing = crate::DEFAULT_BAR_SPACING;
    }
    if !vp.right_index.is_finite() {
        vp.right_index = 0.0;
    }
    vp
}

fn format_volume(v: f64) -> String {
    let a = v.abs();
    if !v.is_finite() {
        "-".into()
    } else if a >= 1e9 {
        format!("{:.2}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.2}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.2}K", v / 1e3)
    } else {
        format!("{v:.2}")
    }
}

fn build_legend(i: &FrameInput, idx: Option<usize>) -> Legend {
    let candles = i.series.candles();
    let tick = i.series.symbol.tick_size;
    let title = format!(
        "{} · {} · {}",
        i.series.symbol.display,
        i.series.timeframe.label(),
        i.source_name
    );
    let idx = idx
        .filter(|k| *k < candles.len())
        .or(candles.len().checked_sub(1));
    let Some(k) = idx else {
        let dash = || "-".to_string();
        return Legend {
            title,
            o: dash(),
            h: dash(),
            l: dash(),
            c: dash(),
            v: dash(),
            change: dash(),
            up: true,
        };
    };
    let c = &candles[k];
    let prev = if k > 0 { candles[k - 1].close } else { c.open };
    Legend {
        title,
        o: format_price(c.open, tick),
        h: format_price(c.high, tick),
        l: format_price(c.low, tick),
        c: format_price(c.close, tick),
        v: format_volume(c.volume),
        change: format_change(prev, c.close, tick),
        up: c.close >= prev,
    }
}

/// `+12.34 (+1.23%)`: the change from `from` to `to`, at the tick's decimals.
fn format_change(from: f64, to: f64, tick: f64) -> String {
    let diff = to - from;
    let pct = if from != 0.0 {
        diff / from * 100.0
    } else {
        0.0
    };
    let sign = if diff >= 0.0 { "+" } else { "-" };
    format!(
        "{sign}{} ({sign}{:.2}%)",
        format_price(diff.abs(), tick),
        pct.abs()
    )
}

/// A duration in its two largest units: `2w 3d`, `1d 3h`, `45m`.
fn format_span(ms: i64) -> String {
    const UNITS: [(&str, u64); 4] = [("w", 10_080), ("d", 1_440), ("h", 60), ("m", 1)];
    let mins = ms.unsigned_abs() / 60_000;
    let Some(k) = UNITS.iter().position(|(_, n)| mins >= *n) else {
        return "0m".into();
    };
    let (u, n) = UNITS[k];
    let rest = mins % n;
    match UNITS.get(k + 1) {
        Some((u2, n2)) if rest / n2 > 0 => format!("{}{u} {}{u2}", mins / n, rest / n2),
        _ => format!("{}{u}", mins / n),
    }
}

pub fn build_frame(i: &FrameInput) -> Frame {
    let candles = i.series.candles();
    let tf = i.series.timeframe;
    let tick = i.series.symbol.tick_size;
    let colors = i.colors;
    let vp = sane_viewport(i.vp);
    let (pw, ph) = (i.layout.plot.w, i.layout.plot.h);
    let (ox, oy) = (i.layout.plot.x, i.layout.plot.y);
    let pa = i.layout.price_axis;
    let ta = i.layout.time_axis;
    let label_x = pa.x + AXIS_LABEL_PAD;

    let mut plot = Out(Vec::new());
    let mut price_axis = Out(Vec::new());
    let mut time_axis = Out(Vec::new());

    // Boxed price-axis labels drawn later: the last price and the crosshair.
    let crosshair = i
        .crosshair
        .map(|(x, y)| (x - ox, y - oy))
        .filter(|(cx, cy)| (0.0..=pw).contains(cx) && (0.0..=ph).contains(cy));
    let mut boxed: Vec<f32> = candles
        .last()
        .map(|c| vp.y_of_price(c.close))
        .filter(|y| (0.0..=ph).contains(y))
        .into_iter()
        .chain(crosshair.map(|(_, cy)| cy))
        .collect();
    // Anchor price labels of the shown drawings: (y, background, price).
    let mut labels: Vec<(f32, u32, f64)> = Vec::new();
    if !i.drawings_hidden {
        for d in i.drawings.iter().filter(|d| is_shown(d, tf)) {
            if !(d.style.price_labels || i.selected == Some(d.id)) {
                continue;
            }
            let (prices, n) = anchor_prices(&d.kind);
            for &p in &prices[..n] {
                let y = vp.y_of_price(p);
                if (0.0..=ph).contains(&y) {
                    labels.push((y, with_alpha(d.style.color, 0xFF), p));
                }
            }
        }
    }
    boxed.extend(labels.iter().map(|l| l.0));

    // Grid and axis tick labels.
    for v in price_ticks(vp.price, vp.height, tick) {
        let y = vp.y_of_price(v);
        if (0.0..=ph).contains(&y) {
            plot.line((0.0, y), (pw, y), colors.grid, 1.0, Dash::Solid);
            if hidden_by_boxed_label(y, &boxed) {
                continue;
            }
            price_axis.text(
                (label_x, oy + y),
                format_price(v, tick),
                colors.text,
                TextAlign::Left,
                None,
            );
        }
    }
    for t in time_ticks(candles, tf, &vp, i.tz) {
        let x = vp.x_of_index(t.index as f64);
        plot.line((x, 0.0), (x, ph), colors.grid, 1.0, Dash::Solid);
        let color = if t.major {
            colors.text
        } else {
            colors.muted_text
        };
        time_axis.text(
            (ox + x, ta.y + ta.h / 2.0),
            t.label,
            color,
            TextAlign::Center,
            None,
        );
    }

    // Volume bars, then candles; only the visible slice is touched.
    let range = vp.visible_range(candles.len());
    let vis = &candles[range.clone()];
    let bar_w = (vp.bar_spacing * BODY_FRACTION).max(1.0);
    let max_vol = vis
        .iter()
        .map(|c| c.volume)
        .filter(|v| v.is_finite())
        .fold(0.0, f64::max);
    if max_vol > 0.0 {
        let vol_h = ph * VOLUME_FRACTION;
        for (k, c) in vis.iter().enumerate() {
            if c.volume.is_nan() || c.volume <= 0.0 {
                continue;
            }
            let h = (c.volume / max_vol) as f32 * vol_h;
            let x = vp.x_of_index((range.start + k) as f64);
            let color = if c.close >= c.open {
                colors.up_volume
            } else {
                colors.down_volume
            };
            plot.rect(x - bar_w / 2.0, ph - h, bar_w, h, color);
        }
    }
    let thin = vp.bar_spacing < THIN_BAR_SPACING;
    for (k, c) in vis.iter().enumerate() {
        let x = vp.x_of_index((range.start + k) as f64);
        let color = if c.close >= c.open {
            colors.up
        } else {
            colors.down
        };
        let (yh, yl) = (vp.y_of_price(c.high), vp.y_of_price(c.low));
        plot.line((x, yh), (x, yl), color, 1.0, Dash::Solid);
        if !thin {
            let (yo, yc) = (vp.y_of_price(c.open), vp.y_of_price(c.close));
            let top = yo.min(yc);
            plot.rect(x - bar_w / 2.0, top, bar_w, (yo - yc).abs().max(1.0), color);
        }
    }

    // Drawings and selection handles.
    let proj = ChartProjector {
        candles,
        tf,
        vp: &vp,
    };
    if !i.drawings_hidden {
        for d in i.drawings.iter().filter(|d| is_shown(d, tf)) {
            draw_drawing(d, &proj, i, &mut plot);
        }
        if let Some(sel) = i
            .selected
            .and_then(|id| i.drawings.iter().find(|d| d.id == id))
            .filter(|d| is_shown(d, tf))
        {
            // Locked: hollow, half-transparent handles.
            let (color, filled) = if sel.locked {
                (with_alpha(colors.selection, LOCKED_HANDLE_ALPHA), false)
            } else {
                (colors.selection, true)
            };
            for h in handles(sel, &proj) {
                plot.push(Prim::Handle {
                    center: h,
                    color,
                    filled,
                });
            }
        }
    }
    for (y, bg, p) in labels {
        price_axis.text(
            (label_x, oy + y),
            format_price(p, tick),
            contrast_text(bg),
            TextAlign::Left,
            Some(bg),
        );
    }

    // Last price.
    if let Some(last) = candles.last() {
        let y = vp.y_of_price(last.close);
        if (0.0..=ph).contains(&y) {
            let color = if last.close >= last.open {
                colors.last_price_up
            } else {
                colors.last_price_down
            };
            plot.line((0.0, y), (pw, y), color, 1.0, Dash::Dashed);
            price_axis.text(
                (label_x, oy + y),
                format_price(last.close, tick),
                colors.axis_label_text,
                TextAlign::Left,
                Some(color),
            );
        }
    }

    // Crosshair (input is in chart element coordinates).
    let mut hover_idx = None;
    if let Some((cx, cy)) = crosshair {
        let idx = vp.index_of_x(cx).round();
        let sx = vp.x_of_index(idx);
        if (0.0..candles.len() as f64).contains(&idx) {
            hover_idx = Some(idx as usize);
        }
        plot.line((sx, 0.0), (sx, ph), colors.crosshair, 1.0, Dash::Dashed);
        plot.line((0.0, cy), (pw, cy), colors.crosshair, 1.0, Dash::Dashed);
        price_axis.text(
            (label_x, oy + cy),
            format_price(vp.price_of_y(cy), tick),
            colors.axis_label_text,
            TextAlign::Left,
            Some(colors.axis_label_bg),
        );
        let ts = index_to_time(candles, tf, idx);
        time_axis.text(
            (ox + sx, ta.y + ta.h / 2.0),
            format_crosshair_time(ts, tf, i.tz),
            colors.axis_label_text,
            TextAlign::Center,
            Some(colors.axis_label_bg),
        );
    }

    let mut plot = plot.0;
    if ox != 0.0 || oy != 0.0 {
        plot.iter_mut().for_each(|p| p.translate(ox, oy));
    }
    Frame {
        plot,
        price_axis: price_axis.0,
        time_axis: time_axis.0,
        legend: build_legend(i, hover_idx),
    }
}

/// Whether `d` is drawn on timeframe `tf`.
fn is_shown(d: &Drawing, tf: Timeframe) -> bool {
    !d.hidden && d.style.visibility.shows(tf)
}

/// The anchor prices a drawing labels on the price axis (the first `n`).
fn anchor_prices(k: &DrawingKind) -> ([f64; 2], usize) {
    match k {
        DrawingKind::TrendLine { a, b }
        | DrawingKind::Ray { a, b }
        | DrawingKind::Rectangle { a, b } => ([a.price, b.price], 2),
        DrawingKind::HorizontalLine { price } => ([*price, 0.0], 1),
        DrawingKind::VerticalLine { .. } => ([0.0; 2], 0),
    }
}

/// White or XView's dark text, whichever reads on `bg`.
fn contrast_text(bg: u32) -> u32 {
    let ch = |s: u32| ((bg >> s) & 0xFF) as f32 / 255.0;
    let lum = 0.299 * ch(24) + 0.587 * ch(16) + 0.114 * ch(8);
    if lum > 0.6 { DARK_LABEL_TEXT } else { 0xFFFF_FFFF }
}

const LOCKED_HANDLE_ALPHA: u8 = 0x80;
const DARK_LABEL_TEXT: u32 = 0x1F2933FF;
const ARROW_LEN: f64 = 10.0;
const ARROW_HALF_WIDTH: f64 = 4.0;
const MIDDLE_DOT_RADIUS: f64 = 3.0;
const MIDDLE_DOT_SEGMENTS: usize = 12;
/// Gap between a drawing and its text or stats.
const TEXT_GAP: f32 = 4.0;
const STATS_GAP: f32 = 8.0;
/// Offset below a point for stats placed under it.
const STATS_BELOW: f32 = 14.0;
/// How far outside the plot a rectangle edge is kept (hidden by the clip).
const RECT_OVERSHOOT: f64 = 8.0;

type P64 = (f64, f64);

/// `ChartProjector::to_screen` in f64, for clipping far-off anchors before
/// they are cast to f32.
fn screen64(proj: &ChartProjector, a: Anchor) -> P64 {
    let vp = proj.vp;
    let idx = time_to_index(proj.candles, proj.tf, a.time_ms);
    let span = vp.price.max - vp.price.min;
    (
        vp.width as f64 - (vp.right_index - idx) * vp.bar_spacing as f64,
        vp.height as f64 - (a.price - vp.price.min) / span * vp.height as f64,
    )
}

fn f32p(p: P64) -> (f32, f32) {
    (p.0 as f32, p.1 as f32)
}

/// Liang–Barsky: the part of `p0 + t (p1 - p0)` for `t` in `t0..=t1` (either
/// may be infinite) inside `0..=w × 0..=h`. An end cut by an edge lands on it
/// exactly.
fn clip_line(p0: P64, p1: P64, mut t0: f64, mut t1: f64, w: f64, h: f64) -> Option<[(f32, f32); 2]> {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let (mut e0, mut e1) = (None, None);
    for (p, q, edge) in [
        (-dx, p0.0, (true, 0.0)),
        (dx, w - p0.0, (true, w)),
        (-dy, p0.1, (false, 0.0)),
        (dy, h - p0.1, (false, h)),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t0 {
                t0 = r;
                e0 = Some(edge);
            }
        } else if r < t1 {
            t1 = r;
            e1 = Some(edge);
        }
    }
    if !(t0.is_finite() && t1.is_finite()) || t0 > t1 {
        return None;
    }
    let at = |t: f64, edge: Option<(bool, f64)>| {
        let (mut x, mut y) = (p0.0 + t * dx, p0.1 + t * dy);
        match edge {
            Some((true, v)) => x = v,
            Some((false, v)) => y = v,
            None => {}
        }
        f32p((x, y))
    };
    Some([at(t0, e0), at(t1, e1)])
}

/// Whether `p` is within `pad` px of the plot.
fn near_plot(p: P64, w: f32, h: f32, pad: f64) -> bool {
    (-pad..=w as f64 + pad).contains(&p.0) && (-pad..=h as f64 + pad).contains(&p.1)
}

/// A filled 10×8 px arrowhead with its tip at `tip`, pointing along unit `u`.
fn arrowhead(plot: &mut Out, tip: P64, u: P64, color: u32) {
    let base = (tip.0 - u.0 * ARROW_LEN, tip.1 - u.1 * ARROW_LEN);
    let n = (-u.1 * ARROW_HALF_WIDTH, u.0 * ARROW_HALF_WIDTH);
    plot.push(Prim::Polygon {
        pts: vec![
            f32p(tip),
            f32p((base.0 + n.0, base.1 + n.1)),
            f32p((base.0 - n.0, base.1 - n.1)),
        ],
        color,
    });
}

/// The middle-point dot.
fn dot(plot: &mut Out, c: P64, color: u32) {
    let pts = (0..MIDDLE_DOT_SEGMENTS)
        .map(|k| {
            let t = k as f64 / MIDDLE_DOT_SEGMENTS as f64 * std::f64::consts::TAU;
            f32p((
                c.0 + MIDDLE_DOT_RADIUS * t.cos(),
                c.1 + MIDDLE_DOT_RADIUS * t.sin(),
            ))
        })
        .collect();
    plot.push(Prim::Polygon { pts, color });
}

/// Line height of drawing text.
fn text_line_height(t: &TextStyle) -> f32 {
    t.size as f32 * 1.25
}

/// Height of the drawing's text block (0 without text).
fn text_block_height(t: &TextStyle) -> f32 {
    if t.content.is_empty() {
        0.0
    } else {
        t.content.split('\n').count() as f32 * text_line_height(t)
    }
}

/// Emits the drawing's text, one `Prim::Text` per line, the block's top at `top`.
fn text_block(plot: &mut Out, t: &TextStyle, x: f32, align: TextAlign, top: f32) {
    let lh = text_line_height(t);
    for (k, line) in t.content.split('\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        plot.push(Prim::Text {
            pos: (x, top + lh * (k as f32 + 0.5)),
            text: line.to_string(),
            color: t.color,
            align,
            bg: None,
            size: t.size as f32,
            bold: t.bold,
            italic: t.italic,
        });
    }
}

/// Stats text of a two-point drawing (`rect` for a rectangle).
fn stats_text(d: &Drawing, a: Anchor, b: Anchor, pa: P64, pb: P64, rect: bool, i: &FrameInput) -> String {
    let tick = i.series.symbol.tick_size;
    let mut s = format_change(a.price, b.price, tick);
    if d.style.stats != StatsMode::Values {
        return s;
    }
    let (cs, tf) = (i.series.candles(), i.series.timeframe);
    let bars = (time_to_index(cs, tf, b.time_ms) - time_to_index(cs, tf, a.time_ms)).round();
    s.push_str(&format!(", {bars:.0} bars"));
    if !rect {
        let deg = (-(pb.1 - pa.1)).atan2(pb.0 - pa.0).to_degrees();
        s.push_str(&format!(
            ", {}, {:.1}°",
            format_span(b.time_ms - a.time_ms),
            deg
        ));
    }
    s
}

fn stats_visible(d: &Drawing, i: &FrameInput) -> bool {
    d.style.stats != StatsMode::Hidden && (d.style.always_stats || i.selected == Some(d.id))
}

fn push_stats(plot: &mut Out, text: String, pos: (f32, f32), align: TextAlign, i: &FrameInput) {
    plot.push(Prim::Text {
        pos,
        text,
        color: i.colors.stats_text,
        align,
        bg: Some(i.colors.stats_bg),
        size: DEFAULT_TEXT_SIZE,
        bold: false,
        italic: false,
    });
}

fn draw_drawing(d: &Drawing, proj: &ChartProjector, i: &FrameInput, plot: &mut Out) {
    let (w, h) = (i.layout.plot.w, i.layout.plot.h);
    let st = &d.style;
    let color = st.color;
    let width = st.width as f32;
    let dash = Dash::from(st.line_style);
    let at = |a: Anchor| proj.to_screen(a);
    match &d.kind {
        DrawingKind::TrendLine { a, b } => draw_two_point(d, *a, *b, false, proj, i, plot),
        DrawingKind::Ray { a, b } => draw_two_point(d, *a, *b, true, proj, i, plot),
        DrawingKind::HorizontalLine { price } => {
            let y = at(Anchor {
                time_ms: 0,
                price: *price,
            })
            .1;
            plot.line((0.0, y), (w, y), color, width, dash);
            if !st.text.content.is_empty() && (0.0..=h).contains(&y) {
                let (x, align) = match st.text.halign {
                    HAlign::Left => (TEXT_GAP, TextAlign::Left),
                    HAlign::Center => (w / 2.0, TextAlign::Center),
                    HAlign::Right => (w - TEXT_GAP, TextAlign::Right),
                };
                let bh = text_block_height(&st.text);
                let top = match st.text.valign {
                    VAlign::Top => y - TEXT_GAP - bh,
                    VAlign::Middle => y - bh / 2.0,
                    VAlign::Bottom => y + TEXT_GAP,
                };
                text_block(plot, &st.text, x, align, top);
            }
        }
        DrawingKind::VerticalLine { time_ms } => {
            let x = at(Anchor {
                time_ms: *time_ms,
                price: 0.0,
            })
            .0;
            plot.line((x, 0.0), (x, h), color, width, dash);
            if !st.text.content.is_empty() && (0.0..=w).contains(&x) {
                let (tx, align) = match st.text.halign {
                    HAlign::Left => (x - TEXT_GAP, TextAlign::Right),
                    HAlign::Center => (x, TextAlign::Center),
                    HAlign::Right => (x + TEXT_GAP, TextAlign::Left),
                };
                text_block(plot, &st.text, tx, align, TEXT_GAP);
            }
        }
        DrawingKind::Rectangle { a, b } => draw_rect(d, *a, *b, proj, i, plot),
    }
}

/// Trend line or ray, with extend, arrow ends, middle point, text and stats.
fn draw_two_point(
    d: &Drawing,
    a: Anchor,
    b: Anchor,
    ray: bool,
    proj: &ChartProjector,
    i: &FrameInput,
    plot: &mut Out,
) {
    let (w, h) = (i.layout.plot.w, i.layout.plot.h);
    let st = &d.style;
    let (pa, pb) = (screen64(proj, a), screen64(proj, b));
    let inf = f64::INFINITY;
    let (t0, t1) = match (ray, st.extend) {
        (true, _) => (0.0, inf),
        (false, Extend::None) => (0.0, 1.0),
        (false, Extend::Left) => (-inf, 1.0),
        (false, Extend::Right) => (0.0, inf),
        (false, Extend::Both) => (-inf, inf),
    };
    let Some([s0, s1]) = clip_line(pa, pb, t0, t1, w as f64, h as f64) else {
        // Nothing of it is on screen: no extras either.
        return;
    };
    plot.line(s0, s1, st.color, st.width as f32, Dash::from(st.line_style));

    let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
    let len = dx.hypot(dy);
    if len >= 1.0 {
        let u = (dx / len, dy / len);
        let pad = ARROW_LEN;
        // An infinite end's arrow sits where the line leaves the plot.
        let left_tip = if t0.is_infinite() { (s0.0 as f64, s0.1 as f64) } else { pa };
        let right_tip = if t1.is_infinite() { (s1.0 as f64, s1.1 as f64) } else { pb };
        if st.left_end == LineEnd::Arrow && near_plot(left_tip, w, h, pad) {
            arrowhead(plot, left_tip, (-u.0, -u.1), st.color);
        }
        if st.right_end == LineEnd::Arrow && near_plot(right_tip, w, h, pad) {
            arrowhead(plot, right_tip, u, st.color);
        }
    }
    let mid = ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0);
    if st.middle_point && near_plot(mid, w, h, MIDDLE_DOT_RADIUS) {
        dot(plot, mid, st.color);
    }

    // Text: by the segment's bounding box; halign picks the a-end, middle or b-end.
    let (a32, b32, m32) = (f32p(pa), f32p(pb), f32p(mid));
    let toward = |from: (f32, f32), to: (f32, f32)| {
        if to.0 >= from.0 {
            TextAlign::Left
        } else {
            TextAlign::Right
        }
    };
    if !st.text.content.is_empty() {
        let (p, align) = match st.text.halign {
            HAlign::Left => (a32, toward(a32, b32)),
            HAlign::Center => (m32, TextAlign::Center),
            HAlign::Right => (b32, toward(b32, a32)),
        };
        let bh = text_block_height(&st.text);
        let top = match st.text.valign {
            VAlign::Top => a32.1.min(b32.1) - TEXT_GAP - bh,
            VAlign::Middle => p.1 - bh / 2.0,
            VAlign::Bottom => a32.1.max(b32.1) + TEXT_GAP,
        };
        text_block(plot, &st.text, p.0, align, top);
    }

    if stats_visible(d, i) {
        // Beside the chosen end, on its outer side; centered under the middle.
        let beside = |p: (f32, f32), other: (f32, f32)| {
            if p.0 >= other.0 {
                ((p.0 + STATS_GAP, p.1), TextAlign::Left)
            } else {
                ((p.0 - STATS_GAP, p.1), TextAlign::Right)
            }
        };
        let (pos, align) = match st.stats_pos {
            StatsPos::Right => beside(b32, a32),
            StatsPos::Left => beside(a32, b32),
            StatsPos::Center => ((m32.0, m32.1 + STATS_BELOW), TextAlign::Center),
        };
        let text = stats_text(d, a, b, pa, pb, false, i);
        push_stats(plot, text, pos, align, i);
    }
}

/// Rectangle: fill, border, horizontal extend, center point, text and stats.
fn draw_rect(
    d: &Drawing,
    a: Anchor,
    b: Anchor,
    proj: &ChartProjector,
    i: &FrameInput,
    plot: &mut Out,
) {
    let (w, h) = (i.layout.plot.w as f64, i.layout.plot.h as f64);
    let st = &d.style;
    let (pa, pb) = (screen64(proj, a), screen64(proj, b));
    let (mut x0, mut x1) = (pa.0.min(pb.0), pa.0.max(pb.0));
    let (y0, y1) = (pa.1.min(pb.1), pa.1.max(pb.1));
    if matches!(st.extend, Extend::Left | Extend::Both) {
        x0 = x0.min(0.0);
    }
    if matches!(st.extend, Extend::Right | Extend::Both) {
        x1 = x1.max(w);
    }
    if x1 < 0.0 || x0 > w || y1 < 0.0 || y0 > h {
        return;
    }
    // Far-off edges are pulled in just past the clip, so f32 stays exact.
    let pad = RECT_OVERSHOOT + st.width as f64;
    let (cx0, cx1) = (x0.max(-pad), x1.min(w + pad));
    let (cy0, cy1) = (y0.max(-pad), y1.min(h + pad));
    let r = RectF {
        x: cx0 as f32,
        y: cy0 as f32,
        w: (cx1 - cx0) as f32,
        h: (cy1 - cy0) as f32,
    };
    plot.push(Prim::Rect {
        r,
        color: st.fill.unwrap_or(with_alpha(st.color, 0x33)),
    });
    plot.push(Prim::StrokeRect {
        r,
        color: st.color,
        width: st.width as f32,
    });
    let center = ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0);
    if st.middle_point && near_plot(center, w as f32, h as f32, MIDDLE_DOT_RADIUS) {
        dot(plot, center, st.color);
    }
    // Text inside the box (the unextended one, clamped to the plot).
    let (bx0, bx1) = (pa.0.min(pb.0).max(0.0) as f32, pa.0.max(pb.0).min(w) as f32);
    let (by0, by1) = (y0.max(0.0) as f32, y1.min(h) as f32);
    if !st.text.content.is_empty() {
        let (x, align) = match st.text.halign {
            HAlign::Left => (bx0 + TEXT_GAP, TextAlign::Left),
            HAlign::Center => ((bx0 + bx1) / 2.0, TextAlign::Center),
            HAlign::Right => (bx1 - TEXT_GAP, TextAlign::Right),
        };
        let bh = text_block_height(&st.text);
        let top = match st.text.valign {
            VAlign::Top => by0 + TEXT_GAP,
            VAlign::Middle => (by0 + by1) / 2.0 - bh / 2.0,
            VAlign::Bottom => by1 - TEXT_GAP - bh,
        };
        text_block(plot, &st.text, x, align, top);
    }
    if stats_visible(d, i) {
        let y = by1 + STATS_BELOW;
        let (pos, align) = match st.stats_pos {
            StatsPos::Left => ((bx0, y), TextAlign::Left),
            StatsPos::Center => (((bx0 + bx1) / 2.0, y), TextAlign::Center),
            StatsPos::Right => ((bx1, y), TextAlign::Right),
        };
        let text = stats_text(d, a, b, pa, pb, true, i);
        push_stats(plot, text, pos, align, i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PriceRange;
    use std::time::Instant;
    use xv_core::{Candle, SourceId, Symbol, Timeframe};
    use xv_drawings::{Anchor, DrawingKind, Style, Tool};

    const H: i64 = 3_600_000;
    const T0: i64 = 1_700_000_000_000 - 1_700_000_000_000 % (24 * H);

    fn symbol() -> Symbol {
        Symbol {
            source: SourceId("binance".into()),
            id: "BTCUSDT".into(),
            display: "BTCUSDT".into(),
            description: "Bitcoin".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            tick_size: 0.01,
        }
    }

    fn wave(n: usize) -> Vec<Candle> {
        (0..n)
            .map(|i| {
                let m = 100.0 + (i as f64 * 0.37).sin() * 20.0;
                Candle {
                    open_time_ms: T0 + i as i64 * H,
                    open: m,
                    high: m + 3.0,
                    low: m - 3.0,
                    close: m + (i as f64).cos(),
                    volume: 10.0 + (i % 7) as f64,
                }
            })
            .collect()
    }

    fn series(cs: Vec<Candle>) -> Series {
        Series::new(symbol(), Timeframe::H1, cs, false)
    }

    fn fit(vp: &mut Viewport, s: &Series) {
        vp.scroll_to_latest(s.len());
        vp.autofit(s.candles());
    }

    fn run<'a>(
        s: &'a Series,
        vp: &'a Viewport,
        layout: &'a ChartLayout,
        f: impl FnOnce(&mut FrameInput<'a>),
    ) -> Frame {
        let colors: &'static ChartColors = Box::leak(Box::new(ChartColors::light()));
        let mut input = FrameInput {
            series: s,
            vp,
            layout,
            crosshair: None,
            drawings: &[],
            drawings_hidden: false,
            selected: None,
            colors,
            tz: TzOffset::UTC,
            source_name: "Binance",
        };
        f(&mut input);
        build_frame(&input)
    }

    fn coords(p: &Prim) -> Vec<f32> {
        match p {
            Prim::Rect { r, .. } => vec![r.x, r.y, r.w, r.h],
            Prim::StrokeRect { r, width, .. } => vec![r.x, r.y, r.w, r.h, *width],
            Prim::Line { a, b, width, .. } => vec![a.0, a.1, b.0, b.1, *width],
            Prim::Text { pos, .. } => vec![pos.0, pos.1],
            Prim::Handle { center, .. } => vec![center.0, center.1],
            Prim::Polygon { pts, .. } => pts.iter().flat_map(|p| [p.0, p.1]).collect(),
        }
    }

    #[test]
    fn layout_splits_axes() {
        let l = ChartLayout::new(1000.0, 600.0);
        assert_eq!(
            l.plot,
            RectF {
                x: 0.0,
                y: 0.0,
                w: 928.0,
                h: 572.0
            }
        );
        assert_eq!(
            l.price_axis,
            RectF {
                x: 928.0,
                y: 0.0,
                w: 72.0,
                h: 572.0
            }
        );
        assert_eq!(
            l.time_axis,
            RectF {
                x: 0.0,
                y: 572.0,
                w: 928.0,
                h: 28.0
            }
        );
        let tiny = ChartLayout::new(10.0, 10.0);
        assert!(tiny.plot.w >= 0.0 && tiny.plot.h >= 0.0);
    }

    #[test]
    fn frame_only_touches_visible() {
        let s = series(wave(100_000));
        let layout = ChartLayout::new(1672.0, 628.0); // plot 1600 wide, 8px/bar -> 200 bars
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        let colors = ChartColors::light();
        let f = run(&s, &vp, &layout, |_| {});
        let bodies = f
            .plot
            .iter()
            .filter(|p| matches!(p, Prim::Rect { color, .. } if *color == colors.up || *color == colors.down))
            .count();
        assert!(bodies > 150 && bodies <= 205, "bodies = {bodies}");
        assert!(f.plot.len() < 1000);
    }

    #[test]
    fn non_finite_never_emitted() {
        let layout = ChartLayout::new(800.0, 500.0);
        let flat = Candle {
            open_time_ms: T0,
            open: 5.0,
            high: 5.0,
            low: 5.0,
            close: 5.0,
            volume: 0.0,
        };
        let sets = [vec![], vec![flat], vec![flat; 5], wave(300)];
        let drawings = vec![
            Drawing {
                id: DrawingId(1),
                name: None,
                locked: false,
                hidden: false,
                style: full_style(),
                kind: DrawingKind::TrendLine {
                    a: Anchor {
                        time_ms: T0,
                        price: 5.0,
                    },
                    b: Anchor {
                        time_ms: T0 + 3 * H,
                        price: 6.0,
                    },
                },
            },
            Drawing {
                id: DrawingId(2),
                name: None,
                locked: false,
                hidden: false,
                style: full_style(),
                kind: DrawingKind::Ray {
                    a: Anchor {
                        time_ms: T0,
                        price: 5.0,
                    },
                    b: Anchor {
                        time_ms: T0 + 3 * H,
                        price: 6.0,
                    },
                },
            },
            Drawing {
                id: DrawingId(3),
                name: None,
                locked: false,
                hidden: false,
                style: full_style(),
                kind: DrawingKind::HorizontalLine { price: 5.0 },
            },
            Drawing {
                id: DrawingId(4),
                name: None,
                locked: false,
                hidden: false,
                style: full_style(),
                kind: DrawingKind::VerticalLine { time_ms: T0 + H },
            },
            Drawing {
                id: DrawingId(5),
                name: None,
                locked: false,
                hidden: false,
                style: full_style(),
                kind: DrawingKind::Rectangle {
                    a: Anchor {
                        time_ms: T0,
                        price: 5.0,
                    },
                    b: Anchor {
                        time_ms: T0 + 2 * H,
                        price: 7.0,
                    },
                },
            },
        ];
        for cs in sets {
            let s = series(cs);
            let mut vps = vec![];
            let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
            fit(&mut vp, &s);
            vps.push(vp.clone());
            let mut degenerate = vp.clone();
            degenerate.price = PriceRange { min: 5.0, max: 5.0 };
            vps.push(degenerate);
            let mut nan = vp;
            nan.price = PriceRange {
                min: f64::NAN,
                max: f64::NAN,
            };
            vps.push(nan);
            for vp in &vps {
                for sel in [None, Some(DrawingId(5)), Some(DrawingId(3))] {
                    let f = run(&s, vp, &layout, |i| {
                        i.drawings = &drawings;
                        i.selected = sel;
                        i.crosshair = Some((300.0, 200.0));
                    });
                    for p in f.plot.iter().chain(&f.price_axis).chain(&f.time_axis) {
                        assert!(coords(p).iter().all(|v| v.is_finite()), "{p:?}");
                    }
                }
            }
        }
    }

    fn full_style() -> Style {
        Style {
            extend: Extend::Both,
            left_end: LineEnd::Arrow,
            right_end: LineEnd::Arrow,
            middle_point: true,
            price_labels: true,
            stats: StatsMode::Values,
            always_stats: true,
            text: TextStyle {
                content: "a\nb".into(),
                ..TextStyle::default()
            },
            ..Style::default()
        }
    }

    fn anchor(idx: i64, price: f64) -> Anchor {
        Anchor {
            time_ms: T0 + idx * H,
            price,
        }
    }

    fn trend(id: u64, a: Anchor, b: Anchor, style: Style) -> Drawing {
        Drawing::new(DrawingId(id), DrawingKind::TrendLine { a, b }, style)
    }

    /// 50 bars, plot 800×500, fitted.
    fn setup() -> (Series, ChartLayout, Viewport) {
        let s = series(wave(50));
        let layout = ChartLayout::new(872.0, 528.0);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        (s, layout, vp)
    }

    fn screen(s: &Series, vp: &Viewport, a: Anchor) -> (f32, f32) {
        ChartProjector {
            candles: s.candles(),
            tf: s.timeframe,
            vp,
        }
        .to_screen(a)
    }

    type Seg = ((f32, f32), (f32, f32), Dash);

    fn lines_of(f: &Frame, color: u32) -> Vec<Seg> {
        f.plot
            .iter()
            .filter_map(|p| match p {
                Prim::Line {
                    a, b, color: c, dash, ..
                } if *c == color => Some((*a, *b, *dash)),
                _ => None,
            })
            .collect()
    }

    fn polygons_of(f: &Frame, color: u32) -> Vec<Vec<(f32, f32)>> {
        f.plot
            .iter()
            .filter_map(|p| match p {
                Prim::Polygon { pts, color: c } if *c == color => Some(pts.clone()),
                _ => None,
            })
            .collect()
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn dash_variants_emitted() {
        let (s, layout, vp) = setup();
        let mk = |id, color, line_style| {
            trend(
                id,
                anchor(10, 100.0),
                anchor(20, 105.0),
                Style {
                    color,
                    line_style,
                    ..Style::default()
                },
            )
        };
        let d = [
            mk(1, 0x11111180, LineStyle::Solid),
            mk(2, 0x222222FF, LineStyle::Dashed),
            mk(3, 0x333333FF, LineStyle::Dotted),
        ];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        // Opacity is the color's alpha, passed through.
        assert_eq!(lines_of(&f, 0x11111180)[0].2, Dash::Solid);
        assert_eq!(lines_of(&f, 0x222222FF)[0].2, Dash::Dashed);
        assert_eq!(lines_of(&f, 0x333333FF)[0].2, Dash::Dotted);
    }

    #[test]
    fn extend_both_reaches_plot_edges() {
        let (s, layout, vp) = setup();
        let (a, b) = (anchor(20, 100.0), anchor(30, 101.0));
        let mk = |id, color, extend| {
            trend(
                id,
                a,
                b,
                Style {
                    color,
                    extend,
                    ..Style::default()
                },
            )
        };
        let d = [
            mk(1, 0x010101FF, Extend::Both),
            mk(2, 0x020202FF, Extend::Right),
            mk(3, 0x030303FF, Extend::Left),
        ];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let w = layout.plot.w;
        let (pa, pb) = (screen(&s, &vp, a), screen(&s, &vp, b));
        let (l0, l1, _) = lines_of(&f, 0x010101FF)[0];
        assert_eq!((l0.0.min(l1.0), l0.0.max(l1.0)), (0.0, w));
        let (r0, r1, _) = lines_of(&f, 0x020202FF)[0];
        assert!(near(r0, pa) && r1.0 == w, "{r0:?} {r1:?}");
        let (e0, e1, _) = lines_of(&f, 0x030303FF)[0];
        assert!(e0.0 == 0.0 && near(e1, pb), "{e0:?} {e1:?}");
        // A plain segment with a far-off end is clipped in f64 to the plot.
        let far = [trend(
            4,
            anchor(-10_000_000, 90.0),
            anchor(30, 101.0),
            Style {
                color: 0x040404FF,
                ..Style::default()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &far);
        let (f0, f1, _) = lines_of(&f, 0x040404FF)[0];
        for p in [f0, f1] {
            assert!((0.0..=w).contains(&p.0) && (0.0..=layout.plot.h).contains(&p.1));
        }
        assert!(near(f1, pb));
        // Entirely off screen: nothing.
        let off = [trend(5, anchor(-500, 90.0), anchor(-400, 91.0), Style::default())];
        let f = run(&s, &vp, &layout, |i| i.drawings = &off);
        assert!(lines_of(&f, Style::default().color).is_empty());
    }

    #[test]
    fn arrow_end_emits_polygon() {
        let (s, layout, vp) = setup();
        let (a, b) = (anchor(10, 100.0), anchor(30, 104.0));
        let st = Style {
            color: 0x0A0A0AFF,
            right_end: LineEnd::Arrow,
            ..Style::default()
        };
        let d = [trend(1, a, b, st.clone())];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let polys = polygons_of(&f, 0x0A0A0AFF);
        assert_eq!(polys.len(), 1);
        let pb = screen(&s, &vp, b);
        let tri = &polys[0];
        assert_eq!(tri.len(), 3);
        assert!(near(tri[0], pb));
        let side = (10f32 * 10.0 + 4.0 * 4.0).sqrt();
        for p in &tri[1..] {
            let dd = ((p.0 - pb.0).powi(2) + (p.1 - pb.1).powi(2)).sqrt();
            assert!((dd - side).abs() < 0.01, "{dd}");
        }
        // Base behind the tip (toward a): pointing outward.
        let pa = screen(&s, &vp, a);
        let base = ((tri[1].0 + tri[2].0) / 2.0, (tri[1].1 + tri[2].1) / 2.0);
        let dist = |p: (f32, f32)| ((p.0 - pa.0).powi(2) + (p.1 - pa.1).powi(2)).sqrt();
        assert!(dist(base) < dist(pb));
        // Both ends.
        let d = [trend(
            1,
            a,
            b,
            Style {
                left_end: LineEnd::Arrow,
                ..st.clone()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        assert_eq!(polygons_of(&f, 0x0A0A0AFF).len(), 2);
        // Zero-length segment: no arrow.
        let d = [trend(1, a, a, st)];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        assert!(polygons_of(&f, 0x0A0A0AFF).is_empty());
    }

    #[test]
    fn middle_point_dot() {
        let (s, layout, vp) = setup();
        let (a, b) = (anchor(10, 100.0), anchor(30, 104.0));
        let st = Style {
            color: 0x0B0B0BFF,
            middle_point: true,
            ..Style::default()
        };
        let rect = Drawing::new(
            DrawingId(2),
            DrawingKind::Rectangle {
                a: anchor(12, 90.0),
                b: anchor(20, 96.0),
            },
            Style {
                color: 0x0C0C0CFF,
                ..st.clone()
            },
        );
        let d = [trend(1, a, b, st), rect];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let centroid = |pts: &[(f32, f32)]| {
            let n = pts.len() as f32;
            (
                pts.iter().map(|p| p.0).sum::<f32>() / n,
                pts.iter().map(|p| p.1).sum::<f32>() / n,
            )
        };
        let (pa, pb) = (screen(&s, &vp, a), screen(&s, &vp, b));
        let dot = &polygons_of(&f, 0x0B0B0BFF)[0];
        let c = centroid(dot);
        assert!(near(c, ((pa.0 + pb.0) / 2.0, (pa.1 + pb.1) / 2.0)), "{c:?}");
        for p in dot {
            let r = ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt();
            assert!((r - 3.0).abs() < 0.01);
        }
        let (ra, rb) = (screen(&s, &vp, anchor(12, 90.0)), screen(&s, &vp, anchor(20, 96.0)));
        let c = centroid(&polygons_of(&f, 0x0C0C0CFF)[0]);
        assert!(near(c, ((ra.0 + rb.0) / 2.0, (ra.1 + rb.1) / 2.0)), "{c:?}");
    }

    fn axis_boxed(f: &Frame) -> Vec<(String, u32, u32)> {
        f.price_axis
            .iter()
            .filter_map(|p| match p {
                Prim::Text {
                    text,
                    bg: Some(bg),
                    color,
                    ..
                } => Some((text.clone(), *bg, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn price_labels_on_axis() {
        let (s, layout, vp) = setup();
        let (a, b) = (anchor(10, 100.0), anchor(30, 104.0));
        let dark = trend(
            1,
            a,
            b,
            Style {
                color: 0x8E24AA80,
                price_labels: true,
                ..Style::default()
            },
        );
        let light = Drawing::new(
            DrawingId(2),
            DrawingKind::HorizontalLine { price: 95.0 },
            Style {
                color: 0xFFEB3BFF,
                price_labels: true,
                ..Style::default()
            },
        );
        let off = trend(3, anchor(5, 90.0), anchor(6, 91.0), Style::default());
        let d = [dark.clone(), light, off];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let labels = axis_boxed(&f);
        // Opaque drawing color as background, contrasting text.
        for t in ["100.00", "104.00"] {
            assert!(labels.contains(&(t.into(), 0x8E24AAFF, 0xFFFFFFFF)), "{labels:?}");
        }
        assert!(labels.contains(&("95.00".into(), 0xFFEB3BFF, 0x1F2933FF)));
        assert!(!labels.iter().any(|l| l.0 == "90.00"));
        // A selected drawing shows its labels even with price_labels off.
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = &d;
            i.selected = Some(DrawingId(3));
        });
        assert!(axis_boxed(&f).iter().any(|l| l.0 == "90.00"));
        // Hidden: no labels.
        let mut hidden = dark;
        hidden.hidden = true;
        let d = [hidden];
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = &d;
            i.selected = Some(DrawingId(1));
        });
        assert!(!axis_boxed(&f).iter().any(|l| l.0 == "100.00"));
    }

    #[test]
    fn price_label_hides_tick_label_under_it() {
        let cs: Vec<Candle> = (0..50)
            .map(|i| Candle {
                open_time_ms: T0 + i * H,
                open: 99.0,
                high: 101.0,
                low: 98.0,
                close: 100.0,
                volume: 1.0,
            })
            .collect();
        let s = series(cs);
        let layout = ChartLayout::new(1000.0, 600.0);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        vp.price = PriceRange {
            min: 50.0,
            max: 150.0,
        };
        vp.auto_scale = false;
        let d = [Drawing::new(
            DrawingId(1),
            DrawingKind::HorizontalLine { price: 121.0 },
            Tool::HorizontalLine.default_style(),
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        assert!(!f.price_axis.iter().any(
            |p| matches!(p, Prim::Text { text, bg: None, .. } if text == "120.00")
        ));
        assert!(axis_boxed(&f).iter().any(|l| l.0 == "121.00"));
    }

    fn texts(f: &Frame) -> Vec<&Prim> {
        f.plot
            .iter()
            .filter(|p| matches!(p, Prim::Text { .. }))
            .collect()
    }

    #[test]
    fn stats_compact_text() {
        let (s, layout, vp) = setup();
        let colors = ChartColors::light();
        let (a, b) = (anchor(10, 100.0), anchor(20, 112.34));
        let st = Style {
            stats: StatsMode::Compact,
            always_stats: true,
            ..Style::default()
        };
        let d = [
            trend(1, a, b, st.clone()),
            trend(2, anchor(10, 100.0), anchor(14, 95.0), st.clone()),
        ];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let got: Vec<(String, Option<u32>, u32)> = texts(&f)
            .iter()
            .filter_map(|p| match p {
                Prim::Text {
                    text, bg, color, ..
                } => Some((text.clone(), *bg, *color)),
                _ => None,
            })
            .collect();
        assert!(got.contains(&(
            "+12.34 (+12.34%)".into(),
            Some(colors.stats_bg),
            colors.stats_text
        )));
        assert!(got.iter().any(|g| g.0 == "-5.00 (-5.00%)"));
        // Right: beside the b end, on its outer side.
        let pb = screen(&s, &vp, b);
        let Prim::Text { pos, align, .. } = texts(&f)[0] else {
            panic!()
        };
        assert!(near(*pos, (pb.0 + 8.0, pb.1)) && *align == TextAlign::Left);

        // Not always: only while selected.
        let d = [trend(
            1,
            a,
            b,
            Style {
                always_stats: false,
                stats: StatsMode::Values,
                ..st
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        assert!(texts(&f).is_empty());
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = &d;
            i.selected = Some(DrawingId(1));
        });
        let pa = screen(&s, &vp, a);
        let deg = (-(pb.1 - pa.1) as f64).atan2((pb.0 - pa.0) as f64).to_degrees();
        let want = format!("+12.34 (+12.34%), 10 bars, 10h, {deg:.1}°");
        assert!(
            matches!(texts(&f)[0], Prim::Text { text, .. } if *text == want),
            "{:?} vs {want}",
            texts(&f)
        );

        // Rectangle: change, %, bars.
        let d = [Drawing::new(
            DrawingId(1),
            DrawingKind::Rectangle {
                a: anchor(10, 90.0),
                b: anchor(20, 100.0),
            },
            Style {
                stats: StatsMode::Values,
                always_stats: true,
                ..Style::default()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        assert!(matches!(texts(&f)[0], Prim::Text { text, .. } if text == "+10.00 (+11.11%), 10 bars"));
    }

    #[test]
    fn span_formats_two_largest_units() {
        let m = 60_000;
        assert_eq!(format_span(45 * m), "45m");
        assert_eq!(format_span((27 * 60) * m), "1d 3h");
        assert_eq!(format_span(-(17 * 1440) * m), "2w 3d");
        assert_eq!(format_span(1440 * m), "1d");
        assert_eq!(format_span(10 * 1000), "0m");
    }

    #[test]
    fn text_lines_and_alignment() {
        let (s, layout, vp) = setup();
        let (a, b) = (anchor(10, 100.0), anchor(30, 104.0));
        let text = |halign, valign| TextStyle {
            content: "one\ntwo".into(),
            color: 0x123456FF,
            size: 20,
            bold: true,
            italic: true,
            valign,
            halign,
        };
        let d = [trend(
            1,
            a,
            b,
            Style {
                text: text(HAlign::Center, VAlign::Top),
                ..Style::default()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let t = texts(&f);
        assert_eq!(t.len(), 2);
        let (pa, pb) = (screen(&s, &vp, a), screen(&s, &vp, b));
        let top = pa.1.min(pb.1) - 4.0 - 50.0;
        for (k, (p, want)) in t.iter().zip(["one", "two"]).enumerate() {
            let Prim::Text {
                pos,
                text,
                size,
                bold,
                italic,
                align,
                color,
                bg,
            } = p
            else {
                panic!()
            };
            assert_eq!(
                (text.as_str(), *size, *bold, *italic, *align, *color, *bg),
                (want, 20.0, true, true, TextAlign::Center, 0x123456FF, None)
            );
            let want_pos = ((pa.0 + pb.0) / 2.0, top + 25.0 * (k as f32 + 0.5));
            assert!(near(*pos, want_pos), "{pos:?} {want_pos:?}");
        }
        // Left: at the a end, extending toward b; Bottom: below the box.
        let d = [trend(
            1,
            a,
            b,
            Style {
                text: text(HAlign::Left, VAlign::Bottom),
                ..Style::default()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let Prim::Text { pos, align, .. } = texts(&f)[0] else {
            panic!()
        };
        assert!(near(*pos, (pa.0, pa.1.max(pb.1) + 4.0 + 12.5)));
        assert_eq!(*align, TextAlign::Left);
        // H-line: Right of the plot, centered on the line.
        let d = [Drawing::new(
            DrawingId(1),
            DrawingKind::HorizontalLine { price: 100.0 },
            Style {
                text: text(HAlign::Right, VAlign::Middle),
                ..Style::default()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let y = vp.y_of_price(100.0);
        let Prim::Text { pos, align, .. } = texts(&f)[0] else {
            panic!()
        };
        assert!(near(*pos, (layout.plot.w - 4.0, y - 25.0 + 12.5)));
        assert_eq!(*align, TextAlign::Right);
        // Rectangle: inside, top left.
        let (ra, rb) = (anchor(10, 90.0), anchor(20, 100.0));
        let d = [Drawing::new(
            DrawingId(1),
            DrawingKind::Rectangle { a: ra, b: rb },
            Style {
                text: text(HAlign::Left, VAlign::Top),
                ..Style::default()
            },
        )];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let (sa, sb) = (screen(&s, &vp, ra), screen(&s, &vp, rb));
        let Prim::Text { pos, align, .. } = texts(&f)[0] else {
            panic!()
        };
        assert!(near(*pos, (sa.0.min(sb.0) + 4.0, sa.1.min(sb.1) + 4.0 + 12.5)));
        assert_eq!(*align, TextAlign::Left);
    }

    #[test]
    fn rect_fill_default_and_custom() {
        let (s, layout, vp) = setup();
        let rect = |id, fill, extend| {
            Drawing::new(
                DrawingId(id),
                DrawingKind::Rectangle {
                    a: anchor(10, 90.0),
                    b: anchor(20, 100.0),
                },
                Style {
                    color: 0x445566FF,
                    fill,
                    extend,
                    ..Style::default()
                },
            )
        };
        let fills = |f: &Frame| -> Vec<(u32, RectF)> {
            f.plot
                .iter()
                .filter_map(|p| match p {
                    Prim::Rect { r, color } if *color & 0xFFFF_FF00 != 0 && r.h > 0.0 => {
                        Some((*color, *r))
                    }
                    _ => None,
                })
                .filter(|(c, _)| *c == 0x44556633 || *c == 0x00FF0080)
                .collect()
        };
        let d = [rect(1, None, Extend::None)];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        assert_eq!(fills(&f).len(), 1);
        assert_eq!(fills(&f)[0].0, 0x44556633);
        let d = [rect(1, Some(0x00FF0080), Extend::Right)];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let (c, r) = fills(&f)[0];
        assert_eq!(c, 0x00FF0080);
        let x0 = screen(&s, &vp, anchor(10, 90.0)).0;
        assert!((r.x - x0).abs() < 0.01 && r.x + r.w >= layout.plot.w);
        let d = [rect(1, None, Extend::Left)];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let (_, r) = fills(&f)[0];
        let x1 = screen(&s, &vp, anchor(20, 90.0)).0;
        assert!(r.x <= 0.0 && (r.x + r.w - x1).abs() < 0.01);
    }

    #[test]
    fn visibility_skips_drawing() {
        let (s, layout, vp) = setup();
        let st = Style {
            color: 0x0D0D0DFF,
            price_labels: true,
            ..Style::default()
        };
        let mut d = trend(1, anchor(10, 100.0), anchor(20, 104.0), st);
        let handles = |f: &Frame| -> Vec<(u32, bool)> {
            f.plot
                .iter()
                .filter_map(|p| match p {
                    Prim::Handle { color, filled, .. } => Some((*color, *filled)),
                    _ => None,
                })
                .collect()
        };
        let sel = ChartColors::light().selection;
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = std::slice::from_ref(&d);
            i.selected = Some(DrawingId(1));
        });
        assert_eq!(handles(&f), vec![(sel, true); 2]);
        // Locked and selected: hollow, half-transparent handles.
        d.locked = true;
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = std::slice::from_ref(&d);
            i.selected = Some(DrawingId(1));
        });
        assert_eq!(handles(&f), vec![(with_alpha(sel, 0x80), false); 2]);
        d.locked = false;
        for hide in [
            |d: &mut Drawing| d.hidden = true,
            |d: &mut Drawing| d.style.visibility.hours.on = false,
        ] {
            let mut d = d.clone();
            hide(&mut d);
            let f = run(&s, &vp, &layout, |i| {
                i.drawings = std::slice::from_ref(&d);
                i.selected = Some(DrawingId(1));
            });
            assert!(lines_of(&f, 0x0D0D0DFF).is_empty());
            assert!(handles(&f).is_empty());
            assert!(axis_boxed(&f).iter().all(|l| l.1 != 0x0D0D0DFF));
        }
        // Minutes off does not matter on H1.
        d.style.visibility.minutes.on = false;
        let f = run(&s, &vp, &layout, |i| i.drawings = std::slice::from_ref(&d));
        assert_eq!(lines_of(&f, 0x0D0D0DFF).len(), 1);
    }

    #[test]
    fn ray_is_clipped_to_plot() {
        let s = series(wave(50));
        let layout = ChartLayout::new(872.0, 528.0);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        let d = [Drawing {
            id: DrawingId(1),
            name: None,
            locked: false,
            hidden: false,
            style: Style {
                color: 0x112233FF,
                width: 3,
                ..Style::default()
            },
            kind: DrawingKind::Ray {
                a: Anchor {
                    time_ms: T0 + 40 * H,
                    price: 100.0,
                },
                b: Anchor {
                    time_ms: T0 + 44 * H,
                    price: 105.0,
                },
            },
        }];
        let f = run(&s, &vp, &layout, |i| i.drawings = &d);
        let ray = f
            .plot
            .iter()
            .find_map(|p| match p {
                Prim::Line {
                    b,
                    color: 0x112233FF,
                    ..
                } => Some(*b),
                _ => None,
            })
            .expect("ray line");
        assert_eq!(ray.0, layout.plot.x + layout.plot.w);
    }

    #[test]
    fn horizontal_line_adds_axis_label_and_rect_has_alpha_fill() {
        let s = series(wave(50));
        let layout = ChartLayout::new(872.0, 528.0);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        let d = [
            Drawing {
                id: DrawingId(1),
                name: None,
                locked: false,
                hidden: false,
                style: Style {
                    color: 0xAABBCCFF,
                    width: 1,
                    price_labels: true,
                    ..Style::default()
                },
                kind: DrawingKind::HorizontalLine { price: 100.0 },
            },
            Drawing {
                id: DrawingId(2),
                name: None,
                locked: false,
                hidden: false,
                style: Style {
                    color: 0x112233FF,
                    width: 1,
                    ..Style::default()
                },
                kind: DrawingKind::Rectangle {
                    a: Anchor {
                        time_ms: T0 + 10 * H,
                        price: 90.0,
                    },
                    b: Anchor {
                        time_ms: T0 + 20 * H,
                        price: 110.0,
                    },
                },
            },
        ];
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = &d;
            i.selected = Some(DrawingId(2));
        });
        assert!(f.price_axis.iter().any(
            |p| matches!(p, Prim::Text { text, bg: Some(0xAABBCCFF), .. } if text == "100.00")
        ));
        assert!(f.plot.iter().any(|p| matches!(
            p,
            Prim::Rect {
                color: 0x11223333,
                ..
            }
        )));
        assert!(f.plot.iter().any(|p| matches!(
            p,
            Prim::StrokeRect {
                color: 0x112233FF,
                ..
            }
        )));
        assert_eq!(
            f.plot
                .iter()
                .filter(|p| matches!(p, Prim::Handle { .. }))
                .count(),
            4
        );
        // hidden drawings vanish, handles included
        let f = run(&s, &vp, &layout, |i| {
            i.drawings = &d;
            i.selected = Some(DrawingId(2));
            i.drawings_hidden = true;
        });
        assert!(
            !f.plot
                .iter()
                .any(|p| matches!(p, Prim::Handle { .. } | Prim::StrokeRect { .. }))
        );
    }

    #[test]
    fn tick_labels_under_last_price_and_crosshair_labels_are_hidden() {
        let cs: Vec<Candle> = (0..50)
            .map(|i| Candle {
                open_time_ms: T0 + i * H,
                open: 99.0,
                high: 101.0,
                low: 98.0,
                close: 100.0,
                volume: 1.0,
            })
            .collect();
        let s = series(cs);
        let layout = ChartLayout::new(1000.0, 600.0);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        vp.price = PriceRange {
            min: 50.0,
            max: 150.0,
        };
        vp.auto_scale = false;
        let plain = |f: &Frame| -> Vec<String> {
            f.price_axis
                .iter()
                .filter_map(|p| match p {
                    Prim::Text { text, bg: None, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let boxed = |f: &Frame| -> Vec<String> {
            f.price_axis
                .iter()
                .filter_map(|p| match p {
                    Prim::Text {
                        text, bg: Some(_), ..
                    } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        // Ticks every 20: the 100.00 tick sits under the last-price label.
        let f = run(&s, &vp, &layout, |_| {});
        let mut ticks = plain(&f);
        ticks.sort();
        assert_eq!(ticks, ["120.00", "140.00", "60.00", "80.00"]);
        assert_eq!(boxed(&f), ["100.00"]);
        // A crosshair 5 px below the 120 tick hides it too.
        let y120 = vp.y_of_price(120.0);
        let f = run(&s, &vp, &layout, |i| {
            i.crosshair = Some((300.0, y120 + 5.0))
        });
        let mut ticks = plain(&f);
        ticks.sort();
        assert_eq!(ticks, ["140.00", "60.00", "80.00"]);
        assert_eq!(boxed(&f).len(), 2);
        // Grid lines stay.
        let grid = f
            .plot
            .iter()
            .filter(
                |p| matches!(p, Prim::Line { a, b, .. } if a.1 == b.1 && (a.1 - y120).abs() < 0.01),
            )
            .count();
        assert!(grid >= 1);
    }

    #[test]
    fn legend_and_crosshair() {
        let s = series(wave(50));
        let layout = ChartLayout::new(872.0, 528.0);
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        fit(&mut vp, &s);
        let f = run(&s, &vp, &layout, |_| {});
        assert_eq!(f.legend.title, "BTCUSDT · 1h · Binance");
        let last = s.last().unwrap();
        assert_eq!(f.legend.c, crate::format_price(last.close, 0.01));
        assert!(f.legend.change.contains('%'));
        // dashed last price line and a colored label
        assert!(
            f.plot
                .iter()
                .any(|p| matches!(p, Prim::Line { dash: Dash::Dashed, .. }))
        );
        assert!(
            f.price_axis
                .iter()
                .any(|p| matches!(p, Prim::Text { bg: Some(_), .. }))
        );

        // crosshair on bar 10
        let x = vp.x_of_index(10.0);
        let f2 = run(&s, &vp, &layout, |i| i.crosshair = Some((x, 100.0)));
        assert_eq!(
            f2.legend.c,
            crate::format_price(s.candles()[10].close, 0.01)
        );
        assert!(
            f2.plot
                .iter()
                .filter(|p| matches!(p, Prim::Line { dash: Dash::Dashed, .. }))
                .count()
                >= 3
        );
        assert!(
            f2.time_axis
                .iter()
                .any(|p| matches!(p, Prim::Text { bg: Some(_), .. }))
        );
        // crosshair outside the plot is ignored
        let f3 = run(&s, &vp, &layout, |i| {
            i.crosshair = Some((layout.plot.w + 20.0, 100.0))
        });
        assert_eq!(f3.plot.len(), f.plot.len());
    }

    #[test]
    #[ignore]
    fn build_frame_perf() {
        let s = series(wave(100_000));
        let layout = ChartLayout::new(4072.0, 828.0); // plot 4000 wide, 2 px/bar -> 2000 bars
        let mut vp = Viewport::new(layout.plot.w, layout.plot.h);
        vp.bar_spacing = 2.0;
        fit(&mut vp, &s);
        let colors = ChartColors::dark();
        let input = FrameInput {
            series: &s,
            vp: &vp,
            layout: &layout,
            crosshair: Some((500.0, 300.0)),
            drawings: &[],
            drawings_hidden: false,
            selected: None,
            colors: &colors,
            tz: TzOffset::UTC,
            source_name: "Binance",
        };
        let n = 1000;
        let t = Instant::now();
        let mut total = 0;
        for _ in 0..n {
            total += build_frame(&input).plot.len();
        }
        let avg = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        eprintln!("build_frame avg {avg:.3} ms, {} prims", total / n);
        assert!(avg < 2.0, "avg {avg} ms");
    }
}
