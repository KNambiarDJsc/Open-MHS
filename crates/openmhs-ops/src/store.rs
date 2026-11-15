//! The ops store: one SQLite file, numbered migrations, idempotent ingestion.

use std::path::Path;

use openmhs_events::{Record, parse_record, sha256_digest};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::Value;

use crate::OpsError;

/// (version, SQL). Never edit a released migration: add a new one. The checksum of each applied migration is kept
/// and checked at open, so an edited migration is noticed instead of silently diverging.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("migrations/0001_init.sql")),
    (2, include_str!("migrations/0002_scenarios.sql")),
];

pub struct Store {
    pub(crate) conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum IngestStatus {
    Accepted,
    Duplicate,
    /// The id exists with different content. The new content is kept beside the old, never instead of it.
    Conflict {
        existing_hash: String,
    },
    Rejected {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IngestResult {
    /// `event`, `manifest`, `artifact`, or `unknown` when the record could not be read at all.
    pub record: &'static str,
    pub id: String,
    #[serde(flatten)]
    pub status: IngestStatus,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, OpsError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, OpsError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, OpsError> {
        // An event store may lose the last transaction on power loss (ADR-0029); the command journal may not.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let ok: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if ok != "ok" {
            return Err(OpsError::Store(format!(
                "the store failed its integrity check: {ok}"
            )));
        }
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), OpsError> {
        let known = MIGRATIONS.iter().map(|m| m.0).max().unwrap_or(0);
        let user_version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if user_version > known {
            return Err(OpsError::Store(format!(
                "the store is at schema version {user_version}, newer than this program ({known}); use a newer openmhs"
            )));
        }
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                 version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_ns INTEGER NOT NULL)",
        )?;
        for (version, sql) in MIGRATIONS {
            // Line endings normalized: a Windows checkout (CRLF) and a Linux one must agree on the checksum.
            let checksum = sha256_digest(&sql.replace('\r', ""));
            let applied: Option<String> = tx
                .query_row(
                    "SELECT checksum FROM schema_migrations WHERE version = ?1",
                    [version],
                    |r| r.get(0),
                )
                .optional()?;
            match applied {
                Some(c) if c == checksum => {}
                Some(c) => {
                    return Err(OpsError::Store(format!(
                        "migration {version} was applied with checksum {c}, but this program has {checksum}: \
                         a released migration was edited"
                    )));
                }
                None => {
                    tx.execute_batch(sql)?;
                    tx.execute(
                        "INSERT INTO schema_migrations (version, checksum, applied_ns) VALUES (?1, ?2, ?3)",
                        params![version, checksum, now_ns()],
                    )?;
                }
            }
        }
        tx.pragma_update(None, "user_version", known)?;
        tx.commit()?;
        Ok(())
    }

    /// Ingest a batch in one transaction. Each record gets its own result; a bad record never fails the batch.
    pub fn ingest(
        &mut self,
        records: &[Value],
        ingested_ns: i64,
    ) -> Result<Vec<IngestResult>, OpsError> {
        let tx = self.conn.transaction()?;
        let mut results = Vec::with_capacity(records.len());
        for value in records {
            results.push(ingest_one(&tx, value, ingested_ns)?);
        }
        tx.commit()?;
        Ok(results)
    }

    /// Counts by table, for `openmhs ops status`.
    pub fn counts(&self) -> Result<Vec<(String, i64)>, OpsError> {
        let mut out = Vec::new();
        for table in [
            "events",
            "event_conflicts",
            "manifests",
            "artifacts",
            "rollouts",
            "task_attempts",
            "incidents",
            "incident_members",
        ] {
            let n: i64 =
                self.conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
            out.push((table.to_owned(), n));
        }
        Ok(out)
    }
}

fn rejected(record: &'static str, id: String, code: &str, message: String) -> IngestResult {
    IngestResult {
        record,
        id,
        status: IngestStatus::Rejected {
            code: code.to_owned(),
            message,
        },
    }
}

