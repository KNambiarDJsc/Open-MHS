//! An adapter running as a child process (docs/architecture/adapter-host.md).
//!
//! [`ProcessAdapter`] implements [`HardwareAdapter`] by relaying each call over the
//! child's stdin and stdout. The engine still owns authorization, the command lifecycle
//! and the audit trail; the process only talks to the device.
//!
//! When the process exits, is killed, or breaks the protocol, every call in flight fails:
//! an `execute` with `adapter.disconnected`, so the engine records `UNKNOWN_OUTCOME` and
//! never retries it. Calls made while no process is running fail with `adapter.not_sent`
//! for commands (nothing was written) and `adapter.disconnected` otherwise. The process
//! is restarted with backoff; too many restarts within the window quarantine the adapter
//! until the gateway restarts.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_conformance::{Fault, Fixture};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Clock, StateValue, SystemClock};
use serde_json::{Value as Json, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, error, info, warn};

use crate::manifest::{Resolved, op_name};
use crate::protocol::{
    self, DeliveriesParams, Incoming, InitFixture, InitializeResult, PROTOCOL_VERSIONS,
    ProgressParams, RpcError, WireCommand, code,
};

/// Environment variables passed through from the host, besides the manifest's own.
#[cfg(windows)]
pub const OS_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
];
#[cfg(not(windows))]
pub const OS_ENV: &[&str] = &["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL", "TZ"];

const STDERR_TAIL: usize = 20;
const STDERR_LINE_MAX: usize = 4096;
/// How long after its stdout closes a process gets to exit on its own.
const EXIT_GRACE: Duration = Duration::from_millis(500);
/// How long past a command's deadline the host keeps waiting for the adapter's answer.
/// The engine's own timeout normally ends the wait first.
const EXECUTE_GRACE_MS: u64 = 1_000;

#[derive(Debug, Clone, Default)]
pub struct HostOptions {
    /// Ask the adapter for its conformance fixture (`fixture.*` methods). Only for the
    /// conformance suite; never set when serving real devices.
    pub fixture: bool,
}

/// Where the adapter process is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Initialized and answering. `generation` counts process starts, from 1.
    Running {
        generation: u64,
    },
    /// Lost; a new process is being started.
    Restarting {
        reason: String,
    },
    /// Not restarted again until the gateway restarts.
    Quarantined {
        reason: String,
    },
    Stopped,
}

#[derive(Clone)]
enum State {
    Running(Arc<Conn>),
    Restarting(String),
    Quarantined(String),
    Stopped,
}

impl State {
    fn status(&self) -> Status {
        match self {
            State::Running(c) => Status::Running {
                generation: c.generation,
            },
            State::Restarting(r) => Status::Restarting { reason: r.clone() },
            State::Quarantined(r) => Status::Quarantined { reason: r.clone() },
            State::Stopped => Status::Stopped,
        }
    }
}

enum Event {
    Closed { generation: u64, reason: String },
    Stop,
}

/// Why a process could not be (re)started.
#[derive(Debug)]
enum StartError {
    /// Worth another try: the process crashed or timed out while starting.
    Retryable(String),
    /// Will fail the same way every time: incompatible protocol, manifest violation,
    /// descriptor changed, missing credential.
    Fatal(String),
}

impl StartError {
    fn message(self) -> String {
        match self {
            StartError::Retryable(m) | StartError::Fatal(m) => m,
        }
    }
}

#[derive(Debug)]
enum CallError {
    /// Nothing was written to the process.
    NotSent(String),
    /// Written, but the process went away before answering.
    Lost(String),
    Timeout,
    Rpc(RpcError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotSent(r) => write!(f, "not sent: {r}"),
            CallError::Lost(r) => write!(f, "process lost: {r}"),
            CallError::Timeout => f.write_str("timed out"),
            CallError::Rpc(e) => write!(f, "{} ({})", e.message, e.code),
        }
    }
}

type Reply = Result<Json, CallError>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One running process: the pipes, the calls waiting for an answer.
struct Conn {
    adapter: String,
    generation: u64,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    /// `None` once the process is gone.
    pending: Mutex<Option<HashMap<u64, oneshot::Sender<Reply>>>>,
    next_id: AtomicU64,
    /// Progress handles of the commands in flight, by command id.
    progress: Mutex<HashMap<String, Progress>>,
    kill: Mutex<Option<oneshot::Sender<String>>>,
    closed: watch::Sender<bool>,
    reason: OnceLock<String>,
    max_bytes: usize,
    stderr_tail: Mutex<VecDeque<String>>,
}

