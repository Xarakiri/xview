use std::collections::HashMap;

use xv_core::{Series, SymbolKey, Timeframe};

const CAPACITY: usize = 32;

type Key = (SymbolKey, Timeframe);

/// LRU cache of series keyed by (symbol, timeframe). `get` and `get_mut`
/// count as a use for recency, hence `&mut self`.
#[derive(Default)]
pub struct SeriesCache {
    map: HashMap<Key, (Series, u64)>,
    clock: u64,
}

impl SeriesCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    pub fn get(&mut self, key: &SymbolKey, tf: Timeframe) -> Option<&Series> {
        self.get_mut(key, tf).map(|s| &*s)
    }

    pub fn get_mut(&mut self, key: &SymbolKey, tf: Timeframe) -> Option<&mut Series> {
        let t = self.tick();
        self.map.get_mut(&(key.clone(), tf)).map(|(s, used)| {
            *used = t;
            s
        })
    }

    /// Drops every timeframe of `key` (e.g. a CSV file that was reloaded).
    pub fn remove_symbol(&mut self, key: &SymbolKey) {
        self.map.retain(|(k, _), _| k != key);
    }

    /// Inserts (or replaces) a series, evicting the least recently used one when full.
    pub fn insert(&mut self, series: Series) {
        let key = (series.symbol.key(), series.timeframe);
        let t = self.tick();
        if !self.map.contains_key(&key)
            && self.map.len() >= CAPACITY
            && let Some(oldest) = self.map.iter().min_by_key(|(_, (_, u))| *u).map(|(k, _)| k.clone())
        {
            self.map.remove(&oldest);
        }
        self.map.insert(key, (series, t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xv_core::{SourceId, Symbol};

    fn series(i: usize) -> Series {
        let sym = Symbol {
            source: SourceId("t".into()),
            id: format!("S{i}"),
            display: format!("S{i}"),
            description: String::new(),
            base: String::new(),
            quote: String::new(),
            tick_size: 0.01,
        };
        Series::new(sym, Timeframe::M1, vec![], false)
    }

    #[test]
    fn cache_evicts_least_recent() {
        let mut c = SeriesCache::new();
        let keys: Vec<_> = (0..33).map(|i| series(i).symbol.key()).collect();
        for i in 0..32 {
            c.insert(series(i));
        }
        // touch the first so the second becomes least recent
        assert!(c.get(&keys[0], Timeframe::M1).is_some());
        c.insert(series(32));
        assert_eq!(c.len(), 32);
        assert!(c.get(&keys[0], Timeframe::M1).is_some());
        assert!(c.get(&keys[1], Timeframe::M1).is_none());
        assert!(c.get(&keys[32], Timeframe::M1).is_some());
    }

    #[test]
    fn remove_symbol_drops_all_timeframes() {
        let mut c = SeriesCache::new();
        let s = series(0);
        let key = s.symbol.key();
        c.insert(s.clone());
        c.insert(Series::new(s.symbol.clone(), Timeframe::H1, vec![], false));
        c.insert(series(1));
        c.remove_symbol(&key);
        assert_eq!(c.len(), 1);
        assert!(c.get(&key, Timeframe::H1).is_none());
    }

    #[test]
    fn cache_evicts_first_when_untouched() {
        let mut c = SeriesCache::new();
        for i in 0..33 {
            c.insert(series(i));
        }
        assert!(c.get(&series(0).symbol.key(), Timeframe::M1).is_none());
        assert!(c.get(&series(1).symbol.key(), Timeframe::M1).is_some());
    }
}
