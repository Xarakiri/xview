//! Debounced drawings saves and settings writes through `xv_store`.
//!
//! Drawings are keyed per symbol (shared across timeframes). Unsaved edits
//! stay queued until a save succeeds, so a failed save (including
//! `xv_store` refusing to overwrite an unparseable file) never drops them;
//! the caller shows every error in the banner.
//!
//! The debounced save writes on the background executor; `flush` (symbol
//! switch, quit) writes synchronously. Every queued set carries a
//! generation, and all writes go through `write_if_newer`, which holds one
//! lock across the check and the write: an older set can never overwrite a
//! newer one, whichever path finishes last.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Context, Task};
use xv_core::SymbolKey;
use xv_drawings::Drawing;
use xv_store::{LoadedDrawings, Settings, load_drawings, save_drawings, save_settings};

use crate::workspace::Workspace;

pub const DRAWINGS_DEBOUNCE: Duration = Duration::from_millis(500);

/// Generation last written per key. The mutex also serializes the writes.
type Written = Arc<Mutex<HashMap<SymbolKey, u64>>>;

/// One queued drawings set, as handed to a writer.
pub type Batch = Vec<(SymbolKey, u64, Vec<Drawing>)>;

pub struct Persistence {
    dir: PathBuf,
    pending: Option<Task<()>>,
    /// Latest unsaved drawings per symbol, with their generation.
    unsaved: HashMap<SymbolKey, (u64, Vec<Drawing>)>,
    next_gen: u64,
    written: Written,
}

/// A failed drawings save, for the banner.
#[derive(Debug)]
pub struct SaveError {
    pub key: SymbolKey,
    pub error: io::Error,
}

/// Writes `ds` unless a set at least as new was already written for `key`.
fn write_if_newer(
    dir: &Path,
    written: &Mutex<HashMap<SymbolKey, u64>>,
    key: &SymbolKey,
    generation: u64,
    ds: &[Drawing],
) -> io::Result<()> {
    let mut w = written.lock().unwrap_or_else(|e| e.into_inner());
    if w.get(key).is_some_and(|g| *g >= generation) {
        return Ok(());
    }
    save_drawings(dir, key, ds)?;
    w.insert(key.clone(), generation);
    Ok(())
}

/// Writes a batch; returns each key's generation and outcome.
pub fn write_batch(
    dir: &Path,
    written: &Mutex<HashMap<SymbolKey, u64>>,
    batch: Batch,
) -> Vec<(SymbolKey, u64, io::Result<()>)> {
    batch
        .into_iter()
        .map(|(key, generation, ds)| {
            let r = write_if_newer(dir, written, &key, generation, &ds);
            (key, generation, r)
        })
        .collect()
}

