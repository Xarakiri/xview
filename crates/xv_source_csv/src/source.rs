use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use xv_core::{Candle, SourceId, Symbol, Timeframe, TzOffset, resample};
use xv_data::{CandleUpdate, DataError, DataSource, HistoryPage};

use crate::{infer_timeframe, parse_csv};

pub const CSV_SOURCE_ID: &str = "csv";

#[derive(thiserror::Error, Debug)]
pub enum CsvError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no valid rows")]
    NoRows,
    #[error("cannot infer timeframe")]
    UnknownTimeframe,
}

struct CsvData {
    symbol: Symbol,
    base_tf: Timeframe,
    base: Arc<Vec<Candle>>,
    /// Resampled series per timeframe, built on demand.
    cache: HashMap<Timeframe, Arc<Vec<Candle>>>,
}

#[derive(Clone, Default)]
pub struct CsvSource {
    files: Arc<RwLock<HashMap<String, CsvData>>>,
}

fn decimals(v: f64) -> u32 {
    let s = format!("{v}");
    s.split_once('.').map_or(0, |(_, frac)| frac.len() as u32)
}

impl CsvSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads (or reloads) a file and returns its symbol. The symbol id is the canonical path.
    pub fn load_file(&self, path: &Path) -> Result<Symbol, CsvError> {
        let text = std::fs::read_to_string(path)?;
        let parsed = parse_csv(&text)?;
        let base_tf = infer_timeframe(&parsed.candles).ok_or(CsvError::UnknownTimeframe)?;
        let abs = path.canonicalize()?;
        let stem = abs.file_stem().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_default();
        let dec = parsed
            .candles
            .iter()
            .flat_map(|c| [c.open, c.high, c.low, c.close])
            .map(decimals)
            .max()
            .unwrap_or(0)
            .min(8);
        let symbol = Symbol {
            source: SourceId(CSV_SOURCE_ID.into()),
            id: abs.to_string_lossy().into_owned(),
            display: stem,
            description: "CSV file".into(),
            base: String::new(),
            quote: String::new(),
            tick_size: 10f64.powi(-(dec as i32)),
        };
        let data = CsvData {
            symbol: symbol.clone(),
            base_tf,
            base: Arc::new(parsed.candles),
            cache: HashMap::new(),
        };
        self.files.write().unwrap_or_else(|e| e.into_inner()).insert(symbol.id.clone(), data);
        Ok(symbol)
    }
}

impl DataSource for CsvSource {
    fn id(&self) -> SourceId {
        SourceId(CSV_SOURCE_ID.into())
    }

    fn name(&self) -> &str {
        "CSV"
    }

    fn search_symbols(&self, query: &str) -> BoxFuture<'static, Result<Vec<Symbol>, DataError>> {
        let q = query.trim().to_lowercase();
        let files = self.files.read().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<Symbol> = files
            .values()
            .filter(|d| d.symbol.display.to_lowercase().contains(&q))
            .map(|d| d.symbol.clone())
            .collect();
        out.sort_by(|a, b| a.display.cmp(&b.display).then(a.id.cmp(&b.id)));
        futures::future::ready(Ok(out)).boxed()
    }

    fn supported_timeframes(&self, symbol: &Symbol) -> Vec<Timeframe> {
        let files = self.files.read().unwrap_or_else(|e| e.into_inner());
        match files.get(&symbol.id) {
            Some(d) => Timeframe::ALL.iter().copied().filter(|tf| *tf >= d.base_tf).collect(),
            None => Vec::new(),
        }
    }

    fn history(
        &self,
        symbol: &Symbol,
        tf: Timeframe,
        end_before_ms: Option<i64>,
        limit: usize,
    ) -> BoxFuture<'static, Result<HistoryPage, DataError>> {
        let mut files = self.files.write().unwrap_or_else(|e| e.into_inner());
        let result = match files.get_mut(&symbol.id) {
            None => Err(DataError::NotFound(symbol.id.clone())),
            Some(d) if tf < d.base_tf => {
                Err(DataError::Unsupported(format!("{} is finer than the file's {}", tf.label(), d.base_tf.label())))
            }
            Some(d) => {
                let series = if tf == d.base_tf {
                    d.base.clone()
                } else {
                    let base = d.base.clone();
                    d.cache.entry(tf).or_insert_with(|| Arc::new(resample(&base, tf, TzOffset::UTC))).clone()
                };
                let end = match end_before_ms {
                    Some(t) => series.partition_point(|c| c.open_time_ms < t),
                    None => series.len(),
                };
                let start = end.saturating_sub(limit);
                Ok(HistoryPage { candles: series[start..end].to_vec(), has_more: start > 0 })
            }
        };
        futures::future::ready(result).boxed()
    }

    fn subscribe(&self, _symbol: &Symbol, _tf: Timeframe) -> Option<BoxStream<'static, CandleUpdate>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::io::Write;

    fn write_1m(n: usize) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().prefix("btc").suffix(".csv").tempfile().unwrap();
        writeln!(f, "time,open,high,low,close,volume").unwrap();
        for i in 0..n {
            writeln!(f, "{},100.25,101.5,99.75,100.5,3", 1762526400 + 60 * i as i64).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn history_resamples_and_pages() {
        let f = write_1m(2000);
        let src = CsvSource::new();
        let sym = src.load_file(f.path()).unwrap();
        assert_eq!(sym.source, SourceId("csv".into()));
        assert!(sym.display.starts_with("BTC"));
        assert_eq!(sym.tick_size, 0.01);
        assert_eq!(src.supported_timeframes(&sym).len(), 11);
        assert!(src.supported_timeframes(&sym).contains(&Timeframe::Mo1));

        let page = block_on(src.history(&sym, Timeframe::M5, None, 100)).unwrap();
        assert_eq!(page.candles.len(), 100);
        assert!(page.has_more);
        let last_1m = (1762526400 + 60 * 1999) * 1000;
        assert_eq!(page.candles[99].open_time_ms, Timeframe::M5.bucket_start(last_1m, TzOffset::UTC));

        let first = page.candles[0].open_time_ms;
        let older = block_on(src.history(&sym, Timeframe::M5, Some(first), 1000)).unwrap();
        assert!(older.candles.iter().all(|c| c.open_time_ms < first));
        assert!(!older.has_more);
    }

    #[test]
    fn errors_and_search() {
        let f = write_1m(10);
        let src = CsvSource::new();
        let sym = src.load_file(f.path()).unwrap();
        assert_eq!(block_on(src.search_symbols("BT")).unwrap().len(), 1);
        assert!(block_on(src.search_symbols("zzz")).unwrap().is_empty());
        let other = Symbol { id: "nope".into(), ..sym.clone() };
        assert!(matches!(block_on(src.history(&other, Timeframe::M1, None, 5)), Err(DataError::NotFound(_))));
        assert!(src.subscribe(&sym, Timeframe::M1).is_none());
    }

    #[test]
    fn coarser_file_rejects_finer_tf() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..5 {
            writeln!(f, "{},1,2,0.5,1.5,1", 1762524000 + 3600 * i).unwrap();
        }
        f.flush().unwrap();
        let src = CsvSource::new();
        let sym = src.load_file(f.path()).unwrap();
        assert!(!src.supported_timeframes(&sym).contains(&Timeframe::M15));
        assert!(matches!(block_on(src.history(&sym, Timeframe::M15, None, 5)), Err(DataError::Unsupported(_))));
    }
}
