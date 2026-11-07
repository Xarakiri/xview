use futures::future::BoxFuture;
use futures::stream::BoxStream;
use xv_core::{Candle, SourceId, Symbol, Timeframe};

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum DataError {
    #[error("{host} is not available from your network (HTTP {status})")]
    RegionBlocked { host: String, status: u16 },
    #[error("network: {0}")]
    Network(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("parse: {0}")]
    Parse(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPage {
    pub candles: Vec<Candle>,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandleUpdate {
    Candle(Candle),
    Disconnected,
    Reconnected,
}

/// A provider of symbols and candles. Returned futures may require a tokio runtime.
pub trait DataSource: Send + Sync + 'static {
    fn id(&self) -> SourceId;
    fn name(&self) -> &str;
    fn search_symbols(&self, query: &str) -> BoxFuture<'static, Result<Vec<Symbol>, DataError>>;
    fn supported_timeframes(&self, symbol: &Symbol) -> Vec<Timeframe>;
    fn history(
        &self,
        symbol: &Symbol,
        tf: Timeframe,
        end_before_ms: Option<i64>,
        limit: usize,
    ) -> BoxFuture<'static, Result<HistoryPage, DataError>>;
    fn subscribe(&self, symbol: &Symbol, tf: Timeframe) -> Option<BoxStream<'static, CandleUpdate>>;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use futures::FutureExt;
    use futures::executor::block_on;

    pub(crate) struct FakeSource {
        id: String,
    }

    impl FakeSource {
        pub(crate) fn new(id: &str) -> Self {
            Self { id: id.into() }
        }

        fn candles() -> Vec<Candle> {
            (0..3)
                .map(|i| Candle {
                    open_time_ms: i * 60_000,
                    open: 1.0,
                    high: 2.0,
                    low: 0.5,
                    close: 1.5,
                    volume: 10.0,
                })
                .collect()
        }
    }

    impl DataSource for FakeSource {
        fn id(&self) -> SourceId {
            SourceId(self.id.clone())
        }
        fn name(&self) -> &str {
            "fake"
        }
        fn search_symbols(&self, _q: &str) -> BoxFuture<'static, Result<Vec<Symbol>, DataError>> {
            futures::future::ready(Ok(vec![])).boxed()
        }
        fn supported_timeframes(&self, _s: &Symbol) -> Vec<Timeframe> {
            vec![Timeframe::M1]
        }
        fn history(
            &self,
            _s: &Symbol,
            _tf: Timeframe,
            _end: Option<i64>,
            _limit: usize,
        ) -> BoxFuture<'static, Result<HistoryPage, DataError>> {
            futures::future::ready(Ok(HistoryPage { candles: Self::candles(), has_more: false })).boxed()
        }
        fn subscribe(&self, _s: &Symbol, _tf: Timeframe) -> Option<BoxStream<'static, CandleUpdate>> {
            None
        }
    }

    #[test]
    fn fake_source_satisfies_trait() {
        let src: Box<dyn DataSource> = Box::new(FakeSource::new("fake"));
        let sym = Symbol {
            source: src.id(),
            id: "X".into(),
            display: "X".into(),
            description: String::new(),
            base: String::new(),
            quote: String::new(),
            tick_size: 0.01,
        };
        let page = block_on(src.history(&sym, Timeframe::M1, None, 10)).unwrap();
        assert_eq!(page.candles.len(), 3);
        assert!(!page.has_more);
        assert!(src.subscribe(&sym, Timeframe::M1).is_none());
        assert_eq!(block_on(src.search_symbols("x")).unwrap(), vec![]);
    }
}
