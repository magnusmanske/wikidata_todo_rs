//! Points in time as users give them to the tools: database timestamps
//! (`YYYYMMDDHHMMSS`, shorter is padded), or the relative phrases people
//! used with PHP's `strtotime`, such as "last week" or "3 days ago". UTC.

use chrono::{Days, Months, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use std::fmt;

/// What the tools' forms say about [`Timestamp::parse`].
pub const HELP: &str = "YYYYMMDDHHMMSS (shorter is fine, e.g. 20240501), or a phrase such as \
    “now”, “today”, “yesterday”, “last week”, “last month”, “-3 days”, “2 weeks ago”, “+1 day 6 hours”";

/// A MediaWiki database timestamp, `YYYYMMDDHHMMSS` (UTC).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn now() -> Self {
        Self::from_datetime(Utc::now().naive_utc())
    }

    pub fn from_datetime(time: NaiveDateTime) -> Self {
        Self(time.format("%Y%m%d%H%M%S").to_string())
    }

    /// Digits (separators such as `-` and `:` are ignored; at least the
    /// year; padded with zeros) or a relative phrase (see [`HELP`]).
    pub fn parse(input: &str) -> Option<Self> {
        Self::parse_at(input, Utc::now().naive_utc())
    }

    /// [`Self::parse`], with relative phrases relative to `now`.
    pub fn parse_at(input: &str, now: NaiveDateTime) -> Option<Self> {
        let input = input.trim();
        if let Some(time) = relative(&input.to_lowercase(), now) {
            return Some(Self::from_datetime(time));
        }
        if !input.chars().all(|c| c.is_ascii_digit() || " -:./TZ".contains(c)) {
            return None;
        }
        let digits: String = input.chars().filter(char::is_ascii_digit).collect();
        (4..=14).contains(&digits.len()).then(|| Self(format!("{digits:0<14}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The time; a zero month or day (from padding) counts as the first.
    pub fn to_datetime(&self) -> Option<NaiveDateTime> {
        let n = |at: usize| self.0.get(at..at + 2)?.parse::<u32>().ok();
        let year = self.0.get(0..4)?.parse().ok()?;
        NaiveDate::from_ymd_opt(year, n(4)?.max(1), n(6)?.max(1))?.and_hms_opt(n(8)?, n(10)?, n(12)?)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy)]
enum Unit {
    Seconds(i64),
    Months(i64),
}

fn unit(word: &str) -> Option<Unit> {
    let word = word.strip_suffix('s').filter(|w| w.len() > 2).unwrap_or(word);
    Some(match word {
        "sec" | "second" => Unit::Seconds(1),
        "min" | "minute" => Unit::Seconds(60),
        "hour" => Unit::Seconds(3600),
        "day" => Unit::Seconds(86400),
        "week" => Unit::Seconds(7 * 86400),
        "fortnight" => Unit::Seconds(14 * 86400),
        "month" => Unit::Months(1),
        "year" => Unit::Months(12),
        _ => return None,
    })
}

fn shift(time: NaiveDateTime, n: i64, unit: Unit) -> Option<NaiveDateTime> {
    match unit {
        Unit::Seconds(s) => time.checked_add_signed(TimeDelta::try_seconds(n.checked_mul(s)?)?),
        Unit::Months(m) => {
            let months = Months::new(u32::try_from(n.checked_mul(m)?.unsigned_abs()).ok()?);
            if n < 0 { time.checked_sub_months(months) } else { time.checked_add_months(months) }
        }
    }
}

/// A relative phrase (lower case) as a time, relative to `now`.
fn relative(input: &str, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let midnight = now.date().and_hms_opt(0, 0, 0)?;
    match input {
        "now" => return Some(now),
        "today" | "midnight" => return Some(midnight),
        "yesterday" => return midnight.checked_sub_days(Days::new(1)),
        "tomorrow" => return midnight.checked_add_days(Days::new(1)),
        _ => {}
    }
    // "-3days" is "-3 days".
    let words: Vec<&str> = input
        .split_whitespace()
        .flat_map(|w| {
            let number_first = w.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '-');
            match w.find(char::is_alphabetic).and_then(|at| w.split_at_checked(at)) {
                Some((n, unit)) if number_first && !n.is_empty() => vec![n, unit],
                _ => vec![w],
            }
        })
        .collect();
    let (terms, sign) = match words.split_last() {
        Some((&"ago", terms)) => (terms, -1),
        _ => (words.as_slice(), 1),
    };
    if terms.is_empty() {
        return None;
    }
    let mut time = now;
    for term in terms.chunks(2) {
        let [n, u] = term else { return None };
        let n: i64 = match *n {
            "last" | "previous" => -1,
            "this" => 0,
            "next" => 1,
            n => n.strip_prefix('+').unwrap_or(n).parse().ok()?,
        };
        time = shift(time, sign * n, unit(u)?)?;
    }
    Some(time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(input: &str) -> Option<String> {
        let now = NaiveDate::from_ymd_opt(2024, 3, 31).unwrap().and_hms_opt(12, 30, 0).unwrap();
        Timestamp::parse_at(input, now).map(|t| t.0)
    }

    #[test]
    fn digits_are_padded() {
        assert_eq!(at("20171001").as_deref(), Some("20171001000000"));
        assert_eq!(at(" 20171007235959 ").as_deref(), Some("20171007235959"));
        assert_eq!(at("2017-10-01 12:00").as_deref(), Some("20171001120000"));
        assert_eq!(at("123"), None);
        assert_eq!(at("201710012359591"), None);
        assert_eq!(at(""), None);
        assert_eq!(at("2017; DROP TABLE"), None);
    }

    #[test]
    fn relative_phrases() {
        assert_eq!(at("now").as_deref(), Some("20240331123000"));
        assert_eq!(at("Yesterday").as_deref(), Some("20240330000000"));
        assert_eq!(at("today").as_deref(), Some("20240331000000"));
        assert_eq!(at("last week").as_deref(), Some("20240324123000"));
        assert_eq!(at("last month").as_deref(), Some("20240229123000"));
        assert_eq!(at("-3 days").as_deref(), Some("20240328123000"));
        assert_eq!(at("-3days").as_deref(), Some("20240328123000"));
        assert_eq!(at("2 weeks ago").as_deref(), Some("20240317123000"));
        assert_eq!(at("+1 day 6 hours").as_deref(), Some("20240401183000"));
        assert_eq!(at("1 year ago").as_deref(), Some("20230331123000"));
        assert_eq!(at("last fortnight").as_deref(), Some("20240317123000"));
        assert_eq!(at("last tuesday"), None);
        assert_eq!(at("ago"), None);
        assert_eq!(at("3 days 2"), None);
    }

    #[test]
    fn as_datetime() {
        let t = Timestamp::parse("2024").unwrap();
        assert_eq!(t.to_datetime().unwrap().to_string(), "2024-01-01 00:00:00");
    }
}
