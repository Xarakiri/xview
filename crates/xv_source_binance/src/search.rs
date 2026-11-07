use xv_core::Symbol;

const QUOTE_PRIORITY: [&str; 4] = ["USDT", "USDC", "FDUSD", "BTC"];
const MAX_RESULTS: usize = 50;

/// Case-insensitive ranking: exact id, id prefix, base prefix, id contains; ties by quote
/// priority, then alphabetically. At most 50 results.
pub fn rank_search(symbols: &[Symbol], q: &str) -> Vec<Symbol> {
    let q = q.trim().to_uppercase();
    let mut hits: Vec<(u8, usize, &Symbol)> = symbols
        .iter()
        .filter_map(|s| {
            let id = s.id.to_uppercase();
            let tier = if id == q {
                0
            } else if id.starts_with(&q) {
                1
            } else if s.base.to_uppercase().starts_with(&q) {
                2
            } else if id.contains(&q) {
                3
            } else {
                return None;
            };
            let quote = s.quote.to_uppercase();
            let qp = QUOTE_PRIORITY.iter().position(|p| *p == quote).unwrap_or(QUOTE_PRIORITY.len());
            Some((tier, qp, s))
        })
        .collect();
    hits.sort_by(|a, b| (a.0, a.1, &a.2.id).cmp(&(b.0, b.1, &b.2.id)));
    hits.into_iter().take(MAX_RESULTS).map(|(_, _, s)| s.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_core::SourceId;

    fn sym(id: &str, base: &str, quote: &str) -> Symbol {
        Symbol {
            source: SourceId("binance".into()),
            id: id.into(),
            display: id.into(),
            description: format!("{base} / {quote}"),
            base: base.into(),
            quote: quote.into(),
            tick_size: 0.01,
        }
    }

    fn ids(v: &[Symbol]) -> Vec<&str> {
        v.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn search_ranking() {
        let syms = vec![
            sym("ETHBTC", "ETH", "BTC"),
            sym("ETHUSDT", "ETH", "USDT"),
            sym("SETHUSDT", "SETH", "USDT"),
            sym("BTCUSDT", "BTC", "USDT"),
            sym("WBETHETH", "WBETH", "ETH"),
        ];
        assert_eq!(ids(&rank_search(&syms, "eth")), ["ETHUSDT", "ETHBTC", "SETHUSDT", "WBETHETH"]);
        assert_eq!(ids(&rank_search(&syms, "ethusdt")), ["ETHUSDT", "SETHUSDT"]);
        assert_eq!(ids(&rank_search(&syms, "BTC")), ["BTCUSDT", "ETHBTC"]);
    }

    #[test]
    fn search_caps_at_50() {
        let syms: Vec<_> = (0..80).map(|i| sym(&format!("AB{i:02}USDT"), "AB", "USDT")).collect();
        assert_eq!(rank_search(&syms, "ab").len(), 50);
    }
}
