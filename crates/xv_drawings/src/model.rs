use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub time_ms: i64,
    pub price: f64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DrawingId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DrawingKind {
    TrendLine { a: Anchor, b: Anchor },
    Ray { a: Anchor, b: Anchor },
    HorizontalLine { price: f64 },
    VerticalLine { time_ms: i64 },
    Rectangle { a: Anchor, b: Anchor },
}

/// XView's own drawing colors, 0xRRGGBBAA.
pub const PALETTE: [u32; 8] = [
    0x2962FFFF, 0xE53935FF, 0x43A047FF, 0xFB8C00FF, 0x8E24AAFF, 0x00ACC1FF, 0xD81B60FF, 0x757575FF,
];

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    /// 0xRRGGBBAA
    pub color: u32,
    /// 1..=4
    pub width: u8,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: PALETTE[0],
            width: 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub id: DrawingId,
    pub kind: DrawingKind,
    pub style: Style,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_style_uses_first_palette_color() {
        let s = Style::default();
        assert_eq!((s.color, s.width), (0x2962FFFF, 2));
    }
}
