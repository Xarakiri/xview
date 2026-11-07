//! Application theme: chart palette plus the app chrome colors. All values are
//! XView's own picks.

use gpui::{Hsla, rgba};
use xv_chart::ChartColors;
use xv_store::ThemeMode;

/// Converts a `0xRRGGBBAA` color into GPUI's `Hsla`.
pub fn u32_to_hsla(c: u32) -> Hsla {
    rgba(c).into()
}

#[derive(Clone, Debug)]
pub struct AppTheme {
    pub mode: ThemeMode,
    pub chart: ChartColors,
    pub bg: Hsla,
    pub panel_bg: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub hover: Hsla,
    pub active_bg: Hsla,
    /// Links and focus rings; no shell control uses it yet.
    #[allow(dead_code)]
    pub accent: Hsla,
    pub badge: Hsla,
}

impl AppTheme {
    pub fn light() -> Self {
        AppTheme {
            mode: ThemeMode::Light,
            chart: ChartColors::light(),
            bg: u32_to_hsla(0xFFFFFFFF),
            panel_bg: u32_to_hsla(0xF1F3F8FF),
            border: u32_to_hsla(0xDFE3EAFF),
            text: u32_to_hsla(0x1F2933FF),
            muted: u32_to_hsla(0x7B8794FF),
            hover: u32_to_hsla(0xEDF0F5FF),
            active_bg: u32_to_hsla(0xE2E7F0FF),
            accent: u32_to_hsla(0x3B6FE8FF),
            badge: u32_to_hsla(0xE5484DFF),
        }
    }

    pub fn dark() -> Self {
        AppTheme {
            mode: ThemeMode::Dark,
            chart: ChartColors::dark(),
            bg: u32_to_hsla(0x14181EFF),
            panel_bg: u32_to_hsla(0x1C2129FF),
            border: u32_to_hsla(0x2A313BFF),
            text: u32_to_hsla(0xD5DAE1FF),
            muted: u32_to_hsla(0x7E8A99FF),
            hover: u32_to_hsla(0x232A33FF),
            active_bg: u32_to_hsla(0x2D3541FF),
            accent: u32_to_hsla(0x6FA3FFFF),
            badge: u32_to_hsla(0xF0566AFF),
        }
    }

    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_to_hsla_maps_channels() {
        let white = u32_to_hsla(0xFFFFFFFF);
        assert!((white.l - 1.0).abs() < 1e-4 && (white.a - 1.0).abs() < 1e-4);
        let clear_black = u32_to_hsla(0x00000000);
        assert!(clear_black.l.abs() < 1e-4 && clear_black.a.abs() < 1e-4);
        let half_red = gpui::Rgba::from(u32_to_hsla(0xFF000080));
        assert!((half_red.r - 1.0).abs() < 1e-3 && half_red.g.abs() < 1e-3);
        assert!((half_red.a - 128.0 / 255.0).abs() < 1e-3);
    }

    #[test]
    fn modes_match_palettes() {
        assert_eq!(AppTheme::for_mode(ThemeMode::Light).mode, ThemeMode::Light);
        assert_eq!(AppTheme::for_mode(ThemeMode::Dark).mode, ThemeMode::Dark);
        assert_ne!(AppTheme::light().bg, AppTheme::dark().bg);
        assert_eq!(
            AppTheme::dark().chart.background,
            ChartColors::dark().background
        );
    }
}
