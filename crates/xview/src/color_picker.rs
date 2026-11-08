//! Color picker popover body: a 10 × 8 swatch grid (grays, XView's ten hues,
//! then six tint/shade rows), recent custom colors with a "+" hex field, and
//! an opacity slider with a % field. Used by the drawing toolbar and, later,
//! the drawing settings dialog.
//!
//! The picker owns no drawing: it emits `Changed` with the full new color
//! (0xRRGGBBAA) and the owner applies it. Picking a swatch keeps the alpha;
//! the opacity controls change only the alpha.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnchoredPositionMode, Bounds, Context, Entity, EventEmitter, Focusable, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, SharedString, Subscription, Window, anchored, canvas, checkerboard,
    deferred, div, linear_color_stop, linear_gradient, point, prelude::*, px,
};

use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{AppTheme, u32_to_hsla};
use crate::widgets::icon;

/// XView's ten base hues (0xRRGGBB), left to right: red, orange, yellow,
/// green, teal, cyan, blue, violet, purple, pink. The blue is the default
/// line color (`PALETTE[0]`), so new drawings show a selected swatch.
pub const HUES: [u32; 10] = [
    0xE0443E, 0xF08C2E, 0xF5D63D, 0x4BAE5E, 0x2E9E8F, 0x3BB5D6, 0x3B6FE8, 0x6A4FD8, 0xA347C2,
    0xE04382,
];
/// Row 1, white to black (0xRRGGBB). 0x1A1A1A is the default text color.
const GRAYS: [u32; 10] = [
    0xFFFFFF, 0xDCDCDC, 0xBBBBBB, 0x9C9C9C, 0x808080, 0x646464, 0x494949, 0x303030, 0x1A1A1A,
    0x000000,
];
/// Rows 3–8: mix of each hue toward white (positive) or black (negative).
const TINTS: [f32; 6] = [0.78, 0.6, 0.42, 0.2, -0.25, -0.48];

pub const COLS: usize = 10;
pub const ROWS: usize = 8;
const SWATCH: f32 = 18.;
const GAP: f32 = 5.;
const PAD: f32 = 12.;
pub const WIDTH: f32 = 2. * PAD + COLS as f32 * SWATCH + (COLS - 1) as f32 * GAP;
const SLIDER_H: f32 = 14.;
const KNOB: f32 = 16.;

/// Mixes `rgb` toward `target` by `t` (0..=1), per sRGB channel.
pub fn mix(rgb: u32, target: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| {
        let (a, b) = (((c >> s) & 0xFF) as f32, ((target >> s) & 0xFF) as f32);
        ((a + (b - a) * t).round() as u32).min(255) << s
    };
    ch(rgb, 16) | ch(rgb, 8) | ch(rgb, 0)
}

/// The 8 rows of 10 colors, as opaque 0xRRGGBBFF.
pub fn palette() -> [[u32; COLS]; ROWS] {
    let mut grid = [[0u32; COLS]; ROWS];
    for c in 0..COLS {
        grid[0][c] = GRAYS[c];
        grid[1][c] = HUES[c];
        for (r, &t) in TINTS.iter().enumerate() {
            grid[r + 2][c] = if t >= 0.0 {
                mix(HUES[c], 0xFFFFFF, t)
            } else {
                mix(HUES[c], 0x000000, -t)
            };
        }
    }
    grid.map(|row| row.map(|rgb| (rgb << 8) | 0xFF))
}

/// Parses `#RRGGBB` (the `#` is optional, hex digits in any case) into
/// 0xRRGGBB. Shorthand `#abc` is not accepted.
pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

/// `#RRGGBB` for a 0xRRGGBB value.
pub fn format_hex(rgb: u32) -> String {
    format!("#{:06X}", rgb & 0xFFFFFF)
}

/// `color` (0xRRGGBBAA) with its alpha set to `pct` percent.
pub fn with_opacity(color: u32, pct: u8) -> u32 {
    let a = (pct.min(100) as f32 * 255.0 / 100.0).round() as u32;
    (color & 0xFFFF_FF00) | a
}

/// The alpha of `color` (0xRRGGBBAA) in percent.
pub fn opacity_pct(color: u32) -> u8 {
    ((color & 0xFF) as f32 * 100.0 / 255.0).round() as u8
}

