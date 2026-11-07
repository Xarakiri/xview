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

    pub fn add(&mut self, kind: DrawingKind, style: Style) -> DrawingId {
        self.checkpoint();
        let id = DrawingId(self.next_id);
        self.next_id += 1;
        self.items.push(Drawing { id, kind, style });
        id
    }

    pub fn replace(&mut self, d: Drawing) {
        if self.get(d.id).is_none() {
            return;
        }
        self.checkpoint();
        if let Some(slot) = self.items.iter_mut().find(|x| x.id == d.id) {
            *slot = d;
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
        let id = s.add(k(1.0), Style::default());
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
        let mut s = DrawingSet::new(vec![Drawing {
            id: DrawingId(7),
            kind: k(1.0),
            style: Style::default(),
        }]);
        assert_eq!(s.add(k(2.0), Style::default()), DrawingId(8));
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
}
