//! Binance public market-data source: REST history, exchangeInfo search, websocket klines.

mod parse;
mod rest;
mod search;
mod ws;

use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use tokio::sync::OnceCell;
use xv_core::{SourceId, Symbol, Timeframe};
use xv_data::{CandleUpdate, DataError, DataSource, HistoryPage};

pub use parse::{interval, map_status, parse_exchange_info, parse_klines, parse_ws_kline};
pub use search::rank_search;
pub use ws::backoff_secs;

pub const BINANCE_SOURCE_ID: &str = "binance";
pub const REST_HOSTS: [&str; 2] = ["https://data-api.binance.vision", "https://api.binance.com"];
pub const WS_HOSTS: [&str; 2] = ["wss://data-stream.binance.vision", "wss://stream.binance.com:9443"];

pub struct BinanceSource {
    client: reqwest::Client,
    symbols: Arc<OnceCell<Vec<Symbol>>>,
}

impl BinanceSource {
    pub fn new() -> Self {
        Self { client: rest::build_client(), symbols: Arc::new(OnceCell::new()) }
    }
}

impl Default for BinanceSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DataSource for BinanceSource {
    fn id(&self) -> SourceId {
        SourceId(BINANCE_SOURCE_ID.into())
    }

    fn name(&self) -> &str {
        "Binance"
    }

    fn search_symbols(&self, query: &str) -> BoxFuture<'static, Result<Vec<Symbol>, DataError>> {
        rest::search(self.client.clone(), self.symbols.clone(), query.to_string()).boxed()
    }

    fn supported_timeframes(&self, _symbol: &Symbol) -> Vec<Timeframe> {
        Timeframe::ALL.to_vec()
    }

    fn history(
        &self,
        symbol: &Symbol,
        tf: Timeframe,
        end_before_ms: Option<i64>,
        limit: usize,
    ) -> BoxFuture<'static, Result<HistoryPage, DataError>> {
        rest::fetch_history(self.client.clone(), symbol.id.clone(), tf, end_before_ms, limit).boxed()
    }

    /// Returns `None` when called outside a tokio runtime, since the stream task needs one.
    fn subscribe(&self, symbol: &Symbol, tf: Timeframe) -> Option<BoxStream<'static, CandleUpdate>> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        Some(ws::subscribe(&handle, &symbol.id, interval(tf)))
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;

    #[tokio::test]
    #[ignore = "needs network"]
    async fn live_btcusdt_history() {
        let src = BinanceSource::new();
        let found = src.search_symbols("btcusdt").await.unwrap();
        assert_eq!(found[0].id, "BTCUSDT");
        let sym = found[0].clone();

        let page = src.history(&sym, Timeframe::H1, None, 10).await.unwrap();
        assert_eq!(page.candles.len(), 10);
        assert!(page.has_more);

        let mut stream = src.subscribe(&sym, Timeframe::M1).expect("tokio runtime present");
        let update = tokio::time::timeout(Duration::from_secs(70), stream.next())
            .await
            .expect("ws update within 70s")
            .expect("stream open");
        assert!(matches!(update, CandleUpdate::Candle(_)), "got {update:?}");
    }
}
