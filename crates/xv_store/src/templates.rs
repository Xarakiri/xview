use crate::atomic::{move_aside, refuse_if_unparseable, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use xv_drawings::Style;

const FILE: &str = "templates.json";
const VERSION: u32 = 1;
const MAX_CUSTOM_COLORS: usize = 10;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedTemplate {
    pub name: String,
    pub style: Style,
}

/// Keys of `last_used` / `named` are `Tool::key()` strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TemplatesFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub last_used: BTreeMap<String, Style>,
    #[serde(default)]
    pub named: BTreeMap<String, Vec<NamedTemplate>>,
    /// Newest first, at most 10.
    #[serde(default)]
    pub custom_colors: Vec<u32>,
}

fn default_version() -> u32 {
    VERSION
}

impl Default for TemplatesFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            last_used: BTreeMap::new(),
            named: BTreeMap::new(),
            custom_colors: Vec::new(),
        }
    }
}

/// Puts `c` first, removing any earlier occurrence, and caps the list at 10.
pub fn push_custom_color(f: &mut TemplatesFile, c: u32) {
    f.custom_colors.retain(|&x| x != c);
    f.custom_colors.insert(0, c);
    f.custom_colors.truncate(MAX_CUSTOM_COLORS);
}

#[derive(Debug)]
pub enum LoadedTemplates {
    Ok(TemplatesFile),
    Missing,
    /// Unreadable or invalid. `path` is the backup when `moved`, otherwise the
    /// original (still in place; `save_templates` refuses to overwrite it).
    Corrupt {
        path: PathBuf,
        moved: bool,
    },
}

pub fn templates_path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

fn parse(s: &str) -> Option<TemplatesFile> {
    serde_json::from_str::<TemplatesFile>(s)
        .ok()
        .filter(|f| f.version == VERSION)
}

pub fn load_templates(dir: &Path) -> LoadedTemplates {
    let path = templates_path(dir);
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoadedTemplates::Missing,
        Err(_) => aside(&path),
        Ok(bytes) => match String::from_utf8(bytes).ok().and_then(|s| parse(&s)) {
            Some(f) => LoadedTemplates::Ok(f),
            None => aside(&path),
        },
    }
}

fn aside(path: &Path) -> LoadedTemplates {
    match move_aside(path) {
        Ok(p) => LoadedTemplates::Corrupt {
            path: p,
            moved: true,
        },
        Err(_) => LoadedTemplates::Corrupt {
            path: path.to_path_buf(),
            moved: false,
        },
    }
}

/// Writes atomically. Refuses (`InvalidData`) to replace an existing file that
/// doesn't parse.
pub fn save_templates(dir: &Path, f: &TemplatesFile) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(f).expect("templates serialize");
    let path = templates_path(dir);
    refuse_if_unparseable(&path, |s| parse(s).is_some())?;
    write_atomic(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(w: u8) -> Style {
        Style {
            width: w,
            ..Style::default()
        }
    }

    #[test]
    fn templates_roundtrip() {
        let t = tempfile::TempDir::new().unwrap();
        assert!(matches!(load_templates(t.path()), LoadedTemplates::Missing));
        let mut f = TemplatesFile::default();
        f.last_used.insert("trend_line".into(), style(3));
        f.named.insert(
            "trend_line".into(),
            vec![NamedTemplate {
                name: "thick".into(),
                style: style(5),
            }],
        );
        push_custom_color(&mut f, 0x112233);
        save_templates(t.path(), &f).unwrap();
        match load_templates(t.path()) {
            LoadedTemplates::Ok(back) => assert_eq!(back, f),
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn templates_corrupt_moved_aside() {
        let t = tempfile::TempDir::new().unwrap();
        let p = templates_path(t.path());
        std::fs::write(&p, "{garbage").unwrap();
        let LoadedTemplates::Corrupt {
            path: moved_to,
            moved: true,
        } = load_templates(t.path())
        else {
            panic!("expected Corrupt")
        };
        assert_eq!(std::fs::read_to_string(&moved_to).unwrap(), "{garbage");
        assert!(!p.exists());
    }

    #[test]
    fn save_refuses_over_unparseable() {
        let t = tempfile::TempDir::new().unwrap();
        let p = templates_path(t.path());
        std::fs::write(&p, "{garbage").unwrap();
        let err = save_templates(t.path(), &TemplatesFile::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{garbage");
    }

    #[test]
    fn custom_colors_capped_at_10() {
        let mut f = TemplatesFile::default();
        for c in 0..15u32 {
            push_custom_color(&mut f, c);
        }
        assert_eq!(f.custom_colors.len(), 10);
        assert_eq!(f.custom_colors[0], 14);
        assert_eq!(f.custom_colors[9], 5);
        push_custom_color(&mut f, 7);
        assert_eq!(f.custom_colors.len(), 10);
        assert_eq!(f.custom_colors[0], 7);
        assert_eq!(f.custom_colors.iter().filter(|&&c| c == 7).count(), 1);
    }
}
