//! Routine telemetry (RFC 0002, milestone 6; strategy RFC §3 to §5).
//!
//! Allowed, successful reads of the devices and capabilities an operator has opted in go to a
//! store of their own, `telemetry.sqlite`, instead of one audit record each. Its quota can
//! fill and roll over without touching the journal, and its write rate cannot slow command
//! journaling. The audit chain still says who read what, when and how often: one
//! `read.summary` record per (principal, device, capability) per window, with the count,
//! first and last reading, minimum and maximum of a quantity, and how many reads could not be
//! kept raw. Loss is explicit: a read the store could not keep is counted, never dropped
//! silently.
//!
//! Nothing here touches `Critical` or `Security` records. A read that is denied, a read that
//! failed, and every read of a device nobody opted in stay individual records in the chain.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use openmhs_policy::glob_matches;
use openmhs_types::{ErrorObject, StateValue};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value as Json, json};

use crate::journal::JournalError;

/// Room kept above the hard limit for the counters that record what was dropped.
const COUNTER_RESERVE_BYTES: u64 = 1 << 20;
/// Rows removed per step when the store is over its soft limit.
const EVICT_BATCH: i64 = 500;

pub const DEFAULT_INTERVAL_MS: u64 = 60_000;
pub const DEFAULT_SOFT_BYTES: u64 = 64 << 20;
pub const DEFAULT_HARD_BYTES: u64 = 128 << 20;
/// Raw telemetry is kept this long once summarized (strategy RFC §10: 7 days).
pub const DEFAULT_RETENTION_MS: u64 = 7 * 24 * 3_600_000;

/// One opt-in: reads of devices and capabilities matching both patterns are telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryRule {
    pub device: String,
    pub capability: String,
}

#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub rules: Vec<TelemetryRule>,
    /// Length of a summary window.
    pub interval_ms: u64,
    /// Over this many bytes the oldest summarized raw readings are removed.
    pub soft_bytes: u64,
    /// Over this many bytes no raw reading is stored; summaries go on and count what was
    /// dropped.
    pub hard_bytes: u64,
    /// Summarized raw readings older than this are removed.
    pub retention_ms: u64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            interval_ms: DEFAULT_INTERVAL_MS,
            soft_bytes: DEFAULT_SOFT_BYTES,
            hard_bytes: DEFAULT_HARD_BYTES,
            retention_ms: DEFAULT_RETENTION_MS,
        }
    }
}

impl TelemetryConfig {
    pub fn matches(&self, device: &str, capability: &str) -> bool {
        self.rules
            .iter()
            .any(|r| glob_matches(&r.device, device) && glob_matches(&r.capability, capability))
    }

    pub fn validate(&self) -> Result<(), ErrorObject> {
        let bad = |message: &str| Err(ErrorObject::new("telemetry.config", message));
        if self.rules.is_empty() {
            return bad("no device and capability is opted in to telemetry");
        }
        if self.interval_ms == 0 {
            return bad("the summary interval must be more than zero");
        }
        if self.soft_bytes >= self.hard_bytes {
            return bad("the soft limit must be smaller than the hard limit");
        }
        Ok(())
    }
}

/// What happened to a reading handed to the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Stored,
    /// The store is at its hard limit: the read was counted for its summary, not kept.
    Dropped,
}

/// One `read.summary` record, ready for the audit chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The record's subject.
    pub device: String,
    pub data: Json,
    /// Reads the window saw: kept and dropped.
    pub reads: u64,
    pub dropped: u64,
}

type DropKey = (i64, String, String, String);

/// What a flush would write, and what it would then mark done.
#[derive(Debug)]
pub struct FlushPlan {
    pub summaries: Vec<Summary>,
    through_id: i64,
    dropped: Vec<(DropKey, i64)>,
}

