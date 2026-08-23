use std::time::{SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset, macros::format_description};

pub fn timestamp(value: &str) -> String {
    if value.trim().is_empty() || value == "none" {
        return "-".to_owned();
    }
    let Ok(seconds) = value.parse::<i64>() else {
        return parse_timestamp(value)
            .map_or_else(|| value.to_owned(), |seconds| timestamp_at(seconds, now()));
    };
    timestamp_at(seconds, now())
}

pub fn optional_timestamp(value: Option<&str>) -> String {
    value.map_or_else(|| "-".to_owned(), timestamp)
}

pub fn timestamp_at(seconds: i64, current: i64) -> String {
    let date = OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|value| {
            let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
            value.to_offset(offset).format(&format_description!("[year]-[month]-[day] [hour]:[minute]:[second] [offset_hour sign:mandatory]:[offset_minute]")).ok()
        })
        .unwrap_or_else(|| "invalid timestamp".to_owned());
    format!("{date} ({})", relative(seconds, current))
}

pub fn relative(seconds: i64, current: i64) -> String {
    let difference = seconds.saturating_sub(current);
    let amount = difference.unsigned_abs();
    let (value, unit) = if amount < 60 {
        (amount, "s")
    } else if amount < 3_600 {
        (amount / 60, "m")
    } else if amount < 86_400 {
        (amount / 3_600, "h")
    } else {
        (amount / 86_400, "d")
    };
    if difference >= 0 {
        format!("in {value}{unit}")
    } else {
        format!("{value}{unit} ago")
    }
}

pub fn duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    format!(
        "{}h {:02}m {:02}s",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

pub fn elapsed(started_at: &str) -> String {
    let started = parse_timestamp(started_at);
    started.map_or_else(
        || "unknown".to_owned(),
        |value| duration(now().saturating_sub(value)),
    )
}

fn parse_timestamp(value: &str) -> Option<i64> {
    if let Ok(seconds) = value.parse() {
        return Some(seconds);
    }
    if let Ok(value) = OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
    {
        return Some(value.unix_timestamp());
    }
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(value, &format)
        .ok()
        .map(|value| value.assume_utc().unix_timestamp())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::{duration, optional_timestamp, relative, timestamp_at};

    #[test]
    fn relative_handles_past_and_future() {
        assert_eq!(relative(1_000, 900), "in 1m");
        assert_eq!(relative(800, 900), "1m ago");
    }

    #[test]
    fn duration_is_zero_padded() {
        assert_eq!(duration(3_661), "1h 01m 01s");
    }

    #[test]
    fn timestamp_formats_past_and_future_relative_values() {
        assert!(timestamp_at(1_700_000_000, 1_700_000_000).contains("in 0s"));
        assert!(timestamp_at(1_699_999_940, 1_700_000_000).ends_with("(1m ago)"));
        assert!(timestamp_at(1_700_000_060, 1_700_000_000).ends_with("(in 1m)"));
    }

    #[test]
    fn missing_timestamp_is_operator_friendly() {
        assert_eq!(optional_timestamp(None), "-");
        assert_eq!(optional_timestamp(Some("")), "-");
    }
}
