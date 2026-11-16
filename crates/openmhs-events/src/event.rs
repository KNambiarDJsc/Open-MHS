//! The event envelope (RFC 0005 §1-§2) and artifact references (§4).

use serde_json::Value;

use crate::canonical::{canonical_json, sha256_digest};
use crate::error::RecordError;
use crate::time_fmt::parse_rfc3339_ns;
use crate::{MAX_EVENT_BYTES, MAX_EXT_BYTES, check_major};

pub const PROVENANCE: &[&str] = &["observed", "reported", "imported", "operator", "generated"];
pub const REPORTED: &[&str] = &[
    "succeeded",
    "failed",
    "aborted",
    "canceled",
    "timeout",
    "unknown",
];
pub const VERIFIED: &[&str] = &["satisfied", "violated", "unknown"];
pub const SEVERITY: &[&str] = &["info", "warning", "error", "critical"];
pub const RELATIONS: &[&str] = &[
    "part_of",
    "follows",
    "retry_of",
    "responds_to",
    "derived_from",
    "same_as",
];

/// An artifact referenced as evidence: which one, and which part of it.
#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceRef {
    pub artifact: String,
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: String,
    pub event_id: String,
}

/// A validated event: the fields correlation reads, and the original content in canonical form.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub event_id: String,
    pub kind: String,
    pub source_id: String,
    pub source_kind: String,
    pub boot_id: Option<String>,
    pub seq: Option<i64>,
    pub observed_ns: Option<i64>,
    pub clock_domain: String,
    pub uncertainty_ms: Option<f64>,
    pub tenant: String,
    pub site: Option<String>,
    pub robot: String,
    pub task_id: Option<String>,
    pub task_type: Option<String>,
    /// What is being attempted under which conditions; attempts that share it are replicates, not independent trials.
    pub scenario_id: Option<String>,
    /// Wall-clock seconds the executing system spent on this task (`cost.wall_s`). Distinct from the event's own
    /// time, which may be simulated, replayed or imported.
    pub wall_s: Option<f64>,
    pub episode_id: Option<String>,
    pub action_id: Option<String>,
    pub goal_id: Option<String>,
    pub trace_id: Option<String>,
    pub manifest: Option<String>,
    pub reported: Option<String>,
    pub verified: Option<String>,
    pub annotated: Option<String>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub code: Option<String>,
    pub component: Option<String>,
    pub provenance: String,
    pub evidence: Vec<EvidenceRef>,
    pub relations: Vec<Relation>,
    /// Canonical JSON of the whole record as received.
    pub body: String,
    /// `sha256:` digest of `body`.
    pub content_hash: String,
}

fn at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    if cur.is_null() { None } else { Some(cur) }
}

fn text(v: &Value, path: &[&str]) -> Result<Option<String>, RecordError> {
    match at(v, path) {
        None => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Err(RecordError::new(
            "event.field",
            format!("`{}` is empty; leave it out instead", path.join(".")),
        )),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(RecordError::new(
            "event.field",
            format!("`{}` must be a string", path.join(".")),
        )),
    }
}

fn required(v: &Value, path: &[&str]) -> Result<String, RecordError> {
    text(v, path)?
        .ok_or_else(|| RecordError::new("event.field", format!("`{}` is required", path.join("."))))
}

fn one_of(
    value: Option<String>,
    allowed: &[&str],
    field: &str,
) -> Result<Option<String>, RecordError> {
    match value {
        Some(v) if !allowed.contains(&v.as_str()) => Err(RecordError::new(
            "event.value",
            format!("`{field}` is {v:?}; expected one of {}", allowed.join(", ")),
        )),
        other => Ok(other),
    }
}