impl FlushPlan {
    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryStatus {
    pub store_id: String,
    pub rows: u64,
    pub used_bytes: u64,
    /// Readings up to this id are summarized in the chain.
    pub summarized_through: i64,
    /// Reads dropped at the hard limit whose summary has not been written yet.
    pub dropped_pending: u64,
}

pub struct TelemetryStore {
    conn: Mutex<Connection>,
    store_id: String,
    soft_bytes: u64,
    hard_bytes: u64,
}

fn used_bytes(conn: &Connection) -> Result<u64, JournalError> {
    let page = |pragma: &str| -> Result<i64, JournalError> {
        Ok(conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))?)
    };
    let used = (page("page_count")? - page("freelist_count")?).max(0) * page("page_size")?;
    Ok(u64::try_from(used).unwrap_or(0))
}

/// The numeric value of a reading, and its unit if it has one.
fn number_of(state: &Json) -> Option<(f64, Option<String>)> {
    let value = state.get("value")?;
    if let Some(n) = value.as_f64() {
        return Some((n, None));
    }
    let n = value.get("value")?.as_f64()?;
    Some((
        n,
        value.get("unit").and_then(Json::as_str).map(str::to_owned),
    ))
}

#[derive(Default)]
struct Window {
    kept: u64,
    dropped: i64,
    first: Option<(i64, i64, Json)>,
    last: Option<(i64, i64, Json)>,
    min: Option<f64>,
    max: Option<f64>,
    unit: Option<Option<String>>,
    mixed_units: bool,
    quality: [u64; 3],
    from_id: i64,
    to_id: i64,
}

impl Window {
    fn add(&mut self, id: i64, at_ms: i64, quality: &str, state: Json) {
        self.kept += 1;
        if self.from_id == 0 {
            self.from_id = id;
        }
        self.to_id = id;
        match quality {
            "GOOD" => self.quality[0] += 1,
            "UNCERTAIN" => self.quality[1] += 1,
            _ => self.quality[2] += 1,
        }
        if let Some((n, unit)) = number_of(&state) {
            match &self.unit {
                None => self.unit = Some(unit),
                Some(seen) if *seen != unit => self.mixed_units = true,
                Some(_) => {}
            }
            self.min = Some(self.min.map_or(n, |m| m.min(n)));
            self.max = Some(self.max.map_or(n, |m| m.max(n)));
        }
        if self
            .first
            .as_ref()
            .is_none_or(|(at, i, _)| (at_ms, id) < (*at, *i))
        {
            self.first = Some((at_ms, id, state.clone()));
        }
        if self
            .last
            .as_ref()
            .is_none_or(|(at, i, _)| (at_ms, id) > (*at, *i))
        {
            self.last = Some((at_ms, id, state));
        }
    }

    fn reading(entry: &Option<(i64, i64, Json)>) -> Json {
        entry.as_ref().map_or(
            Json::Null,
            |(at, _, state)| json!({ "at_ms": at, "state": state }),
        )
    }

    fn extreme(&self, n: Option<f64>) -> Json {
        match (n, &self.unit, self.mixed_units) {
            (Some(n), Some(Some(unit)), false) => json!({ "value": n, "unit": unit }),
            (Some(n), Some(None), false) => json!({ "value": n }),
            _ => Json::Null,
        }
    }
}

