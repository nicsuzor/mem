use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateParseError {
    InvalidFormat { param: &'static str, value: String },
}

impl fmt::Display for DateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateParseError::InvalidFormat { param, value } => write!(
                f,
                "Invalid date format for '{param}': \"{value}\". Expected YYYY-MM-DD or RFC 3339 timestamp (e.g. '2026-10-04' or '2026-10-04T04:08:49Z')."
            ),
        }
    }
}

impl std::error::Error for DateParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeforeBound {
    /// Date-only filter (e.g. `before="2026-10-05"`): matches any instant whose UTC calendar date is <= specified date,
    /// i.e., instant < next_day_at_midnight_utc.
    DateExclusiveUpper(DateTime<Utc>),
    /// Timestamp filter (e.g. `before="2026-10-05T00:00:00Z"`): matches any instant <= timestamp.
    TimestampInclusive(DateTime<Utc>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DateFilter {
    pub since: Option<DateTime<Utc>>,
    pub before: Option<BeforeBound>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedBound {
    Date(NaiveDate),
    Timestamp(DateTime<Utc>),
}

pub fn parse_bound_str(s: &str, param: &'static str) -> Result<ParsedBound, DateParseError> {
    let trimmed = s.trim().trim_matches(['\'', '"']);
    if trimmed.is_empty() {
        return Err(DateParseError::InvalidFormat {
            param,
            value: s.to_string(),
        });
    }

    // 1. Exactly YYYY-MM-DD (10 chars, NaiveDate)
    if trimmed.len() == 10 {
        if let Ok(d) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
            return Ok(ParsedBound::Date(d));
        }
    }

    // 2. RFC 3339 timestamp with timezone (support case-insensitive T/Z)
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(ParsedBound::Timestamp(dt.with_timezone(&Utc)));
    }
    let upper = trimmed.to_ascii_uppercase();
    if let Ok(dt) = DateTime::parse_from_rfc3339(&upper) {
        return Ok(ParsedBound::Timestamp(dt.with_timezone(&Utc)));
    }

    // 3. Naive date-time without timezone (assumed UTC)
    for fmt in &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            return Ok(ParsedBound::Timestamp(ndt.and_utc()));
        }
    }

    Err(DateParseError::InvalidFormat {
        param,
        value: s.to_string(),
    })
}

pub fn parse_modified_instant(m: &str) -> Option<DateTime<Utc>> {
    let trimmed = m.trim().trim_matches(['\'', '"']);
    if trimmed.is_empty() {
        return None;
    }

    // 1. Try RFC 3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc));
    }
    let upper = trimmed.to_ascii_uppercase();
    if let Ok(dt) = DateTime::parse_from_rfc3339(&upper) {
        return Some(dt.with_timezone(&Utc));
    }

    // 2. Try NaiveDateTime (assume UTC)
    for fmt in &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(trimmed, fmt) {
            return Some(ndt.and_utc());
        }
    }

    // 3. Try date-only (10 chars, assume UTC 00:00:00)
    if trimmed.len() >= 10 {
        if let Ok(d) = NaiveDate::parse_from_str(&trimmed[..10], "%Y-%m-%d") {
            return d.and_hms_opt(0, 0, 0).map(|ndt| ndt.and_utc());
        }
    }

    None
}

impl DateFilter {
    pub fn is_active(&self) -> bool {
        self.since.is_some() || self.before.is_some()
    }

    pub fn parse(since: Option<&str>, before: Option<&str>) -> Result<Self, DateParseError> {
        let since = match since {
            Some(s) => match parse_bound_str(s, "since")? {
                ParsedBound::Date(d) => Some(d.and_hms_opt(0, 0, 0).unwrap().and_utc()),
                ParsedBound::Timestamp(ts) => Some(ts),
            },
            None => None,
        };

        let before = match before {
            Some(s) => match parse_bound_str(s, "before")? {
                ParsedBound::Date(d) => {
                    let next_day = d.succ_opt().ok_or_else(|| DateParseError::InvalidFormat {
                        param: "before",
                        value: s.to_string(),
                    })?;
                    Some(BeforeBound::DateExclusiveUpper(
                        next_day.and_hms_opt(0, 0, 0).unwrap().and_utc(),
                    ))
                }
                ParsedBound::Timestamp(ts) => Some(BeforeBound::TimestampInclusive(ts)),
            },
            None => None,
        };

        Ok(DateFilter { since, before })
    }

