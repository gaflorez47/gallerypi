use chrono::{Datelike, Month, TimeZone, Utc};

/// Format a (year, month) pair as "Jan 2024".
pub fn format_month_label(year: i32, month: i32) -> String {
    let month_name = Month::try_from(month as u8)
        .map(|m| m.name())
        .unwrap_or("Unknown");
    format!("{} {}", &month_name[..3], year)
}

/// Convert Unix timestamp to (year, month).
pub fn timestamp_to_year_month(ts: i64) -> (i32, i32) {
    let dt = Utc.timestamp_opt(ts, 0).single().unwrap_or_default();
    (dt.year(), dt.month() as i32)
}