impl TelemetryStore {
    pub fn open(path: &Path, config: &TelemetryConfig) -> Result<Self, JournalError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Raw telemetry may lose its last moments to a power cut; the journal does not.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                 one         INTEGER PRIMARY KEY CHECK (one = 1),
                 store_id    TEXT NOT NULL,
                 watermark   INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS reads (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 at_ms      INTEGER NOT NULL,
                 principal  TEXT NOT NULL,
                 device     TEXT NOT NULL,
                 capability TEXT NOT NULL,
                 quality    TEXT NOT NULL,
                 state      TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS reads_at ON reads(at_ms);
             CREATE TABLE IF NOT EXISTS dropped (
                 bucket_ms  INTEGER NOT NULL,
                 principal  TEXT NOT NULL,
                 device     TEXT NOT NULL,
                 capability TEXT NOT NULL,
                 count      INTEGER NOT NULL,
                 PRIMARY KEY (bucket_ms, principal, device, capability)
             );",
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO meta (one, store_id, watermark) VALUES (1, ?1, 0)",
            [uuid::Uuid::now_v7().to_string()],
        )?;
        let store_id: String = conn.query_row("SELECT store_id FROM meta", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let cap = (config.hard_bytes + COUNTER_RESERVE_BYTES)
            .div_ceil(u64::try_from(page_size).unwrap_or(4096));
        conn.pragma_update_and_check(
            None,
            "max_page_count",
            i64::try_from(cap).unwrap_or(i64::MAX),
            |r| r.get::<_, i64>(0),
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            store_id,
            soft_bytes: config.soft_bytes,
            hard_bytes: config.hard_bytes,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Names this store in the summaries written for it. A store that is deleted and made
    /// again gets a new id, so old summaries are never mistaken for its readings.
    pub fn store_id(&self) -> &str {
        &self.store_id
    }

    fn count_dropped(
        conn: &Connection,
        bucket_ms: i64,
        who: (&str, &str, &str),
    ) -> Result<(), JournalError> {
        conn.execute(
            "INSERT INTO dropped (bucket_ms, principal, device, capability, count)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT (bucket_ms, principal, device, capability) DO UPDATE SET count = count + 1",
            params![bucket_ms, who.0, who.1, who.2],
        )?;
        Ok(())
    }

    /// Keep one successful read, or count it if the store is at its hard limit. Over the
    /// soft limit the oldest summarized readings go first; a reading no summary covers yet
    /// is never evicted.
    pub fn record(
        &self,
        at_ms: i64,
        interval_ms: i64,
        principal: &str,
        device: &str,
        capability: &str,
        state: &StateValue,
    ) -> Result<Recorded, JournalError> {
        let conn = self.lock();
        let bucket = at_ms.div_euclid(interval_ms) * interval_ms;
        let mut used = used_bytes(&conn)?;
        if used >= self.soft_bytes {
            while used >= self.soft_bytes {
                let evicted = conn.execute(
                    "DELETE FROM reads WHERE id IN (SELECT id FROM reads WHERE id <= \
                     (SELECT watermark FROM meta) ORDER BY id LIMIT ?1)",
                    [EVICT_BATCH],
                )?;
                if evicted == 0 {
                    break;
                }
                used = used_bytes(&conn)?;
            }
        }
        if used >= self.hard_bytes {
            Self::count_dropped(&conn, bucket, (principal, device, capability))?;
            return Ok(Recorded::Dropped);
        }
        conn.execute(
            "INSERT INTO reads (at_ms, principal, device, capability, quality, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                at_ms,
                principal,
                device,
                capability,
                format!("{:?}", state.quality).to_uppercase(),
                serde_json::to_string(state)?
            ],
        )?;
        Ok(Recorded::Stored)
    }

    /// Work out the summaries of every complete window not yet in the chain. With `force`
    /// (at shutdown) the window still open is included. Writes nothing.
    pub fn plan(
        &self,
        now_ms: i64,
        interval_ms: i64,
        force: bool,
    ) -> Result<FlushPlan, JournalError> {
        let conn = self.lock();
        let watermark: i64 = conn.query_row("SELECT watermark FROM meta", [], |r| r.get(0))?;
        let cutoff = if force {
            i64::MAX
        } else {
            now_ms.div_euclid(interval_ms) * interval_ms
        };
        // Readings go in id order up to the first one of a window still open, so that
        // everything at or below the new watermark is summarized.
        let first_open: Option<i64> = conn.query_row(
            "SELECT MIN(id) FROM reads WHERE id > ?1 AND at_ms >= ?2",
            params![watermark, cutoff],
            |r| r.get(0),
        )?;
        let mut windows: BTreeMap<DropKey, Window> = BTreeMap::new();
        let mut through_id = watermark;
        {
            let mut stmt = conn.prepare(
                "SELECT id, at_ms, principal, device, capability, quality, state FROM reads \
                 WHERE id > ?1 AND id < ?2 ORDER BY id",
            )?;
            let mut rows = stmt.query(params![watermark, first_open.unwrap_or(i64::MAX)])?;
            while let Some(row) = rows.next()? {
                let (id, at_ms): (i64, i64) = (row.get(0)?, row.get(1)?);
                let key = (
                    at_ms.div_euclid(interval_ms) * interval_ms,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                );
                let state: Json = serde_json::from_str(&row.get::<_, String>(6)?)?;
                windows
                    .entry(key)
                    .or_default()
                    .add(id, at_ms, &row.get::<_, String>(5)?, state);
                through_id = id;
            }
        }
        let mut dropped = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT bucket_ms, principal, device, capability, count FROM dropped \
                 WHERE bucket_ms < ?1",
            )?;
            let mut rows = stmt.query([cutoff])?;
            while let Some(row) = rows.next()? {
                let key = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
                let count: i64 = row.get(4)?;
                windows.entry(key.clone()).or_default().dropped += count;
                dropped.push((key, count));
            }
        }
        let summaries = windows
            .into_iter()
            .map(|((bucket, principal, device, capability), w)| {
                let reads = w.kept + u64::try_from(w.dropped).unwrap_or(0);
                let dropped_reads = u64::try_from(w.dropped).unwrap_or(0);
                let data = json!({
                    "principal": principal,
                    "capability": capability,
                    "window_start_ms": bucket,
                    "window_end_ms": bucket + interval_ms,
                    "reads": reads,
                    "stored": w.kept,
                    "dropped": dropped_reads,
                    "first": Window::reading(&w.first),
                    "last": Window::reading(&w.last),
                    "min": w.extreme(w.min),
                    "max": w.extreme(w.max),
                    "quality": { "good": w.quality[0], "uncertain": w.quality[1], "bad": w.quality[2] },
                    "store": {
                        "id": self.store_id,
                        "from_id": (w.kept > 0).then_some(w.from_id),
                        "to_id": (w.kept > 0).then_some(w.to_id),
                        "through_id": through_id,
                    },
                });
                Summary {
                    device,
                    data,
                    reads,
                    dropped: dropped_reads,
                }
            })
            .collect();
        Ok(FlushPlan {
            summaries,
            through_id,
            dropped,
        })
    }

    /// Mark a plan's summaries as written: advance the watermark, take their drop counts
    /// off the counters (a read dropped since the plan stays counted), and remove summarized
    /// readings older than `retention_ms`. Returns how many were removed.
    pub fn commit(
        &self,
        plan: &FlushPlan,
        now_ms: i64,
        retention_ms: i64,
    ) -> Result<u64, JournalError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE meta SET watermark = MAX(watermark, ?1)",
            [plan.through_id],
        )?;
        for ((bucket, principal, device, capability), count) in &plan.dropped {
            tx.execute(
                "UPDATE dropped SET count = count - ?5 WHERE bucket_ms = ?1 AND principal = ?2 \
                 AND device = ?3 AND capability = ?4",
                params![bucket, principal, device, capability, count],
            )?;
        }
        tx.execute("DELETE FROM dropped WHERE count <= 0", [])?;
        let pruned = tx.execute(
            "DELETE FROM reads WHERE id <= (SELECT watermark FROM meta) AND at_ms < ?1",
            [now_ms.saturating_sub(retention_ms)],
        )?;
        tx.commit()?;
        Ok(pruned as u64)
    }

    /// After a crash between the chain's commit and [`commit`](Self::commit): the chain's
    /// newest summary of this store says how far it got.
    pub fn reconcile(&self, through_id: i64) -> Result<(), JournalError> {
        self.lock().execute(
            "UPDATE meta SET watermark = MAX(watermark, ?1)",
            [through_id],
        )?;
        Ok(())
    }

    pub fn status(&self) -> Result<TelemetryStatus, JournalError> {
        let conn = self.lock();
        let (rows, watermark): (i64, i64) = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM reads), watermark FROM meta",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let pending: Option<i64> = conn
            .query_row("SELECT SUM(count) FROM dropped", [], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(TelemetryStatus {
            store_id: self.store_id.clone(),
            rows: u64::try_from(rows).unwrap_or(0),
            used_bytes: used_bytes(&conn)?,
            summarized_through: watermark,
            dropped_pending: u64::try_from(pending.unwrap_or(0)).unwrap_or(0),
        })
    }
}
