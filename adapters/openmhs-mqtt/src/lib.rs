//! An OpenMHS adapter for devices that publish their state and take commands over MQTT.
//!
//! The profile maps each property to a JSON pointer in a **state topic** the device
//! publishes (retained, with a timestamp the device sets), and each write or action to a
//! **command topic** with a JSON payload template. Every command carries its
//! `command_id`; the device answers on an **acknowledgement topic** with that id and
//! whether it acted.
//!
//! Commands are published once at QoS 0, never queued while disconnected and never
//! replayed after a reconnect: a refusal on the acknowledgement topic ends `REJECTED`; no
//! acknowledgement, a lost connection or an unreadable one leave the outcome uncertain
//! (`UNKNOWN_OUTCOME`). Reads first confirm the broker still answers (a subscription
//! round trip, which also re-delivers the retained state), so a cached value is never
//! reported over a dead link. See `docs/adapters/mqtt.md`.

pub mod sim;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_profile::{Loaded, Mapping, ProfileError, codec, template};
use openmhs_types::descriptor::{Capability, Operation};
use openmhs_types::{Clock, Quality, Source, StateValue, Timestamp};
use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, Outgoing, QoS};
use serde::Deserialize;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

/// The `protocol` of MQTT profiles.
pub const PROTOCOL: &str = "mqtt";

/// How long to wait before connecting again after the broker connection is lost.
const RETRY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// The broker.
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Must be unique on the broker: a second client with the same id disconnects the first.
    pub client_id: String,
    /// `${env:NAME}` is replaced by that environment variable.
    #[serde(default)]
    pub username: Option<String>,
    /// `${env:NAME}` is replaced by that environment variable.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "default_keep_alive")]
    pub keep_alive_s: u64,
    /// How long to wait for the broker to answer, and for a command's acknowledgement.
    #[serde(default = "default_reply_timeout")]
    pub reply_timeout_ms: u64,
    /// Where the device acknowledges commands.
    pub ack: AckRule,
}

fn default_port() -> u16 {
    1883
}

fn default_keep_alive() -> u64 {
    5
}

fn default_reply_timeout() -> u64 {
    1000
}

/// `{"id": "<command_id>", "ok": true}` means done; `"ok": false` means refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckRule {
    pub topic: String,
    /// JSON pointer to the command id the acknowledgement answers.
    pub id_pointer: String,
    /// JSON pointer to a boolean: `true` done, `false` refused.
    pub ok_pointer: String,
    /// JSON pointer to the reason for a refusal.
    #[serde(default)]
    pub error_pointer: Option<String>,
}

/// A topic the device publishes its state on, as JSON.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateRule {
    pub topic: String,
    /// JSON pointer to the value, e.g. `/air/temperature`.
    pub pointer: String,
    /// JSON pointer to when the value was measured: RFC 3339 text or Unix milliseconds.
    /// Without it, the time the message arrived is used.
    #[serde(default)]
    pub timestamp_pointer: Option<String>,
}

/// A message that sets a property or runs an action.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRule {
    pub topic: String,
    /// `{"fan": "{{value}}", "id": "{{command_id}}"}`; actions use their parameter
    /// names. `{{command_id}}` is required: it is how acknowledgements are matched.
    pub payload: serde_json::Value,
}

/// How one capability maps to MQTT.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topics {
    #[serde(default)]
    pub state: Option<StateRule>,
    #[serde(default)]
    pub command: Option<CommandRule>,
}

fn check_topic(t: &str) -> Result<(), String> {
    if t.is_empty() || t.contains(['+', '#', '\0']) {
        return Err(format!(
            "topic '{t}' must be a plain topic name, without wildcards"
        ));
    }
    Ok(())
}

fn check_pointer(p: &str) -> Result<(), String> {
    if !p.is_empty() && !p.starts_with('/') {
        return Err(format!(
            "'{p}' is not a JSON pointer (e.g. \"/air/temperature\")"
        ));
    }
    Ok(())
}

