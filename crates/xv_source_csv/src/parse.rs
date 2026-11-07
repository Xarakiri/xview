use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use xv_core::{Candle, Timeframe};

use crate::CsvError;

const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCsv {
    pub candles: Vec<Candle>,
    /// (1-based line number, reason) for rejected rows. Headers and blank lines are not reported.
    pub skipped: Vec<(usize, String)>,
}

/// Parses a timestamp into epoch milliseconds.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(if n.abs() < 100_000_000_000 { n.checked_mul(1000)? } else { n });
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis())
}

fn parse_value(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        Ok(_) => Err(format!("non-finite value '{}'", s.trim())),
        Err(_) => Err(format!("invalid number '{}'", s.trim())),
    }
}

fn parse_row(fields: &[&str]) -> Result<Candle, String> {
    if fields.len() < 5 {
        return Err(format!("expected at least 5 fields, found {}", fields.len()));
    }
    let t = parse_time(fields[0]).ok_or_else(|| format!("invalid time '{}'", fields[0].trim()))?;
    let open = parse_value(fields[1])?;
    let mut high = parse_value(fields[2])?;
    let mut low = parse_value(fields[3])?;
    let close = parse_value(fields[4])?;
    let volume = match fields.get(5).map(|v| v.trim()) {
        None | Some("") => 0.0,
        Some(v) => parse_value(v)?,
    };
    if high < low {
        std::mem::swap(&mut high, &mut low);
    }
    Ok(Candle { open_time_ms: t, open, high, low, close, volume })
}

pub fn parse_csv(text: &str) -> Result<ParsedCsv, CsvError> {
    let mut rows: BTreeMap<i64, Candle> = BTreeMap::new();
    let mut skipped = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() {
            continue;
        }
        let sep = if line.contains(';') { ';' } else { ',' };
        let fields: Vec<&str> = line.split(sep).collect();
        if parse_time(fields[0]).is_none() && fields[1..].iter().all(|f| f.trim().parse::<f64>().is_err()) {
            continue; // header
        }
        match parse_row(&fields) {
            Ok(c) => {
                rows.insert(c.open_time_ms, c);
            }
            Err(e) => skipped.push((i + 1, e)),
        }
    }
    if rows.is_empty() {
        return Err(CsvError::NoRows);
    }
    Ok(ParsedCsv { candles: rows.into_values().collect(), skipped })
}

/// Guesses the bar size from the most common gap between candles.
pub fn infer_timeframe(candles: &[Candle]) -> Option<Timeframe> {
    if candles.len() < 2 {
        return None;
    }
    let mut counts: HashMap<Timeframe, usize> = HashMap::new();
    for w in candles.windows(2) {
        let gap = w[1].open_time_ms - w[0].open_time_ms;
        if gap <= 0 {
            continue;
        }
        let tf = if (28 * DAY_MS..=31 * DAY_MS).contains(&gap) {
            Some(Timeframe::Mo1)
        } else {
            Timeframe::ALL
                .iter()
                .copied()
                .map(|tf| (tf, (gap - tf.nominal_ms()).abs() as f64 / tf.nominal_ms() as f64))
                .filter(|(_, err)| *err <= 0.10)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(tf, _)| tf)
        };
        if let Some(tf) = tf {
            *counts.entry(tf).or_default() += 1;
        }
    }
    // Most frequent class; ties go to the smaller timeframe.
    counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(tf, _)| tf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_variants() {
        let want = Some(1762526220000);
        assert_eq!(parse_time("1762526220"), want);
        assert_eq!(parse_time("1762526220000"), want);
        assert_eq!(parse_time("2025-11-07T14:37:00Z"), want);
        assert_eq!(parse_time("2025-11-07 14:37"), want);
        assert_eq!(parse_time("2025-11-07T19:37:00+05:00"), want);
        assert_eq!(parse_time("2025-11-07"), Some(1762473600000));
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn messy_csv_is_cleaned_and_reported() {
        let text = "time,open,high,low,close,volume\n\
1762526460,1,2,0.5,1.5,10\n\
\n\
1762526400,1,2,0.5,1.5,10\n\
1762526460,9,9.5,8,9.2,7\n\
1762526520,1,2,0.5,NaN,1\n\
1762526580,3,4,2,3.5,1\n\
1762526640,1,2\n";
        let p = parse_csv(text).unwrap();
        let lines: Vec<usize> = p.skipped.iter().map(|s| s.0).collect();
        assert_eq!(lines, vec![6, 8]);
        let ts: Vec<i64> = p.candles.iter().map(|c| c.open_time_ms).collect();
        assert_eq!(ts, vec![1762526400000, 1762526460000, 1762526580000]);
        assert_eq!(p.candles[1].open, 9.0); // later duplicate wins
    }

    #[test]
    fn semicolons_swap_and_no_rows() {
        let p = parse_csv("1762526400;1;0.5;2;1.5\n").unwrap();
        assert_eq!((p.candles[0].high, p.candles[0].low, p.candles[0].volume), (2.0, 0.5, 0.0));
        assert!(matches!(parse_csv("time,open,high,low,close\n"), Err(CsvError::NoRows)));
    }

    fn bar(t: i64) -> Candle {
        Candle { open_time_ms: t, open: 1.0, high: 1.0, low: 1.0, close: 1.0, volume: 0.0 }
    }

    #[test]
    fn infer_hourly_with_weekend_gaps() {
        let mut cs = Vec::new();
        for day in 0..21i64 {
            if matches!(day % 7, 2 | 3) {
                continue;
            }
            for h in 0..24 {
                cs.push(bar(day * DAY_MS + h * 3_600_000));
            }
        }
        assert_eq!(infer_timeframe(&cs), Some(Timeframe::H1));
        assert_eq!(infer_timeframe(&cs[..1]), None);
    }

    #[test]
    fn infer_monthly() {
        let cs: Vec<_> = [31, 28, 31, 30].iter().scan(0i64, |t, d| {
            *t += d * DAY_MS;
            Some(bar(*t))
        }).collect();
        assert_eq!(infer_timeframe(&cs), Some(Timeframe::Mo1));
    }
}
