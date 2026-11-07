use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candle {
    pub open_time_ms: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candle_json_round_trip() {
        let c = Candle { open_time_ms: 1, open: 1.0, high: 2.0, low: 0.5, close: 1.5, volume: 3.0 };
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Candle>(&s).unwrap(), c);
    }
}