impl Mapping for Topics {
    fn check(&self, cap: &Capability) -> Result<(), String> {
        let has = |op| cap.operations.contains(&op);
        let commands = has(Operation::Write) || has(Operation::Invoke);
        match (self.state.is_some(), has(Operation::Read)) {
            (false, true) => return Err("the capability allows read, so it needs 'state'".into()),
            (true, false) => {
                return Err("has 'state' but the capability does not allow read".into());
            }
            _ => {}
        }
        match (self.command.is_some(), commands) {
            (false, true) => {
                return Err("the capability allows write or invoke, so it needs 'command'".into());
            }
            (true, false) => {
                return Err(
                    "has 'command' but the capability allows neither write nor invoke".into(),
                );
            }
            _ => {}
        }
        if let Some(s) = &self.state {
            check_topic(&s.topic)?;
            check_pointer(&s.pointer)?;
            if let Some(t) = &s.timestamp_pointer {
                check_pointer(t)?;
            }
        }
        if let Some(c) = &self.command {
            check_topic(&c.topic)?;
            let names = template::json_names(&c.payload)?;
            if !names.iter().any(|n| n == "command_id") {
                return Err("the 'command' payload must carry {{command_id}}, so acknowledgements can be matched".into());
            }
            for n in names.iter().filter(|n| *n != "command_id") {
                let ok = if has(Operation::Write) {
                    n == "value"
                } else {
                    cap.params.as_ref().is_some_and(|p| p.contains_key(n))
                };
                if !ok {
                    return Err(format!(
                        "the 'command' payload uses {{{{{n}}}}}, which is not the value or a parameter"
                    ));
                }
            }
            if has(Operation::Write) && !names.iter().any(|n| n == "value") {
                return Err(
                    "the 'command' payload of a writable property must use {{value}}".into(),
                );
            }
        }
        Ok(())
    }
}

pub type Profile = Loaded<Connection, Topics>;

/// Parse and validate an MQTT profile.
pub fn load(json: &str) -> Result<Profile, ProfileError> {
    let profile: Profile = openmhs_profile::load(json, PROTOCOL)?;
    let c = &profile.connection;
    let mut problems = Vec::new();
    for (pointer, r) in [
        ("/connection/ack/topic", check_topic(&c.ack.topic)),
        (
            "/connection/ack/id_pointer",
            check_pointer(&c.ack.id_pointer),
        ),
        (
            "/connection/ack/ok_pointer",
            check_pointer(&c.ack.ok_pointer),
        ),
    ] {
        if let Err(e) = r {
            problems.push((pointer.to_owned(), e));
        }
    }
    if profile
        .mapping
        .values()
        .any(|t| t.state.as_ref().is_some_and(|s| s.topic == c.ack.topic))
    {
        problems.push((
            "/connection/ack/topic".into(),
            "must not also be a state topic".into(),
        ));
    }
    if c.client_id.is_empty() {
        problems.push(("/connection/client_id".into(), "must not be empty".into()));
    }
    if problems.is_empty() {
        Ok(profile)
    } else {
        Err(ProfileError { problems })
    }
}

/// The topics the adapter subscribes to: every state topic and the acknowledgement topic.
fn subscriptions(profile: &Profile) -> BTreeSet<String> {
    let mut topics: BTreeSet<String> = profile
        .mapping
        .values()
        .filter_map(|t| t.state.as_ref().map(|s| s.topic.clone()))
        .collect();
    topics.insert(profile.connection.ack.topic.clone());
    topics
}

/// The newest message on a state topic.
struct Message {
    payload: Bytes,
    received: Timestamp,
}

/// What came back on the acknowledgement topic for the command in flight.
enum Ack {
    Done,
    Refused(String),
    Unreadable(String),
}

/// A SUBSCRIBE handed to the client, in the order the event loop will send them.
enum Pending {
    /// The subscription made after each connect; its SUBACK means ready.
    Initial,
    /// A read's liveness probe.
    Probe(oneshot::Sender<()>),
}

