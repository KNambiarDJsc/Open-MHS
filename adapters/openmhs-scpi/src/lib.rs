//! An OpenMHS adapter for SCPI instruments (power supplies, meters, loads) over TCP or a
//! serial port, driven by a **command-set profile**: which SCPI query reads each
//! property, which command sets it, and which command runs each action.
//!
//! Every command is sent once, as `<command>;*OPC?`, so the instrument answers `1` when
//! it has finished. The adapter then reads the error queue (`SYST:ERR?`): an error
//! there means the instrument refused the command (`REJECTED`). No answer, a broken
//! connection, or an unreadable answer leave the outcome uncertain (`UNKNOWN_OUTCOME`),
//! and the gateway reconciles by reading back. See `docs/adapters/scpi.md`.

pub mod link;
pub mod sim;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_profile::{Loaded, Mapping, ProfileError, codec, template};
use openmhs_types::descriptor::{Capability, Operation, ValueSpec, ValueType};
use openmhs_types::{Clock, Quality, StateValue, Value};
use serde::Deserialize;

use crate::link::{LineLink, LinkError, StreamLink};

/// The `protocol` of SCPI profiles.
pub const PROTOCOL: &str = "scpi";

/// Where the instrument is, and the commands every SCPI instrument shares.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub transport: Transport,
    /// How long to wait for any one reply.
    #[serde(default = "default_reply_timeout")]
    pub reply_timeout_ms: u64,
    /// Appended to every command; the instrument answers `1` when done.
    #[serde(default = "default_completion")]
    pub completion: String,
    /// Reads one entry from the error queue: `0,"No error"` or `-222,"Data out of range"`.
    #[serde(default = "default_error_query")]
    pub error_query: String,
    /// Asks the instrument to identify itself; used to connect and for health.
    #[serde(default = "default_identify")]
    pub identify: String,
}

fn default_reply_timeout() -> u64 {
    1000
}
fn default_completion() -> String {
    "*OPC?".into()
}
fn default_error_query() -> String {
    "SYST:ERR?".into()
}
fn default_identify() -> String {
    "*IDN?".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Transport {
    /// `host:port`; SCPI instruments usually listen on 5025.
    Tcp { address: String },
    /// A serial port at `baud` (default 9600), 8N1.
    Serial {
        port: String,
        #[serde(default = "default_baud")]
        baud: u32,
    },
}

fn default_baud() -> u32 {
    9600
}

/// How one capability maps to SCPI.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// Reads the property, e.g. `SOUR:VOLT?`.
    #[serde(default)]
    pub query: Option<String>,
    /// Sets the property; must use `{{value}}`, e.g. `SOUR:VOLT {{value}}`.
    #[serde(default)]
    pub set: Option<String>,
    /// Runs the action; may use its parameters, e.g. `OUTP:PROT:CLE` or `TRIG:DEL {{delay}}`.
    #[serde(default)]
    pub send: Option<String>,
    /// How this boolean is written and read: `["ON", "OFF"]`. Default `["1", "0"]`;
    /// reading also accepts `ON`/`OFF` and `1`/`0`.
    #[serde(default)]
    pub booleans: Option<[String; 2]>,
}

fn plain(text: &str, what: &str) -> Result<(), String> {
    if text.contains(['\n', '\r', ';']) {
        return Err(format!(
            "{what} must be one SCPI command, without ';' or line breaks"
        ));
    }
    Ok(())
}

impl Mapping for Command {
    fn check(&self, cap: &Capability) -> Result<(), String> {
        let has = |op| cap.operations.contains(&op);
        match (&self.query, has(Operation::Read)) {
            (None, true) => return Err("readable, so it needs a 'query'".into()),
            (Some(_), false) => return Err("has a 'query' but is not readable".into()),
            (Some(q), true) => {
                plain(q, "'query'")?;
                if !template::names(q)?.is_empty() || !q.trim_end().ends_with('?') {
                    return Err("'query' must be a fixed query ending in '?'".into());
                }
            }
            (None, false) => {}
        }
        match (&self.set, has(Operation::Write)) {
            (None, true) => return Err("writable, so it needs a 'set' command".into()),
            (Some(_), false) => return Err("has a 'set' command but is not writable".into()),
            (Some(s), true) => {
                plain(s, "'set'")?;
                if template::names(s)? != ["value"] {
                    return Err(
                        "'set' must use {{value}} exactly once, e.g. \"SOUR:VOLT {{value}}\""
                            .into(),
                    );
                }
            }
            (None, false) => {}
        }
        match (&self.send, has(Operation::Invoke)) {
            (None, true) => return Err("invokable, so it needs a 'send' command".into()),
            (Some(_), false) => return Err("has a 'send' command but is not an action".into()),
            (Some(s), true) => {
                plain(s, "'send'")?;
                let params = cap.params.as_ref();
                for n in template::names(s)? {
                    if !params.is_some_and(|p| p.contains_key(&n)) {
                        return Err(format!(
                            "'send' uses {{{{{n}}}}}, which is not a parameter of the action"
                        ));
                    }
                }
            }
            (None, false) => {}
        }
        if self.booleans.is_some()
            && cap
                .value
                .as_ref()
                .is_none_or(|v| v.ty != ValueType::Boolean)
        {
            return Err("'booleans' only applies to boolean properties".into());
        }
        Ok(())
    }
}

