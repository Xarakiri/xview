use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolKey {
    pub source: SourceId,
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub source: SourceId,
    pub id: String,
    pub display: String,
    pub description: String,
    pub base: String,
    pub quote: String,
    pub tick_size: f64,
}

impl Symbol {
    pub fn key(&self) -> SymbolKey {
        SymbolKey { source: self.source.clone(), id: self.id.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_uses_source_and_id() {
        let s = Symbol {
            source: SourceId("binance".into()),
            id: "BTCUSDT".into(),
            display: "BTCUSDT".into(),
            description: "Bitcoin / TetherUS".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            tick_size: 0.01,
        };
        assert_eq!(s.key(), SymbolKey { source: SourceId("binance".into()), id: "BTCUSDT".into() });
    }
}
