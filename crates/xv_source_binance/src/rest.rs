use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OnceCell;
use xv_core::{Symbol, Timeframe};
use xv_data::{DataError, HistoryPage};

use crate::parse::{interval, map_status, parse_exchange_info, parse_klines};
use crate::{REST_HOSTS, rank_search};

/// exchangeInfo is ~18 MB uncompressed (~0.3 MB gzipped); allow slow links.
const EXCHANGE_INFO_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAX_LIMIT: usize = 1000;

pub(crate) fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("XView/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client")
}

/// GET `path` with `query` against each REST host in order. Moves on after a transport error
/// or a region block; any other failure is returned immediately.
async fn get_text(
    client: &reqwest::Client,
    path: &str,
    query: &[(&str, String)],
    timeout: Duration,
) -> Result<String, DataError> {
    let mut last = DataError::Network("no hosts configured".into());
    for host in REST_HOSTS {
        let resp = match client.get(format!("{host}{path}")).query(query).timeout(timeout).send().await {
            Ok(r) => r,
            Err(e) => {
                last = DataError::Network(format!("{host}: {e}"));
                continue;
            }
        };
        match map_status(resp.status().as_u16(), host) {
            None => {
                return resp.text().await.map_err(|e| DataError::Network(format!("{host}: {e}")));
            }
            Some(e @ DataError::RegionBlocked { .. }) => last = e,
            Some(e) => return Err(e),
        }
    }
    Err(last)
}

pub(crate) async fn fetch_history(
    client: reqwest::Client,
    symbol_id: String,
    tf: Timeframe,
    end_before_ms: Option<i64>,
    limit: usize,
) -> Result<HistoryPage, DataError> {
    let requested = limit.min(MAX_LIMIT);
    let mut query = vec![
        ("symbol", symbol_id),
        ("interval", interval(tf).to_string()),
        ("limit", requested.to_string()),
    ];
    if let Some(end) = end_before_ms {
        query.push(("endTime", (end - 1).to_string()));
    }
    let body = get_text(&client, "/api/v3/klines", &query, Duration::from_secs(10)).await?;
    let candles = parse_klines(&body)?;
    let has_more = candles.len() == requested;
    Ok(HistoryPage { candles, has_more })
}

pub(crate) async fn search(
    client: reqwest::Client,
    cache: Arc<OnceCell<Vec<Symbol>>>,
    query: String,
) -> Result<Vec<Symbol>, DataError> {
    // get_or_try_init leaves the cell empty on error, so the next search retries.
    let all = cache
        .get_or_try_init(|| async {
            let body = get_text(&client, "/api/v3/exchangeInfo", &[], EXCHANGE_INFO_TIMEOUT).await?;
            parse_exchange_info(&body)
        })
        .await?;
    Ok(rank_search(all, &query))
}