    pub fn from_args(args: &serde_json::Value) -> Result<Self, rmcp::ErrorData> {
        let extract = |param: &'static str| -> Result<Option<&str>, rmcp::ErrorData> {
            let Some(val) = args.get(param) else {
                return Ok(None);
            };
            if val.is_null() {
                return Ok(None);
            }
            val.as_str().map(Some).ok_or_else(|| rmcp::ErrorData {
                code: rmcp::model::ErrorCode::INVALID_PARAMS,
                message: std::borrow::Cow::from(format!(
                    "Invalid '{param}' parameter: expected string, found {val}"
                )),
                data: None,
            })
        };

        let since_str = extract("since")?;
        let before_str = extract("before")?;

        Self::parse(since_str, before_str).map_err(|e| rmcp::ErrorData {
            code: rmcp::model::ErrorCode::INVALID_PARAMS,
            message: std::borrow::Cow::from(e.to_string()),
            data: None,
        })
    }

    pub fn matches(&self, modified: Option<&str>) -> bool {
        if !self.is_active() {
            return true;
        }
        let Some(m) = modified else {
            return false;
        };
        let Some(instant) = parse_modified_instant(m) else {
            return false;
        };
        if let Some(s) = self.since {
            if instant < s {
                return false;
            }
        }
        if let Some(b) = self.before {
            match b {
                BeforeBound::DateExclusiveUpper(next_day) => {
                    if instant >= next_day {
                        return false;
                    }
                }
                BeforeBound::TimestampInclusive(upper) => {
                    if instant > upper {
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bound_str_dates_and_timestamps() {
        assert!(matches!(
            parse_bound_str("2026-10-04", "since").unwrap(),
            ParsedBound::Date(d) if d == NaiveDate::from_ymd_opt(2026, 10, 4).unwrap()
        ));
        assert!(matches!(
            parse_bound_str("2026-10-04T04:08:49Z", "since").unwrap(),
            ParsedBound::Timestamp(_)
        ));
        assert!(matches!(
            parse_bound_str("2026-10-04t04:08:49z", "since").unwrap(),
            ParsedBound::Timestamp(_)
        ));
        assert!(matches!(
            parse_bound_str("2026-10-04T04:08:49+00:00", "since").unwrap(),
            ParsedBound::Timestamp(_)
        ));
        assert!(matches!(
            parse_bound_str("2026-10-04T04:08:49", "since").unwrap(),
            ParsedBound::Timestamp(_)
        ));
        assert!(matches!(
            parse_bound_str("2026-10-04 04:08:49", "since").unwrap(),
            ParsedBound::Timestamp(_)
        ));

        // Invalid inputs
        assert!(parse_bound_str("not-a-date", "since").is_err());
        assert!(parse_bound_str("", "since").is_err());
        assert!(parse_bound_str("2026-13-45", "since").is_err());
    }

    #[test]
    fn test_date_filter_matches() {
        // since timestamp
        let filter = DateFilter::parse(Some("2026-10-04T04:08:49Z"), None).unwrap();
        assert!(filter.matches(Some("2026-10-04T09:18:19Z")));
        assert!(filter.matches(Some("2026-10-04T04:08:49Z")));
        assert!(!filter.matches(Some("2026-10-04T04:08:48Z")));
        assert!(!filter.matches(Some("2026-10-03T23:59:59Z")));

        // before timestamp
        let filter_before = DateFilter::parse(None, Some("2026-10-05T00:00:00Z")).unwrap();
        assert!(filter_before.matches(Some("2026-10-04T23:59:59Z")));
        assert!(filter_before.matches(Some("2026-10-05T00:00:00Z")));
        assert!(!filter_before.matches(Some("2026-10-05T00:00:01Z")));
        assert!(!filter_before.matches(Some("2026-10-05T12:00:00Z")));

        // before date-only (inclusive of the whole calendar day in UTC)
        let filter_before_date = DateFilter::parse(None, Some("2026-10-05")).unwrap();
        assert!(filter_before_date.matches(Some("2026-10-05T00:00:00Z")));
        assert!(filter_before_date.matches(Some("2026-10-05T23:59:59Z")));
        assert!(!filter_before_date.matches(Some("2026-10-06T00:00:00Z")));

        // since date-only
        let filter_since_date = DateFilter::parse(Some("2026-10-05"), None).unwrap();
        assert!(filter_since_date.matches(Some("2026-10-05T00:00:00Z")));
        assert!(filter_since_date.matches(Some("2026-10-05T12:00:00Z")));
        assert!(!filter_since_date.matches(Some("2026-10-04T23:59:59Z")));

        // None modified
        assert!(!filter.matches(None));
        let inactive = DateFilter::default();
        assert!(inactive.matches(None));
        assert!(inactive.matches(Some("2026-10-05")));
    }
}
