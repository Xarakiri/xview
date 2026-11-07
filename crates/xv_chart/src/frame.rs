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
    format_price, index_to_time, price_ticks, time_ticks,
};
use xv_core::{Series, TzOffset};
use xv_drawings::{Anchor, Drawing, DrawingId, DrawingKind, Projector, handles};

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
        dashed: bool,
    },
    Text {
        pos: (f32, f32),
        text: String,
        color: u32,
        align: TextAlign,
        bg: Option<u32>,
    },
    Handle {
        center: (f32, f32),
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

    fn line(&mut self, a: (f32, f32), b: (f32, f32), color: u32, width: f32, dashed: bool) {
        self.push(Prim::Line {
            a,
            b,
            color,
            width,
            dashed,
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
        });
    }
}

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
    let diff = c.close - prev;
    let pct = if prev != 0.0 {
        diff / prev * 100.0
    } else {
        0.0
    };
    let sign = if diff >= 0.0 { "+" } else { "-" };
    Legend {
        title,
        o: format_price(c.open, tick),
        h: format_price(c.high, tick),
        l: format_price(c.low, tick),
        c: format_price(c.close, tick),
        v: format_volume(c.volume),
        change: format!(
            "{sign}{} ({sign}{:.2}%)",
            format_price(diff.abs(), tick),
            pct.abs()
        ),
        up: diff >= 0.0,
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
    let boxed: Vec<f32> = candles
        .last()
        .map(|c| vp.y_of_price(c.close))
        .filter(|y| (0.0..=ph).contains(y))
        .into_iter()
        .chain(crosshair.map(|(_, cy)| cy))
        .collect();

    // Grid and axis tick labels.
    for v in price_ticks(vp.price, vp.height, tick) {
        let y = vp.y_of_price(v);
        if (0.0..=ph).contains(&y) {
            plot.line((0.0, y), (pw, y), colors.grid, 1.0, false);
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
        plot.line((x, 0.0), (x, ph), colors.grid, 1.0, false);
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
        plot.line((x, yh), (x, yl), color, 1.0, false);
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
        for d in i.drawings {
            draw_drawing(d, &proj, i, &mut plot, &mut price_axis, label_x, oy);
        }
        if let Some(sel) = i
            .selected
            .and_then(|id| i.drawings.iter().find(|d| d.id == id))
        {
            for h in handles(sel, &proj) {
                plot.push(Prim::Handle {
                    center: h,
                    color: colors.selection,
                });
            }
        }
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
            plot.line((0.0, y), (pw, y), color, 1.0, true);
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
        plot.line((sx, 0.0), (sx, ph), colors.crosshair, 1.0, true);
        plot.line((0.0, cy), (pw, cy), colors.crosshair, 1.0, true);
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

fn draw_drawing(
    d: &Drawing,
    proj: &ChartProjector,
    i: &FrameInput,
    plot: &mut Out,
    price_axis: &mut Out,
    label_x: f32,
    oy: f32,
) {
    let (w, h) = (i.layout.plot.w, i.layout.plot.h);
    let color = d.style.color;
    let width = d.style.width as f32;
    let at = |a: Anchor| proj.to_screen(a);
    match &d.kind {
        DrawingKind::TrendLine { a, b } => plot.line(at(*a), at(*b), color, width, false),
        DrawingKind::Ray { a, b } => {
            let (pa, pb) = (at(*a), at(*b));
            let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
            let end = if dx > 0.0 && pa.0 <= w {
                Some((w, pa.1 + dy / dx * (w - pa.0)))
            } else if dx < 0.0 && pa.0 >= 0.0 {
                Some((0.0, pa.1 + dy / dx * (0.0 - pa.0)))
            } else if dx == 0.0 && dy > 0.0 {
                Some((pa.0, h))
            } else if dx == 0.0 && dy < 0.0 {
                Some((pa.0, 0.0))
            } else {
                None
            };
            if let Some(end) = end {
                plot.line(pa, end, color, width, false);
            }
        }
        DrawingKind::HorizontalLine { price } => {
            let y = at(Anchor {
                time_ms: 0,
                price: *price,
            })
            .1;
            plot.line((0.0, y), (w, y), color, width, false);
            if (0.0..=h).contains(&y) {
                price_axis.text(
                    (label_x, oy + y),
                    format_price(*price, i.series.symbol.tick_size),
                    i.colors.axis_label_text,
                    TextAlign::Left,
                    Some(color),
                );
            }
        }
        DrawingKind::VerticalLine { time_ms } => {
            let x = at(Anchor {
                time_ms: *time_ms,
                price: 0.0,
            })
            .0;
            plot.line((x, 0.0), (x, h), color, width, false);
        }
        DrawingKind::Rectangle { a, b } => {
            let (pa, pb) = (at(*a), at(*b));
            let r = RectF {
                x: pa.0.min(pb.0),
                y: pa.1.min(pb.1),
                w: (pa.0 - pb.0).abs(),
                h: (pa.1 - pb.1).abs(),
            };
            plot.push(Prim::Rect {
                r,
                color: with_alpha(color, 0x33),
            });
            plot.push(Prim::StrokeRect { r, color, width });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PriceRange;
    use std::time::Instant;
    use xv_core::{Candle, SourceId, Symbol, Timeframe};
    use xv_drawings::{Anchor, DrawingKind, Style};

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
    fn frame_has_no_nan() {
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
                style: Style::default(),
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
                style: Style::default(),
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
                style: Style::default(),
                kind: DrawingKind::HorizontalLine { price: 5.0 },
            },
            Drawing {
                id: DrawingId(4),
                name: None,
                locked: false,
                hidden: false,
                style: Style::default(),
                kind: DrawingKind::VerticalLine { time_ms: T0 + H },
            },
            Drawing {
                id: DrawingId(5),
                name: None,
                locked: false,
                hidden: false,
                style: Style::default(),
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
                .any(|p| matches!(p, Prim::Line { dashed: true, .. }))
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
                .filter(|p| matches!(p, Prim::Line { dashed: true, .. }))
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
