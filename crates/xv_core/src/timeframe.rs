use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

const MIN_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MIN_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TzOffset {
    pub seconds: i32,
}

impl TzOffset {
    pub const UTC: TzOffset = TzOffset { seconds: 0 };

    pub fn local_now() -> TzOffset {
        use chrono::Offset;
        TzOffset {
            seconds: chrono::Local::now().offset().fix().local_minus_utc(),
        }
    }

    fn ms(self) -> i64 {
        self.seconds as i64 * 1000
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Timeframe {
    M1,
    M3,
    M5,
    M15,
    M30,
    H1,
    H2,
    H4,
    D1,
    W1,
    Mo1,
}

impl Timeframe {
    pub const ALL: [Timeframe; 11] = [
        Timeframe::M1,
        Timeframe::M3,
        Timeframe::M5,
        Timeframe::M15,
        Timeframe::M30,
        Timeframe::H1,
        Timeframe::H2,
        Timeframe::H4,
        Timeframe::D1,
        Timeframe::W1,
        Timeframe::Mo1,
    ];

    pub const TOP_BAR: [Timeframe; 10] = [
        Timeframe::M1,
        Timeframe::M3,
        Timeframe::M5,
        Timeframe::M15,
        Timeframe::M30,
        Timeframe::H1,
        Timeframe::H4,
        Timeframe::D1,
        Timeframe::W1,
        Timeframe::Mo1,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Timeframe::M1 => "1m",
            Timeframe::M3 => "3m",
            Timeframe::M5 => "5m",
            Timeframe::M15 => "15m",
            Timeframe::M30 => "30m",
            Timeframe::H1 => "1h",
            Timeframe::H2 => "2h",
            Timeframe::H4 => "4h",
            Timeframe::D1 => "D",
            Timeframe::W1 => "W",
            Timeframe::Mo1 => "M",
        }
    }

    pub fn nominal_ms(self) -> i64 {
        match self {
            Timeframe::M1 => MIN_MS,
            Timeframe::M3 => 3 * MIN_MS,
            Timeframe::M5 => 5 * MIN_MS,
            Timeframe::M15 => 15 * MIN_MS,
            Timeframe::M30 => 30 * MIN_MS,
            Timeframe::H1 => HOUR_MS,
            Timeframe::H2 => 2 * HOUR_MS,
            Timeframe::H4 => 4 * HOUR_MS,
            Timeframe::D1 => DAY_MS,
            Timeframe::W1 => 7 * DAY_MS,
            Timeframe::Mo1 => 30 * DAY_MS,
        }
    }

    pub fn bucket_start(self, ts_ms: i64, tz: TzOffset) -> i64 {
        let local = ts_ms + tz.ms();
        let local_start = match self {
            Timeframe::W1 => {
                // The epoch day (1970-01-01) was a Thursday, 3 days after Monday.
                let days = local.div_euclid(DAY_MS);
                (days - (days + 3).rem_euclid(7)) * DAY_MS
            }
            Timeframe::Mo1 => {
                let date = date_of(local);
                month_start_ms(date.year(), date.month())
            }
            _ => {
                let n = self.nominal_ms();
                local.div_euclid(n) * n
            }
        };
        local_start - tz.ms()
    }

    pub fn next_bucket(self, bucket_start_ms: i64, tz: TzOffset) -> i64 {
        match self {
            Timeframe::Mo1 => {
                let date = date_of(bucket_start_ms + tz.ms());
                let (y, m) = if date.month() == 12 {
                    (date.year() + 1, 1)
                } else {
                    (date.year(), date.month() + 1)
                };
                month_start_ms(y, m) - tz.ms()
            }
            _ => bucket_start_ms + self.nominal_ms(),
        }
    }
}

fn date_of(local_ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(local_ms)
        .expect("timestamp out of range")
        .date_naive()
}

fn month_start_ms(year: i32, month: u32) -> i64 {
    NaiveDate::from_ymd_opt(year, month, 1)
        .expect("valid month start")
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_utc()
        .timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_match_spec() {
        let l: Vec<_> = Timeframe::ALL.iter().map(|t| t.label()).collect();
        assert_eq!(l, ["1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "D", "W", "M"]);
        assert!(!Timeframe::TOP_BAR.contains(&Timeframe::H2));
    }

    #[test]
    fn bucket_start_intraday_and_daily() {
        let t = 1_762_526_220_000; // 2025-11-07T14:37:00Z
        assert_eq!(Timeframe::H1.bucket_start(t, TzOffset::UTC), 1_762_524_000_000);
        assert_eq!(Timeframe::D1.bucket_start(t, TzOffset::UTC), 1_762_473_600_000);
        assert_eq!(
            Timeframe::D1.bucket_start(t, TzOffset { seconds: 5 * 3600 }),
            1_762_455_600_000
        );
    }

    #[test]
    fn weekly_starts_monday_monthly_starts_first() {
        let t = 1_762_526_220_000;
        assert_eq!(Timeframe::W1.bucket_start(t, TzOffset::UTC), 1_762_128_000_000);
        let m = Timeframe::Mo1.bucket_start(t, TzOffset::UTC);
        assert_eq!(m, 1_761_955_200_000);
        assert_eq!(Timeframe::Mo1.next_bucket(m, TzOffset::UTC), 1_764_547_200_000);
    }
}