struct PendingGuard<'a> {
    conn: &'a Conn,
    id: u64,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(p) = lock(&self.conn.pending).as_mut() {
            p.remove(&self.id);
        }
    }
}

struct ProgressGuard<'a> {
    conn: &'a Conn,
    command_id: String,
}

impl Drop for ProgressGuard<'_> {
    fn drop(&mut self) {
        lock(&self.conn.progress).remove(&self.command_id);
    }
}

impl Conn {
    fn reason(&self) -> String {
        self.reason
            .get()
            .cloned()
            .unwrap_or_else(|| "adapter process is gone".into())
    }

    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Ask the waiter task to kill the process. The first reason wins.
    fn kill(&self, reason: String) {
        if let Some(tx) = lock(&self.kill).take() {
            let _ = tx.send(reason);
        }
    }

    /// Called once, by the waiter task, after the process has exited.
    fn close(&self, reason: String) {
        let _ = self.reason.set(reason.clone());
        let pending = lock(&self.pending).take();
        for (_, tx) in pending.into_iter().flatten() {
            let _ = tx.send(Err(CallError::Lost(reason.clone())));
        }
        self.closed.send_replace(true);
    }

    async fn wait_closed(&self, within: Duration) -> bool {
        let mut rx = self.closed.subscribe();
        tokio::time::timeout(within, rx.wait_for(|c| *c))
            .await
            .is_ok()
    }

    fn stderr_tail(&self) -> String {
        let tail = lock(&self.stderr_tail);
        if tail.is_empty() {
            String::new()
        } else {
            let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
            format!("; last stderr: {}", lines.join(" | "))
        }
    }

    async fn send_line(&self, mut line: Vec<u8>) -> Result<(), CallError> {
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        let Some(w) = stdin.as_mut() else {
            return Err(CallError::NotSent("the adapter's stdin is closed".into()));
        };
        let written = async {
            w.write_all(&line).await?;
            w.flush().await
        }
        .await;
        if let Err(e) = written {
            drop(stdin);
            let why = format!("writing to the adapter failed: {e}");
            self.kill(why.clone());
            return Err(CallError::Lost(why));
        }
        Ok(())
    }

    async fn call(&self, method: &str, params: Json, timeout: Duration) -> Reply {
        let deadline = tokio::time::Instant::now() + timeout;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        match lock(&self.pending).as_mut() {
            Some(p) => {
                p.insert(id, tx);
            }
            None => return Err(CallError::NotSent(self.reason())),
        }
        let _guard = PendingGuard { conn: self, id };
        let line = serde_json::to_vec(&protocol::request(id, method, params))
            .map_err(|e| CallError::NotSent(e.to_string()))?;
        if line.len() > self.max_bytes {
            return Err(CallError::NotSent(format!(
                "request of {} bytes exceeds the {}-byte message limit",
                line.len(),
                self.max_bytes
            )));
        }
        match tokio::time::timeout_at(deadline, self.send_line(line)).await {
            Ok(sent) => sent?,
            Err(_) => {
                // Part of the line may be in the pipe; the stream can no longer be framed.
                let why = "a write to the adapter timed out; framing lost".to_string();
                self.kill(why.clone());
                return Err(CallError::Lost(why));
            }
        }
        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(CallError::Lost(self.reason())),
            Err(_) => Err(CallError::Timeout),
        }
    }

    /// Handle one message from the adapter. An error is a protocol violation.
    async fn dispatch(&self, msg: Incoming, inner: &Weak<Inner>) -> Result<(), String> {
        match msg {
            Incoming::Response { id, result } => {
                if id >= self.next_id.load(Ordering::SeqCst) {
                    return Err(format!("response to request {id}, which was never sent"));
                }
                let tx = lock(&self.pending).as_mut().and_then(|p| p.remove(&id));
                match tx {
                    Some(tx) => {
                        let _ = tx.send(result.map_err(CallError::Rpc));
                    }
                    // The caller gave up (timeout, cancelled); the answer comes too late.
                    None => debug!(adapter = %self.adapter, id, "late response ignored"),
                }
            }
            Incoming::Notification { method, params } => match method.as_str() {
                "progress" => {
                    let p: ProgressParams = serde_json::from_value(params)
                        .map_err(|e| format!("malformed progress notification: {e}"))?;
                    if p.event != "accepted" {
                        return Err(format!("unknown progress event {:?}", p.event));
                    }
                    if let Some(progress) = lock(&self.progress).get(&p.command_id) {
                        progress.accepted();
                    }
                }
                "fixture.deliveries" => {
                    let d: DeliveriesParams = serde_json::from_value(params)
                        .map_err(|e| format!("malformed fixture.deliveries notification: {e}"))?;
                    if let Some(inner) = inner.upgrade() {
                        inner.deliveries.store(d.count, Ordering::SeqCst);
                    }
                }
                other => {
                    debug!(adapter = %self.adapter, method = other, "unknown notification ignored")
                }
            },
            Incoming::Request { id } => {
                let reply = protocol::error_response(
                    id,
                    code::METHOD_NOT_FOUND,
                    "adapters may not send requests to the host",
                );
                if let Ok(line) = serde_json::to_vec(&reply) {
                    let _ = self.send_line(line).await;
                }
            }
        }
        Ok(())
    }
}

