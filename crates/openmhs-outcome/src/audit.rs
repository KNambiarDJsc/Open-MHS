//! Evidence records from an OpenMHS audit export (`openmhs audit export`).
//!
//! Each audited read becomes a `device_state` record, and each command receipt a `controller` record whose
//! value is the command's state. Nothing is invented: a read that failed has no value and is skipped, and
//! the reason is returned beside the records.
//!
//! Clock domains: a reading from a simulator, an adapter or the gateway is stamped by the gateway's clock,
//! so it shares the domain `openmhs-gateway` with command receipts. A reading the device stamped itself is in
//! `device:<id>`, with its uncertainty unknown, so a contract that compares it with a command must say how far
//! the two clocks can be apart, or it will not be accepted across clocks.

use serde_json::Value;

use crate::{Provenance, Quality, Record, SourceInfo, SourceKind, TimeInfo};

pub const GATEWAY_CLOCK: &str = "openmhs-gateway";

fn millis_to_rfc3339(ms: i64) -> String {
    openmhs_types::Timestamp::from_millis(ms).to_rfc3339()
}

/// Records from an evidence bundle, and a note for every audit entry or receipt that could not become one.
pub fn records_from_bundle(bundle: &Value) -> Result<(Vec<Record>, Vec<String>), String> {
    let audit = bundle
        .get("audit")
        .and_then(Value::as_array)
        .ok_or("not an evidence bundle: no `audit` array")?;
    let receipts = bundle
        .get("receipts")
        .and_then(Value::as_array)
        .ok_or("not an evidence bundle: no `receipts` array")?;
    let mut out = Vec::new();
    let mut skipped = Vec::new();

    for entry in audit {
        if entry.get("kind").and_then(Value::as_str) != Some("read") {
            continue;
        }
        let seq = entry.get("seq").and_then(Value::as_i64).unwrap_or(-1);
        let device = entry
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let at_ms = entry.get("at_ms").and_then(Value::as_i64);
        let data: Value = match entry
            .get("data")
            .and_then(Value::as_str)
            .map(serde_json::from_str)
        {
            Some(Ok(v)) => v,
            _ => {
                skipped.push(format!("audit {seq}: data is not JSON"));
                continue;
            }
        };
        let Some(capability) = data.get("capability").and_then(Value::as_str) else {
            skipped.push(format!("audit {seq}: no capability"));
            continue;
        };
        let Some(state) = data.get("state") else {
            // A read that failed. There is no value, and none is made up.
            skipped.push(format!(
                "audit {seq}: read of {capability} failed, so it has no value"
            ));
            continue;
        };
        let (Some(value), Some(timestamp)) = (
            state.get("value"),
            state.get("timestamp").and_then(Value::as_str),
        ) else {
            skipped.push(format!(
                "audit {seq}: read of {capability} has no value or timestamp"
            ));
            continue;
        };
        let source = state
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("device");
        let quality = match state.get("quality").and_then(Value::as_str) {
            Some("GOOD") => Quality::Good,
            Some("UNCERTAIN") => Quality::Uncertain,
            _ => Quality::Bad,
        };
        let (domain, provenance) = match source {
            "device" => (format!("device:{device}"), Provenance::Device),
            "simulator" => (GATEWAY_CLOCK.to_owned(), Provenance::Simulator),
            _ => (GATEWAY_CLOCK.to_owned(), Provenance::Device),
        };
        out.push(Record {
            id: format!("audit-{seq}"),
            signal: capability.to_owned(),
            value: value.clone(),
            source: SourceInfo {
                system: Some("openmhs".into()),
                device: Some(device),
                kind: SourceKind::DeviceState,
                provenance: Some(provenance),
            },
            time: Some(TimeInfo {
                source: Some(timestamp.to_owned()),
                ingest: at_ms.map(millis_to_rfc3339),
                clock_domain: Some(domain),
                uncertainty_ms: None,
            }),
            quality,
            refs: Default::default(),
            relations: vec![],
            calibration: None,
            frame: None,
        });
    }

    for r in receipts {
        let Some(id) = r.get("command_id").and_then(Value::as_str) else {
            skipped.push("a receipt with no command_id".into());
            continue;
        };
        let target = r.pointer("/request/target");
        let device = target
            .and_then(|t| t.get("device"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let capability = target
            .and_then(|t| t.get("capability"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let state = r.get("state").and_then(Value::as_str).unwrap_or("");
        // The time of the state it ended in: when this command's own story finished.
        let at = r
            .get("transitions")
            .and_then(Value::as_array)
            .and_then(|t| t.last())
            .and_then(|t| t.get("at"))
            .and_then(Value::as_str);
        let Some(at) = at else {
            skipped.push(format!("receipt {id}: no transitions"));
            continue;
        };
        let mut refs = std::collections::BTreeMap::new();
        refs.insert("command".to_owned(), id.to_owned());
        if let Some(c) = r.get("correlation_id").and_then(Value::as_str) {
            refs.insert("correlation".to_owned(), c.to_owned());
        }
        if let Some(k) = r.get("idempotency_key").and_then(Value::as_str) {
            refs.insert("idempotency_key".to_owned(), k.to_owned());
        }
        out.push(Record {
            id: format!("command-{id}"),
            signal: format!("command:{capability}"),
            value: Value::String(state.to_owned()),
            source: SourceInfo {
                system: Some("openmhs".into()),
                device: Some(device.to_owned()),
                kind: SourceKind::Controller,
                provenance: None,
            },
            time: Some(TimeInfo {
                source: Some(at.to_owned()),
                ingest: None,
                clock_domain: Some(GATEWAY_CLOCK.to_owned()),
                uncertainty_ms: None,
            }),
            quality: Quality::Good,
            refs,
            relations: vec![],
            calibration: None,
            frame: None,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((out, skipped))
}
