use crate::model::*;
use serde::{Deserialize, Serialize};

pub const FILE_VERSION: u32 = 1;

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

/// The version is checked before the body so future formats report `UnsupportedVersion`.
pub fn from_json(s: &str) -> Result<DrawingsFile, DrawingsError> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| DrawingsError::Parse(e.to_string()))?;
    let version = v
        .get("version")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| DrawingsError::Parse("missing version".into()))?;
    if version != FILE_VERSION as u64 {
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
            drawings: vec![Drawing {
                id: DrawingId(1),
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
            }],
        };
        let json = to_json(&f);
        assert!(json.contains("\"type\": \"ray\""));
        let back = from_json(&json).unwrap();
        assert_eq!(
            (back.version, &back.symbol, &back.drawings),
            (1, &f.symbol, &f.drawings)
        );
        assert!(matches!(
            from_json(r#"{"version":2,"symbol":"x","drawings":[]}"#),
            Err(DrawingsError::UnsupportedVersion(2))
        ));
        assert!(matches!(from_json("garbage"), Err(DrawingsError::Parse(_))));
    }
}