fn ingest_one(
    tx: &Transaction<'_>,
    value: &Value,
    ingested_ns: i64,
) -> Result<IngestResult, OpsError> {
    let id_hint = value
        .get("event_id")
        .or_else(|| value.get("artifact_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let record = match parse_record(value) {
        Ok(r) => r,
        Err(e) => {
            let kind = match value.get("schema").and_then(Value::as_str) {
                Some(s) if s.starts_with("openmhs.event/") => "event",
                Some(s) if s.starts_with("openmhs.deployment/") => "manifest",
                Some(s) if s.starts_with("openmhs.artifact/") => "artifact",
                _ => "unknown",
            };
            return Ok(rejected(kind, id_hint, e.code, e.message));
        }
    };
    match record {
        Record::Event(e) => {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT content_hash FROM events WHERE tenant = ?1 AND event_id = ?2",
                    params![e.tenant, e.event_id],
                    |r| r.get(0),
                )
                .optional()?;
            let status = match existing {
                Some(h) if h == e.content_hash => IngestStatus::Duplicate,
                Some(h) => {
                    tx.execute(
                        "INSERT OR IGNORE INTO event_conflicts (tenant, event_id, content_hash, body, ingested_ns)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![e.tenant, e.event_id, e.content_hash, e.body, ingested_ns],
                    )?;
                    IngestStatus::Conflict { existing_hash: h }
                }
                None => {
                    tx.execute(
                        "INSERT INTO events (tenant, event_id, type, source_id, boot_id, seq, observed_ns,
                             clock_domain, uncertainty_ms, ingested_ns, site, robot, task_id, task_type, episode_id,
                             action_id, goal_id, trace_id, manifest, reported, verified, severity, category, code,
                             component, provenance, content_hash, body, scenario_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
                        params![
                            e.tenant, e.event_id, e.kind, e.source_id, e.boot_id, e.seq, e.observed_ns,
                            e.clock_domain, e.uncertainty_ms, ingested_ns, e.site, e.robot, e.task_id, e.task_type,
                            e.episode_id, e.action_id, e.goal_id, e.trace_id, e.manifest, e.reported, e.verified,
                            e.severity, e.category, e.code, e.component, e.provenance, e.content_hash, e.body, e.scenario_id
                        ],
                    )?;
                    IngestStatus::Accepted
                }
            };
            Ok(IngestResult {
                record: "event",
                id: e.event_id,
                status,
            })
        }
        Record::Manifest(m) => {
            let n = tx.execute(
                "INSERT OR IGNORE INTO manifests (digest, release, body, first_seen_ns) VALUES (?1, ?2, ?3, ?4)",
                params![m.digest, m.labels.get("release"), m.body, ingested_ns],
            )?;
            let status = if n == 1 {
                IngestStatus::Accepted
            } else {
                IngestStatus::Duplicate
            };
            Ok(IngestResult {
                record: "manifest",
                id: m.digest,
                status,
            })
        }
        Record::Artifact(a) => {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT content_hash FROM artifacts WHERE tenant = ?1 AND artifact_id = ?2",
                    params![a.tenant, a.artifact_id],
                    |r| r.get(0),
                )
                .optional()?;
            let status = match existing {
                Some(h) if h == a.content_hash => IngestStatus::Duplicate,
                Some(h) => IngestStatus::Conflict { existing_hash: h },
                None => {
                    tx.execute(
                        "INSERT INTO artifacts (tenant, artifact_id, uri, media_type, bytes, start_ns, end_ns, robot,
                             content_hash, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            a.tenant, a.artifact_id, a.uri, a.media_type, a.bytes, a.start_ns, a.end_ns, a.robot,
                            a.content_hash, a.body
                        ],
                    )?;
                    IngestStatus::Accepted
                }
            };
            Ok(IngestResult {
                record: "artifact",
                id: a.artifact_id,
                status,
            })
        }
    }
}

pub(crate) fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