fn outcome(v: &Value, kind: &str, allowed: Option<&[&str]>) -> Result<Option<String>, RecordError> {
    let path = ["outcome", kind, "value"];
    let value = text(v, &path)?;
    if at(v, &["outcome", kind]).is_some() && value.is_none() {
        return Err(RecordError::new(
            "event.field",
            format!("`outcome.{kind}` needs a `value`"),
        ));
    }
    match allowed {
        Some(allowed) => one_of(value, allowed, &path.join(".")),
        None => Ok(value),
    }
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
        && !s.starts_with('.')
        && !s.ends_with('.')
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|h| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn time_ns(v: &Value, path: &[&str]) -> Result<Option<i64>, RecordError> {
    match text(v, path)? {
        None => Ok(None),
        Some(s) => parse_rfc3339_ns(&s).map(Some).ok_or_else(|| {
            RecordError::new(
                "event.time",
                format!("`{}` is not RFC 3339: {s:?}", path.join(".")),
            )
        }),
    }
}

impl Event {
    pub fn parse(v: &Value) -> Result<Self, RecordError> {
        if !v.is_object() {
            return Err(RecordError::new("event.shape", "an event is a JSON object"));
        }
        let schema = required(v, &["schema"])?;
        check_major(&schema, "openmhs.event/")?;
        let body = canonical_json(v);
        if body.len() > MAX_EVENT_BYTES {
            return Err(RecordError::new(
                "event.too_large",
                format!(
                    "{} bytes; the limit is {MAX_EVENT_BYTES}: put large content in an artifact",
                    body.len()
                ),
            ));
        }
        if let Some(ext) = at(v, &["ext"]) {
            if !ext.is_object() {
                return Err(RecordError::new("event.field", "`ext` must be an object"));
            }
            let n = canonical_json(ext).len();
            if n > MAX_EXT_BYTES {
                return Err(RecordError::new(
                    "event.too_large",
                    format!("`ext` is {n} bytes; the limit is {MAX_EXT_BYTES}"),
                ));
            }
        }

        let event_id = required(v, &["event_id"])?;
        if event_id.len() > 128 {
            return Err(RecordError::new(
                "event.field",
                "`event_id` is longer than 128 characters",
            ));
        }
        let kind = required(v, &["type"])?;
        if !is_identifier(&kind) {
            return Err(RecordError::new(
                "event.type",
                format!("`type` {kind:?} must be lowercase words joined by dots"),
            ));
        }
        let provenance = one_of(
            Some(required(v, &["provenance"])?),
            PROVENANCE,
            "provenance",
        )?
        .unwrap_or_default();

        let observed_ns = time_ns(v, &["time", "observed"])?;
        if observed_ns.is_none() && provenance != "imported" {
            return Err(RecordError::new(
                "event.time",
                "`time.observed` is required, except for imported events that cannot know it",
            ));
        }
        let clock_domain = required(v, &["time", "clock_domain"])?;
        let uncertainty_ms = match at(v, &["time", "uncertainty_ms"]) {
            None => None,
            Some(u) => match u.as_f64() {
                Some(x) if x.is_finite() && x >= 0.0 => Some(x),
                _ => {
                    return Err(RecordError::new(
                        "event.time",
                        "`time.uncertainty_ms` must be a non-negative number",
                    ));
                }
            },
        };
        if at(v, &["time", "ingested"]).is_some() {
            return Err(RecordError::new(
                "event.time",
                "`time.ingested` is set by the store, never by a producer",
            ));
        }
        let seq = match at(v, &["source", "seq"]) {
            None => None,
            Some(s) => Some(s.as_i64().filter(|n| *n >= 0).ok_or_else(|| {
                RecordError::new("event.field", "`source.seq` must be a non-negative integer")
            })?),
        };

        let manifest = text(v, &["deployment", "manifest"])?;
        if let Some(m) = &manifest
            && !is_digest(m)
        {
            return Err(RecordError::new(
                "event.field",
                format!("`deployment.manifest` must be sha256:<64 lowercase hex>, not {m:?}"),
            ));
        }

        let wall_s = match at(v, &["cost", "wall_s"]) {
            None => None,
            Some(w) => match w.as_f64() {
                Some(x) if x.is_finite() && x >= 0.0 => Some(x),
                _ => {
                    return Err(RecordError::new(
                        "event.field",
                        "`cost.wall_s` must be a non-negative number of seconds",
                    ));
                }
            },
        };

        let mut evidence = Vec::new();
        if let Some(items) = at(v, &["evidence"]) {
            let items = items
                .as_array()
                .ok_or_else(|| RecordError::new("event.field", "`evidence` must be an array"))?;
            for item in items {
                let artifact = required(item, &["artifact"])?;
                let (start_ns, end_ns) = match at(item, &["range"]) {
                    None => (None, None),
                    Some(r) => {
                        let pair = r.as_array().filter(|a| a.len() == 2).ok_or_else(|| {
                            RecordError::new(
                                "event.field",
                                "`evidence[].range` must be [start, end]",
                            )
                        })?;
                        let parse = |x: &Value| x.as_str().and_then(parse_rfc3339_ns);
                        match (parse(&pair[0]), parse(&pair[1])) {
                            (Some(s), Some(e)) if s <= e => (Some(s), Some(e)),
                            _ => {
                                return Err(RecordError::new(
                                    "event.field",
                                    "`evidence[].range` must hold two RFC 3339 times, start first",
                                ));
                            }
                        }
                    }
                };
                evidence.push(EvidenceRef {
                    artifact,
                    start_ns,
                    end_ns,
                });
            }
        }

        let mut relations = Vec::new();
        if let Some(items) = at(v, &["relations"]) {
            let items = items
                .as_array()
                .ok_or_else(|| RecordError::new("event.field", "`relations` must be an array"))?;
            for item in items {
                let kind = one_of(
                    Some(required(item, &["type"])?),
                    RELATIONS,
                    "relations[].type",
                )?
                .unwrap_or_default();
                relations.push(Relation {
                    kind,
                    event_id: required(item, &["event_id"])?,
                });
            }
        }

        Ok(Self {
            event_id,
            kind,
            source_id: required(v, &["source", "id"])?,
            source_kind: required(v, &["source", "kind"])?,
            boot_id: text(v, &["source", "boot_id"])?,
            seq,
            observed_ns,
            clock_domain,
            uncertainty_ms,
            tenant: required(v, &["scope", "tenant"])?,
            site: text(v, &["scope", "site"])?,
            robot: required(v, &["scope", "robot"])?,
            task_id: text(v, &["subject", "task_id"])?,
            task_type: text(v, &["subject", "task_type"])?,
            scenario_id: text(v, &["subject", "scenario_id"])?,
            wall_s,
            episode_id: text(v, &["subject", "episode_id"])?,
            action_id: text(v, &["subject", "action_id"])?,
            goal_id: text(v, &["subject", "goal_id"])?,
            trace_id: text(v, &["trace", "trace_id"])?,
            manifest,
            reported: outcome(v, "reported", Some(REPORTED))?,
            verified: outcome(v, "verified", Some(VERIFIED))?,
            annotated: outcome(v, "annotated", None)?,
            severity: one_of(
                text(v, &["attributes", "severity"])?,
                SEVERITY,
                "attributes.severity",
            )?,
            category: text(v, &["attributes", "category"])?,
            code: text(v, &["attributes", "code"])?,
            component: text(v, &["attributes", "component"])?,
            provenance,
            evidence,
            relations,
            content_hash: sha256_digest(&body),
            body,
        })
    }
}

/// A reference to a recording, video or dataset slice. Open-MHS never fetches it (RFC 0005 §4).
#[derive(Debug, Clone, PartialEq)]
pub struct Artifact {
    pub tenant: String,
    pub artifact_id: String,
    pub uri: String,
    pub media_type: Option<String>,
    pub bytes: Option<i64>,
    pub start_ns: Option<i64>,
    pub end_ns: Option<i64>,
    pub robot: Option<String>,
    pub body: String,
    pub content_hash: String,
}

impl Artifact {
    pub fn parse(v: &Value) -> Result<Self, RecordError> {
        let schema = required(v, &["schema"])?;
        check_major(&schema, "openmhs.artifact/")?;
        let (start_ns, end_ns) = match at(v, &["time_range"]) {
            None => (None, None),
            Some(r) => {
                let pair = r.as_array().filter(|a| a.len() == 2).ok_or_else(|| {
                    RecordError::new("artifact.field", "`time_range` must be [start, end]")
                })?;
                let parse = |x: &Value| x.as_str().and_then(parse_rfc3339_ns);
                match (parse(&pair[0]), parse(&pair[1])) {
                    (Some(s), Some(e)) if s <= e => (Some(s), Some(e)),
                    _ => {
                        return Err(RecordError::new(
                            "artifact.field",
                            "`time_range` must be two RFC 3339 times, start first",
                        ));
                    }
                }
            }
        };
        let body = canonical_json(v);
        Ok(Self {
            tenant: required(v, &["tenant"])?,
            artifact_id: required(v, &["artifact_id"])?,
            uri: required(v, &["uri"])?,
            media_type: text(v, &["media_type"])?,
            bytes: at(v, &["bytes"]).and_then(Value::as_i64),
            start_ns,
            end_ns,
            robot: text(v, &["robot"])?,
            content_hash: sha256_digest(&body),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type Change = Box<dyn Fn(&mut Value)>;

    fn minimal() -> Value {
        json!({
            "schema": "openmhs.event/0.1", "event_id": "e-1", "type": "task.started",
            "source": {"id": "app", "kind": "sdk"},
            "time": {"observed": "2026-11-14T09:31:07.412Z", "clock_domain": "robot-12/chrony"},
            "scope": {"tenant": "acme", "robot": "robot-12"},
            "subject": {"task_id": "pick-918", "task_type": "pick"},
            "provenance": "observed"
        })
    }

    #[test]
    fn a_minimal_event_parses_and_keeps_its_content() {
        let mut v = minimal();
        v["ext"] = json!({"vendor": {"anything": [1, 2, 3]}});
        v["future_field"] = json!("kept");
        let e = Event::parse(&v).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(e.task_id.as_deref(), Some("pick-918"));
        assert!(e.body.contains("future_field"));
        assert!(e.body.contains("anything"));
        assert!(e.content_hash.starts_with("sha256:"));
    }

    #[test]
    fn refusals_say_what_is_wrong() {
        let cases: Vec<(&str, Change)> = vec![
            (
                "event.field",
                Box::new(|v: &mut Value| v["scope"]["robot"] = Value::Null),
            ),
            (
                "event.type",
                Box::new(|v: &mut Value| v["type"] = json!("Task Started")),
            ),
            (
                "event.value",
                Box::new(|v: &mut Value| v["provenance"] = json!("rumour")),
            ),
            (
                "event.time",
                Box::new(|v: &mut Value| v["time"]["observed"] = json!("today")),
            ),
            (
                "event.time",
                Box::new(|v: &mut Value| v["time"]["ingested"] = json!("2026-11-14T00:00:00Z")),
            ),
            (
                "event.time",
                Box::new(|v: &mut Value| v["time"]["uncertainty_ms"] = json!(-1)),
            ),
            (
                "event.value",
                Box::new(|v: &mut Value| v["outcome"] = json!({"reported": {"value": "great"}})),
            ),
            (
                "event.field",
                Box::new(|v: &mut Value| v["outcome"] = json!({"verified": {"contract": "x"}})),
            ),
            (
                "event.field",
                Box::new(|v: &mut Value| v["deployment"] = json!({"manifest": "v47"})),
            ),
            (
                "record.schema_version",
                Box::new(|v: &mut Value| v["schema"] = json!("openmhs.event/1.0")),
            ),
            (
                "event.field",
                Box::new(|v: &mut Value| v["cost"] = json!({"wall_s": -3})),
            ),
            (
                "event.field",
                Box::new(|v: &mut Value| v["cost"] = json!({"wall_s": "fast"})),
            ),
            (
                "event.too_large",
                Box::new(|v: &mut Value| v["ext"] = json!({"x": "a".repeat(40_000)})),
            ),
        ];
        for (code, change) in cases {
            let mut v = minimal();
            change(&mut v);
            match Event::parse(&v) {
                Err(e) => assert_eq!(e.code, code, "{e}"),
                Ok(_) => panic!("accepted {v}"),
            }
        }
    }

    #[test]
    fn the_wall_clock_cost_of_a_task_is_read() {
        let mut v = minimal();
        assert_eq!(Event::parse(&v).unwrap().wall_s, None);
        v["cost"] = json!({"wall_s": 812.5});
        assert_eq!(Event::parse(&v).unwrap().wall_s, Some(812.5));
        v["cost"] = json!({"wall_s": 0});
        assert_eq!(Event::parse(&v).unwrap().wall_s, Some(0.0));
    }

    #[test]
    fn an_imported_event_may_lack_an_observed_time() {
        let mut v = minimal();
        v["time"] = json!({"clock_domain": "unknown"});
        assert!(Event::parse(&v).is_err());
        v["provenance"] = json!("imported");
        assert!(Event::parse(&v).is_ok());
    }

    #[test]
    fn outcomes_and_evidence_are_read() {
        let mut v = minimal();
        v["type"] = json!("task.ended");
        v["outcome"] = json!({"reported": {"value": "succeeded", "source": "ros2.action_status"},
                               "verified": {"value": "violated", "contract": "lift-at-target"}});
        v["evidence"] = json!([{"artifact": "sha256:ab", "range": ["2026-11-14T09:30:40Z", "2026-11-14T09:31:10Z"]}]);
        let e = Event::parse(&v).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(e.reported.as_deref(), Some("succeeded"));
        assert_eq!(e.verified.as_deref(), Some("violated"));
        assert_eq!(e.evidence.len(), 1);
        assert!(e.evidence[0].start_ns < e.evidence[0].end_ns);
    }
}
