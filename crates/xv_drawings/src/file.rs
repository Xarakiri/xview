use crate::model::*;
use serde::{Deserialize, Serialize};

pub const FILE_VERSION: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub struct DrawingsFile {
    pub version: u32,
    pub symbol: String,
    pub drawings: Vec<Drawing>,
}

#[derive(thiserror::Error, Debug)]
pub enum DrawingsError {
    #[error("parse: {0}")]
    Parse(String),
    #[error("unsupported version {0}")]
    UnsupportedVersion(u32),
}

pub fn to_json(f: &DrawingsFile) -> String {
    serde_json::to_string_pretty(f).expect("drawings serialize")
}

/// Versions 1 (M1) and 2 load; v1 fields missing from v2 take defaults, and the
/// returned `version` is the file's own. The version is checked before the body so future formats report `UnsupportedVersion`.
pub fn from_json(s: &str) -> Result<DrawingsFile, DrawingsError> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| DrawingsError::Parse(e.to_string()))?;
    let version = v
        .get("version")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| DrawingsError::Parse("missing version".into()))?;
    if !(1..=FILE_VERSION as u64).contains(&version) {
        return Err(DrawingsError::UnsupportedVersion(
            version.min(u32::MAX as u64) as u32,
        ));
    }
    serde_json::from_value(v).map_err(|e| DrawingsError::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_and_version_guard() {
        let f = DrawingsFile {
            version: FILE_VERSION,
            symbol: "binance:BTCUSDT".into(),
            drawings: vec![Drawing::new(
                DrawingId(1),
                DrawingKind::Ray {
                    a: Anchor {
                        time_ms: 10,
                        price: 1.5,
                    },
                    b: Anchor {
                        time_ms: 20,
                        price: 2.5,
                    },
                },
                Style::default(),
            )],
        };
        let json = to_json(&f);
        assert!(json.contains("\"type\": \"ray\""));
        let back = from_json(&json).unwrap();
        assert_eq!(
            (back.version, &back.symbol, &back.drawings),
            (2, &f.symbol, &f.drawings)
        );
        assert!(matches!(
            from_json(r#"{"version":3,"symbol":"x","drawings":[]}"#),
            Err(DrawingsError::UnsupportedVersion(3))
        ));
        assert!(matches!(from_json("garbage"), Err(DrawingsError::Parse(_))));
    }

    #[test]
    fn v1_file_loads_with_defaults() {
        let json = include_str!("../tests/fixtures/m1_v1.json");
        assert!(json.contains("\"version\": 1"));
        let f = from_json(json).unwrap();
        assert_eq!(f.drawings.len(), 5);
        for (i, d) in f.drawings.iter().enumerate() {
            let want = Style {
                color: PALETTE[i],
                width: (i as u8 % 4) + 1,
                ..Style::default()
            };
            assert_eq!(d.style, want);
            assert_eq!((&d.name, d.locked, d.hidden), (&None, false, false));
        }
        assert!(matches!(f.drawings[4].kind, DrawingKind::Rectangle { .. }));
        assert_eq!(f.drawings[4].style.fill, None);
        let out = to_json(&DrawingsFile {
            version: FILE_VERSION,
            ..f
        });
        assert!(out.contains("\"version\": 2"));
        let back = from_json(&out).unwrap();
        assert_eq!(back.version, 2);
        assert_eq!(back.drawings.len(), 5);
        assert_eq!(back.drawings[4].style.fill, None);
    }

    #[test]
    fn v2_new_fields_round_trip() {
        let mut d = Drawing::new(
            DrawingId(1),
            DrawingKind::HorizontalLine { price: 1.0 },
            Style::default(),
        );
        d.style.line_style = LineStyle::Dashed;
        d.style.fill = Some(0x11223344);
        d.style.visibility.minutes = Range {
            on: true,
            min: 5,
            max: 15,
        };
        d.style.text.content = "hi".into();
        d.name = Some("lvl".into());
        d.locked = true;
        let f = DrawingsFile {
            version: FILE_VERSION,
            symbol: "x".into(),
            drawings: vec![d.clone()],
        };
        let json = to_json(&f);
        assert!(json.contains("\"dashed\""));
        assert_eq!(from_json(&json).unwrap().drawings, vec![d]);
    }
}
