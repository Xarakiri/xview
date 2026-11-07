use serde_json::Value;
use xv_core::{Candle, SourceId, Symbol, Timeframe};
use xv_data::DataError;

use crate::BINANCE_SOURCE_ID;

pub fn interval(tf: Timeframe) -> &'static str {
    match tf {
        Timeframe::M1 => "1m",
        Timeframe::M3 => "3m",
        Timeframe::M5 => "5m",
        Timeframe::M15 => "15m",
        Timeframe::M30 => "30m",
        Timeframe::H1 => "1h",
        Timeframe::H2 => "2h",
        Timeframe::H4 => "4h",
        Timeframe::D1 => "1d",
        Timeframe::W1 => "1w",
        Timeframe::Mo1 => "1M",
    }
}

fn perr(msg: impl Into<String>) -> DataError {
    DataError::Parse(msg.into())
}

fn num(v: &Value, what: &str) -> Result<f64, DataError> {
    v.as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|x| x.is_finite())
        .ok_or_else(|| perr(format!("bad {what}")))
}

fn candle_from(
    t: &Value,
    o: &Value,
    h: &Value,
    l: &Value,
    c: &Value,
    v: &Value,
) -> Result<Candle, DataError> {
    Ok(Candle {
        open_time_ms: t.as_i64().ok_or_else(|| perr("bad open time"))?,
        open: num(o, "open")?,
        high: num(h, "high")?,
        low: num(l, "low")?,
        close: num(c, "close")?,
        volume: num(v, "volume")?,
    })
}

/// Parses the `/api/v3/klines` response: an array of arrays with string prices.
pub fn parse_klines(json: &str) -> Result<Vec<Candle>, DataError> {
    let v: Value = serde_json::from_str(json).map_err(|e| perr(e.to_string()))?;
    let rows = v.as_array().ok_or_else(|| perr("klines: expected array"))?;
    rows.iter()
        .map(|row| match row.as_array().map(Vec::as_slice) {
            Some([t, o, h, l, c, v, ..]) => candle_from(t, o, h, l, c, v),
            _ => Err(perr("klines: malformed row")),
        })
        .collect()
}

/// Parses `/api/v3/exchangeInfo`, keeping only symbols with status `TRADING`.
pub fn parse_exchange_info(json: &str) -> Result<Vec<Symbol>, DataError> {
    let v: Value = serde_json::from_str(json).map_err(|e| perr(e.to_string()))?;
    let list = v["symbols"].as_array().ok_or_else(|| perr("exchangeInfo: missing symbols"))?;
    let mut out = Vec::new();
    for s in list {
        if s["status"].as_str() != Some("TRADING") {
            continue;
        }
        let (Some(id), Some(base), Some(quote)) =
            (s["symbol"].as_str(), s["baseAsset"].as_str(), s["quoteAsset"].as_str())
        else {
            return Err(perr("exchangeInfo: malformed symbol"));
        };
        let tick = s["filters"]
            .as_array()
            .and_then(|f| f.iter().find(|f| f["filterType"].as_str() == Some("PRICE_FILTER")))
            .and_then(|f| f["tickSize"].as_str())
            .and_then(|t| t.parse::<f64>().ok())
            .filter(|t| *t > 0.0);
        // A symbol without a usable tick size cannot be charted; skip it.
        let Some(tick_size) = tick else { continue };
        out.push(Symbol {
            source: SourceId(BINANCE_SOURCE_ID.into()),
            id: id.into(),
            display: id.into(),
            description: format!("{base} / {quote}"),
            base: base.into(),
            quote: quote.into(),
            tick_size,
        });
    }
    Ok(out)
}

/// Parses a websocket kline event (`{"e":"kline", "k": {...}}`).
pub fn parse_ws_kline(msg: &str) -> Result<Candle, DataError> {
    let v: Value = serde_json::from_str(msg).map_err(|e| perr(e.to_string()))?;
    if v["e"].as_str() != Some("kline") {
        return Err(perr("not a kline event"));
    }
    let k = &v["k"];
    candle_from(&k["t"], &k["o"], &k["h"], &k["l"], &k["c"], &k["v"])
}

/// Maps a non-success HTTP status to an error; `None` for 2xx.
pub fn map_status(status: u16, host: &str) -> Option<DataError> {
    match status {
        200..=299 => None,
        451 | 403 => Some(DataError::RegionBlocked { host: host.into(), status }),
        _ => Some(DataError::Network(format!("{host}: HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_core::SourceId;

    const KLINES: &str = include_str!("../tests/fixtures/klines.json");
    const EXCHANGE: &str = include_str!("../tests/fixtures/exchange_info.json");
    const WS: &str = include_str!("../tests/fixtures/ws_kline.json");

    #[test]
    fn parses_klines_fixture() {
        let c = parse_klines(KLINES).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(
            c[0],
            Candle {
                open_time_ms: 1794034800000,
                open: 76056.11,
                high: 76120.32,
                low: 76000.0,
                close: 76094.95,
                volume: 209.83732,
            }
        );
        assert_eq!(c[2].open_time_ms, 1794042000000);
    }

    #[test]
    fn klines_garbage_is_parse_error() {
        assert!(matches!(parse_klines("{}"), Err(DataError::Parse(_))));
        assert!(matches!(parse_klines("[[1,\"x\"]]"), Err(DataError::Parse(_))));
    }

    #[test]
    fn exchange_info_filters_non_trading_and_reads_tick() {
        let s = parse_exchange_info(EXCHANGE).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].id, "BTCUSDT");
        assert_eq!(s[0].tick_size, 0.01);
        assert_eq!(s[0].display, "BTCUSDT");
        assert_eq!(s[0].description, "BTC / USDT");
        assert_eq!(s[0].base, "BTC");
        assert_eq!(s[0].quote, "USDT");
        assert_eq!(s[0].source, SourceId("binance".into()));
        assert_eq!(s[1].id, "ETHUSDT");
        assert_eq!(s[1].tick_size, 0.01);
    }

    #[test]
    fn parses_ws_kline() {
        let c = parse_ws_kline(WS).unwrap();
        assert_eq!(
            c,
            Candle {
                open_time_ms: 1794042000000,
                open: 76194.99,
                high: 76249.99,
                low: 76070.33,
                close: 76100.32,
                volume: 143.37419,
            }
        );
        assert!(parse_ws_kline("{\"result\":null,\"id\":1}").is_err());
    }

    #[test]
    fn status_451_is_region_blocked() {
        assert_eq!(
            map_status(451, "data-api.binance.vision"),
            Some(DataError::RegionBlocked { host: "data-api.binance.vision".into(), status: 451 })
        );
        assert!(matches!(map_status(403, "h"), Some(DataError::RegionBlocked { status: 403, .. })));
        assert!(matches!(map_status(500, "h"), Some(DataError::Network(_))));
        assert_eq!(map_status(200, "h"), None);
    }

    #[test]
    fn interval_strings() {
        let v: Vec<_> = Timeframe::ALL.iter().map(|t| interval(*t)).collect();
        assert_eq!(v, ["1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "1d", "1w", "1M"]);
    }
}
