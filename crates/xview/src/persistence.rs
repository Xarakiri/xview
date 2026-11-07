//! Debounced drawings saves and settings writes through `xv_store`.
//!
//! Drawings are keyed per symbol (shared across timeframes). Unsaved edits
//! stay queued until a save succeeds, so a failed save (including
//! `xv_store` refusing to overwrite an unparseable file) never drops them;
//! the caller shows every error in the banner.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Task};
use xv_core::SymbolKey;
use xv_drawings::Drawing;
use xv_store::{Settings, save_drawings, save_settings};

use crate::workspace::Workspace;

pub const DRAWINGS_DEBOUNCE: Duration = Duration::from_millis(500);

pub struct Persistence {
    dir: PathBuf,
    pending: Option<Task<()>>,
    /// Latest unsaved drawings per symbol.
    unsaved: HashMap<SymbolKey, Vec<Drawing>>,
}

/// A failed drawings save, for the banner.
#[derive(Debug)]
pub struct SaveError {
    pub key: SymbolKey,
    pub error: io::Error,
}

impl Persistence {
    pub fn new(dir: PathBuf) -> Self {
        Persistence {
            dir,
            pending: None,
            unsaved: HashMap::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Queues `ds` for `key` and (re)starts the debounce; a newer call
    /// replaces the pending timer.
    pub fn schedule_drawings_save(
        &mut self,
        key: SymbolKey,
        ds: Vec<Drawing>,
        cx: &mut Context<Workspace>,
    ) {
        self.unsaved.insert(key, ds);
        self.pending = Some(cx.spawn(async move |ws, cx| {
            cx.background_executor().timer(DRAWINGS_DEBOUNCE).await;
            ws.update(cx, |ws, cx| ws.flush_drawings(cx)).ok();
        }));
    }

    /// Saves everything queued, synchronously. Failed keys stay queued.
    pub fn flush(&mut self) -> Result<(), Vec<SaveError>> {
        let mut errors = Vec::new();
        let mut keys: Vec<SymbolKey> = self.unsaved.keys().cloned().collect();
        keys.sort_by(|a, b| (&a.source.0, &a.id).cmp(&(&b.source.0, &b.id)));
        for key in keys {
            let ds = &self.unsaved[&key];
            match save_drawings(&self.dir, &key, ds) {
                Ok(()) => {
                    self.unsaved.remove(&key);
                }
                Err(error) => errors.push(SaveError { key, error }),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
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
    use xv_store::{LoadedDrawings, drawings_path, load_drawings};

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
        p.unsaved.insert(key("BTCUSDT"), vec![hline(1, 100.0)]);
        p.unsaved.insert(key("ETHUSDT"), vec![hline(2, 5.0)]);
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
        p.unsaved.insert(k.clone(), vec![hline(1, 1.0)]);
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
}