#[derive(Default)]
struct Inner {
    /// Connected and subscribed: reads and commands may go out.
    ready: bool,
    /// The current connection's client. Each connection gets a new one, so nothing
    /// queued during an earlier connection can be sent on a later one.
    client: Option<AsyncClient>,
    /// Why the last connection attempt or connection failed.
    last_error: Option<String>,
    initial_pkid: Option<u16>,
    unsent: VecDeque<Pending>,
    probes: HashMap<u16, oneshot::Sender<()>>,
    cache: HashMap<String, Message>,
    /// The command waiting for its acknowledgement, by id.
    waiting: Option<(String, oneshot::Sender<Ack>)>,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Woken on every change: ready, a new message.
    changed: Notify,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct MqttAdapter {
    profile: Arc<Profile>,
    broker: (String, u16),
    credentials: Option<(String, String)>,
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
    /// One command in flight, so an unreadable acknowledgement belongs to it.
    commands: tokio::sync::Mutex<()>,
    source: Source,
    clock: Arc<dyn Clock>,
}

impl MqttAdapter {
    /// An adapter for `profile`. Fails if a credential names an unset environment variable.
    pub fn new(profile: Profile, clock: Arc<dyn Clock>) -> Result<Self, String> {
        let c = &profile.connection;
        let credentials = match &c.username {
            Some(user) => {
                let password = c
                    .password
                    .as_deref()
                    .map(template::expand_env)
                    .transpose()?
                    .unwrap_or_default();
                Some((template::expand_env(user)?, password))
            }
            None => None,
        };
        Ok(Self {
            broker: (c.host.clone(), c.port),
            credentials,
            profile: Arc::new(profile),
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner::default()),
                changed: Notify::new(),
            }),
            task: Mutex::new(None),
            commands: tokio::sync::Mutex::new(()),
            source: Source::Device,
            clock,
        })
    }

    /// Use another broker than the profile's, e.g. a simulator.
    #[must_use]
    pub fn at(mut self, host: impl Into<String>, port: u16) -> Self {
        self.broker = (host.into(), port);
        self
    }

    /// Mark readings as coming from a simulator rather than hardware.
    #[must_use]
    pub fn simulated(mut self) -> Self {
        self.source = Source::Simulator;
        self
    }

    fn reply_timeout(&self) -> Duration {
        Duration::from_millis(self.profile.connection.reply_timeout_ms)
    }

    /// Start connecting, once.
    fn start(&self) {
        let mut task = self.task.lock().unwrap_or_else(PoisonError::into_inner);
        if task.is_some() {
            return;
        }
        let c = &self.profile.connection;
        let mut options =
            MqttOptions::new(c.client_id.clone(), self.broker.0.clone(), self.broker.1);
        options.set_keep_alive(Duration::from_secs(c.keep_alive_s.max(1)));
        // No session to resume: nothing the broker kept for us is replayed.
        options.set_clean_session(true);
        if let Some((user, password)) = &self.credentials {
            options.set_credentials(user.clone(), password.clone());
        }
        *task = Some(tokio::spawn(drive(
            options,
            self.shared.clone(),
            self.profile.clone(),
            self.clock.clone(),
        )));
    }

    fn disconnected(&self, what: &str) -> AdapterError {
        let why = self
            .shared
            .lock()
            .last_error
            .clone()
            .unwrap_or_else(|| "not connected yet".into());
        AdapterError::new(
            AdapterErrorKind::Disconnected,
            format!("{what}: no broker connection ({why})"),
        )
    }

    /// Wait until connected and subscribed.
    async fn ready(&self) -> Result<(), AdapterError> {
        self.start();
        let wait = async {
            loop {
                let notified = self.shared.changed.notified();
                if self.shared.lock().ready {
                    return;
                }
                notified.await;
            }
        };
        // Long enough for the connect, the subscribe and the retained state.
        let limit = self.reply_timeout().max(Duration::from_secs(1)) * 3;
        tokio::time::timeout(limit, wait)
            .await
            .map_err(|_| self.disconnected("connect"))
    }

    /// Confirm the broker still answers: subscribe to `topic` again and wait for the
    /// SUBACK. The broker then also sends the topic's retained message again.
    async fn probe(&self, topic: &str) -> Result<(), AdapterError> {
        self.start();
        let (tx, rx) = oneshot::channel();
        {
            let mut inner = self.shared.lock();
            let Some(client) = inner.client.as_ref().filter(|_| inner.ready) else {
                drop(inner);
                return Err(self.disconnected(topic));
            };
            if client.try_subscribe(topic, QoS::AtMostOnce).is_err() {
                drop(inner);
                return Err(self.disconnected(topic));
            }
            inner.unsent.push_back(Pending::Probe(tx));
        }
        match tokio::time::timeout(self.reply_timeout(), rx).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(self.disconnected(topic)),
            Err(_) => Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                format!(
                    "{topic}: the broker did not answer within {} ms",
                    self.profile.connection.reply_timeout_ms
                ),
            )),
        }
    }

    /// The newest message on `topic`, waiting briefly for a first one.
    async fn latest(&self, topic: &str) -> Result<(Bytes, Timestamp), AdapterError> {
        let wait = async {
            loop {
                let notified = self.shared.changed.notified();
                {
                    let inner = self.shared.lock();
                    if let Some(m) = inner.cache.get(topic) {
                        return Ok((m.payload.clone(), m.received));
                    }
                    if !inner.ready {
                        return Err(());
                    }
                }
                notified.await;
            }
        };
        match tokio::time::timeout(self.reply_timeout(), wait).await {
            Ok(Ok(m)) => Ok(m),
            Ok(Err(())) => Err(self.disconnected(topic)),
            Err(_) => Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                format!(
                    "{topic}: nothing published within {} ms",
                    self.profile.connection.reply_timeout_ms
                ),
            )),
        }
    }

    /// The topic and payload that carry `command`.
    fn message(&self, command: &AdapterCommand) -> Result<(&str, serde_json::Value), AdapterError> {
        let not_sent = |m: String| AdapterError::new(AdapterErrorKind::NotSent, m);
        let (_, topics) = self.profile.get(&command.capability).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no capability '{}'", command.capability),
            )
        })?;
        let rule = topics.command.as_ref().ok_or_else(|| {
            not_sent(format!(
                "cannot {:?} '{}'",
                command.operation, command.capability
            ))
        })?;
        let mut vars: BTreeMap<String, serde_json::Value> = [(
            "command_id".to_owned(),
            serde_json::json!(command.command_id),
        )]
        .into();
        match (command.operation, &command.value) {
            (Operation::Write, Some(v)) => {
                vars.insert("value".into(), codec::to_json(v));
            }
            (Operation::Invoke, _) => {
                vars.extend(
                    command
                        .params
                        .iter()
                        .map(|(k, v)| (k.clone(), codec::to_json(v))),
                );
            }
            _ => {
                return Err(not_sent(format!(
                    "cannot {:?} '{}'",
                    command.operation, command.capability
                )));
            }
        }
        let payload = template::render_json(&rule.payload, &vars).map_err(not_sent)?;
        Ok((&rule.topic, payload))
    }
}

