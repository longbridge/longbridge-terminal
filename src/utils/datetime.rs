use time::OffsetDateTime;

/// Format a Unix timestamp (seconds) as `"YYYY-MM-DD"`.
pub fn format_date(ts: i64) -> String {
    match OffsetDateTime::from_unix_timestamp(ts) {
        Ok(dt) => format!("{:04}-{:02}-{:02}", dt.year(), dt.month() as u8, dt.day()),
        Err(_) => ts.to_string(),
    }
}

/// Format a Unix timestamp (seconds) as RFC 3339 (e.g. `"2024-01-15T07:50:00Z"`).
/// Falls back to the original string if parsing fails.
pub fn format_timestamp(ts: i64) -> String {
    match OffsetDateTime::from_unix_timestamp(ts) {
        Ok(dt) => format_datetime(dt),
        Err(_) => ts.to_string(),
    }
}

/// Format an `OffsetDateTime` as RFC 3339 (e.g. `"2024-01-15T07:50:00Z"`).
pub fn format_datetime(dt: OffsetDateTime) -> String {
    dt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch() {
        let dt = OffsetDateTime::from_unix_timestamp(0).unwrap();
        assert_eq!(format_datetime(dt), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn realistic_timestamp() {
        let dt = OffsetDateTime::from_unix_timestamp(1_705_305_000).unwrap();
        assert_eq!(format_datetime(dt), "2024-01-15T07:50:00Z");
    }
}