/// `color` (0xRRGGBBAA) with the channels of `rgb` (0xRRGGBB), keeping alpha.
pub fn with_rgb(color: u32, rgb: u32) -> u32 {
    ((rgb & 0xFFFFFF) << 8) | (color & 0xFF)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColorPickerEvent {
    /// The full new color, 0xRRGGBBAA.
    Changed(u32),
    /// A hex color was entered: add it to the recent custom colors.
    CustomColorAdded(u32),
    /// A text field inside the picker gave up focus (Enter or Esc): the owner
    /// should move focus back where it belongs.
    InputClosed,
}

pub struct ColorPicker {
    theme: AppTheme,
    color: u32,
    /// Recent custom colors, newest first (0xRRGGBBAA, opaque).
    custom: Vec<u32>,
    hex: Option<(Entity<TextInput>, Subscription)>,
    pct: Entity<TextInput>,
    slider: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
    _pct_sub: Subscription,
}

impl EventEmitter<ColorPickerEvent> for ColorPicker {}

impl ColorPicker {
    pub fn new(color: u32, custom: Vec<u32>, theme: AppTheme, cx: &mut Context<Self>) -> Self {
        let pct = cx.new(|cx| {
            TextInput::new(
                opacity_pct(color).to_string(),
                "",
                |c| c.is_ascii_digit(),
                3,
                theme.clone(),
                cx,
            )
            .with_suffix("%")
        });
        let sub = cx.subscribe(&pct, |this, _, e: &TextInputEvent, cx| match e {
            TextInputEvent::Changed(t) => {
                if let Ok(v) = t.parse::<u32>() {
                    this.set_opacity(v.min(100) as u8, false, cx);
                }
            }
            TextInputEvent::Submit(_) | TextInputEvent::Cancel => {
                let p = opacity_pct(this.color).to_string();
                this.pct.update(cx, |f, cx| f.set_text(p, cx));
                cx.emit(ColorPickerEvent::InputClosed);
            }
        });
        ColorPicker {
            theme,
            color,
            custom,
            hex: None,
            pct,
            slider: Rc::new(Cell::new(Bounds::default())),
            dragging: false,
            _pct_sub: sub,
        }
    }

    pub fn set_custom(&mut self, custom: Vec<u32>, cx: &mut Context<Self>) {
        if custom != self.custom {
            self.custom = custom;
            cx.notify();
        }
    }

    fn change(&mut self, color: u32, cx: &mut Context<Self>) {
        if color != self.color {
            self.color = color;
            cx.emit(ColorPickerEvent::Changed(color));
            cx.notify();
        }
    }

    fn pick(&mut self, rgb: u32, cx: &mut Context<Self>) {
        self.change(with_rgb(self.color, rgb), cx);
    }

    /// Sets the alpha; `sync_field` rewrites the % field (not while typing).
    fn set_opacity(&mut self, pct: u8, sync_field: bool, cx: &mut Context<Self>) {
        self.change(with_opacity(self.color, pct), cx);
        if sync_field {
            let p = pct.to_string();
            self.pct.update(cx, |f, cx| f.set_text(p, cx));
        }
    }

    fn slider_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let b = self.slider.get();
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return;
        }
        let t = (f32::from(x - b.origin.x) / w).clamp(0.0, 1.0);
        self.set_opacity((t * 100.0).round() as u8, true, cx);
    }

    fn open_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme.clone();
        let start = format_hex(self.color >> 8);
        let input = cx.new(|cx| {
            TextInput::new(start, "#RRGGBB", |c| c == '#' || c.is_ascii_hexdigit(), 7, theme, cx)
        });
        let sub = cx.subscribe(&input, |this, _, e: &TextInputEvent, cx| match e {
            TextInputEvent::Changed(t) => {
                let bad = parse_hex(t).is_none();
                if let Some((input, _)) = &this.hex {
                    input.update(cx, |f, cx| f.set_invalid(bad, cx));
                }
                cx.notify();
            }
            TextInputEvent::Submit(t) => {
                if let Some(rgb) = parse_hex(t) {
                    this.pick(rgb, cx);
                    cx.emit(ColorPickerEvent::CustomColorAdded((rgb << 8) | 0xFF));
                    this.close_hex(cx);
                }
            }
            TextInputEvent::Cancel => this.close_hex(cx),
        });
        window.focus(&input.focus_handle(cx), cx);
        self.hex = Some((input, sub));
        cx.notify();
    }

    fn close_hex(&mut self, cx: &mut Context<Self>) {
        if self.hex.take().is_some() {
            cx.emit(ColorPickerEvent::InputClosed);
            cx.notify();
        }
    }

    /// Forces the hex field open (debug script screenshots).
    #[cfg(debug_assertions)]
    pub fn debug_open_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_hex(window, cx);
    }

    /// Sets the color as a swatch or slider would (debug script).
    #[cfg(debug_assertions)]
    pub fn debug_set_color(&mut self, color: u32, cx: &mut Context<Self>) {
        self.change(color, cx);
        let p = opacity_pct(color).to_string();
        self.pct.update(cx, |f, cx| f.set_text(p, cx));
    }

    fn swatch(
        &self,
        id: SharedString,
        color: u32,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = &self.theme;
        let light = color >> 8 == 0xFFFFFF;
        let rgb = color >> 8;
        div()
            .id(id)
            .relative()
            .flex_none()
            .size(px(SWATCH))
            .rounded(px(3.))
            .bg(u32_to_hsla(color))
            .when(light, |d| d.border_1().border_color(t.border))
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-3.))
                        .left(px(-3.))
                        .size(px(SWATCH + 6.))
                        .rounded(px(5.))
                        .border_2()
                        .border_color(t.text),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| this.pick(rgb, cx)))
    }

    fn slider(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let pct = opacity_pct(self.color) as f32 / 100.0;
        let opaque = u32_to_hsla(self.color | 0xFF);
        let clear = u32_to_hsla(self.color & 0xFFFF_FF00);
        let cell = self.slider.clone();
        let track = div()
            .id("opacity-track")
            .relative()
            .flex_1()
            .h(px(KNOB))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.dragging = true;
                    this.slider_to(e.position.x, cx);
                }),
            )
            .child(
                canvas(move |b, _, _| cell.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px((KNOB - SLIDER_H) / 2.))
                    .h(px(SLIDER_H))
                    .rounded(px(SLIDER_H / 2.))
                    .border_1()
                    .border_color(t.text)
                    .bg(gpui::white())
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .bg(checkerboard(gpui::hsla(0., 0., 0.75, 1.), 4.)),
                    )
                    .child(div().absolute().size_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(clear, 0.),
                        linear_color_stop(opaque, 1.),
                    ))),
            )
            .child(
                // The knob, its center at `pct` of the track.
                div()
                    .absolute()
                    .top_0()
                    .left(gpui::relative(pct))
                    .ml(px(-KNOB / 2.))
                    .size(px(KNOB))
                    .rounded(px(KNOB / 2.))
                    .border_2()
                    .border_color(t.text)
                    .bg(gpui::white()),
            );
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(track)
            .child(div().flex_none().w(px(64.)).child(self.pct.clone()));
        if self.dragging {
            // Follows the pointer anywhere in the window until release.
            let vp = window.viewport_size();
            row = row.child(
                deferred(
                    anchored()
                        .position_mode(AnchoredPositionMode::Window)
                        .position(point(px(0.), px(0.)))
                        .child(
                            div()
                                .id("opacity-drag")
                                .w(vp.width)
                                .h(vp.height)
                                .occlude()
                                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                                    if e.pressed_button == Some(MouseButton::Left) {
                                        this.slider_to(e.position.x, cx);
                                    } else {
                                        this.dragging = false;
                                        cx.notify();
                                    }
                                }))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.dragging = false;
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
                .priority(3),
            );
        }
        row
    }
}