impl Drop for MqttAdapter {
    fn drop(&mut self) {
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

/// Keep a broker connection: connect, subscribe, file incoming messages. A lost
/// connection is dropped with its client and everything queued on it, and a new one is
/// made, so nothing is ever sent late.
async fn drive(
    options: MqttOptions,
    shared: Arc<Shared>,
    profile: Arc<Profile>,
    clock: Arc<dyn Clock>,
) {
    let topics = subscriptions(&profile);
    loop {
        let (client, mut eventloop) = AsyncClient::new(options.clone(), 16);
        let error = loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Incoming::ConnAck(_))) => {
                    let mut inner = shared.lock();
                    let filters = topics
                        .iter()
                        .map(|t| rumqttc::SubscribeFilter::new(t.clone(), QoS::AtMostOnce));
                    if client.try_subscribe_many(filters).is_ok() {
                        inner.unsent.push_back(Pending::Initial);
                    }
                }
                Ok(Event::Outgoing(Outgoing::Subscribe(pkid))) => {
                    let mut inner = shared.lock();
                    match inner.unsent.pop_front() {
                        Some(Pending::Initial) => inner.initial_pkid = Some(pkid),
                        Some(Pending::Probe(tx)) => {
                            inner.probes.insert(pkid, tx);
                        }
                        None => {}
                    }
                }
                Ok(Event::Incoming(Incoming::SubAck(ack))) => {
                    let mut inner = shared.lock();
                    if inner.initial_pkid == Some(ack.pkid) {
                        inner.initial_pkid = None;
                        inner.ready = true;
                        inner.client = Some(client.clone());
                        inner.last_error = None;
                        drop(inner);
                        shared.changed.notify_waiters();
                    } else if let Some(tx) = inner.probes.remove(&ack.pkid) {
                        let _ = tx.send(());
                    }
                }
                Ok(Event::Incoming(Incoming::Publish(p))) => {
                    incoming(
                        &shared,
                        &profile.connection.ack,
                        &p.topic,
                        p.payload,
                        clock.now(),
                    );
                    shared.changed.notify_waiters();
                }
                Ok(_) => {}
                Err(e) => break e,
            }
        };
        {
            let mut inner = shared.lock();
            inner.ready = false;
            inner.client = None;
            inner.last_error = Some(error.to_string());
            inner.unsent.clear();
            inner.probes.clear();
            inner.initial_pkid = None;
            inner.cache.clear();
            // The command in flight may or may not have reached the device.
            inner.waiting = None;
        }
        shared.changed.notify_waiters();
        drop((client, eventloop));
        tokio::time::sleep(RETRY).await;
    }
}

