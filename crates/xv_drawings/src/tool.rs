use crate::model::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Cursor,
    TrendLine,
    ExtendedLine,
    Ray,
    HorizontalLine,
    VerticalLine,
    Rectangle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlaceResult {
    Pending(Anchor),
    Done(DrawingKind),
}

/// Feeds one click into the placement state machine. `first` is the pending first anchor.
pub fn place_click(tool: Tool, first: Option<Anchor>, at: Anchor) -> Option<PlaceResult> {
    let two = |make: fn(Anchor, Anchor) -> DrawingKind| match first {
        None => PlaceResult::Pending(at),
        Some(a) => PlaceResult::Done(make(a, at)),
    };
    Some(match tool {
        Tool::Cursor => return None,
        Tool::HorizontalLine => PlaceResult::Done(DrawingKind::HorizontalLine { price: at.price }),
        Tool::VerticalLine => PlaceResult::Done(DrawingKind::VerticalLine {
            time_ms: at.time_ms,
        }),
        Tool::TrendLine | Tool::ExtendedLine => two(|a, b| DrawingKind::TrendLine { a, b }),
        Tool::Ray => two(|a, b| DrawingKind::Ray { a, b }),
        Tool::Rectangle => two(|a, b| DrawingKind::Rectangle { a, b }),
    })
}

impl Tool {
    /// Stable snake_case name.
    pub fn key(self) -> &'static str {
        match self {
            Tool::Cursor => "cursor",
            Tool::TrendLine => "trend_line",
            Tool::ExtendedLine => "extended_line",
            Tool::Ray => "ray",
            Tool::HorizontalLine => "horizontal_line",
            Tool::VerticalLine => "vertical_line",
            Tool::Rectangle => "rectangle",
        }
    }

    /// The style a new drawing of this tool starts with.
    pub fn default_style(self) -> Style {
        match self {
            Tool::ExtendedLine => Style {
                extend: Extend::Both,
                ..Style::default()
            },
            Tool::HorizontalLine => Style {
                price_labels: true,
                ..Style::default()
            },
            _ => Style::default(),
        }
    }
}

/// The tool that would create `d`: a trend line extended both ways is an
/// extended line.
pub fn tool_of(d: &Drawing) -> Tool {
    match d.kind {
        DrawingKind::TrendLine { .. } if d.style.extend == Extend::Both => Tool::ExtendedLine,
        DrawingKind::TrendLine { .. } => Tool::TrendLine,
        DrawingKind::Ray { .. } => Tool::Ray,
        DrawingKind::HorizontalLine { .. } => Tool::HorizontalLine,
        DrawingKind::VerticalLine { .. } => Tool::VerticalLine,
        DrawingKind::Rectangle { .. } => Tool::Rectangle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(t: i64, p: f64) -> Anchor {
        Anchor {
            time_ms: t,
            price: p,
        }
    }

    #[test]
    fn place_click_flow() {
        let (p, q) = (a(1, 1.0), a(2, 2.0));
        assert_eq!(
            place_click(Tool::TrendLine, None, p),
            Some(PlaceResult::Pending(p))
        );
        assert_eq!(
            place_click(Tool::TrendLine, Some(p), q),
            Some(PlaceResult::Done(DrawingKind::TrendLine { a: p, b: q }))
        );
        assert_eq!(
            place_click(Tool::HorizontalLine, None, q),
            Some(PlaceResult::Done(DrawingKind::HorizontalLine {
                price: 2.0
            }))
        );
        assert_eq!(
            place_click(Tool::VerticalLine, None, q),
            Some(PlaceResult::Done(DrawingKind::VerticalLine { time_ms: 2 }))
        );
        assert_eq!(
            place_click(Tool::Rectangle, Some(p), q),
            Some(PlaceResult::Done(DrawingKind::Rectangle { a: p, b: q }))
        );
        assert_eq!(place_click(Tool::Cursor, None, p), None);
    }

    #[test]
    fn extended_line_tool_places_trendline() {
        let (p, q) = (a(1, 1.0), a(2, 2.0));
        assert_eq!(
            place_click(Tool::ExtendedLine, None, p),
            Some(PlaceResult::Pending(p))
        );
        assert_eq!(
            place_click(Tool::ExtendedLine, Some(p), q),
            Some(PlaceResult::Done(DrawingKind::TrendLine { a: p, b: q }))
        );
        assert_eq!(Tool::ExtendedLine.default_style().extend, Extend::Both);
        assert!(Tool::HorizontalLine.default_style().price_labels);
        assert_eq!(Tool::Ray.default_style(), Style::default());
        assert_eq!(Tool::ExtendedLine.key(), "extended_line");
        assert_eq!(Tool::HorizontalLine.key(), "horizontal_line");
    }

    #[test]
    fn tool_of_roundtrip() {
        let (p, q) = (a(1, 1.0), a(2, 2.0));
        for tool in [
            Tool::TrendLine,
            Tool::ExtendedLine,
            Tool::Ray,
            Tool::HorizontalLine,
            Tool::VerticalLine,
            Tool::Rectangle,
        ] {
            let Some(PlaceResult::Done(kind)) = place_click(tool, Some(p), q) else {
                panic!("{tool:?} did not finish");
            };
            let d = Drawing::new(DrawingId(1), kind, tool.default_style());
            assert_eq!(tool_of(&d), tool);
        }
    }
}
