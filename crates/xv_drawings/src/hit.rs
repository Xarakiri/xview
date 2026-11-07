use crate::model::*;

pub const HIT_TOLERANCE: f32 = 6.0;

/// Maps between chart anchors and plot-area pixels.
pub trait Projector {
    fn to_screen(&self, a: Anchor) -> (f32, f32);
    fn to_anchor(&self, p: (f32, f32)) -> Anchor;
    fn width(&self) -> f32;
    fn height(&self) -> f32;

    /// Moves an anchor by a screen offset. An axis with a zero offset is left
    /// exactly untouched, so a drag along one axis cannot disturb the other.
    fn shift_anchor(&self, a: Anchor, dx: f32, dy: f32) -> Anchor {
        let moved = if dx == 0.0 && dy == 0.0 {
            a
        } else {
            let s = self.to_screen(a);
            self.to_anchor((s.0 + dx, s.1 + dy))
        };
        Anchor {
            time_ms: if dx == 0.0 { a.time_ms } else { moved.time_ms },
            price: if dy == 0.0 { a.price } else { moved.price },
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    Body,
    Handle(usize),
}

type Pt = (f32, f32);

fn dist(a: Pt, b: Pt) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Distance from `p` to segment a-b, or to the ray from a through b when `ray`.
fn dist_to_line(p: Pt, a: Pt, b: Pt, ray: bool) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return dist(p, a);
    }
    let mut t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2;
    t = if ray { t.max(0.0) } else { t.clamp(0.0, 1.0) };
    dist(p, (a.0 + t * dx, a.1 + t * dy))
}

/// Screen positions of a drawing's grab handles.
/// Rectangles expose four corners: a, (b.time, a.price), b, (a.time, b.price).
pub fn handles(d: &Drawing, proj: &dyn Projector) -> Vec<Pt> {
    match &d.kind {
        DrawingKind::TrendLine { a, b } | DrawingKind::Ray { a, b } => {
            vec![proj.to_screen(*a), proj.to_screen(*b)]
        }
        DrawingKind::HorizontalLine { price } => {
            let y = proj
                .to_screen(Anchor {
                    time_ms: 0,
                    price: *price,
                })
                .1;
            vec![(proj.width() / 2.0, y)]
        }
        DrawingKind::VerticalLine { time_ms } => {
            let x = proj
                .to_screen(Anchor {
                    time_ms: *time_ms,
                    price: 0.0,
                })
                .0;
            vec![(x, proj.height() / 2.0)]
        }
        DrawingKind::Rectangle { a, b } => vec![
            proj.to_screen(*a),
            proj.to_screen(Anchor {
                time_ms: b.time_ms,
                price: a.price,
            }),
            proj.to_screen(*b),
            proj.to_screen(Anchor {
                time_ms: a.time_ms,
                price: b.price,
            }),
        ],
    }
}

fn body_hit(d: &Drawing, p: Pt, proj: &dyn Projector) -> bool {
    let tol = HIT_TOLERANCE;
    match &d.kind {
        DrawingKind::TrendLine { a, b } => {
            dist_to_line(p, proj.to_screen(*a), proj.to_screen(*b), false) <= tol
        }
        DrawingKind::Ray { a, b } => {
            dist_to_line(p, proj.to_screen(*a), proj.to_screen(*b), true) <= tol
        }
        DrawingKind::HorizontalLine { price } => {
            (proj
                .to_screen(Anchor {
                    time_ms: 0,
                    price: *price,
                })
                .1
                - p.1)
                .abs()
                <= tol
        }
        DrawingKind::VerticalLine { time_ms } => {
            (proj
                .to_screen(Anchor {
                    time_ms: *time_ms,
                    price: 0.0,
                })
                .0
                - p.0)
                .abs()
                <= tol
        }
        DrawingKind::Rectangle { a, b } => {
            let (pa, pb) = (proj.to_screen(*a), proj.to_screen(*b));
            let (x0, x1) = (pa.0.min(pb.0) - tol, pa.0.max(pb.0) + tol);
            let (y0, y1) = (pa.1.min(pb.1) - tol, pa.1.max(pb.1) + tol);
            p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1
        }
    }
}

/// Handles take priority over bodies; among equals the last (topmost) drawing wins.
pub fn hit_test(ds: &[Drawing], p: Pt, proj: &dyn Projector) -> Option<(DrawingId, Hit)> {
    for d in ds.iter().rev() {
        if let Some(i) = handles(d, proj)
            .iter()
            .position(|h| dist(*h, p) <= HIT_TOLERANCE)
        {
            return Some((d.id, Hit::Handle(i)));
        }
    }
    ds.iter()
        .rev()
        .find(|d| body_hit(d, p, proj))
        .map(|d| (d.id, Hit::Body))
}