impl Persistence {
    pub fn new(dir: PathBuf) -> Self {
        Persistence {
            dir,
            pending: None,
            unsaved: HashMap::new(),
            next_gen: 0,
            written: Arc::default(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn queue(&mut self, key: SymbolKey, ds: Vec<Drawing>) {
        self.next_gen += 1;
        self.unsaved.insert(key, (self.next_gen, ds));
    }

    /// Queues `ds` for `key` and (re)starts the debounce; a newer call
    /// replaces the pending timer. The write runs in the background.
    pub fn schedule_drawings_save(
        &mut self,
        key: SymbolKey,
        ds: Vec<Drawing>,
        cx: &mut Context<Workspace>,
    ) {
        self.queue(key, ds);
        self.pending = Some(cx.spawn(async move |ws, cx| {
            cx.background_executor().timer(DRAWINGS_DEBOUNCE).await;
            let Ok((dir, written, batch)) = ws.update(cx, |ws, _| {
                let p = &ws.persistence;
                (p.dir.clone(), p.written.clone(), p.batch())
            }) else {
                return;
            };
            let results = cx
                .background_executor()
                .spawn(async move { write_batch(&dir, &written, batch) })
                .await;
            ws.update(cx, |ws, cx| {
                let errors = ws.persistence.finish(results);
                ws.report_save_errors(errors, cx);
            })
            .ok();
        }));
    }

    /// Everything queued, sorted by key.
    fn batch(&self) -> Batch {
        let mut b: Batch = self
            .unsaved
            .iter()
            .map(|(k, (g, ds))| (k.clone(), *g, ds.clone()))
            .collect();
        b.sort_by(|a, b| (&a.0.source.0, &a.0.id).cmp(&(&b.0.source.0, &b.0.id)));
        b
    }

    /// Records write outcomes: a key leaves the queue once the set queued
    /// for it is on disk (a newer edit since keeps it queued).
    fn finish(&mut self, results: Vec<(SymbolKey, u64, io::Result<()>)>) -> Vec<SaveError> {
        let mut errors = Vec::new();
        for (key, generation, r) in results {
            match r {
                Ok(()) => {
                    if self.unsaved.get(&key).is_some_and(|(g, _)| *g == generation) {
                        self.unsaved.remove(&key);
                    }
                }
                Err(error) => errors.push(SaveError { key, error }),
            }
        }
        errors
    }

    /// Saves everything queued, synchronously. Failed keys stay queued.
    pub fn flush(&mut self) -> Result<(), Vec<SaveError>> {
        let results = write_batch(&self.dir, &self.written, self.batch());
        let errors = self.finish(results);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// The drawings to show for `key`: a queued (unsaved) set wins over the
    /// disk copy, so returning to a symbol whose save failed keeps its edits.
    pub fn load_drawings(&self, key: &SymbolKey) -> LoadedDrawings {
        match self.unsaved.get(key) {
            Some((_, ds)) => LoadedDrawings::Ok(ds.clone()),
            None => load_drawings(&self.dir, key),
        }
    }

    #[cfg(test)]
    pub fn has_unsaved(&self) -> bool {
        !self.unsaved.is_empty()
    }

    pub fn save_settings(&self, s: &Settings) -> io::Result<()> {
        save_settings(&self.dir, s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_core::SourceId;
    use xv_drawings::{DrawingId, DrawingKind, Style};
    use xv_store::drawings_path;

    fn key(id: &str) -> SymbolKey {
        SymbolKey {
            source: SourceId("binance".into()),
            id: id.into(),
        }
    }

    fn hline(id: u64, price: f64) -> Drawing {
        Drawing {
            id: DrawingId(id),
            kind: DrawingKind::HorizontalLine { price },
            style: Style::default(),
        }
    }

    #[test]
    fn flush_saves_every_queued_symbol() {
        let t = tempfile::TempDir::new().unwrap();
        let mut p = Persistence::new(t.path().to_path_buf());
        p.queue(key("BTCUSDT"), vec![hline(1, 100.0)]);
        p.queue(key("ETHUSDT"), vec![hline(2, 5.0)]);
        p.flush().unwrap();
        assert!(!p.has_unsaved());
        match load_drawings(t.path(), &key("ETHUSDT")) {
            LoadedDrawings::Ok(ds) => assert_eq!(ds, vec![hline(2, 5.0)]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refused_save_keeps_drawings_queued() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("BTCUSDT");
        let path = drawings_path(t.path(), &k);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let mut p = Persistence::new(t.path().to_path_buf());
        p.queue(k.clone(), vec![hline(1, 1.0)]);
        let errs = p.flush().unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].error.kind(), io::ErrorKind::InvalidData);
        assert!(p.has_unsaved());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        // Once the user removes the bad file, the queued drawings are written.
        std::fs::remove_file(&path).unwrap();
        p.flush().unwrap();
        assert!(matches!(load_drawings(t.path(), &k), LoadedDrawings::Ok(ds) if ds.len() == 1));
    }

    #[test]
    fn queued_set_wins_over_disk_after_a_failed_save() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("BTCUSDT");
        let path = drawings_path(t.path(), &k);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // An older good copy is on disk...
        save_drawings(t.path(), &k, &[hline(1, 1.0)]).unwrap();
        let mut p = Persistence::new(t.path().to_path_buf());
        p.queue(k.clone(), vec![hline(1, 1.0), hline(2, 2.0)]);
        // ...the save of the newer set fails (directory made read-only)...
        let dir = path.parent().unwrap();
        let mut perms = std::fs::metadata(dir).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(dir, perms.clone()).unwrap();
        assert!(p.flush().is_err());
        // ...so switching back to the symbol must show the queued set.
        match p.load_drawings(&k) {
            LoadedDrawings::Ok(ds) => assert_eq!(ds.len(), 2),
            other => panic!("{other:?}"),
        }
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(dir, perms).unwrap();
        p.flush().unwrap();
        assert!(matches!(load_drawings(t.path(), &k), LoadedDrawings::Ok(ds) if ds.len() == 2));
        assert!(matches!(p.load_drawings(&k), LoadedDrawings::Ok(ds) if ds.len() == 2));
    }

    #[test]
    fn older_background_write_never_overwrites_a_newer_flush() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("BTCUSDT");
        let mut p = Persistence::new(t.path().to_path_buf());
        p.queue(k.clone(), vec![hline(1, 1.0)]);
        // The debounced task snapshots generation 1...
        let stale = p.batch();
        // ...the user edits again and switches symbol: flush writes gen 2.
        p.queue(k.clone(), vec![hline(1, 1.0), hline(2, 2.0)]);
        p.flush().unwrap();
        // The background write of gen 1 finishes last: it must be a no-op.
        let results = write_batch(t.path(), &p.written, stale);
        assert!(results.iter().all(|r| r.2.is_ok()));
        assert!(p.finish(results).is_empty());
        assert!(matches!(load_drawings(t.path(), &k), LoadedDrawings::Ok(ds) if ds.len() == 2));
        assert!(!p.has_unsaved());
    }

    #[test]
    fn background_result_keeps_newer_edits_queued() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("BTCUSDT");
        let mut p = Persistence::new(t.path().to_path_buf());
        p.queue(k.clone(), vec![hline(1, 1.0)]);
        let batch = p.batch();
        let results = write_batch(t.path(), &p.written, batch);
        // An edit lands while the write was in flight.
        p.queue(k.clone(), vec![hline(1, 1.0), hline(2, 2.0)]);
        assert!(p.finish(results).is_empty());
        assert!(p.has_unsaved());
        p.flush().unwrap();
        assert!(matches!(load_drawings(t.path(), &k), LoadedDrawings::Ok(ds) if ds.len() == 2));
    }
}