/// File a message: the newest state, or the acknowledgement of the command in flight.
fn incoming(shared: &Shared, ack: &AckRule, topic: &str, payload: Bytes, received: Timestamp) {
    let mut inner = shared.lock();
    if topic != ack.topic {
        inner
            .cache
            .insert(topic.to_owned(), Message { payload, received });
        return;
    }
    let verdict = match &inner.waiting {
        Some((id, _)) => judge(ack, id, &payload),
        None => None, // nothing in flight: a late answer
    };
    if let (Some(verdict), Some((_, tx))) = (verdict, inner.waiting.take()) {
        let _ = tx.send(verdict);
    }
}

/// What an acknowledgement says, if it answers command `id`.
fn judge(rule: &AckRule, id: &str, payload: &[u8]) -> Option<Ack> {
    let text = || {
        String::from_utf8_lossy(payload)
            .chars()
            .take(200)
            .collect::<String>()
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return Some(Ack::Unreadable(format!("not JSON: {:?}", text())));
    };
    let answers = match json.pointer(&rule.id_pointer) {
        Some(serde_json::Value::String(s)) => s == id,
        Some(_) => false,
        None => {
            return Some(Ack::Unreadable(format!(
                "no command id at '{}': {}",
                rule.id_pointer,
                text()
            )));
        }
    };
    if !answers {
        return None; // a late answer to an earlier command
    }
    Some(
        match json
            .pointer(&rule.ok_pointer)
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => Ack::Done,
            Some(false) => Ack::Refused(
                rule.error_pointer
                    .as_deref()
                    .and_then(|p| json.pointer(p)?.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "refused".into()),
            ),
            None => Ack::Unreadable(format!("no boolean at '{}': {}", rule.ok_pointer, text())),
        },
    )
}

/// When a reading was measured, from the message if the profile says where.
fn measured_at(
    doc: &serde_json::Value,
    pointer: Option<&str>,
) -> Result<Option<Timestamp>, String> {
    let Some(p) = pointer else { return Ok(None) };
    match doc.pointer(p) {
        Some(serde_json::Value::String(s)) => Timestamp::parse_rfc3339(s)
            .map(Some)
            .map_err(|e| format!("timestamp '{s}': {e}")),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .map(|ms| Some(Timestamp::from_millis(ms)))
            .ok_or_else(|| format!("timestamp {n} is not whole milliseconds")),
        other => Err(format!("no timestamp at '{p}' ({other:?})")),
    }
}

