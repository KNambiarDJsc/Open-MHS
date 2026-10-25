//! Human-readable output. Every command also has `--json`, which prints the API result
//! unchanged.

use openmhs_api::{AuditEntry, AuditVerifyResult, ReadResult, TokenInfo};
use openmhs_policy::Grant;
use openmhs_types::descriptor::Capability;
use openmhs_types::receipt::PreflightResult;
use openmhs_types::{Descriptor, Outcome, Receipt, StateValue, Timestamp, Value};
use serde::Serialize;

pub fn json<T: Serialize>(v: &T) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("error: cannot serialize output: {e}"),
    }
}

/// Wire name of a serde enum value, e.g. `UNKNOWN_OUTCOME` or `require_approval`.
pub fn name<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => "?".into(),
    }
}

pub fn value(v: &Value) -> String {
    match v {
        Value::Quantity(q) => format!("{} {}", trim_float(q.value), q.unit),
        Value::Integer(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Text(s) => s.clone(),
    }
}

fn json_value(v: &serde_json::Value) -> String {
    serde_json::from_value::<Value>(v.clone()).map_or_else(|_| v.to_string(), |v| value(&v))
}

fn trim_float(f: f64) -> String {
    let s = format!("{f:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// `HH:MM:SS.mmm` in UTC. Computed, not cut from RFC 3339, which drops trailing zeros.
pub fn clock_time(t: Timestamp) -> String {
    let ms = t.millis().rem_euclid(86_400_000);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn reading(s: &StateValue, now_age_ms: Option<i64>) -> String {
    let age = now_age_ms
        .map(|a| format!(", {a} ms old"))
        .unwrap_or_default();
    format!(
        "{}  ({}, {}{age})",
        value(&s.value),
        name(&s.quality),
        name(&s.source)
    )
}

pub fn read_result(r: &ReadResult) {
    let warn = if r.usable {
        String::new()
    } else {
        match r.max_age_ms {
            Some(max) if r.age_ms > i64::try_from(max).unwrap_or(i64::MAX) => {
                format!("  STALE: older than {max} ms; not usable for decisions")
            }
            _ => "  NOT USABLE for decisions".into(),
        }
    };
    println!(
        "{} {} = {}{warn}",
        r.device,
        r.capability,
        reading(&r.state, Some(r.age_ms))
    );
}

pub fn outcome_label(o: Outcome) -> &'static str {
    match o {
        Outcome::Pending => "pending",
        Outcome::Succeeded => "succeeded (confirmed by readback)",
        Outcome::Unverified => "unverified (device says applied; not confirmed)",
        Outcome::Failed => "failed",
        Outcome::Denied => "denied (nothing sent to the device)",
        Outcome::NotExecuted => "not executed (nothing sent to the device)",
        Outcome::Uncertain => "UNCERTAIN (may or may not have taken effect; reconcile by readback)",
    }
}

fn request_summary(r: &Receipt) -> String {
    let req = &r.request;
    let mut s = format!(
        "{} / {}  {}",
        req.target.device,
        req.target.capability,
        name(&req.operation)
    );
    if let Some(v) = &req.value {
        s += &format!(" {}", value(v));
    }
    if let Some(params) = &req.params {
        let p: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{k}={}", value(v)))
            .collect();
        s += &format!(" ({})", p.join(", "));
    }
    s
}

pub fn preflight(p: &PreflightResult) {
    let rules: Vec<&str> = p.rules.iter().map(|m| m.rule_id.as_str()).collect();
    let mut line = format!("would    {}", name(&p.decision));
    if !rules.is_empty() {
        line += &format!("  (rules: {})", rules.join(", "));
    }
    println!("{line}");
    for e in &p.errors {
        let rule = e
            .rule_id
            .as_ref()
            .map(|id| format!(" [rule {id}]"))
            .unwrap_or_default();
        println!("  denied {}: {}{rule}", e.code, e.message);
        if let Some(hint) = unit_hint(&e.code, &e.message) {
            println!("  hint   {hint}");
        }
    }
    println!("(advisory: nothing was run; a real submission is checked again)");
}

/// A bare number where a quantity is required is refused on purpose (units are never
/// guessed); say how to give the unit.
fn unit_hint(code: &str, message: &str) -> Option<String> {
    if code != "command.value.type_mismatch" {
        return None;
    }
    let key = "\"unit\": \"";
    let start = message.find(key)? + key.len();
    let unit = message[start..]
        .split('"')
        .next()
        .filter(|u| !u.is_empty())?;
    Some(format!(
        "give the unit: --value 5{unit}, or --value 5 --unit {unit}"
    ))
}

pub fn receipt(r: &Receipt) {
    println!("command  {}  by {}", r.command_id, r.principal);
    println!("target   {}", request_summary(r));
    if let Some(k) = &r.idempotency_key {
        println!("key      {k}");
    }
    println!(
        "state    {}  -> {}",
        name(&r.state),
        outcome_label(r.outcome)
    );
    let rules: Vec<&str> = r.policy.rules.iter().map(|m| m.rule_id.as_str()).collect();
    let mut policy = name(&r.policy.decision);
    if !rules.is_empty() {
        policy += &format!("  (rules: {})", rules.join(", "));
    }
    if let Some(by) = &r.policy.approved_by {
        policy += &format!("  approved by {by}");
    }
    println!("policy   {policy}");
    for e in &r.policy.errors {
        let rule = e
            .rule_id
            .as_ref()
            .map(|id| format!(" [rule {id}]"))
            .unwrap_or_default();
        println!("  denied {}: {}{rule}", e.code, e.message);
        if let Some(hint) = unit_hint(&e.code, &e.message) {
            println!("  hint   {hint}");
        }
    }
    if let Some(a) = &r.adapter_result {
        let mut parts = Vec::new();
        if a.acknowledged == Some(true) {
            parts.push("acknowledged".to_owned());
        }
        if a.reported_applied == Some(true) {
            parts.push("reported applied".to_owned());
        }
        if let Some(result) = &a.result
            && !result.is_empty()
        {
            let kv: Vec<String> = result
                .iter()
                .map(|(k, v)| format!("{k}={}", value(v)))
                .collect();
            parts.push(format!("result {}", kv.join(", ")));
        }
        if let Some(e) = &a.error {
            parts.push(format!("error {}: {}", e.code, e.message));
        }
        if !parts.is_empty() {
            println!("device   {}", parts.join("; "));
        }
    }
    if let Some(v) = &r.verification {
        let mut line = name(&v.mode);
        if let Some(c) = &v.capability {
            line += &format!(" {c}");
        }
        if let Some(e) = &v.expected {
            line += &format!("  expected {}", value(e));
        }
        if let Some(o) = &v.observed {
            line += &format!("  observed {}", reading(o, None));
        }
        match v.matched {
            Some(true) => line += "  MATCH",
            Some(false) => line += "  MISMATCH",
            None => {}
        }
        if let Some(reason) = &v.reason {
            line += &format!("  ({reason})");
        }
        println!("verify   {line}");
    }
    for (i, t) in r.transitions.iter().enumerate() {
        let label = if i == 0 { "history" } else { "" };
        let detail = t
            .detail
            .as_ref()
            .map(|d| format!("  {d}"))
            .unwrap_or_default();
        println!(
            "{label:<8} {}  {}{detail}",
            clock_time(t.at),
            name(&t.state)
        );
    }
}

pub fn receipt_line(r: &Receipt) {
    println!(
        "{}  {:<22} {:<12} {}",
        r.command_id,
        name(&r.state),
        r.principal,
        request_summary(r)
    );
}

pub fn devices(devices: &[Descriptor]) {
    if devices.is_empty() {
        println!("no devices registered (start the gateway with --sim for the virtual lab)");
        return;
    }
    for d in devices {
        println!(
            "{:<16} {:<44} {:<22} {} capabilities",
            d.device.id,
            d.device.name.as_deref().unwrap_or(&d.device.model),
            d.adapter.id,
            d.capabilities.len()
        );
    }
}

pub fn graph(g: &openmhs_types::graph::Graph) {
    if g.relationships.is_empty() {
        println!("{} devices, no relationships declared", g.devices.len());
    }
    for e in &g.relationships {
        let r = &e.relationship;
        let from = match &r.component {
            Some(c) => format!("{}/{c}", e.from),
            None => e.from.clone(),
        };
        let object = r
            .target
            .as_deref()
            .or(r.resource.as_deref())
            .or(r.frame.as_deref())
            .unwrap_or("");
        let caps = if r.capabilities.is_empty() {
            String::new()
        } else {
            format!(" ({})", r.capabilities.join(", "))
        };
        let origin = match e.origin {
            openmhs_types::graph::Origin::Descriptor => "descriptor",
            openmhs_types::graph::Origin::Topology => "topology",
        };
        println!(
            "{from:<24} {:<16} {object}{caps}  [{origin}]",
            r.kind.name()
        );
    }
    for (resource, devices) in &g.resources {
        let devices: Vec<&str> = devices.iter().map(String::as_str).collect();
        println!("resource {resource}: {}", devices.join(", "));
    }
    for issue in &g.issues {
        println!("issue: [{}] {}", issue.code, issue.message);
    }
}

fn value_spec(cap: &Capability) -> String {
    let spec = cap
        .value
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or_default();
    spec_text(&spec)
}

fn spec_text(spec: &serde_json::Value) -> String {
    let mut s = spec["type"].as_str().unwrap_or("").to_owned();
    if let Some(u) = spec["unit"].as_str() {
        s += &format!(" [{u}]");
    }
    if spec["range"].is_object() {
        s += &format!(" {}..{}", spec["range"]["min"], spec["range"]["max"]);
    }
    if let Some(vals) = spec["values"].as_array() {
        let v: Vec<String> = vals.iter().map(json_value).collect();
        s += &format!(" {{{}}}", v.join("|"));
    }
    s
}

pub fn descriptor(d: &Descriptor) {
    println!(
        "{}  {}",
        d.device.id,
        d.device.name.as_deref().unwrap_or("")
    );
    let field = |label: &str, v: &Option<String>| {
        if let Some(v) = v {
            println!("  {label:<13}{v}");
        }
    };
    println!("  manufacturer {}", d.device.manufacturer);
    println!("  model        {}", d.device.model);
    field("serial", &d.device.serial);
    field("firmware", &d.device.firmware);
    println!("  adapter      {} {}", d.adapter.id, d.adapter.version);
    println!();
    for c in &d.capabilities {
        let ops: Vec<String> = c.operations.iter().map(name).collect();
        println!(
            "  {:<32} {:<8} {:<18} side effect: {}",
            c.name,
            name(&c.kind),
            ops.join(","),
            name(&c.side_effect)
        );
        if c.value.is_some() {
            println!("      value     {}", value_spec(c));
        }
        if let Some(params) = &c.params {
            for (k, spec) in params {
                let spec = serde_json::to_value(spec).unwrap_or_default();
                println!("      param     {k}: {}", spec_text(&spec));
            }
        }
        for p in &c.preconditions {
            let p = serde_json::to_value(p).unwrap_or_default();
            println!(
                "      requires  {}: {} {} {}",
                p["id"].as_str().unwrap_or("?"),
                p["capability"].as_str().unwrap_or("?"),
                p["op"].as_str().unwrap_or("?"),
                json_value(&p["value"])
            );
        }
        if let Some(v) = &c.verification {
            let v = serde_json::to_value(v).unwrap_or_default();
            let target = v["capability"].as_str().unwrap_or(&c.name);
            let line = match v["mode"].as_str() {
                Some("readback") => format!("readback of {target}"),
                _ => format!("none ({})", v["reason"].as_str().unwrap_or("")),
            };
            println!("      verify    {line}");
        }
        if let Some(f) = &c.freshness {
            println!("      fresh     within {} ms", f.max_age_ms);
        }
    }
}

pub fn grants(grants: &[Grant]) {
    if grants.is_empty() {
        println!("no grants: every operation is denied");
        return;
    }
    for g in grants {
        let ops: Vec<String> = g.operations.iter().map(name).collect();
        let max = g
            .max_side_effect
            .map(|m| format!(" up to {}", name(&m)))
            .unwrap_or_default();
        println!(
            "{:<20} {:<16} {} {} on {} / {}{max}",
            g.id,
            name(&g.effect),
            g.principal,
            ops.join(","),
            g.device,
            g.capability
        );
    }
}

pub fn tokens(tokens: &[TokenInfo]) {
    if tokens.is_empty() {
        println!("no API tokens (the operator token in the runtime file always works)");
        return;
    }
    for t in tokens {
        let status = if t.revoked_at.is_some() {
            "revoked".to_owned()
        } else {
            t.expires_at
                .map_or_else(|| "no expiry".to_owned(), |e| format!("expires {e}"))
        };
        println!(
            "{:<21} {:<9} {:<24} {} scope(s), {status}",
            t.id,
            name(&t.role),
            t.principal,
            t.scopes.len(),
        );
    }
}

pub fn approval(a: &openmhs_types::Approval) {
    println!("approval   {}", a.id);
    println!("status     {}", a.status.name());
    println!(
        "for        {} / {} ({}) by {}",
        a.device,
        a.capability,
        name(&a.operation),
        a.requester
    );
    println!("fingerprint {}", a.fingerprint);
    if let Some(c) = &a.command_id {
        println!("command    {c}");
    }
    if let Some(by) = &a.decided_by {
        println!("decided by {by}");
    }
    if let Some(r) = &a.reason {
        println!("reason     {r}");
    }
    println!("expires    {}", a.expires_at);
}

pub fn approvals(list: &[openmhs_types::Approval]) {
    if list.is_empty() {
        println!("no approvals");
        return;
    }
    for a in list {
        println!(
            "{:<37} {:<9} {:<24} {} / {}",
            a.id,
            a.status.name(),
            a.requester,
            a.device,
            a.capability
        );
    }
}

fn lease_status(l: &openmhs_types::lease::Lease) -> &'static str {
    use openmhs_types::Clock;
    match l.status_at(openmhs_types::SystemClock.now()) {
        openmhs_types::lease::LeaseStatus::Active => "ACTIVE",
        openmhs_types::lease::LeaseStatus::Released => "RELEASED",
        openmhs_types::lease::LeaseStatus::Expired => "EXPIRED",
    }
}

pub fn lease(l: &openmhs_types::lease::Lease) {
    let join = |set: &std::collections::BTreeSet<String>| {
        set.iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!("lease      {}", l.id);
    println!("status     {}", lease_status(l));
    println!("holder     {}", l.holder);
    println!("devices    {}", join(&l.claim.devices));
    if !l.claim.resources.is_empty() {
        println!("resources  {}", join(&l.claim.resources));
    }
    if let Some(r) = &l.reason {
        println!("reason     {r}");
    }
    if let Some(p) = &l.plan_id {
        println!("plan       {p}");
    }
    println!("expires    {}", l.expires_at);
    if let (Some(at), Some(by)) = (&l.released_at, &l.released_by) {
        println!("released   {at} by {by}");
    }
}

pub fn leases(list: &[openmhs_types::lease::Lease]) {
    if list.is_empty() {
        println!("no leases");
        return;
    }
    for l in list {
        let devices: Vec<&str> = l.claim.devices.iter().map(String::as_str).collect();
        println!(
            "{:<37} {:<8} {:<24} {}",
            l.id,
            lease_status(l),
            l.holder,
            devices.join(", ")
        );
    }
}

pub fn created_token(t: &openmhs_api::CreatedToken) {
    println!(
        "created {} for {} ({})",
        t.info.id,
        t.info.principal,
        name(&t.info.role)
    );
    for s in &t.info.scopes {
        let ops: Vec<String> = s.operations.iter().map(|o| name(o).to_owned()).collect();
        println!(
            "scope: {} / {} [{}]",
            s.device,
            s.capability,
            ops.join(", ")
        );
    }
    if let Some(e) = t.info.expires_at {
        println!("expires: {e}");
    }
    println!("token: {}", t.token);
    println!("This is shown once. Pass it with --token or OPENMHS_TOKEN.");
}

pub fn audit_verify(v: &AuditVerifyResult) {
    if v.valid {
        println!(
            "audit log intact: {} records, hash chain verified",
            v.records
        );
    } else {
        println!(
            "AUDIT LOG TAMPERED: record {} fails verification ({} records)",
            v.first_invalid.unwrap_or_default(),
            v.records
        );
    }
}

pub fn audit_entries(entries: &[AuditEntry]) {
    for e in entries {
        let mut data = e.data.to_string();
        if data.chars().count() > 110 {
            data = data.chars().take(107).collect::<String>() + "...";
        }
        println!(
            "#{:<5} {}  {:<22} {:<38} {data}",
            e.seq,
            clock_time(e.at),
            e.kind,
            e.subject
        );
    }
}

pub fn plan(p: &openmhs_types::plan::Plan) {
    println!("plan       {}", p.id);
    println!("state      {}", p.state.name());
    println!("principal  {}", p.principal);
    if let Some(d) = &p.spec.description {
        println!("about      {d}");
    }
    if let Some(r) = &p.reason {
        println!("reason     {r}");
    }
    if let Some(l) = &p.lease_id {
        println!("lease      {l}");
    }
    println!("steps");
    for s in &p.steps {
        let mark = if p.current.as_deref() == Some(&s.id) {
            ">"
        } else {
            " "
        };
        let command = s.last_command().unwrap_or("-");
        println!("  {mark} {:<24} {:<24} {command}", s.id, name(&s.status));
        if let Some(e) = &s.error {
            println!("      {}: {}", e.code, e.message);
        }
    }
    for d in &p.decisions {
        let reason = d
            .reason
            .as_deref()
            .map(|r| format!(": {r}"))
            .unwrap_or_default();
        println!(
            "decision   {} by {} at {}{reason}",
            d.decision.name(),
            d.by,
            d.at
        );
    }
}

pub fn plans(list: &[openmhs_types::plan::Plan]) {
    if list.is_empty() {
        println!("no plans");
        return;
    }
    for p in list {
        let done = p
            .steps
            .iter()
            .filter(|s| s.status == openmhs_types::plan::StepStatus::Succeeded)
            .count();
        println!(
            "{:<37} {:<18} {:<24} {done}/{} steps",
            p.id,
            p.state.name(),
            p.principal,
            p.steps.len()
        );
    }
}

pub fn plan_preflight(p: &openmhs_types::plan::PlanPreflight) {
    println!("ok         {}", p.ok);
    println!(
        "outcome    {} (in the twin; it has no physics)",
        p.outcome.name()
    );
    let devices: Vec<&str> = p.lease.claim.devices.iter().map(String::as_str).collect();
    let lease = if p.lease.available {
        "available"
    } else {
        "unavailable"
    };
    println!("lease      {lease}: {}", devices.join(", "));
    if let Some(e) = &p.lease.error {
        println!("           {}: {}", e.code, e.message);
    }
    println!("steps");
    for s in &p.steps {
        let undo = s
            .compensation
            .map(|c| format!(" (compensation: {})", name(&c)))
            .unwrap_or_default();
        println!("  {:<24} {}{undo}", s.id, name(&s.verdict));
        for e in &s.errors {
            println!("      {}: {}", e.code, e.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_time_keeps_three_digits() {
        let at = |s: &str| clock_time(Timestamp::parse_rfc3339(s).unwrap());
        assert_eq!(at("2026-10-23T17:00:09.62Z"), "17:00:09.620");
        assert_eq!(at("2026-10-23T17:00:09.9Z"), "17:00:09.900");
        assert_eq!(at("2026-10-23T17:00:09Z"), "17:00:09.000");
        assert_eq!(at("2026-10-23T00:00:00.001Z"), "00:00:00.001");
        assert_eq!(at("1969-12-31T23:59:59.999Z"), "23:59:59.999");
    }

    #[test]
    fn a_bare_number_for_a_quantity_gets_a_unit_hint() {
        let m = r#"expected a quantity {"value": …, "unit": "%"}, got integer"#;
        assert_eq!(
            unit_hint("command.value.type_mismatch", m).as_deref(),
            Some("give the unit: --value 5%, or --value 5 --unit %")
        );
        assert_eq!(unit_hint("command.value.out_of_range", m), None);
        assert_eq!(
            unit_hint("command.value.type_mismatch", "expected a boolean"),
            None
        );
    }
}