pub type Profile = Loaded<Connection, Command>;

/// Parse and validate an SCPI profile.
pub fn load(json: &str) -> Result<Profile, ProfileError> {
    openmhs_profile::load(json, PROTOCOL)
}

/// `v` as SCPI text.
fn encode(m: &Command, v: &Value) -> String {
    match (v, &m.booleans) {
        (Value::Bool(b), Some([on, off])) => if *b { on } else { off }.clone(),
        _ => codec::to_text(v),
    }
}

/// SCPI text as a value of `spec`.
fn decode(m: &Command, spec: &ValueSpec, text: &str) -> Result<Value, String> {
    if let Some([on, off]) = &m.booleans {
        let t = text.trim();
        if t.eq_ignore_ascii_case(on) {
            return Ok(Value::Bool(true));
        }
        if t.eq_ignore_ascii_case(off) {
            return Ok(Value::Bool(false));
        }
    }
    codec::from_text(spec, text)
}

/// Parse one error-queue entry: `-222,"Data out of range"`.
fn parse_error(reply: &str) -> Option<(i32, String)> {
    let (code, message) = reply.split_once(',').unwrap_or((reply, ""));
    let code = code.trim().trim_start_matches('+').parse().ok()?;
    Some((code, message.trim().trim_matches('"').to_owned()))
}

pub struct ScpiAdapter {
    profile: Profile,
    link: Arc<dyn LineLink>,
    clock: Arc<dyn Clock>,
}

impl ScpiAdapter {
    /// An adapter for `profile`, reaching the instrument through its declared transport.
    pub fn new(profile: Profile, clock: Arc<dyn Clock>) -> Self {
        let identify = profile.connection.identify.clone();
        let link: Arc<dyn LineLink> = match &profile.connection.transport {
            Transport::Tcp { address } => {
                Arc::new(StreamLink::tcp(address.clone()).synced_by(identify))
            }
            Transport::Serial { port, baud } => {
                Arc::new(StreamLink::serial(port.clone(), *baud).synced_by(identify))
            }
        };
        Self::with_link(profile, link, clock)
    }

    /// An adapter for `profile` over a link of your own, e.g. to a simulator.
    pub fn with_link(profile: Profile, link: Arc<dyn LineLink>, clock: Arc<dyn Clock>) -> Self {
        Self {
            profile,
            link,
            clock,
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.profile.connection.reply_timeout_ms)
    }

    /// A query that changes nothing. Any failure means the answer is unavailable.
    async fn query(&self, request: &str) -> Result<String, AdapterError> {
        self.link
            .transact(request, self.timeout())
            .await
            .map_err(|e| match e {
                LinkError::Timeout => AdapterError::new(
                    AdapterErrorKind::Timeout,
                    format!("no reply to '{request}'"),
                ),
                LinkError::Down(_) | LinkError::Lost(_) => {
                    AdapterError::new(AdapterErrorKind::Disconnected, e.to_string())
                }
            })
    }

    /// Empty the error queue so that errors read after a command are that command's.
    /// Nothing has been sent yet, so any failure here means the command was not sent.
    async fn clear_errors(&self) -> Result<(), AdapterError> {
        let q = &self.profile.connection.error_query;
        for _ in 0..16 {
            let reply = self.query(q).await.map_err(|e| {
                AdapterError::new(
                    AdapterErrorKind::NotSent,
                    format!("before sending: {}", e.message),
                )
            })?;
            match parse_error(&reply) {
                Some((0, _)) => return Ok(()),
                Some(_) => {}
                None => {
                    return Err(AdapterError::new(
                        AdapterErrorKind::NotSent,
                        format!("before sending: unreadable reply to '{q}': {reply:?}"),
                    ));
                }
            }
        }
        Err(AdapterError::new(
            AdapterErrorKind::NotSent,
            "before sending: the error queue did not empty",
        ))
    }

