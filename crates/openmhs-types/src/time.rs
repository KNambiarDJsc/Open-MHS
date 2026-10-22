//! Timestamps and clocks.
//!
//! Internally a timestamp is milliseconds since the Unix epoch (UTC). On the wire it is
//! an RFC 3339 string.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    pub const fn millis(self) -> i64 {
        self.0
    }

    pub fn plus_ms(self, ms: u64) -> Self {
        Self(self.0.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX)))
    }

    pub fn minus_ms(self, ms: u64) -> Self {
        Self(self.0.saturating_sub(i64::try_from(ms).unwrap_or(i64::MAX)))
    }

    /// Milliseconds from `self` until `later`; negative if `later` is earlier.
    pub fn ms_until(self, later: Timestamp) -> i64 {
        later.0.saturating_sub(self.0)
    }

    pub fn to_rfc3339(self) -> String {
        let nanos = i128::from(self.0) * 1_000_000;
        OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .ok()
            .and_then(|t| t.format(&Rfc3339).ok())
            .unwrap_or_else(|| format!("invalid-timestamp-{}", self.0))
    }

    pub fn parse_rfc3339(s: &str) -> Result<Self, time::error::Parse> {
        let t = OffsetDateTime::parse(s, &Rfc3339)?;
        let ms = t.unix_timestamp_nanos() / 1_000_000;
        Ok(Self(i64::try_from(ms).unwrap_or(i64::MAX)))
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Timestamp::parse_rfc3339(&s).map_err(serde::de::Error::custom)
    }
}

/// Source of "now". The engine and simulators share one clock so tests can control time.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// Wall-clock time.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let ms = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
        Timestamp(i64::try_from(ms).unwrap_or(i64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trip() {
        let t = Timestamp::from_millis(1_792_750_500_123);
        let s = t.to_rfc3339();
        assert!(s.ends_with('Z'), "{s}");
        assert_eq!(Timestamp::parse_rfc3339(&s).unwrap(), t);
    }

    #[test]
    fn serde_uses_strings() {
        let t = Timestamp::from_millis(0);
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            "\"1970-01-01T00:00:00Z\""
        );
    }
}