#[async_trait]
impl HardwareAdapter for MqttAdapter {
    fn adapter_id(&self) -> &str {
        &self.profile.descriptor.adapter.id
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        Ok(self.profile.descriptor_json.clone())
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.ready().await
    }

    async fn health(&self) -> Health {
        let topic = self.profile.connection.ack.topic.clone();
        match self.probe(&topic).await {
            Ok(()) => Health::Healthy,
            Err(e) if e.kind == AdapterErrorKind::Disconnected => Health::Disconnected(e.message),
            Err(e) => Health::Degraded(e.to_string()),
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        let unsupported = || {
            AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no readable capability '{capability}'"),
            )
        };
        let (cap, topics) = self.profile.get(capability).ok_or_else(unsupported)?;
        let (Some(rule), Some(spec)) = (&topics.state, &cap.value) else {
            return Err(unsupported());
        };
        self.probe(&rule.topic).await?;
        let (payload, received) = self.latest(&rule.topic).await?;
        let malformed = |m: String| {
            AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("{}: {m}", rule.topic),
            )
        };
        let doc: serde_json::Value =
            serde_json::from_slice(&payload).map_err(|e| malformed(format!("not JSON: {e}")))?;
        let raw = doc
            .pointer(&rule.pointer)
            .ok_or_else(|| malformed(format!("nothing at '{}'", rule.pointer)))?;
        let value = codec::from_json(spec, raw)
            .map_err(|e| malformed(format!("'{}': {e}", rule.pointer)))?;
        let timestamp = measured_at(&doc, rule.timestamp_pointer.as_deref())
            .map_err(malformed)?
            .unwrap_or(received);
        Ok(StateValue {
            value,
            timestamp,
            source: self.source,
            quality: Quality::Good,
        })
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let (topic, payload) = self.message(&command)?;
        let topic = topic.to_owned();
        let _one = self.commands.lock().await;
        self.start();
        let (tx, rx) = oneshot::channel();
        {
            let mut inner = self.shared.lock();
            let Some(client) = inner.client.clone().filter(|_| inner.ready) else {
                drop(inner);
                let e = self.disconnected(&topic);
                return Err(AdapterError::new(AdapterErrorKind::NotSent, e.message));
            };
            // Published once, at most once: nothing queues it for a later connection.
            if let Err(e) = client.try_publish(&topic, QoS::AtMostOnce, false, payload.to_string())
            {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotSent,
                    format!("{topic}: {e}"),
                ));
            }
            inner.waiting = Some((command.command_id.clone(), tx));
        }
        let ack = tokio::time::timeout(self.reply_timeout(), rx).await;
        self.shared.lock().waiting = None;
        match ack {
            Ok(Ok(Ack::Done)) => {
                progress.accepted();
                Ok(ExecuteReport::Applied {
                    result: BTreeMap::new(),
                })
            }
            Ok(Ok(Ack::Refused(reason))) => Ok(ExecuteReport::Rejected { reason }),
            Ok(Ok(Ack::Unreadable(m))) => Err(AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("{}: {m}", self.profile.connection.ack.topic),
            )),
            Ok(Err(_)) => Err(AdapterError::new(
                AdapterErrorKind::Disconnected,
                format!("{topic}: the broker connection was lost before an acknowledgement"),
            )),
            Err(_) => Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                format!(
                    "{topic}: no acknowledgement on {} within {} ms",
                    self.profile.connection.ack.topic, self.profile.connection.reply_timeout_ms
                ),
            )),
        }
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        let client = self.shared.lock().client.take();
        if let Some(c) = client {
            let _ = c.try_disconnect();
        }
        if let Some(task) = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
        self.shared.lock().ready = false;
        Ok(())
    }
}