    fn command_text(&self, cmd: &AdapterCommand) -> Result<String, AdapterError> {
        let not_sent = |m: String| AdapterError::new(AdapterErrorKind::NotSent, m);
        let (_, m) = self.profile.get(&cmd.capability).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no capability '{}'", cmd.capability),
            )
        })?;
        let (tpl, vars): (_, BTreeMap<String, String>) = match (cmd.operation, &cmd.value) {
            (Operation::Write, Some(v)) => (&m.set, [("value".to_owned(), encode(m, v))].into()),
            (Operation::Invoke, _) => (
                &m.send,
                cmd.params
                    .iter()
                    .map(|(k, v)| (k.clone(), encode(m, v)))
                    .collect(),
            ),
            _ => {
                return Err(not_sent(format!(
                    "cannot {:?} '{}'",
                    cmd.operation, cmd.capability
                )));
            }
        };
        let tpl = tpl
            .as_deref()
            .ok_or_else(|| not_sent(format!("no SCPI command for '{}'", cmd.capability)))?;
        template::render_text(tpl, &vars).map_err(not_sent)
    }
}

#[async_trait]
impl HardwareAdapter for ScpiAdapter {
    fn adapter_id(&self) -> &str {
        &self.profile.descriptor.adapter.id
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        Ok(self.profile.descriptor_json.clone())
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.query(&self.profile.connection.identify)
            .await
            .map(|_| ())
    }

    async fn health(&self) -> Health {
        match self
            .link
            .transact(&self.profile.connection.identify, self.timeout())
            .await
        {
            Ok(_) => Health::Healthy,
            Err(LinkError::Timeout) => Health::Degraded("the instrument does not answer".into()),
            Err(e) => Health::Disconnected(e.to_string()),
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        let unsupported = || {
            AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no readable capability '{capability}'"),
            )
        };
        let (cap, m) = self.profile.get(capability).ok_or_else(unsupported)?;
        let (Some(q), Some(spec)) = (&m.query, &cap.value) else {
            return Err(unsupported());
        };
        let reply = self.query(q).await?;
        let value = decode(m, spec, &reply).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("'{q}' answered {reply:?}: {e}"),
            )
        })?;
        Ok(StateValue {
            value,
            timestamp: self.clock.now(),
            source: self.link.source(),
            quality: Quality::Good,
        })
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let text = self.command_text(&command)?;
        self.clear_errors().await?;

        // Sent once. Whatever happens next is reported, never retried.
        let request = format!("{text};{}", self.profile.connection.completion);
        let done = self
            .link
            .transact(&request, self.timeout())
            .await
            .map_err(|e| match e {
                LinkError::Down(m) => AdapterError::new(AdapterErrorKind::NotSent, m),
                LinkError::Lost(m) => AdapterError::new(AdapterErrorKind::Disconnected, m),
                LinkError::Timeout => AdapterError::new(
                    AdapterErrorKind::Timeout,
                    format!("no reply to '{request}'"),
                ),
            })?;
        if !matches!(done.trim(), "1" | "+1") {
            return Err(AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("'{request}' answered {done:?} instead of 1"),
            ));
        }

        // The instrument finished; its error queue says whether it refused.
        let q = &self.profile.connection.error_query;
        let status = self.link.transact(q, self.timeout()).await.map_err(|e| {
            let m = format!("'{text}' completed, but its error status could not be read: {e}");
            match e {
                LinkError::Timeout => AdapterError::new(AdapterErrorKind::Timeout, m),
                _ => AdapterError::new(AdapterErrorKind::Disconnected, m),
            }
        })?;
        match parse_error(&status) {
            Some((0, _)) => {
                progress.accepted();
                Ok(ExecuteReport::Applied {
                    result: BTreeMap::new(),
                })
            }
            Some((code, message)) => Ok(ExecuteReport::Rejected {
                reason: format!("{code}: {message}"),
            }),
            None => Err(AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("'{q}' answered {status:?}"),
            )),
        }
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_queue_entries_parse() {
        assert_eq!(parse_error("0,\"No error\""), Some((0, "No error".into())));
        assert_eq!(
            parse_error("-222,\"Data out of range\""),
            Some((-222, "Data out of range".into()))
        );
        assert_eq!(parse_error("+0"), Some((0, String::new())));
        assert_eq!(parse_error("\u{1}#?K"), None);
    }

    #[test]
    fn the_example_profile_loads_and_bad_mappings_are_named() {
        load(include_str!("../profiles/bench-psu.json")).unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../profiles/bench-psu.json")).unwrap();
        v["mapping"]["psu.voltage.setpoint"]["set"] = serde_json::json!("SOUR:VOLT 5");
        v["mapping"]
            .as_object_mut()
            .unwrap()
            .remove("psu.output.enabled");
        let err = load(&v.to_string()).unwrap_err().to_string();
        assert!(
            err.contains("/mapping/psu.voltage.setpoint: 'set' must use {{value}}"),
            "{err}"
        );
        assert!(
            err.contains("/mapping/psu.output.enabled: missing"),
            "{err}"
        );
    }
}
