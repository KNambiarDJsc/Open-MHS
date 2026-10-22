//! Observed state values (`common.schema.json#/$defs/state_value`).

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;
use crate::value::Value;

/// Readings timestamped further than this into the future are not trusted as fresh.
pub const MAX_FUTURE_SKEW_MS: i64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Device,
    Adapter,
    Gateway,
    Simulator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Quality {
    Good,
    Uncertain,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateValue {
    pub value: Value,
    pub timestamp: Timestamp,
    pub source: Source,
    pub quality: Quality,
}

impl StateValue {
    /// Age of the reading at `now` in milliseconds (negative if timestamped in the future).
    pub fn age_ms(&self, now: Timestamp) -> i64 {
        self.timestamp.ms_until(now)
    }

    /// `GOOD` quality and, if `max_age_ms` is given, no older than that. Readings from
    /// more than [`MAX_FUTURE_SKEW_MS`] in the future are never fresh.
    pub fn is_usable(&self, now: Timestamp, max_age_ms: Option<u64>) -> bool {
        let age = self.age_ms(now);
        if self.quality != Quality::Good || age < -MAX_FUTURE_SKEW_MS {
            return false;
        }
        match max_age_ms {
            Some(max) => age <= i64::try_from(max).unwrap_or(i64::MAX),
            None => true,
        }
    }
}
