use anyhow::Result;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::SystemTime;

/// Extract the media date from EXIF DateTimeOriginal, falling back to file mtime.
/// Returns Unix timestamp in seconds.
pub fn extract_date(path: &Path, mtime: i64) -> i64 {
    try_exif_date(path).unwrap_or(mtime)
}

fn try_exif_date(path: &Path) -> Option<i64> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;

    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let dt_str = match &field.value {
        exif::Value::Ascii(v) => v.first()?,
        _ => return None,
    };

    // Format: "2023:04:15 14:22:01"
    let s = std::str::from_utf8(dt_str).ok()?;
    parse_exif_datetime(s)
}

fn parse_exif_datetime(s: &str) -> Option<i64> {
    // "YYYY:MM:DD HH:MM:SS"
    let s = s.trim_end_matches('\0').trim();
    if s.len() < 19 {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    let hour: u32 = s[11..13].parse().ok()?;
    let min: u32 = s[14..16].parse().ok()?;
    let sec: u32 = s[17..19].parse().ok()?;

    use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, min, sec)?;
    let dt = NaiveDateTime::new(date, time);
    Some(Utc.from_utc_datetime(&dt).timestamp())
}

/// Get file mtime as Unix timestamp.
pub fn file_mtime(path: &Path) -> Result<i64> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let secs = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(secs)
}
