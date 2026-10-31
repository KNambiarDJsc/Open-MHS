//! A file journal with commands in known states, for the segment, seal and compaction tests.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use openmhs_engine::journal::{Inserted, Journal};
use openmhs_types::{CommandRequest, CommandState, Receipt, Timestamp};
use serde_json::json;

pub const AGENT: &str = "agent.test";

pub fn journal_path(dir: &Path) -> PathBuf {
    dir.join("journal.sqlite")
}

pub fn open(dir: &Path) -> Journal {
    Journal::open(&journal_path(dir)).unwrap()
}

/// A new command in `CREATED`, optionally under an idempotency key.
pub fn command(j: &Journal, id: &str, key: Option<&str>, at: i64) -> Receipt {
    let mut request = json!({
        "target": { "device": "sim.psu-01", "capability": "power.output.voltage.setpoint" },
        "operation": "write",
        "value": { "value": 1.0, "unit": "V" },
    });
    if let Some(key) = key {
        request["idempotency_key"] = key.into();
    }
    let request: CommandRequest = serde_json::from_value(request).unwrap();
    let receipt = Receipt::created(id.into(), AGENT.into(), request, Timestamp::from_millis(at));
    match j
        .insert_command(&receipt, &format!("hash-{id}"), &json!({}))
        .unwrap()
    {
        Inserted::New(r) => r,
        Inserted::Existing { .. } => panic!("{id} already exists"),
    }
}

pub fn step(j: &Journal, id: &str, to: CommandState, at: i64) {
    j.transition(id, to, Timestamp::from_millis(at), None, |_| {})
        .unwrap();
}

/// `CREATED -> VALIDATING -> DENIED`: a finished command.
pub fn finished(j: &Journal, id: &str, key: Option<&str>, at: i64) {
    command(j, id, key, at);
    step(j, id, CommandState::Validating, at + 1);
    step(j, id, CommandState::Denied, at + 2);
}

/// `CREATED -> VALIDATING -> AWAITING_APPROVAL`: unfinished, so it must stay live.
pub fn held(j: &Journal, id: &str, key: Option<&str>, at: i64) {
    command(j, id, key, at);
    step(j, id, CommandState::Validating, at + 1);
    step(j, id, CommandState::AwaitingApproval, at + 2);
}

/// `n` routine audit records (reads), which nothing else depends on.
pub fn noise(j: &Journal, n: i64, at: i64) {
    for i in 0..n {
        j.append_audit(
            Timestamp::from_millis(at + i),
            "read",
            "sim.psu-01",
            &json!({ "principal": AGENT, "capability": "x", "i": i }),
        )
        .unwrap();
    }
}
