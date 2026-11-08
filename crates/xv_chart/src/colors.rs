/// Chart palette. All colors are `0xRRGGBBAA`, XView's own choices.
#[derive(Clone, Debug)]
pub struct ChartColors {
    pub background: u32,
    pub grid: u32,
    pub text: u32,
    pub muted_text: u32,
    pub up: u32,
    pub down: u32,
    pub up_volume: u32,
    pub down_volume: u32,
    pub crosshair: u32,
    pub axis_label_bg: u32,
    pub axis_label_text: u32,
    pub last_price_up: u32,
    pub last_price_down: u32,
    pub selection: u32,
    /// Background box and text of drawing stats.
    pub stats_bg: u32,
    pub stats_text: u32,
}

impl ChartColors {
    pub fn light() -> Self {
        ChartColors {
            background: 0xFBFCFDFF,
            grid: 0xE8ECF1FF,
            text: 0x1F2933FF,
            muted_text: 0x7B8794FF,
            up: 0x1E9E74FF,
            down: 0xD9455AFF,
            up_volume: 0x1E9E7459,
            down_volume: 0xD9455A59,
            crosshair: 0x6B7785FF,
            axis_label_bg: 0x2D3748FF,
            axis_label_text: 0xFFFFFFFF,
            last_price_up: 0x1E9E74FF,
            last_price_down: 0xD9455AFF,
            selection: 0x3B6FE8FF,
            stats_bg: 0xF1F3F6F2,
            stats_text: 0x1F2933FF,
        }
    }

    pub fn dark() -> Self {
        ChartColors {
            background: 0x14181EFF,
            grid: 0x242B34FF,
            text: 0xD5DAE1FF,
            muted_text: 0x7E8A99FF,
            up: 0x34C28FFF,
            down: 0xF0566AFF,
            up_volume: 0x34C28F59,
            down_volume: 0xF0566A59,
            crosshair: 0x8795A6FF,
            axis_label_bg: 0x3A4452FF,
            axis_label_text: 0xF4F6F8FF,
            last_price_up: 0x34C28FFF,
            last_price_down: 0xF0566AFF,
            selection: 0x6FA3FFFF,
            stats_bg: 0x2A313BF2,
            stats_text: 0xD5DAE1FF,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_differ_and_use_own_hex_values() {
        let (l, d) = (ChartColors::light(), ChartColors::dark());
        assert_ne!(l.background, d.background);
        for c in [&l, &d] {
            for banned in [0x26A69AFF, 0xEF5350FF, 0x131722FF] {
                assert!(![c.up, c.down, c.background].contains(&banned));
            }
        }
    }

    /// Well-known third-party accents, compared as RGB (any alpha).
    const BANNED_RGB: [u32; 4] = [0x26A69A, 0xEF5350, 0x131722, 0x2962FF];

    #[test]
    fn no_chart_or_drawing_color_is_a_banned_accent() {
        for c in [ChartColors::light(), ChartColors::dark()] {
            let all = [
                c.background, c.grid, c.text, c.muted_text, c.up, c.down, c.up_volume,
                c.down_volume, c.crosshair, c.axis_label_bg, c.axis_label_text, c.last_price_up,
                c.last_price_down, c.selection, c.stats_bg, c.stats_text,
            ];
            for v in all.iter().chain(xv_drawings::PALETTE.iter()) {
                assert!(!BANNED_RGB.contains(&(v >> 8)), "{v:08X}");
            }
        }
        // The default drawing color is XView's accent.
        assert_eq!(xv_drawings::PALETTE[0], ChartColors::light().selection);
        assert_eq!(xv_drawings::Style::default().color, ChartColors::light().selection);
    }
}
