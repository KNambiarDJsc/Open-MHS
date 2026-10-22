//! Integration tests that drive the real `openmhs` binary against a real gateway
//! process (`openmhs serve --sim`), over the local API, exactly as a user would.

// Helpers outside #[test] functions unwrap too; clippy's in-test allowance doesn't reach them.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_openmhs");
const PSU: &str = "sim.psu-01";
const SETPOINT: &str = "power.output.voltage.setpoint";

const OK: i32 = 0;
const ERROR: i32 = 1;
const DENIED: i32 = 2;
const UNCERTAIN: i32 = 4;
const PENDING: i32 = 5;

/// A gateway process with its own home directory. Killed on drop.
struct Gateway {
    home: tempfile::TempDir,
    child: Child,
}

fn runtime(home: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(home.join("gateway.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn spawn_serve(home: &Path) -> Child {
    Command::new(BIN)
        .args(["serve", "--sim", "--port", "0", "--home"])
        .arg(home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn openmhs serve")
}

/// Wait until the gateway process `pid` has published its runtime file.
fn wait_ready(home: &Path, pid: u32) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if runtime(home).is_some_and(|r| r["pid"] == pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("gateway did not start within 30 s");
}

impl Gateway {
    fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let child = spawn_serve(home.path());
        wait_ready(home.path(), child.id());
        Self { home, child }
    }

    fn home(&self) -> PathBuf {
        self.home.path().to_owned()
    }

    /// Hard-kill the process (no graceful shutdown), then start a new one on the same home.
    fn crash_and_restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = spawn_serve(self.home.path());
        wait_ready(self.home.path(), self.child.id());
    }

    /// Run the CLI with `--json` as the operator, or as `token`'s principal.
    fn cli(&self, token: Option<&str>, args: &[&str]) -> (i32, Value) {
        let mut cmd = Command::new(BIN);
        cmd.arg("--json").arg("--home").arg(self.home.path());
        if let Some(t) = token {
            cmd.args(["--token", t]);
        }
        let out = cmd.args(args).output().expect("run openmhs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let json = serde_json::from_str(&stdout).unwrap_or(Value::Null);
        (out.status.code().unwrap_or(-1), json)
    }

    fn op(&self, args: &[&str]) -> (i32, Value) {
        self.cli(None, args)
    }

    fn agent_token(&self) -> String {
        let (code, t) = self.op(&["tokens", "create", "agent:test"]);
        assert_eq!(code, OK, "{t}");
        t["token"].as_str().unwrap().to_owned()
    }

    fn grant_psu(&self, effect: &str) {
        for args in [
            vec![
                "grants",
                "add",
                "--id",
                "read-all",
                "--principal",
                "agent:*",
                "--device",
                "*",
                "--capability",
                "*",
                "--ops",
                "read",
                "--effect",
                "allow",
            ],
            vec![
                "grants",
                "add",
                "--id",
                "psu",
                "--principal",
                "agent:*",
                "--device",
                PSU,
                "--capability",
                "power.output.*",
                "--ops",
                "write",
                "--effect",
                effect,
            ],
        ] {
            let (code, out) = self.op(&args);
            assert_eq!(code, OK, "{out}");
        }
    }

    fn fault(&self, fault: &str) {
        let (code, out) = self.op(&["sim", "fault", PSU, fault]);
        assert_eq!(code, OK, "{out}");
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn set_volts(gw: &Gateway, token: &str, value: &str, extra: &[&str]) -> (i32, Value) {
    let mut args = vec!["exec", PSU, SETPOINT, "--value", value];
    args.extend_from_slice(extra);
    gw.cli(Some(token), &args)
}

fn first_error(receipt: &Value) -> &str {
    receipt["policy"]["errors"][0]["code"]
        .as_str()
        .unwrap_or("")
}

#[test]
fn denied_write_without_grant() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    let (code, r) = set_volts(&gw, &agent, "1V", &[]);
    assert_eq!(code, DENIED);
    assert_eq!(r["state"], "DENIED");
    assert_eq!(first_error(&r), "policy.denied.no_grant");
    assert!(r["adapter_result"].is_null(), "nothing reached the device");
}

#[test]
fn denied_write_out_of_range() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("allow");
    let (code, r) = set_volts(&gw, &agent, "48V", &[]);
    assert_eq!(code, DENIED);
    assert_eq!(first_error(&r), "command.value.out_of_range");
}

#[test]
fn denied_write_on_stale_telemetry() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("allow");
    gw.fault("stale=5000");
    let (code, r) = set_volts(&gw, &agent, "1V", &[]);
    assert_eq!(code, DENIED);
    assert_eq!(first_error(&r), "command.state.stale");
    // The agent also sees the staleness on a direct read.
    let (code, read) = gw.cli(Some(&agent), &["read", PSU, "protection.tripped"]);
    assert_eq!(code, OK);
    assert_eq!(read["usable"], false);
}

#[test]
fn approved_write_is_verified() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("require_approval");
    let (code, pending) = set_volts(&gw, &agent, "1.5V", &[]);
    assert_eq!(code, PENDING, "{pending}");
    assert_eq!(pending["state"], "AWAITING_APPROVAL");
    let id = pending["command_id"].as_str().unwrap();

    // The agent can neither approve nor see the approval queue's other entries.
    let (code, _) = gw.cli(Some(&agent), &["approve", id]);
    assert_eq!(code, DENIED);

    let (code, r) = gw.op(&["approve", id]);
    assert_eq!(code, OK, "{r}");
    assert_eq!(r["state"], "VERIFIED");
    assert_eq!(r["policy"]["approved_by"], "operator:local");
    assert_eq!(r["verification"]["matched"], true);
    assert_eq!(r["verification"]["observed"]["value"]["value"], 1.5);

    let (_, shown) = gw.cli(Some(&agent), &["commands", "show", id]);
    assert_eq!(shown, r, "the stored receipt is the one returned");
}

