use crate::atomic::{move_aside, refuse_if_unparseable, write_atomic};
use std::path::{Path, PathBuf};
use xv_core::SymbolKey;
use xv_drawings::{Drawing, DrawingsFile, FILE_VERSION, from_json, to_json};

#[derive(Debug)]
pub enum LoadedDrawings {
    Ok(Vec<Drawing>),
    Missing,
    /// Unreadable or invalid. `path` is the backup when `moved`, otherwise the
    /// original (still in place; `save_drawings` refuses to overwrite it).
    Corrupt {
        path: PathBuf,
        moved: bool,
    },
}

pub(crate) fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

/// `dir/drawings/<source>/<sanitized id>-<fnv1a64 hex>.json`
pub fn drawings_path(dir: &Path, key: &SymbolKey) -> PathBuf {
    let mut source = sanitize(&key.source.0);
    if source.is_empty() || source.chars().all(|c| c == '.') {
        source = "_".into();
    }
    dir.join("drawings").join(source).join(format!(
        "{}-{:016x}.json",
        sanitize(&key.id),
        fnv1a64(&key.id)
    ))
}

pub fn load_drawings(dir: &Path, key: &SymbolKey) -> LoadedDrawings {
    let path = drawings_path(dir, key);
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoadedDrawings::Missing,
        Err(_) => aside(&path),
        Ok(bytes) => match String::from_utf8(bytes)
            .ok()
            .and_then(|s| from_json(&s).ok())
        {
            Some(f) => LoadedDrawings::Ok(f.drawings),
            None => aside(&path),
        },
    }
}

fn aside(path: &Path) -> LoadedDrawings {
    match move_aside(path) {
        Ok(p) => LoadedDrawings::Corrupt {
            path: p,
            moved: true,
        },
        Err(_) => LoadedDrawings::Corrupt {
            path: path.to_path_buf(),
            moved: false,
        },
    }
}

/// Writes `ds` atomically. Refuses (`InvalidInput`, nothing written) a
/// drawing with a non-finite price: JSON would store it as null and the file
/// would no longer load. Refuses (`InvalidData`) to replace an existing file
/// that doesn't parse.
pub fn save_drawings(dir: &Path, key: &SymbolKey, ds: &[Drawing]) -> std::io::Result<()> {
    if let Some(d) = ds.iter().find(|d| !d.is_finite()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("drawing {} has a non-finite price", d.id.0),
        ));
    }
    let file = DrawingsFile {
        version: FILE_VERSION,
        symbol: format!("{}:{}", key.source.0, key.id),
        drawings: ds.to_vec(),
    };
    let path = drawings_path(dir, key);
    refuse_if_unparseable(&path, |s| from_json(s).is_ok())?;
    write_atomic(&path, to_json(&file).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_core::{SourceId, SymbolKey};
    use xv_drawings::{Anchor, Drawing, DrawingId, DrawingKind, Style};

    fn key(id: &str) -> SymbolKey {
        SymbolKey {
            source: SourceId("csv".into()),
            id: id.into(),
        }
    }

    fn drawing(n: u64) -> Drawing {
        Drawing {
            id: DrawingId(n),
            kind: DrawingKind::Ray {
                a: Anchor {
                    time_ms: 10,
                    price: 1.5,
                },
                b: Anchor {
                    time_ms: 20,
                    price: 2.5,
                },
            },
            style: Style::default(),
        }
    }

    #[test]
    fn drawings_round_trip() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("/a/b.csv");
        assert!(matches!(
            load_drawings(t.path(), &k),
            LoadedDrawings::Missing
        ));
        let ds = vec![drawing(1), drawing(2)];
        save_drawings(t.path(), &k, &ds).unwrap();
        match load_drawings(t.path(), &k) {
            LoadedDrawings::Ok(back) => assert_eq!(back, ds),
            _ => panic!("expected Ok"),
        }
        save_drawings(t.path(), &k, &[]).unwrap();
        assert!(matches!(load_drawings(t.path(), &k), LoadedDrawings::Ok(v) if v.is_empty()));
    }

    #[test]
    fn corrupt_drawings_file_is_moved_aside() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("x");
        let p = drawings_path(t.path(), &k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{garbage").unwrap();
        let LoadedDrawings::Corrupt {
            path: moved_to,
            moved: true,
        } = load_drawings(t.path(), &k)
        else {
            panic!("expected Corrupt")
        };
        assert_eq!(std::fs::read_to_string(&moved_to).unwrap(), "{garbage");
        assert!(!p.exists());
    }

    #[test]
    fn unsupported_version_is_moved_aside_and_backups_never_overwritten() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("x");
        let p = drawings_path(t.path(), &k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut seen = std::collections::HashSet::new();
        for i in 0..3 {
            std::fs::write(&p, format!(r#"{{"version":9,"n":{i}}}"#)).unwrap();
            let LoadedDrawings::Corrupt {
                path: moved_to,
                moved: true,
            } = load_drawings(t.path(), &k)
            else {
                panic!("expected Corrupt")
            };
            assert!(seen.insert(moved_to.clone()));
            assert!(
                std::fs::read_to_string(&moved_to)
                    .unwrap()
                    .contains(&format!("\"n\":{i}"))
            );
        }
    }

    #[test]
    fn paths_are_distinct_for_similar_ids() {
        let t = std::path::Path::new("/d");
        assert_ne!(
            drawings_path(t, &key("/a/b.csv")),
            drawings_path(t, &key("/a_b.csv"))
        );
        let long = "z".repeat(200);
        let name = drawings_path(t, &key(&long));
        assert!(name.file_name().unwrap().len() < 64 + 1 + 16 + 5 + 1);
    }

    #[test]
    fn save_refuses_to_overwrite_unbacked_corrupt_drawings() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("x");
        let p = drawings_path(t.path(), &k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{garbage").unwrap();
        let err = save_drawings(t.path(), &k, &[drawing(1)]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{garbage");
    }

    #[test]
    fn save_refuses_non_finite_drawings_and_keeps_the_old_file() {
        let t = tempfile::TempDir::new().unwrap();
        let k = key("x");
        save_drawings(t.path(), &k, &[drawing(1)]).unwrap();
        let p = drawings_path(t.path(), &k);
        let before = std::fs::read_to_string(&p).unwrap();
        for price in [f64::NAN, f64::INFINITY] {
            let mut bad = drawing(2);
            bad.kind = DrawingKind::HorizontalLine { price };
            let err = save_drawings(t.path(), &k, &[drawing(1), bad]).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        }
        assert!(matches!(load_drawings(t.path(), &k), LoadedDrawings::Ok(v) if v == [drawing(1)]));
    }

    #[cfg(unix)]
    #[test]
    fn move_aside_failure_reports_unmoved() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::TempDir::new().unwrap();
        let k = key("x");
        let p = drawings_path(t.path(), &k);
        let dir = p.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, "{garbage").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let can_write = std::fs::write(dir.join("probe"), "x").is_ok();
        let res = load_drawings(t.path(), &k);
        let guard = save_drawings(t.path(), &k, &[drawing(1)]);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if can_write {
            return; // running as root: cannot simulate rename failure
        }
        match res {
            LoadedDrawings::Corrupt { path, moved: false } => assert_eq!(path, p),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{garbage");
        assert!(guard.is_err());
    }

    #[test]
    fn fnv_known_vector() {
        assert_eq!(fnv1a64(""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64("a"), 0xaf63dc4c8601ec8c);
    }
}