impl Drawing {
    /// Moves the drawing by a screen-space offset.
    pub fn translated(&self, dx: f32, dy: f32, proj: &dyn Projector) -> Drawing {
        let mv = |a: Anchor| proj.shift_anchor(a, dx, dy);
        let kind = match &self.kind {
            DrawingKind::TrendLine { a, b } => DrawingKind::TrendLine {
                a: mv(*a),
                b: mv(*b),
            },
            DrawingKind::Ray { a, b } => DrawingKind::Ray {
                a: mv(*a),
                b: mv(*b),
            },
            DrawingKind::Rectangle { a, b } => DrawingKind::Rectangle {
                a: mv(*a),
                b: mv(*b),
            },
            DrawingKind::HorizontalLine { price } => DrawingKind::HorizontalLine {
                price: proj
                    .shift_anchor(
                        Anchor {
                            time_ms: 0,
                            price: *price,
                        },
                        0.0,
                        dy,
                    )
                    .price,
            },
            DrawingKind::VerticalLine { time_ms } => DrawingKind::VerticalLine {
                time_ms: proj
                    .shift_anchor(
                        Anchor {
                            time_ms: *time_ms,
                            price: 0.0,
                        },
                        dx,
                        0.0,
                    )
                    .time_ms,
            },
        };
        Drawing {
            id: self.id,
            kind,
            style: self.style,
        }
    }