#[test]
fn duplicate_submission_runs_once() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("allow");
    let key = ["--idempotency-key", "k1"];
    let (code, first) = set_volts(&gw, &agent, "1V", &key);
    assert_eq!(code, OK, "{first}");
    let (code, second) = set_volts(&gw, &agent, "1V", &key);
    assert_eq!(code, OK);
    assert_eq!(first["command_id"], second["command_id"]);
    assert_eq!(first, second, "same receipt, not a re-execution");

    let (_, list) = gw.cli(Some(&agent), &["commands", "list"]);
    assert_eq!(list.as_array().unwrap().len(), 1, "only one command exists");

    let (code, conflict) = set_volts(&gw, &agent, "2V", &key);
    assert_eq!(code, ERROR);
    assert_eq!(
        conflict["error"]["data"]["code"],
        "command.idempotency.conflict"
    );
}

#[test]
fn timeout_becomes_unknown_outcome_and_reconciles() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("allow");
    gw.fault("apply-then-hang");
    let (code, r) = set_volts(&gw, &agent, "1V", &["--deadline-ms", "1000"]);
    assert_eq!(code, UNCERTAIN, "{r}");
    assert_eq!(r["state"], "UNKNOWN_OUTCOME");
    let states: Vec<&str> = r["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["state"].as_str().unwrap())
        .collect();
    assert!(states.contains(&"TIMED_OUT"), "{states:?}");
    assert!(!states.contains(&"FAILED"));

    gw.fault("clear");
    let id = r["command_id"].as_str().unwrap();
    let (code, rec) = gw.cli(Some(&agent), &["commands", "reconcile", id]);
    assert_eq!(code, OK, "{rec}");
    assert_eq!(rec["state"], "RECONCILED_APPLIED");
}

/// Invariant 13: misbehaving adapters are contained; the gateway keeps serving and
/// classifies each failure honestly.
#[test]
fn misbehaving_adapters_are_contained() {
    let gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("allow");
    let deadline = ["--deadline-ms", "800"];
    let reconcile = |id: &str| gw.cli(Some(&agent), &["commands", "reconcile", id]);

    // Adapter panics before applying: uncertain to the engine, resolved by readback.
    gw.fault("crash");
    let (code, r) = set_volts(&gw, &agent, "1V", &deadline);
    assert_eq!(
        (code, r["state"].as_str()),
        (UNCERTAIN, Some("UNKNOWN_OUTCOME"))
    );
    assert_eq!(r["adapter_result"]["error"]["code"], "adapter.internal");
    gw.fault("clear");
    let (_, rec) = reconcile(r["command_id"].as_str().unwrap());
    assert_eq!(rec["state"], "RECONCILED_NOT_APPLIED");

    // Garbled response after applying.
    std::thread::sleep(Duration::from_millis(1100)); // slew window
    gw.fault("apply-then-malformed");
    let (code, r) = set_volts(&gw, &agent, "1V", &deadline);
    assert_eq!(
        (code, r["state"].as_str()),
        (UNCERTAIN, Some("UNKNOWN_OUTCOME"))
    );
    gw.fault("clear");
    let (_, rec) = reconcile(r["command_id"].as_str().unwrap());
    assert_eq!(rec["state"], "RECONCILED_APPLIED");

    // Command lost entirely (hang without applying).
    std::thread::sleep(Duration::from_millis(1100));
    gw.fault("hang");
    let (code, r) = set_volts(&gw, &agent, "2V", &deadline);
    assert_eq!(
        (code, r["state"].as_str()),
        (UNCERTAIN, Some("UNKNOWN_OUTCOME"))
    );
    gw.fault("clear");
    let (_, rec) = reconcile(r["command_id"].as_str().unwrap());
    assert_eq!(rec["state"], "RECONCILED_NOT_APPLIED");

    // Link down: reads fail with a classified error, nothing is guessed.
    gw.fault("link-down");
    let (code, err) = gw.cli(Some(&agent), &["read", PSU, SETPOINT]);
    assert_eq!(code, ERROR);
    assert_eq!(err["error"]["data"]["code"], "adapter.disconnected");
    gw.fault("link-up");

    // Still serving, and the audit log is intact.
    let (code, info) = gw.op(&["info"]);
    assert_eq!((code, info["devices"].as_u64()), (OK, Some(3)));
    let (code, audit) = gw.op(&["audit", "verify"]);
    assert_eq!((code, audit["valid"].as_bool()), (OK, Some(true)));
}