enum Line {
    Data(Vec<u8>),
    TooLong,
    Eof,
}

/// Read up to the next `\n`. A line longer than `max` is `TooLong`; with `discard`, the
/// rest of it is consumed first so the next read starts on a fresh line.
async fn read_line<R: AsyncBufRead + Unpin>(
    r: &mut R,
    max: usize,
    discard: bool,
) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let mut overflow = false;
    loop {
        let available = r.fill_buf().await?;
        if available.is_empty() {
            // A final line without its newline is incomplete; the process is exiting.
            return Ok(if overflow { Line::TooLong } else { Line::Eof });
        }
        let (chunk, done) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (&available[..i], i + 1),
            None => (available, available.len()),
        };
        let complete = done > chunk.len();
        if !overflow && buf.len() + chunk.len() > max {
            overflow = true;
            if !discard {
                return Ok(Line::TooLong);
            }
        }
        if !overflow {
            buf.extend_from_slice(chunk);
        }
        r.consume(done);
        if complete {
            if overflow {
                return Ok(Line::TooLong);
            }
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Line::Data(buf));
        }
    }
}

fn preview(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    let mut out: String = text.chars().take(120).collect();
    if text.chars().count() > 120 {
        out.push('…');
    }
    format!("{out:?}")
}

async fn read_loop(conn: Arc<Conn>, stdout: ChildStdout, inner: Weak<Inner>) {
    let mut reader = BufReader::new(stdout);
    let violation = loop {
        let line = match read_line(&mut reader, conn.max_bytes, false).await {
            Ok(Line::Data(line)) => line,
            Ok(Line::Eof) => break None,
            Ok(Line::TooLong) => {
                break Some(format!(
                    "protocol violation: a message exceeds the {}-byte limit",
                    conn.max_bytes
                ));
            }
            Err(e) => break Some(format!("reading the adapter's stdout failed: {e}")),
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let handled = match protocol::parse_incoming(&line) {
            Ok(msg) => conn.dispatch(msg, &inner).await,
            Err(e) => Err(format!("{e}: {}", preview(&line))),
        };
        if let Err(e) = handled {
            break Some(format!("protocol violation on stdout: {e}"));
        }
    };
    match violation {
        Some(why) => {
            warn!(adapter = %conn.adapter, generation = conn.generation, "{why}");
            conn.kill(why);
        }
        None => {
            if !conn.wait_closed(EXIT_GRACE).await {
                conn.kill("the adapter closed its stdout".into());
            }
        }
    }
}

async fn stderr_loop(conn: Arc<Conn>, stderr: tokio::process::ChildStderr, secrets: Vec<String>) {
    let mut reader = BufReader::new(stderr);
    loop {
        let line = match read_line(&mut reader, STDERR_LINE_MAX, true).await {
            Ok(Line::Data(line)) => String::from_utf8_lossy(&line).into_owned(),
            Ok(Line::TooLong) => "[line too long; dropped]".to_string(),
            Ok(Line::Eof) | Err(_) => return,
        };
        let mut line = line;
        for s in &secrets {
            line = line.replace(s.as_str(), "[redacted]");
        }
        info!(target: "openmhs_host::adapter", adapter = %conn.adapter, "{line}");
        let mut tail = lock(&conn.stderr_tail);
        if tail.len() == STDERR_TAIL {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

async fn wait_loop(
    conn: Arc<Conn>,
    mut child: Child,
    kill: oneshot::Receiver<String>,
    events: mpsc::UnboundedSender<Event>,
) {
    let reason = tokio::select! {
        status = child.wait() => match status {
            Ok(s) => format!("process exited ({s})"),
            Err(e) => format!("waiting for the process failed: {e}"),
        },
        Ok(why) = kill => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            format!("{why}; process killed")
        }
    };
    // Close stdin so nothing more is written to a dead pipe.
    conn.stdin.lock().await.take();
    conn.close(reason.clone());
    let _ = events.send(Event::Closed {
        generation: conn.generation,
        reason,
    });
}

async fn heartbeat_loop(conn: Arc<Conn>, every: Duration, allowed: u32) {
    let mut closed = conn.closed.subscribe();
    let mut missed = 0u32;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = closed.wait_for(|c| *c) => return,
        }
        match conn.call("ping", Json::Null, every).await {
            Ok(_) | Err(CallError::Rpc(_)) => missed = 0,
            Err(CallError::Timeout) => {
                missed += 1;
                warn!(adapter = %conn.adapter, missed, "heartbeat unanswered");
                if missed >= allowed {
                    conn.kill(format!(
                        "{missed} heartbeats in a row went unanswered; presumed hung"
                    ));
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

struct Inner {
    id: String,
    resolved: Resolved,
    options: HostOptions,
    /// The descriptor of the first process; every later process must report the same.
    descriptor: OnceLock<Json>,
    state: Mutex<State>,
    status: watch::Sender<Status>,
    events: mpsc::UnboundedSender<Event>,
    /// Whether the engine has called `connect`; repeated after each restart.
    connected: AtomicBool,
    fixture: Mutex<Option<InitFixture>>,
    deliveries: AtomicU64,
    restarts: Mutex<VecDeque<Instant>>,
    secrets: Vec<String>,
    generations: AtomicU64,
}

impl Inner {
    fn set_state(&self, state: State) {
        let status = state.status();
        *lock(&self.state) = state;
        self.status.send_replace(status);
    }

    fn call_timeout(&self) -> Duration {
        Duration::from_millis(self.resolved.manifest.limits.call_timeout_ms)
    }

    /// The running process, or why there is none.
    fn current(&self) -> Result<Arc<Conn>, String> {
        match &*lock(&self.state) {
            State::Running(c) if !c.is_closed() => Ok(c.clone()),
            State::Running(c) => Err(format!("adapter process lost: {}", c.reason())),
            State::Restarting(r) => Err(format!("adapter process restarting after: {r}")),
            State::Quarantined(r) => Err(format!("adapter quarantined: {r}")),
            State::Stopped => Err("adapter stopped".into()),
        }
    }

    async fn spawn(self: &Arc<Self>) -> Result<Arc<Conn>, StartError> {
        let r = &self.resolved;
        let m = &r.manifest;
        let mut cmd = tokio::process::Command::new(&r.program);
        cmd.args(&r.args).current_dir(&r.cwd).env_clear();
        for name in OS_ENV {
            if let Some(v) = std::env::var_os(name) {
                cmd.env(name, v);
            }
        }
        cmd.envs(&r.env);
        for name in &m.credentials {
            match std::env::var_os(name) {
                Some(v) => {
                    cmd.env(name, v);
                }
                None => {
                    return Err(StartError::Fatal(format!(
                        "credential {name} is not set in the host environment"
                    )));
                }
            }
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            StartError::Retryable(format!("cannot start {}: {e}", r.program.display()))
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(StartError::Retryable("child pipes missing".into()));
        };
        let generation = self.generations.fetch_add(1, Ordering::SeqCst) + 1;
        let (kill_tx, kill_rx) = oneshot::channel();
        let conn = Arc::new(Conn {
            adapter: self.id.clone(),
            generation,
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            pending: Mutex::new(Some(HashMap::new())),
            next_id: AtomicU64::new(1),
            progress: Mutex::new(HashMap::new()),
            kill: Mutex::new(Some(kill_tx)),
            closed: watch::channel(false).0,
            reason: OnceLock::new(),
            max_bytes: m.limits.max_message_bytes,
            stderr_tail: Mutex::new(VecDeque::new()),
        });
        tokio::spawn(wait_loop(conn.clone(), child, kill_rx, self.events.clone()));
        tokio::spawn(read_loop(conn.clone(), stdout, Arc::downgrade(self)));
        tokio::spawn(stderr_loop(conn.clone(), stderr, self.secrets.clone()));

        match self.handshake(&conn).await {
            Ok(()) => {
                tokio::spawn(heartbeat_loop(
                    conn.clone(),
                    Duration::from_millis(m.limits.heartbeat_ms),
                    m.limits.missed_heartbeats,
                ));
                Ok(conn)
            }
            Err(e) => {
                let detail = match &e {
                    StartError::Retryable(m) | StartError::Fatal(m) => m.clone(),
                };
                conn.kill(format!("start failed: {detail}"));
                conn.wait_closed(Duration::from_secs(2)).await;
                let tail = conn.stderr_tail();
                Err(match e {
                    StartError::Retryable(m) => StartError::Retryable(m + &tail),
                    StartError::Fatal(m) => StartError::Fatal(m + &tail),
                })
            }
        }
    }

    async fn handshake(&self, conn: &Conn) -> Result<(), StartError> {
        let m = &self.resolved.manifest;
        let t = Duration::from_millis(m.limits.start_timeout_ms);
        let offered: Vec<&str> = m
            .runtime
            .protocol
            .iter()
            .map(String::as_str)
            .filter(|v| PROTOCOL_VERSIONS.contains(v))
            .collect();
        let params = json!({
            "protocol_versions": offered,
            "host": {"name": "openmhs", "version": env!("CARGO_PKG_VERSION")},
            "fixture": self.options.fixture,
            "config": m.config,
            "permissions": {
                "network": m.endpoints.network,
                "serial": m.endpoints.serial,
                "filesystem": {"read": m.filesystem.read, "write": m.filesystem.write},
            },
            "limits": {"max_message_bytes": m.limits.max_message_bytes},
        });
        let init = conn
            .call("initialize", params, t)
            .await
            .map_err(|e| match e {
                CallError::Rpc(e) if e.code == code::UNSUPPORTED_VERSION => {
                    let supported = e.data.and_then(|d| d.get("supported").cloned());
                    StartError::Fatal(format!(
                        "the adapter supports none of the offered protocol versions {offered:?} \
                         (it supports {})",
                        supported.map_or("?".into(), |s| s.to_string())
                    ))
                }
                other => StartError::Retryable(format!("initialize failed: {other}")),
            })?;
        let init: InitializeResult = serde_json::from_value(init)
            .map_err(|e| StartError::Fatal(format!("malformed initialize result: {e}")))?;
        if !offered.contains(&init.protocol_version.as_str()) {
            return Err(StartError::Fatal(format!(
                "the adapter chose protocol version {:?}, which was not offered ({offered:?})",
                init.protocol_version
            )));
        }
        if init.adapter.id != m.adapter.id {
            return Err(StartError::Fatal(format!(
                "the process says it is adapter {:?}; the manifest is for {:?}",
                init.adapter.id, m.adapter.id
            )));
        }
        let descriptor = conn
            .call("describe", Json::Null, t)
            .await
            .map_err(|e| StartError::Retryable(format!("describe failed: {e}")))?;
        let parsed = openmhs_types::validate_descriptor(&descriptor).map_err(|issues| {
            let issues: Vec<String> = issues.iter().map(ToString::to_string).collect();
            StartError::Fatal(format!("invalid descriptor: {}", issues.join("; ")))
        })?;
        let violations = m.violations(&parsed);
        if !violations.is_empty() {
            return Err(StartError::Fatal(format!(
                "manifest violation: {}",
                violations.join("; ")
            )));
        }
        match self.descriptor.get() {
            Some(first) if *first != descriptor => {
                return Err(StartError::Fatal(
                    "the restarted process reports a different descriptor".into(),
                ));
            }
            Some(_) => {}
            None => {
                let _ = self.descriptor.set(descriptor);
            }
        }
        *lock(&self.fixture) = if self.options.fixture {
            init.fixture
        } else {
            None
        };
        if self.connected.load(Ordering::SeqCst)
            && let Err(e) = conn.call("connect", Json::Null, t).await
        {
            // The device may be unreachable; that is the adapter's health, not a reason
            // to restart the process.
            warn!(adapter = %self.id, "connect after restart failed: {e}");
        }
        Ok(())
    }

    async fn supervise(self: Arc<Self>, mut events: mpsc::UnboundedReceiver<Event>) {
        while let Some(event) = events.recv().await {
            let (generation, reason) = match event {
                Event::Stop => return,
                Event::Closed { generation, reason } => (generation, reason),
            };
            let current = matches!(
                &*lock(&self.state),
                State::Running(c) if c.generation == generation
            );
            if !current {
                continue;
            }
            warn!(adapter = %self.id, generation, "adapter process lost: {reason}");
            self.set_state(State::Restarting(reason.clone()));
            if !self.restart(reason, &mut events).await {
                return;
            }
        }
    }

    /// Start new processes until one runs, the budget is spent, or the host stops.
    /// Returns false if the host stopped.
    async fn restart(
        self: &Arc<Self>,
        mut reason: String,
        events: &mut mpsc::UnboundedReceiver<Event>,
    ) -> bool {
        let policy = self.resolved.manifest.restart.clone();
        let window = Duration::from_secs(policy.window_s);
        loop {
            let attempt = {
                let mut restarts = lock(&self.restarts);
                let now = Instant::now();
                restarts.retain(|t| now.duration_since(*t) < window);
                if restarts.len() >= policy.max_restarts as usize {
                    drop(restarts);
                    self.quarantine(format!(
                        "crash loop: {} restarts within {}s; last failure: {reason}",
                        policy.max_restarts, policy.window_s
                    ));
                    return true;
                }
                restarts.push_back(now);
                restarts.len() as u32
            };
            let backoff = policy
                .initial_backoff_ms
                .saturating_mul(1u64 << (attempt - 1).min(20))
                .min(policy.max_backoff_ms);
            let sleep = tokio::time::sleep(Duration::from_millis(backoff));
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    _ = &mut sleep => break,
                    event = events.recv() => match event {
                        None | Some(Event::Stop) => return false,
                        // From a process that is already gone.
                        Some(Event::Closed { .. }) => {}
                    },
                }
            }
            if matches!(*lock(&self.state), State::Stopped) {
                return false;
            }
            info!(adapter = %self.id, attempt, "restarting adapter process");
            match self.spawn().await {
                Ok(conn) => {
                    let stopped = matches!(*lock(&self.state), State::Stopped);
                    if stopped {
                        conn.kill("host stopped".into());
                        return false;
                    }
                    info!(adapter = %self.id, generation = conn.generation, "adapter process restarted");
                    self.set_state(State::Running(conn));
                    return true;
                }
                Err(StartError::Fatal(r)) => {
                    self.quarantine(r);
                    return true;
                }
                Err(StartError::Retryable(r)) => {
                    warn!(adapter = %self.id, "restart failed: {r}");
                    reason = r;
                }
            }
        }
    }

    fn quarantine(&self, reason: String) {
        error!(adapter = %self.id, "adapter quarantined: {reason}");
        if let State::Running(c) = &*lock(&self.state) {
            c.kill(format!("quarantined: {reason}"));
        }
        self.set_state(State::Quarantined(reason));
    }

    /// Stop for good: no restarts; the process is killed if still running.
    fn stop_now(&self, why: &str) -> Option<Arc<Conn>> {
        let previous = std::mem::replace(&mut *lock(&self.state), State::Stopped);
        self.status.send_replace(Status::Stopped);
        let _ = self.events.send(Event::Stop);
        match previous {
            State::Running(c) => {
                c.kill(why.to_string());
                Some(c)
            }
            _ => None,
        }
    }

    async fn call(&self, method: &str, params: Json) -> Result<Json, String> {
        let conn = self.current()?;
        conn.call(method, params, self.call_timeout())
            .await
            .map_err(|e| e.to_string())
    }
}

/// An adapter in a child process, speaking the adapter-host protocol.
pub struct ProcessAdapter {
    id: String,
    inner: Arc<Inner>,
}

impl ProcessAdapter {
    /// Start the process, negotiate the protocol, and check its descriptor against the
    /// manifest. Fails if any of that fails; restarts only happen after a good start.
    pub async fn start(resolved: Resolved, options: HostOptions) -> Result<Self, String> {
        let id = resolved.manifest.adapter.id.clone();
        let secrets = resolved
            .manifest
            .credentials
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            // Very short values would redact ordinary words.
            .filter(|v| v.len() >= 4)
            .collect();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            id: id.clone(),
            resolved,
            options,
            descriptor: OnceLock::new(),
            state: Mutex::new(State::Stopped),
            status: watch::channel(Status::Stopped).0,
            events: events_tx,
            connected: AtomicBool::new(false),
            fixture: Mutex::new(None),
            deliveries: AtomicU64::new(0),
            restarts: Mutex::new(VecDeque::new()),
            secrets,
            generations: AtomicU64::new(0),
        });
        let conn = inner.spawn().await.map_err(StartError::message)?;
        inner.set_state(State::Running(conn));
        tokio::spawn(inner.clone().supervise(events_rx));
        Ok(Self { id, inner })
    }

    pub fn status(&self) -> Status {
        lock(&self.inner.state).status()
    }

    /// Wait until the status satisfies `f`, or `within` passes. Returns the last status.
    pub async fn wait_for_status(
        &self,
        within: Duration,
        f: impl FnMut(&Status) -> bool,
    ) -> Status {
        let mut rx = self.inner.status.subscribe();
        let _ = tokio::time::timeout(within, rx.wait_for(f)).await;
        rx.borrow().clone()
    }

    /// The conformance fixture, if the adapter was started with [`HostOptions::fixture`]
    /// and offered one in `initialize`.
    pub fn fixture(&self) -> Option<ProcessFixture> {
        let offered = lock(&self.inner.fixture).is_some();
        (self.inner.options.fixture && offered).then(|| ProcessFixture {
            inner: self.inner.clone(),
        })
    }

    fn check(&self, capability: &str, op: Operation) -> Result<(), AdapterError> {
        if self.inner.resolved.manifest.allows(capability, op) {
            Ok(())
        } else {
            Err(AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!(
                    "{} on {capability:?} is not allowed by the adapter's manifest",
                    op_name(op)
                ),
            ))
        }
    }
}

impl Drop for ProcessAdapter {
    fn drop(&mut self) {
        self.inner.stop_now("host dropped the adapter");
    }
}

fn call_error(e: CallError, during: &str) -> AdapterError {
    match e {
        CallError::NotSent(r) => AdapterError::new(AdapterErrorKind::NotSent, r),
        CallError::Lost(r) => AdapterError::new(
            AdapterErrorKind::Disconnected,
            format!("adapter process lost during {during}: {r}"),
        ),
        CallError::Timeout => AdapterError::new(
            AdapterErrorKind::Timeout,
            format!("adapter process did not answer {during} in time"),
        ),
        CallError::Rpc(e) => AdapterError::new(e.kind(), e.message),
    }
}

fn not_running(why: String) -> AdapterError {
    AdapterError::new(AdapterErrorKind::Disconnected, why)
}

#[async_trait]
impl HardwareAdapter for ProcessAdapter {
    fn adapter_id(&self) -> &str {
        &self.id
    }

    async fn describe(&self) -> Result<Json, AdapterError> {
        self.inner.descriptor.get().cloned().ok_or_else(|| {
            AdapterError::new(AdapterErrorKind::Internal, "adapter never described itself")
        })
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        self.inner.connected.store(true, Ordering::SeqCst);
        let conn = self.inner.current().map_err(not_running)?;
        conn.call("connect", Json::Null, self.inner.call_timeout())
            .await
            .map(|_| ())
            .map_err(|e| call_error(e, "connect"))
    }

    async fn health(&self) -> Health {
        let conn = match self.inner.current() {
            Ok(c) => c,
            Err(why) => return Health::Disconnected(why),
        };
        match conn
            .call("health", Json::Null, self.inner.call_timeout())
            .await
        {
            Ok(v) => protocol::parse_health(v)
                .unwrap_or_else(|e| Health::Degraded(format!("malformed health answer: {e}"))),
            Err(CallError::Timeout) => {
                Health::Degraded("the adapter process did not answer health in time".into())
            }
            Err(CallError::Rpc(e)) => Health::Degraded(e.message),
            Err(e) => Health::Disconnected(e.to_string()),
        }
    }

    async fn read(&self, capability: &str) -> Result<StateValue, AdapterError> {
        self.check(capability, Operation::Read)?;
        let conn = self.inner.current().map_err(not_running)?;
        let v = conn
            .call(
                "read",
                json!({"capability": capability}),
                self.inner.call_timeout(),
            )
            .await
            .map_err(|e| call_error(e, "read"))?;
        serde_json::from_value(v).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("malformed reading: {e}"),
            )
        })
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        self.check(&command.capability, command.operation)?;
        let conn = self.inner.current().map_err(|why| {
            AdapterError::new(
                AdapterErrorKind::NotSent,
                format!("command not sent: {why}"),
            )
        })?;
        let wait_ms = u64::try_from(SystemClock.now().ms_until(command.deadline))
            .unwrap_or(0)
            .saturating_add(EXECUTE_GRACE_MS)
            .min(3_600_000);
        lock(&conn.progress).insert(command.command_id.clone(), progress);
        let _guard = ProgressGuard {
            conn: &conn,
            command_id: command.command_id.clone(),
        };
        let v = conn
            .call(
                "execute",
                json!({"command": WireCommand::from(&command)}),
                Duration::from_millis(wait_ms),
            )
            .await
            .map_err(|e| call_error(e, "execute"))?;
        protocol::parse_report(v).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("malformed execute result: {e}"),
            )
        })
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        let grace = Duration::from_millis(self.inner.resolved.manifest.limits.shutdown_grace_ms);
        let conn = match &*lock(&self.inner.state) {
            State::Running(c) => Some(c.clone()),
            _ => None,
        };
        // No restarts from here on, whatever the process does.
        let previous = std::mem::replace(&mut *lock(&self.inner.state), State::Stopped);
        self.inner.status.send_replace(Status::Stopped);
        let _ = self.inner.events.send(Event::Stop);
        drop(previous);
        if let Some(conn) = conn {
            if let Err(e) = conn.call("shutdown", Json::Null, grace).await {
                debug!(adapter = %self.id, "shutdown call: {e}");
            }
            conn.stdin.lock().await.take();
            if !conn.wait_closed(grace).await {
                conn.kill("did not exit after shutdown".into());
                conn.wait_closed(grace).await;
            }
        }
        Ok(())
    }
}

