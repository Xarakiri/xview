use crate::atomic::{move_aside, refuse_if_unparseable, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::Path;
use xv_core::{SymbolKey, Timeframe};

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Light,
    Dark,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MagnetSetting {
    #[default]
    Weak,
    Strong,
}

fn default_version() -> u32 {
    1
}

fn default_timeframe() -> Timeframe {
    Timeframe::H1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub last_symbol: Option<SymbolKey>,
    #[serde(default = "default_timeframe")]
    pub last_timeframe: Timeframe,
    #[serde(default)]
    pub window: Option<WindowRect>,
    #[serde(default)]
    pub theme: ThemeMode,
    #[serde(default)]
    pub use_utc: bool,
    #[serde(default)]
    pub magnet_on: bool,
    #[serde(default)]
    pub magnet_mode: MagnetSetting,
    #[serde(default)]
    pub stay_in_drawing_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            last_symbol: None,
            last_timeframe: Timeframe::H1,
            window: None,
            theme: ThemeMode::Light,
            use_utc: false,
            magnet_on: false,
            magnet_mode: MagnetSetting::Weak,
            stay_in_drawing_mode: false,
        }
    }
}

const FILE: &str = "settings.json";

/// Missing -> default. Unreadable/corrupt -> moved aside, default returned.
pub fn load_settings(dir: &Path) -> Settings {
    let path = dir.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        if path.exists() {
            let _ = move_aside(&path);
        }
        return Settings::default();
    };
    match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(_) => {
            let _ = move_aside(&path);
            Settings::default()
        }
    }
}

pub fn save_settings(dir: &Path, s: &Settings) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(s).expect("settings serialize");
    let path = dir.join(FILE);
    refuse_if_unparseable(&path, |s| serde_json::from_str::<Settings>(s).is_ok())?;
    write_atomic(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_core::{SourceId, SymbolKey, Timeframe};

    #[test]
    fn settings_round_trip() {
        let t = tempfile::TempDir::new().unwrap();
        let s = Settings {
            version: 1,
            last_symbol: Some(SymbolKey {
                source: SourceId("binance".into()),
                id: "BTCUSDT".into(),
            }),
            last_timeframe: Timeframe::M15,
            window: Some(WindowRect {
                x: 1.0,
                y: 2.0,
                w: 800.0,
                h: 600.0,
            }),
            theme: ThemeMode::Dark,
            use_utc: true,
            magnet_on: true,
            magnet_mode: MagnetSetting::Strong,
            stay_in_drawing_mode: true,
        };
        save_settings(t.path(), &s).unwrap();
        assert_eq!(load_settings(t.path()), s);
    }

    #[test]
    fn settings_v1_without_new_fields_loads_defaults() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(
            t.path().join("settings.json"),
            r#"{"version":1,"theme":"Dark","use_utc":true}"#,
        )
        .unwrap();
        let s = load_settings(t.path());
        assert!(!s.magnet_on);
        assert_eq!(s.magnet_mode, MagnetSetting::Weak);
        assert!(!s.stay_in_drawing_mode);
        assert_eq!(s.theme, ThemeMode::Dark);
        assert!(s.use_utc);
    }

    #[test]
    fn settings_missing_file_is_default() {
        let t = tempfile::TempDir::new().unwrap();
        assert_eq!(load_settings(t.path()), Settings::default());
    }

    #[test]
    fn settings_missing_fields_use_defaults() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("settings.json"), r#"{"version":1}"#).unwrap();
        assert_eq!(load_settings(t.path()), Settings::default());
    }

    #[test]
    fn corrupt_settings_moved_aside() {
        let t = tempfile::TempDir::new().unwrap();
        std::fs::write(t.path().join("settings.json"), "{garbage").unwrap();
        assert_eq!(load_settings(t.path()), Settings::default());
        assert!(!t.path().join("settings.json").exists());
        let moved: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("settings.json.corrupt-"))
            .collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read_to_string(&moved[0]).unwrap(), "{garbage");
    }

    #[test]
    fn save_settings_refuses_to_overwrite_unbacked_corrupt() {
        let t = tempfile::TempDir::new().unwrap();
        let p = t.path().join("settings.json");
        std::fs::write(&p, "{garbage").unwrap();
        let err = save_settings(t.path(), &Settings::default()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{garbage");
    }
}
