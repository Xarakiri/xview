use serde::{Deserialize, Serialize};
use xv_core::Timeframe;

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub time_ms: i64,
    pub price: f64,
}

impl Anchor {
    pub fn is_finite(&self) -> bool {
        self.price.is_finite()
    }
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

impl DrawingKind {
    /// Whether every price is finite. JSON has no NaN or infinity (serde_json
    /// writes them as null), so a non-finite drawing would make the saved
    /// file unreadable.
    pub fn is_finite(&self) -> bool {
        match self {
            DrawingKind::TrendLine { a, b }
            | DrawingKind::Ray { a, b }
            | DrawingKind::Rectangle { a, b } => a.is_finite() && b.is_finite(),
            DrawingKind::HorizontalLine { price } => price.is_finite(),
            DrawingKind::VerticalLine { .. } => true,
        }
    }
}

/// XView's own drawing colors, 0xRRGGBBAA. The first (the default) is the
/// app accent (`AppTheme::light().accent`, `ChartColors::light().selection`).
pub const PALETTE: [u32; 8] = [
    0x3B6FE8FF, 0xE53935FF, 0x43A047FF, 0xFB8C00FF, 0x8E24AAFF, 0x00ACC1FF, 0xD81B60FF, 0x757575FF,
];

macro_rules! simple_enum {
    ($(#[$m:meta])* $name:ident { $($v:ident),+ } default $d:ident) => {
        $(#[$m])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($v),+ }
        impl Default for $name {
            fn default() -> Self {
                $name::$d
            }
        }
    };
}

simple_enum!(LineStyle { Solid, Dashed, Dotted } default Solid);
simple_enum!(Extend { None, Left, Right, Both } default None);
simple_enum!(LineEnd { Normal, Arrow } default Normal);
simple_enum!(StatsMode { Hidden, Compact, Values } default Hidden);
simple_enum!(StatsPos { Left, Center, Right } default Right);
simple_enum!(VAlign { Top, Middle, Bottom } default Top);
simple_enum!(HAlign { Left, Center, Right } default Right);

/// XView's own dark gray for drawing text.
pub const TEXT_COLOR: u32 = 0x1F2328FF;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub content: String,
    /// 0xRRGGBBAA
    pub color: u32,
    /// 10..=40
    pub size: u8,
    pub bold: bool,
    pub italic: bool,
    pub valign: VAlign,
    pub halign: HAlign,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            content: String::new(),
            color: TEXT_COLOR,
            size: 14,
            bold: false,
            italic: false,
            valign: VAlign::Top,
            halign: HAlign::Right,
        }
    }
}

/// An on/off interval; `min > max` is clamped on use (see `Range::contains`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub on: bool,
    pub min: u16,
    pub max: u16,
}

impl Range {
    pub const fn full(max: u16) -> Self {
        Range {
            on: true,
            min: 1,
            max,
        }
    }