/// The adapter process's conformance fixture (`fixture.*` methods).
pub struct ProcessFixture {
    inner: Arc<Inner>,
}

#[async_trait]
impl Fixture for ProcessFixture {
    fn supports(&self, fault: Fault) -> bool {
        lock(&self.inner.fixture)
            .as_ref()
            .is_some_and(|f| f.faults.iter().any(|n| n == fault.name()))
    }

    async fn inject(&self, fault: Fault) -> Result<(), String> {
        self.inner
            .call("fixture.inject", json!({"fault": fault}))
            .await
            .map(|_| ())
    }

    async fn clear(&self) -> Result<(), String> {
        self.inner
            .call("fixture.clear", Json::Null)
            .await
            .map(|_| ())
    }

    fn deliveries(&self) -> Option<u64> {
        let counts = lock(&self.inner.fixture)
            .as_ref()
            .is_some_and(|f| f.deliveries);
        counts.then(|| self.inner.deliveries.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn lines(input: &[u8], max: usize, discard: bool) -> Vec<String> {
        let mut r = BufReader::with_capacity(4, input);
        let mut out = Vec::new();
        loop {
            match read_line(&mut r, max, discard).await.unwrap() {
                Line::Data(d) => out.push(String::from_utf8(d).unwrap()),
                Line::TooLong => out.push("<too long>".into()),
                Line::Eof => return out,
            }
        }
    }

    #[tokio::test]
    async fn lines_are_framed_and_bounded() {
        assert_eq!(lines(b"ab\r\ncd\n\nef", 10, false).await, ["ab", "cd", ""]);
        assert_eq!(lines(b"0123456789\n", 10, false).await, ["0123456789"]);
        assert_eq!(
            lines(b"0123456789X\nok\n", 10, true).await,
            ["<too long>", "ok"]
        );
        // Without discard, reading stops at the first oversized line.
        let mut r = BufReader::with_capacity(4, &b"0123456789X\nok\n"[..]);
        assert!(matches!(
            read_line(&mut r, 10, false).await.unwrap(),
            Line::TooLong
        ));
    }
}
