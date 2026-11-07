use crate::model::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tool {
    Cursor,
    TrendLine,
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
        Tool::TrendLine => two(|a, b| DrawingKind::TrendLine { a, b }),
        Tool::Ray => two(|a, b| DrawingKind::Ray { a, b }),
        Tool::Rectangle => two(|a, b| DrawingKind::Rectangle { a, b }),
    })
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
}