    /// Moves handle `idx` to the anchor `to`; out-of-range indices change nothing.
    pub fn with_handle(&self, idx: usize, to: Anchor) -> Drawing {
        let kind = match (&self.kind, idx) {
            (DrawingKind::TrendLine { b, .. }, 0) => DrawingKind::TrendLine { a: to, b: *b },
            (DrawingKind::TrendLine { a, .. }, 1) => DrawingKind::TrendLine { a: *a, b: to },
            (DrawingKind::Ray { b, .. }, 0) => DrawingKind::Ray { a: to, b: *b },
            (DrawingKind::Ray { a, .. }, 1) => DrawingKind::Ray { a: *a, b: to },
            (DrawingKind::HorizontalLine { .. }, 0) => {
                DrawingKind::HorizontalLine { price: to.price }
            }
            (DrawingKind::VerticalLine { .. }, 0) => DrawingKind::VerticalLine {
                time_ms: to.time_ms,
            },
            (DrawingKind::Rectangle { a, b }, i) => {
                let (mut a, mut b) = (*a, *b);
                match i {
                    0 => a = to,
                    1 => {
                        b.time_ms = to.time_ms;
                        a.price = to.price;
                    }
                    2 => b = to,
                    3 => {
                        a.time_ms = to.time_ms;
                        b.price = to.price;
                    }
                    _ => {}
                }
                DrawingKind::Rectangle { a, b }
            }
            (k, _) => k.clone(),
        };
        Drawing {
            id: self.id,
            kind,
            style: self.style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct P;
    impl Projector for P {
        fn to_screen(&self, a: Anchor) -> (f32, f32) {
            (a.time_ms as f32 / 1000.0, 1000.0 - a.price as f32)
        }
        fn to_anchor(&self, p: (f32, f32)) -> Anchor {
            Anchor {
                time_ms: (p.0 * 1000.0).round() as i64,
                price: (1000.0 - p.1) as f64,
            }
        }
        fn width(&self) -> f32 {
            800.0
        }
        fn height(&self) -> f32 {
            600.0
        }
    }
    /// Anchor whose screen position is (x, y).
    fn s(x: f32, y: f32) -> Anchor {
        P.to_anchor((x, y))
    }
    fn d(id: u64, kind: DrawingKind) -> Drawing {
        Drawing {
            id: DrawingId(id),
            kind,
            style: Style::default(),
        }
    }

    #[test]
    fn hit_trendline_body_and_handle() {
        let ds = [d(
            1,
            DrawingKind::TrendLine {
                a: s(0.0, 100.0),
                b: s(100.0, 0.0),
            },
        )];
        assert_eq!(
            hit_test(&ds, (50.0, 50.0), &P),
            Some((DrawingId(1), Hit::Body))
        );
        assert_eq!(
            hit_test(&ds, (1.0, 99.0), &P),
            Some((DrawingId(1), Hit::Handle(0)))
        );
        assert_eq!(hit_test(&ds, (50.0, 70.0), &P), None);
    }

    #[test]
    fn ray_hits_only_forward() {
        let ds = [d(
            1,
            DrawingKind::Ray {
                a: s(100.0, 500.0),
                b: s(200.0, 500.0),
            },
        )];
        assert_eq!(
            hit_test(&ds, (400.0, 500.0), &P),
            Some((DrawingId(1), Hit::Body))
        );
        assert_eq!(hit_test(&ds, (50.0, 500.0), &P), None);
    }

    #[test]
    fn rectangle_hits_inside() {
        let ds = [d(
            1,
            DrawingKind::Rectangle {
                a: s(100.0, 100.0),
                b: s(200.0, 200.0),
            },
        )];
        assert_eq!(
            hit_test(&ds, (150.0, 150.0), &P),
            Some((DrawingId(1), Hit::Body))
        );
        assert_eq!(
            hit_test(&ds, (100.0, 150.0), &P),
            Some((DrawingId(1), Hit::Body))
        );
        assert_eq!(hit_test(&ds, (300.0, 150.0), &P), None);
    }

    #[test]
    fn topmost_wins() {
        let ds = [
            d(
                1,
                DrawingKind::TrendLine {
                    a: s(0.0, 100.0),
                    b: s(100.0, 0.0),
                },
            ),
            d(
                2,
                DrawingKind::TrendLine {
                    a: s(0.0, 100.0),
                    b: s(100.0, 0.0),
                },
            ),
        ];
        assert_eq!(hit_test(&ds, (50.0, 50.0), &P).unwrap().0, DrawingId(2));
    }

    #[test]
    fn axis_lines_and_handles() {
        let h = d(1, DrawingKind::HorizontalLine { price: 700.0 }); // y = 300
        let v = d(2, DrawingKind::VerticalLine { time_ms: 50_000 }); // x = 50
        assert_eq!(handles(&h, &P), vec![(400.0, 300.0)]);
        assert_eq!(handles(&v, &P), vec![(50.0, 300.0)]);
        assert_eq!(
            hit_test(&[h.clone()], (123.0, 303.0), &P),
            Some((DrawingId(1), Hit::Body))
        );
        assert_eq!(
            hit_test(&[v], (53.0, 10.0), &P),
            Some((DrawingId(2), Hit::Body))
        );
    }

    /// Time mapping that snaps to 10 s steps (lossy), like a gap snapping to a bar.
    struct Snap;
    impl Projector for Snap {
        fn to_screen(&self, a: Anchor) -> (f32, f32) {
            (a.time_ms as f32 / 1000.0, 1000.0 - a.price as f32)
        }
        fn to_anchor(&self, p: (f32, f32)) -> Anchor {
            Anchor {
                time_ms: (p.0 / 10.0).round() as i64 * 10_000,
                price: (1000.0 - p.1) as f64,
            }
        }
        fn width(&self) -> f32 {
            800.0
        }
        fn height(&self) -> f32 {
            600.0
        }
    }

    #[test]
    fn translate_zero_is_identity() {
        let a = Anchor {
            time_ms: 1_700_000_123_457,
            price: 123.456789,
        };
        for kind in [
            DrawingKind::TrendLine { a, b: a },
            DrawingKind::Ray { a, b: a },
            DrawingKind::Rectangle { a, b: a },
            DrawingKind::HorizontalLine { price: a.price },
            DrawingKind::VerticalLine { time_ms: a.time_ms },
        ] {
            let dr = d(1, kind);
            assert_eq!(dr.translated(0.0, 0.0, &P), dr);
        }
    }

    #[test]
    fn vertical_only_drag_keeps_time() {
        let a = Anchor {
            time_ms: 123_457,
            price: 500.0,
        };
        let b = Anchor {
            time_ms: 987_654,
            price: 400.0,
        };
        let dr = d(1, DrawingKind::TrendLine { a, b });
        let m = dr.translated(0.0, 10.0, &Snap);
        assert_eq!(
            m.kind,
            DrawingKind::TrendLine {
                a: Anchor {
                    time_ms: a.time_ms,
                    price: 490.0
                },
                b: Anchor {
                    time_ms: b.time_ms,
                    price: 390.0
                },
            }
        );
        let v = d(2, DrawingKind::VerticalLine { time_ms: 123_457 });
        assert_eq!(v.translated(0.0, 50.0, &Snap), v);
    }

    #[test]
    fn translate_and_with_handle() {
        let t = d(
            1,
            DrawingKind::TrendLine {
                a: s(0.0, 100.0),
                b: s(100.0, 0.0),
            },
        );
        let m = t.translated(10.0, 20.0, &P);
        assert_eq!(
            m.kind,
            DrawingKind::TrendLine {
                a: s(10.0, 120.0),
                b: s(110.0, 20.0)
            }
        );
        let h = d(2, DrawingKind::HorizontalLine { price: 700.0 });
        // horizontal ignores dx
        assert_eq!(
            h.translated(30.0, 10.0, &P).kind,
            DrawingKind::HorizontalLine { price: 690.0 }
        );
        let moved = t.with_handle(1, s(5.0, 5.0));
        assert_eq!(
            moved.kind,
            DrawingKind::TrendLine {
                a: s(0.0, 100.0),
                b: s(5.0, 5.0)
            }
        );
    }
}
