use crate::model::*;

const UNDO_DEPTH: usize = 200;

/// Drawings plus snapshot-based undo/redo. A gesture (e.g. a drag) is one undo step.
#[derive(Debug, Default)]
pub struct DrawingSet {
    items: Vec<Drawing>,
    next_id: u64,
    undo: Vec<Vec<Drawing>>,
    redo: Vec<Vec<Drawing>>,
    gesture: Option<Vec<Drawing>>,
}

impl DrawingSet {
    pub fn new(items: Vec<Drawing>) -> Self {
        let next_id = items.iter().map(|d| d.id.0).max().map_or(1, |m| m + 1);
        DrawingSet {
            items,
            next_id,
            ..Default::default()
        }
    }

    pub fn all(&self) -> &[Drawing] {
        &self.items
    }

    pub fn get(&self, id: DrawingId) -> Option<&Drawing> {
        self.items.iter().find(|d| d.id == id)
    }

    /// Records the pre-edit state unless a gesture is already collecting edits.
    fn checkpoint(&mut self) {
        if self.gesture.is_some() {
            return;
        }
        self.push_undo(self.items.clone());
        self.redo.clear();
    }

    fn push_undo(&mut self, snap: Vec<Drawing>) {
        self.undo.push(snap);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    /// Adds a drawing; `None` (and no change) when it has a non-finite price.
    pub fn add(&mut self, kind: DrawingKind, style: Style) -> Option<DrawingId> {
        if !kind.is_finite() {
            return None;
        }
        self.checkpoint();
        let id = DrawingId(self.next_id);
        self.next_id += 1;
        self.items.push(Drawing::new(id, kind, style));
        Some(id)
    }

    /// Replaces the drawing with `d`'s id. Ignored when there is none or `d`
    /// has a non-finite price (the drawing keeps its last good shape).
    pub fn replace(&mut self, d: Drawing) {
        if self.get(d.id).is_none() || !d.is_finite() {
            return;
        }
        self.checkpoint();
        if let Some(slot) = self.items.iter_mut().find(|x| x.id == d.id) {
            *slot = d;
        }
    }

    /// Applies `f` to a copy of the drawing as one undo step. False (and no
    /// change) when the id is unknown or the result is non-finite.
    pub fn update(&mut self, id: DrawingId, f: impl FnOnce(&mut Drawing)) -> bool {
        let Some(mut d) = self.get(id).cloned() else {
            return false;
        };
        f(&mut d);
        d.id = id;
        if !d.is_finite() {
            return false;
        }
        self.checkpoint();
        if let Some(slot) = self.items.iter_mut().find(|x| x.id == id) {
            *slot = d;
        }
        true
    }

    /// Moves the drawing to the top of the z-order (end of `all()`).
    pub fn bring_to_front(&mut self, id: DrawingId) {
        let Some(i) = self.items.iter().position(|d| d.id == id) else {
            return;
        };
        if i + 1 == self.items.len() {
            return;
        }
        self.checkpoint();
        let d = self.items.remove(i);
        self.items.push(d);
    }

    /// Moves the drawing to the bottom of the z-order (start of `all()`).
    pub fn send_to_back(&mut self, id: DrawingId) {
        let Some(i) = self.items.iter().position(|d| d.id == id) else {
            return;
        };
        if i == 0 {
            return;
        }
        self.checkpoint();
        let d = self.items.remove(i);
        self.items.insert(0, d);
    }

    /// Adds a copy of `d` under a fresh id, on top. `None` when non-finite.
    pub fn insert_copy(&mut self, d: &Drawing) -> Option<DrawingId> {
        if !d.is_finite() {
            return None;
        }
        self.checkpoint();
        let id = DrawingId(self.next_id);
        self.next_id += 1;
        let mut c = d.clone();
        c.id = id;
        self.items.push(c);
        Some(id)
    }

    pub fn snapshot(&self, id: DrawingId) -> Option<Drawing> {
        self.get(id).cloned()
    }

    /// Puts `d` back (replace by id, or insert when missing) without touching
    /// undo/redo; for the dialog's live preview and Cancel.
    pub fn restore_without_undo(&mut self, d: Drawing) {
        if !d.is_finite() {
            return;
        }
        self.next_id = self.next_id.max(d.id.0.saturating_add(1));
        match self.items.iter_mut().find(|x| x.id == d.id) {
            Some(slot) => *slot = d,
            None => self.items.push(d),
        }
    }

    pub fn remove(&mut self, id: DrawingId) {
        if self.get(id).is_none() {
            return;
        }
        self.checkpoint();
        self.items.retain(|d| d.id != id);
    }

    pub fn clear(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.checkpoint();
        self.items.clear();
    }

    pub fn begin_gesture(&mut self) {
        if self.gesture.is_none() {
            self.gesture = Some(self.items.clone());
        }
    }

    pub fn end_gesture(&mut self) {
        if let Some(before) = self.gesture.take()
            && before != self.items
        {
            self.push_undo(before);
            self.redo.clear();
        }
    }

    pub fn undo(&mut self) -> bool {
        self.end_gesture();
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.items, prev));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.end_gesture();
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.items, next));
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(p: f64) -> DrawingKind {
        DrawingKind::HorizontalLine { price: p }
    }

    #[test]
    fn undo_redo_gesture() {
        let mut s = DrawingSet::new(vec![]);
        let id = s.add(k(1.0), Style::default()).unwrap();
        let after_add = s.all().to_vec();
        s.begin_gesture();
        for p in [2.0, 3.0, 4.0] {
            let mut d = s.get(id).unwrap().clone();
            d.kind = k(p);
            s.replace(d);
        }
        s.end_gesture();
        assert_eq!(s.get(id).unwrap().kind, k(4.0));
        assert!(s.undo());
        assert_eq!(s.all(), &after_add[..]);
        assert!(s.redo());
        assert_eq!(s.get(id).unwrap().kind, k(4.0));
        assert!(s.undo() && s.undo());
        assert!(s.all().is_empty());
        assert!(!s.undo());
    }

    #[test]
    fn ids_continue_after_max_and_remove_clear_undo() {
        let mut s = DrawingSet::new(vec![Drawing::new(DrawingId(7), k(1.0), Style::default())]);
        assert_eq!(s.add(k(2.0), Style::default()), Some(DrawingId(8)));
        s.remove(DrawingId(7));
        assert!(s.get(DrawingId(7)).is_none());
        s.clear();
        assert!(s.all().is_empty());
        s.undo();
        assert_eq!(s.all().len(), 1);
    }

    #[test]
    fn new_edit_clears_redo_and_depth_is_capped() {
        let mut s = DrawingSet::new(vec![]);
        s.add(k(1.0), Style::default());
        s.undo();
        s.add(k(2.0), Style::default());
        assert!(!s.redo());
        for i in 0..300 {
            s.add(k(i as f64), Style::default());
        }
        let mut n = 0;
        while s.undo() {
            n += 1;
        }
        assert_eq!(n, 200);
    }

    #[test]
    fn non_finite_drawings_are_rejected() {
        let mut s = DrawingSet::new(vec![]);
        for p in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(s.add(k(p), Style::default()), None);
            let a = Anchor {
                time_ms: 0,
                price: 1.0,
            };
            let bad = Anchor {
                time_ms: 10,
                price: p,
            };
            assert_eq!(
                s.add(DrawingKind::TrendLine { a, b: bad }, Style::default()),
                None
            );
            assert_eq!(
                s.add(DrawingKind::Rectangle { a: bad, b: a }, Style::default()),
                None
            );
        }
        assert!(s.all().is_empty());
        assert!(!s.undo(), "a rejected add must not leave an undo step");
        let id = s.add(k(1.0), Style::default()).unwrap();
        let mut d = s.get(id).unwrap().clone();
        d.kind = k(f64::NAN);
        s.replace(d);
        assert_eq!(s.get(id).unwrap().kind, k(1.0));
        assert!(DrawingKind::VerticalLine { time_ms: 5 }.is_finite());
    }

    fn three() -> (DrawingSet, [DrawingId; 3]) {
        let mut s = DrawingSet::new(vec![]);
        let ids = [1.0, 2.0, 3.0].map(|p| s.add(k(p), Style::default()).unwrap());
        (s, ids)
    }

    fn order(s: &DrawingSet) -> Vec<DrawingId> {
        s.all().iter().map(|d| d.id).collect()
    }

    #[test]
    fn update_is_one_undo_step() {
        let (mut s, [a, ..]) = three();
        let before = s.get(a).unwrap().clone();
        assert!(s.update(a, |d| {
            d.style.color = 0x112233FF;
            d.style.width = 4;
        }));
        let d = s.get(a).unwrap();
        assert_eq!((d.style.color, d.style.width), (0x112233FF, 4));
        assert!(s.undo());
        assert_eq!(s.get(a).unwrap(), &before);
        assert!(!s.update(DrawingId(99), |_| {}));
    }

    #[test]
    fn update_rejects_non_finite() {
        let (mut s, [a, ..]) = three();
        let before = s.all().to_vec();
        assert!(!s.update(a, |d| d.kind = k(f64::NAN)));
        assert_eq!(s.all(), &before[..]);
        // Only the three adds are on the undo stack.
        let mut n = 0;
        while s.undo() {
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn z_order_ops() {
        let (mut s, [a, b, c]) = three();
        s.bring_to_front(a);
        assert_eq!(order(&s), vec![b, c, a]);
        assert!(s.undo());
        assert_eq!(order(&s), vec![a, b, c]);
        s.send_to_back(c);
        assert_eq!(order(&s), vec![c, a, b]);
        assert!(s.undo());
        assert_eq!(order(&s), vec![a, b, c]);
        // No-ops leave no undo step.
        s.bring_to_front(c);
        s.send_to_back(a);
        assert!(s.undo());
        assert_eq!(order(&s), vec![a, b]);
    }

    #[test]
    fn insert_copy_new_id() {
        let (mut s, [a, ..]) = three();
        let src = s.get(a).unwrap().clone();
        let id = s.insert_copy(&src).unwrap();
        assert_eq!(id, DrawingId(4));
        let c = s.get(id).unwrap();
        assert_eq!((&c.kind, &c.style), (&src.kind, &src.style));
        assert_eq!(s.add(k(9.0), Style::default()), Some(DrawingId(5)));
        assert!(s.undo() && s.undo());
        assert_eq!(s.all().len(), 3);
        assert_eq!(
            s.insert_copy(&Drawing::new(DrawingId(1), k(f64::NAN), Style::default())),
            None
        );
    }

    #[test]
    fn restore_without_undo_leaves_history() {
        let (mut s, [a, ..]) = three();
        let snap = s.snapshot(a).unwrap();
        assert!(s.update(a, |d| d.style.width = 4));
        let (undo_len, redo_len) = (s.undo.len(), s.redo.len());
        s.restore_without_undo(snap.clone());
        assert_eq!(s.get(a).unwrap(), &snap);
        s.remove(a);
        s.undo();
        s.restore_without_undo(snap.clone());
        // missing id is inserted
        s.items.retain(|d| d.id != a);
        let (u, r) = (s.undo.len(), s.redo.len());
        s.restore_without_undo(snap.clone());
        assert_eq!((s.undo.len(), s.redo.len()), (u, r));
        assert_eq!(s.get(a).unwrap(), &snap);
        assert!(undo_len >= 4 && redo_len == 0);
        assert!(s.snapshot(DrawingId(99)).is_none());
    }

    #[test]
    fn restore_without_undo_max_id_does_not_overflow() {
        let (mut s, _) = three();
        s.restore_without_undo(Drawing::new(DrawingId(u64::MAX), k(5.0), Style::default()));
        assert!(s.get(DrawingId(u64::MAX)).is_some());
    }

    #[test]
    fn update_cannot_change_id() {
        let (mut s, [a, ..]) = three();
        assert!(s.update(a, |d| d.id = DrawingId(42)));
        assert!(s.get(a).is_some());
        assert!(s.get(DrawingId(42)).is_none());
    }
}