    /// Whether `n` is shown. An inverted range (min > max) is read as
    /// `min..=min`, never rejected.
    pub fn contains(&self, n: u16) -> bool {
        self.on && n >= self.min && n <= self.max.max(self.min)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Visibility {
    /// 1..=59
    pub minutes: Range,
    /// 1..=24
    pub hours: Range,
    /// 1..=366
    pub days: Range,
    /// 1..=52
    pub weeks: Range,
    /// 1..=12
    pub months: Range,
}

impl Default for Visibility {
    fn default() -> Self {
        Visibility {
            minutes: Range::full(59),
            hours: Range::full(24),
            days: Range::full(366),
            weeks: Range::full(52),
            months: Range::full(12),
        }
    }
}

impl Visibility {
    pub fn shows(&self, tf: Timeframe) -> bool {
        match tf {
            Timeframe::M1 => self.minutes.contains(1),
            Timeframe::M3 => self.minutes.contains(3),
            Timeframe::M5 => self.minutes.contains(5),
            Timeframe::M15 => self.minutes.contains(15),
            Timeframe::M30 => self.minutes.contains(30),
            Timeframe::H1 => self.hours.contains(1),
            Timeframe::H2 => self.hours.contains(2),
            Timeframe::H4 => self.hours.contains(4),
            Timeframe::D1 => self.days.contains(1),
            Timeframe::W1 => self.weeks.contains(1),
            Timeframe::Mo1 => self.months.contains(1),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    /// 0xRRGGBBAA; alpha is the opacity.
    pub color: u32,
    /// 1..=4
    pub width: u8,
    #[serde(default)]
    pub line_style: LineStyle,
    /// Rectangle fill; `None` is the color at alpha 0x33 (as in M1).
    #[serde(default)]
    pub fill: Option<u32>,
    #[serde(default)]
    pub extend: Extend,
    #[serde(default)]
    pub left_end: LineEnd,
    #[serde(default)]
    pub right_end: LineEnd,
    #[serde(default)]
    pub middle_point: bool,
    #[serde(default)]
    pub price_labels: bool,
    #[serde(default)]
    pub stats: StatsMode,
    #[serde(default)]
    pub stats_pos: StatsPos,
    #[serde(default)]
    pub always_stats: bool,
    #[serde(default)]
    pub text: TextStyle,
    #[serde(default)]
    pub visibility: Visibility,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: PALETTE[0],
            width: 2,
            line_style: LineStyle::default(),
            fill: None,
            extend: Extend::default(),
            left_end: LineEnd::default(),
            right_end: LineEnd::default(),
            middle_point: false,
            price_labels: false,
            stats: StatsMode::default(),
            stats_pos: StatsPos::default(),
            always_stats: false,
            text: TextStyle::default(),
            visibility: Visibility::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub id: DrawingId,
    pub kind: DrawingKind,
    pub style: Style,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub hidden: bool,
}

impl Drawing {
    /// A drawing with default name/lock/hidden state.
    pub fn new(id: DrawingId, kind: DrawingKind, style: Style) -> Self {
        Drawing {
            id,
            kind,
            style,
            name: None,
            locked: false,
            hidden: false,
        }
    }

    /// Whether the drawing can be saved and read back (see
    /// `DrawingKind::is_finite`; `Style` holds only integers).
    pub fn is_finite(&self) -> bool {
        self.kind.is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_style_uses_first_palette_color() {
        let s = Style::default();
        assert_eq!((s.color, s.width), (0x3B6FE8FF, 2));
    }

    #[test]
    fn visibility_shows_maps_timeframes() {
        use Timeframe::*;
        let all = [M1, M3, M5, M15, M30, H1, H2, H4, D1, W1, Mo1];
        assert!(all.iter().all(|t| Visibility::default().shows(*t)));
        let mut v = Visibility::default();
        v.minutes = Range {
            on: true,
            min: 5,
            max: 15,
        };
        assert!(!v.shows(M1) && !v.shows(M3) && v.shows(M5) && v.shows(M15) && !v.shows(M30));
        v.hours.on = false;
        assert!(!v.shows(H1) && !v.shows(H4) && v.shows(D1));
        v.weeks.on = false;
        v.months.on = false;
        v.days.on = false;
        assert!(!v.shows(D1) && !v.shows(W1) && !v.shows(Mo1));
    }

    #[test]
    fn inverted_range_is_clamped_not_rejected() {
        let mut v = Visibility::default();
        v.minutes = Range {
            on: true,
            min: 15,
            max: 5,
        };
        assert!(v.shows(Timeframe::M15));
        assert!(!v.shows(Timeframe::M5) && !v.shows(Timeframe::M30));
    }

    #[test]
    fn style_defaults() {
        let s = Style::default();
        assert_eq!(s.fill, None);
        assert_eq!(s.text.size, 14);
        assert_eq!(s.text.color, TEXT_COLOR);
        assert_eq!((s.text.valign, s.text.halign), (VAlign::Top, HAlign::Right));
        assert_eq!(s.stats_pos, StatsPos::Right);
    }

    #[test]
    fn enums_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&LineStyle::Dashed).unwrap(),
            "\"dashed\""
        );
        assert_eq!(
            serde_json::to_string(&StatsMode::Compact).unwrap(),
            "\"compact\""
        );
    }
}
