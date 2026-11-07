use crate::{Candle, Symbol, Timeframe};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UpdateKind {
    Replaced,
    Appended,
    Ignored,
}

#[derive(Clone, Debug)]
pub struct Series {
    pub symbol: Symbol,
    pub timeframe: Timeframe,
    pub has_older: bool,
    candles: Vec<Candle>,
}

/// Sorts by open time and dedupes, keeping the last candle for a timestamp.
fn normalize(mut cs: Vec<Candle>) -> Vec<Candle> {
    cs.sort_by_key(|c| c.open_time_ms);
    let mut out: Vec<Candle> = Vec::with_capacity(cs.len());
    for c in cs {
        match out.last_mut() {
            Some(last) if last.open_time_ms == c.open_time_ms => *last = c,
            _ => out.push(c),
        }
    }
    out
}

impl Series {
    pub fn new(symbol: Symbol, timeframe: Timeframe, candles: Vec<Candle>, has_older: bool) -> Series {
        Series { symbol, timeframe, has_older, candles: normalize(candles) }
    }

    pub fn candles(&self) -> &[Candle] {
        &self.candles
    }

    pub fn len(&self) -> usize {
        self.candles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candles.is_empty()
    }

    pub fn last(&self) -> Option<&Candle> {
        self.candles.last()
    }

    /// Prepends candles strictly older than the current first candle and
    /// returns how many were added.
    pub fn prepend_older(&mut self, older: Vec<Candle>, has_more: bool) -> usize {
        self.has_older = has_more;
        let mut older = normalize(older);
        if let Some(first) = self.candles.first() {
            older.retain(|c| c.open_time_ms < first.open_time_ms);
        }
        let added = older.len();
        if added > 0 {
            older.extend(self.candles.drain(..));
            self.candles = older;
        }
        added
    }

    pub fn apply_update(&mut self, c: Candle) -> UpdateKind {
        match self.candles.last_mut() {
            Some(last) if c.open_time_ms == last.open_time_ms => {
                *last = c;
                UpdateKind::Replaced
            }
            Some(last) if c.open_time_ms < last.open_time_ms => UpdateKind::Ignored,
            _ => {
                self.candles.push(c);
                UpdateKind::Appended
            }
        }
    }

    /// Inserts or overwrites candles by timestamp, keeping order.
    pub fn merge_range(&mut self, cs: Vec<Candle>) {
        for c in normalize(cs) {
            match self.candles.binary_search_by_key(&c.open_time_ms, |x| x.open_time_ms) {
                Ok(i) => self.candles[i] = c,
                Err(i) => self.candles.insert(i, c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceId;

    const MIN: i64 = 60_000;

    fn sym() -> Symbol {
        Symbol {
            source: SourceId("t".into()),
            id: "X".into(),
            display: "X".into(),
            description: String::new(),
            base: "X".into(),
            quote: "Y".into(),
            tick_size: 0.01,
        }
    }

    fn c(min: i64, close: f64) -> Candle {
        Candle { open_time_ms: min * MIN, open: close, high: close, low: close, close, volume: 1.0 }
    }

    fn series(mins: &[i64]) -> Series {
        Series::new(sym(), Timeframe::M1, mins.iter().map(|&m| c(m, m as f64)).collect(), true)
    }

    #[test]
    fn new_sorts_and_dedupes_last_wins() {
        let s = Series::new(sym(), Timeframe::M1, vec![c(2, 2.0), c(1, 1.0), c(2, 9.0)], false);
        assert_eq!(s.len(), 2);
        assert_eq!(s.last().unwrap().close, 9.0);
        assert_eq!(s.candles()[0].open_time_ms, MIN);
    }

    #[test]
    fn prepend_older_drops_overlap_and_counts() {
        let mut s = series(&(10..20).collect::<Vec<_>>());
        let older: Vec<_> = (5..12).map(|m| c(m, 0.0)).collect();
        assert_eq!(s.prepend_older(older, false), 5);
        assert_eq!(s.len(), 15);
        assert!(!s.has_older);
        assert_eq!(s.candles()[0].open_time_ms, 5 * MIN);
        assert_eq!(s.candles()[5].close, 10.0);
    }

    #[test]
    fn apply_update_replaces_last_and_appends_next() {
        let mut s = series(&[0, 1, 2]);
        assert_eq!(s.apply_update(c(2, 7.0)), UpdateKind::Replaced);
        assert_eq!(s.last().unwrap().close, 7.0);
        assert_eq!(s.apply_update(c(3, 3.0)), UpdateKind::Appended);
        assert_eq!(s.len(), 4);
        assert_eq!(s.apply_update(c(1, 5.0)), UpdateKind::Ignored);
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn merge_range_fills_gap_and_overwrites_overlap() {
        let mut s = series(&[0, 1, 2, 6, 7]);
        s.merge_range(vec![c(2, 22.0), c(3, 3.0), c(4, 4.0), c(5, 5.0)]);
        let got: Vec<_> = s.candles().iter().map(|x| (x.open_time_ms / MIN, x.close)).collect();
        assert_eq!(
            got,
            [(0, 0.0), (1, 1.0), (2, 22.0), (3, 3.0), (4, 4.0), (5, 5.0), (6, 6.0), (7, 7.0)]
        );
    }
}