#[test]
fn gateway_crash_mid_dispatch_leaves_unknown_outcome() {
    let mut gw = Gateway::start();
    let agent = gw.agent_token();
    gw.grant_psu("allow");
    gw.fault("hang");

    // Submit in the background; the adapter hangs, so the command stays DISPATCHED.
    let mut submit = Command::new(BIN)
        .args(["--json", "--home"])
        .arg(gw.home())
        .args(["--token", &agent, "exec", PSU, SETPOINT, "--value", "1V"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let id = loop {
        let (_, list) = gw.op(&["commands", "list", "--state", "DISPATCHED"]);
        if let Some(id) = list[0]["command_id"].as_str() {
            break id.to_owned();
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "never dispatched"
        );
        std::thread::sleep(Duration::from_millis(50));
    };

    gw.crash_and_restart();
    let _ = submit.wait();

    let (code, r) = gw.op(&["commands", "show", &id]);
    assert_eq!(code, OK);
    assert_eq!(r["state"], "UNKNOWN_OUTCOME");
    let detail = r["transitions"].as_array().unwrap().last().unwrap()["detail"]
        .as_str()
        .unwrap();
    assert!(detail.contains("gateway restarted"), "{detail}");
    let (_, audit) = gw.op(&["audit", "verify"]);
    assert_eq!(audit["valid"], true);
}

#[test]
fn tokens_and_roles_are_enforced() {
    let gw = Gateway::start();
    let agent = gw.agent_token();

    let (code, _) = gw.cli(Some("omhs_not-a-token"), &["info"]);
    assert_eq!(code, ERROR);

    let (code, info) = gw.cli(Some(&agent), &["info"]);
    assert_eq!(code, OK);
    assert_eq!(info["role"], "agent");

    for args in [
        vec!["grants", "list"],
        vec!["tokens", "create", "agent:other"],
        vec!["audit", "verify"],
        vec!["sim", "fault", PSU, "crash"],
    ] {
        let (code, err) = gw.cli(Some(&agent), &args);
        assert_eq!(code, DENIED, "{args:?}: {err}");
        assert_eq!(err["error"]["data"]["code"], "gateway.forbidden");
    }

    // Revoked tokens stop working.
    let (_, list) = gw.op(&["tokens", "list"]);
    let id = list[0]["id"].as_str().unwrap();
    let (code, _) = gw.op(&["tokens", "revoke", id]);
    assert_eq!(code, OK);
    let (code, _) = gw.cli(Some(&agent), &["info"]);
    assert_eq!(code, ERROR);
}

#[test]
fn grants_persist_across_restart() {
    let mut gw = Gateway::start();
    gw.grant_psu("allow");
    gw.crash_and_restart();
    let (_, grants) = gw.op(&["grants", "list"]);
    let ids: Vec<&str> = grants
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["read-all", "psu"]);
}

#[test]
fn one_gateway_per_home() {
    let gw = Gateway::start();
    let out = Command::new(BIN)
        .args(["serve", "--sim", "--port", "0", "--home"])
        .arg(gw.home())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(ERROR));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"));
}

#[test]
fn refuses_non_loopback_without_flag() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .args(["serve", "--listen", "0.0.0.0", "--port", "0", "--home"])
        .arg(home.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(ERROR));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow-remote"));
}

#[test]
fn no_gateway_running_is_a_clear_error() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(BIN)
        .args(["devices", "list", "--home"])
        .arg(home.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(ERROR));
    assert!(String::from_utf8_lossy(&out.stderr).contains("openmhs serve"));
}