impl Render for ColorPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme.clone();
        let grid = palette();
        let cur = self.color >> 8;
        let in_grid = grid.iter().flatten().any(|c| c >> 8 == cur);
        let in_custom = self.custom.iter().any(|c| c >> 8 == cur);
        // Only the first matching swatch gets the ring.
        let mut ringed = false;
        let mut ring = |c: u32| {
            let hit = !ringed && c >> 8 == cur;
            ringed |= hit;
            hit
        };
        let mut rows = div().flex().flex_col().gap(px(GAP));
        for (r, row) in grid.iter().enumerate() {
            let mut line = div().flex().flex_row().gap(px(GAP));
            for (c, &color) in row.iter().enumerate() {
                let sel = ring(color);
                line = line.child(self.swatch(
                    SharedString::from(format!("swatch-{r}-{c}")),
                    color,
                    sel,
                    cx,
                ));
            }
            // A wider gap between the saturated row and the tints.
            rows = rows.child(line.when(r == 2, |d| d.mt(px(GAP + 2.))));
        }
        let mut custom = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(GAP));
        if !in_grid && !in_custom {
            // The current color is in neither list: show it, selected.
            let c = (cur << 8) | 0xFF;
            let sel = ring(c);
            custom = custom.child(self.swatch("custom-current".into(), c, sel, cx));
        }
        for (i, &c) in self.custom.iter().enumerate() {
            let sel = ring(c);
            custom =
                custom.child(self.swatch(SharedString::from(format!("custom-{i}")), c, sel, cx));
        }
        let hover = t.hover;
        custom = custom.child(
            div()
                .id("custom-add")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(SWATCH + 6.))
                .ml(px(-3.))
                .rounded(px(4.))
                .hover(move |s| s.bg(hover))
                .child(icon("plus", 22., t.text))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.hex.is_some() {
                        this.close_hex(cx);
                    } else {
                        this.open_hex(window, cx);
                    }
                })),
        );
        let hex_row = self.hex.as_ref().map(|(input, _)| {
            let preview = parse_hex(input.read(cx).text());
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_none()
                        .size(px(26.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(t.border)
                        .when_some(preview, |d, rgb| d.bg(u32_to_hsla((rgb << 8) | 0xFF))),
                )
                .child(div().flex_1().child(input.clone()))
        });
        div()
            .id("color-picker")
            .flex()
            .flex_col()
            .w(px(WIDTH))
            .p(px(PAD))
            .gap(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .bg(t.bg)
            .text_color(t.text)
            .shadow_lg()
            .child(rows)
            .child(div().h(px(1.)).bg(t.border))
            .child(custom)
            .children(hex_row)
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(t.muted)
                    .child("Opacity"),
            )
            .child(self.slider(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_grid_10x8_unique() {
        let all: Vec<u32> = palette().iter().flatten().copied().collect();
        assert_eq!(all.len(), 80);
        let set: std::collections::HashSet<u32> = all.iter().copied().collect();
        assert_eq!(set.len(), 80, "duplicate swatches");
        assert!(all.iter().all(|c| c & 0xFF == 0xFF), "swatches are opaque");
        // XView's own hues: none of the well-known third-party values.
        for banned in [0x2962FF, 0xF23645, 0x089981] {
            assert!(!all.contains(&((banned << 8) | 0xFF)), "{banned:06X}");
        }
        // The default line and text colors are selectable swatches.
        assert!(all.contains(&(xv_drawings::PALETTE[0] | 0xFF)));
        assert!(all.contains(&(xv_drawings::TEXT_COLOR | 0xFF)));
        let p = palette();
        assert_eq!(p[0][0], 0xFFFFFFFF);
        assert_eq!(p[0][9], 0x000000FF);
        // Tints get lighter from row 3 down to row 6, then shades darker.
        let lum = |c: u32| ((c >> 24) & 0xFF) + ((c >> 16) & 0xFF) + ((c >> 8) & 0xFF);
        for (r, pair) in p[2..].windows(2).enumerate() {
            for (c, (a, b)) in pair[0].iter().zip(&pair[1]).enumerate() {
                assert!(lum(*a) > lum(*b), "row {} col {c}", r + 2);
            }
        }
    }

    #[test]
    fn hex_parse_roundtrip() {
        assert_eq!(parse_hex("#a1B2c3"), Some(0xA1B2C3));
        assert_eq!(parse_hex("A1B2C3"), Some(0xA1B2C3));
        assert_eq!(parse_hex(" #000000 "), Some(0));
        assert_eq!(parse_hex("#abc"), None);
        assert_eq!(parse_hex("#abcdeg"), None);
        assert_eq!(parse_hex("#abcdef0"), None);
        assert_eq!(parse_hex(""), None);
        for rgb in [0x000000, 0xFFFFFF, 0x3D6EF2, 0x0A0B0C] {
            assert_eq!(parse_hex(&format_hex(rgb)), Some(rgb));
        }
        assert_eq!(format_hex(0x0A0B0C), "#0A0B0C");
    }

    #[test]
    fn opacity_sets_alpha_only() {
        assert_eq!(with_opacity(0x11223344, 100), 0x112233FF);
        assert_eq!(with_opacity(0x112233FF, 0), 0x11223300);
        assert_eq!(with_opacity(0x112233FF, 50), 0x11223380);
        assert_eq!(with_opacity(0x112233FF, 200), 0x112233FF);
        for pct in 0..=100u8 {
            let c = with_opacity(0xABCDEF12, pct);
            assert_eq!(c >> 8, 0xABCDEF);
            assert_eq!(opacity_pct(c), pct);
        }
        assert_eq!(with_rgb(0x11223380, 0xAABBCC), 0xAABBCC80);
    }

    #[test]
    fn mix_reaches_endpoints() {
        assert_eq!(mix(0x804020, 0xFFFFFF, 0.0), 0x804020);
        assert_eq!(mix(0x804020, 0xFFFFFF, 1.0), 0xFFFFFF);
        assert_eq!(mix(0x804020, 0x000000, 0.5), 0x402010);
    }
}
