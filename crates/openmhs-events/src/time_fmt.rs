use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// RFC 3339 text to nanoseconds since the Unix epoch.
pub fn parse_rfc3339_ns(text: &str) -> Option<i64> {
    let t = OffsetDateTime::parse(text, &Rfc3339).ok()?;
    i64::try_from(t.unix_timestamp_nanos()).ok()
}

/// Nanoseconds since the Unix epoch to RFC 3339 UTC with millisecond precision.
pub fn format_ns(ns: i64) -> String {
    match OffsetDateTime::from_unix_timestamp_nanos(i128::from(ns)) {
        Ok(t) => {
            let ms = t.millisecond();
            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{ms:03}Z",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        }
        Err(_) => ns.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_at_millisecond_precision() {
        let ns = parse_rfc3339_ns("2026-11-14T09:31:07.412381Z");
        assert_eq!(ns, Some(1_794_648_667_412_381_000));
        assert_eq!(
            format_ns(1_794_648_667_412_381_000),
            "2026-11-14T09:31:07.412Z"
        );
        assert_eq!(
            parse_rfc3339_ns("1970-01-01T00:00:10.025Z"),
            Some(10_025_000_000)
        );
        assert_eq!(parse_rfc3339_ns("yesterday"), None);
    }
}
