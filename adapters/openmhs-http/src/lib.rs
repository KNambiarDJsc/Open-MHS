//! An OpenMHS adapter for simple networked devices with an HTTP/JSON API (ESP32 and
//! Arduino boards, smart plugs, lab equipment with a REST interface), driven by a
//! **declarative mapping**: which URL and JSON pointer hold each property, and which
//! request with which JSON body sets it or runs an action.
//!
//! Each command is one HTTP request, sent once and never redirected. A refusal (4xx, or
//! `{"ok": false}` when the profile says how success looks) ends `REJECTED`; no answer,
//! a dropped connection, a server error or an unreadable answer leave the outcome
//! uncertain (`UNKNOWN_OUTCOME`). See `docs/adapters/http.md`.
//!
//! `https://` URLs verify the device's certificate, against the system's trust store or
//! `connection.tls.ca_file`. Plain `http://` to anything but loopback is refused unless
//! the profile sets `connection.allow_plaintext` (docs/security/transport-security.md).

pub mod sim;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_profile::tls::{self, Tls};
use openmhs_profile::{Loaded, Mapping, ProfileError, codec, template};
use openmhs_types::descriptor::{Capability, Operation};
use openmhs_types::{Clock, Quality, Source, StateValue, Timestamp};
use serde::Deserialize;

/// The `protocol` of HTTP profiles.
pub const PROTOCOL: &str = "http";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// Where the device's API is, e.g. `https://192.168.1.60`.
    pub base_url: String,
    /// Certificate trust for `https://` URLs. Default: the system's trust store.
    #[serde(default)]
    pub tls: Option<Tls>,
    /// Accept unencrypted `http://` to a device that is not on loopback.
    #[serde(default)]
    pub allow_plaintext: bool,
    /// How long to wait for any one response.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// Sent with every request. `${env:NAME}` is replaced by that environment variable,
    /// so secrets stay out of the profile.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// How the device says whether a command succeeded. Without it, any 2xx answer
    /// counts as success and an unreadable answer cannot be told apart from one.
    #[serde(default)]
    pub response: Option<ResponseRule>,
    /// Which property answers the health check. Default: the first readable one.
    #[serde(default)]
    pub health: Option<String>,
}

fn default_timeout() -> u64 {
    1000
}

/// `{"ok": true}` means done; `{"ok": false, "error": "why"}` means refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseRule {
    /// JSON pointer to a boolean: `true` done, `false` refused.
    pub ok_pointer: String,
    /// JSON pointer to the reason for a refusal.
    #[serde(default)]
    pub error_pointer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Post,
    Put,
    Patch,
}

impl Method {
    fn reqwest(self) -> reqwest::Method {
        match self {
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
        }
    }
}

/// A GET that returns a JSON document holding the property.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadRule {
    pub path: String,
    /// JSON pointer to the value, e.g. `/led/brightness`.
    pub pointer: String,
    /// JSON pointer to when the value was measured: RFC 3339 text or Unix milliseconds.
    /// Without it, the time the answer arrived is used.
    #[serde(default)]
    pub timestamp_pointer: Option<String>,
}

/// A request with a JSON body built from a template.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteRule {
    pub method: Method,
    pub path: String,
    /// `{"brightness": "{{value}}"}`; actions use their parameter names, and any request
    /// may include `{{command_id}}`.
    pub body: serde_json::Value,
}

/// How one capability maps to HTTP.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    #[serde(default)]
    pub read: Option<ReadRule>,
    #[serde(default)]
    pub write: Option<WriteRule>,
    #[serde(default)]
    pub invoke: Option<WriteRule>,
}

fn check_path(p: &str) -> Result<(), String> {
    if !p.starts_with('/') {
        return Err(format!("path '{p}' must start with '/'"));
    }
    Ok(())
}

fn check_pointer(p: &str) -> Result<(), String> {
    if !p.is_empty() && !p.starts_with('/') {
        return Err(format!("'{p}' is not a JSON pointer (e.g. \"/led/on\")"));
    }
    Ok(())
}

impl Mapping for Endpoint {
    fn check(&self, cap: &Capability) -> Result<(), String> {
        let has = |op| cap.operations.contains(&op);
        let pairs = [
            (self.read.is_some(), has(Operation::Read), "read"),
            (self.write.is_some(), has(Operation::Write), "write"),
            (self.invoke.is_some(), has(Operation::Invoke), "invoke"),
        ];
        for (mapped, declared, what) in pairs {
            match (mapped, declared) {
                (false, true) => {
                    return Err(format!(
                        "the capability allows {what}, so it needs '{what}'"
                    ));
                }
                (true, false) => {
                    return Err(format!("has '{what}' but the capability does not allow it"));
                }
                _ => {}
            }
        }
        if let Some(r) = &self.read {
            check_path(&r.path)?;
            check_pointer(&r.pointer)?;
            if let Some(t) = &r.timestamp_pointer {
                check_pointer(t)?;
            }
        }
        if let Some(w) = &self.write {
            check_path(&w.path)?;
            let names = template::json_names(&w.body)?;
            if !names.iter().any(|n| n == "value") {
                return Err("the 'write' body must use {{value}}".into());
            }
            if let Some(n) = names.iter().find(|n| *n != "value" && *n != "command_id") {
                return Err(format!(
                    "the 'write' body uses {{{{{n}}}}}; only value and command_id exist"
                ));
            }
        }
        if let Some(w) = &self.invoke {
            check_path(&w.path)?;
            for n in template::json_names(&w.body)? {
                if n != "command_id" && !cap.params.as_ref().is_some_and(|p| p.contains_key(&n)) {
                    return Err(format!(
                        "the 'invoke' body uses {{{{{n}}}}}, which is not a parameter of the action"
                    ));
                }
            }
        }
        Ok(())
    }
}

pub type Profile = Loaded<Connection, Endpoint>;

/// Parse and validate an HTTP profile.
pub fn load(json: &str) -> Result<Profile, ProfileError> {
    openmhs_profile::load(json, PROTOCOL)
}

/// A JSON pointer that may be empty (the whole document).
fn at<'a>(doc: &'a serde_json::Value, pointer: &str) -> Option<&'a serde_json::Value> {
    doc.pointer(pointer)
}

pub struct HttpAdapter {
    profile: Profile,
    base_url: String,
    client: reqwest::Client,
    headers: reqwest::header::HeaderMap,
    source: Source,
    clock: Arc<dyn Clock>,
    /// Why requests to `base_url` are refused, if they are.
    refusal: Option<String>,
}

/// Why `base_url` may not be used, if it may not.
fn refusal(base_url: &str, allow_plaintext: bool) -> Option<String> {
    match reqwest::Url::parse(base_url) {
        Ok(u) if u.scheme() == "https" => None,
        Ok(u) if u.scheme() == "http" => {
            tls::check_plaintext(u.host_str().unwrap_or(""), allow_plaintext, "HTTP").err()
        }
        Ok(u) => Some(format!(
            "unsupported URL scheme {:?}; use https or http",
            u.scheme()
        )),
        Err(e) => Some(format!("base_url {base_url:?}: {e}")),
    }
}

impl HttpAdapter {
    /// An adapter for `profile`. Fails if a header names an unset environment variable,
    /// or a TLS file cannot be used.
    pub fn new(profile: Profile, clock: Arc<dyn Clock>) -> Result<Self, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        for (k, v) in &profile.connection.headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| format!("header '{k}': {e}"))?;
            let value = reqwest::header::HeaderValue::from_str(&template::expand_env(v)?)
                .map_err(|e| format!("header '{k}': {e}"))?;
            headers.insert(name, value);
        }
        let c = &profile.connection;
        let https = c.base_url.starts_with("https://");
        let tls_config = match (&c.tls, https) {
            (Some(_), false) => {
                return Err("connection.tls is set, but base_url is not https://".into());
            }
            (Some(t), true) => t.client_config(&profile.dir)?,
            (None, true) => Tls::default().client_config(&profile.dir)?,
            // A plaintext device: no TLS connection may ever succeed.
            (None, false) => tls::refuse_all(),
        };
        tls::install_provider();
        let client = reqwest::Client::builder()
            // A redirect would send the command again, elsewhere.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("openmhs-http/", env!("CARGO_PKG_VERSION")))
            .use_preconfigured_tls((*tls_config).clone())
            .build()
            .map_err(|e| tls::chain(&e))?;
        let base_url = c.base_url.trim_end_matches('/').to_owned();
        Ok(Self {
            refusal: refusal(&base_url, c.allow_plaintext),
            base_url,
            profile,
            client,
            headers,
            source: Source::Device,
            clock,
        })
    }

    /// Use another base URL than the profile's, e.g. a simulator.
    #[must_use]
    pub fn at(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_owned();
        self.refusal = refusal(&self.base_url, self.profile.connection.allow_plaintext);
        self
    }

    fn allowed(&self) -> Result<(), AdapterError> {
        match &self.refusal {
            Some(why) => Err(AdapterError::new(AdapterErrorKind::NotSent, why.clone())),
            None => Ok(()),
        }
    }

    /// Mark readings as coming from a simulator rather than hardware.
    #[must_use]
    pub fn simulated(mut self) -> Self {
        self.source = Source::Simulator;
        self
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.profile.connection.timeout_ms)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value, AdapterError> {
        self.allowed()?;
        let response = self
            .client
            .get(self.url(path))
            .headers(self.headers.clone())
            .timeout(self.timeout())
            .send()
            .await
            .map_err(|e| {
                let kind = if e.is_timeout() {
                    AdapterErrorKind::Timeout
                } else {
                    AdapterErrorKind::Disconnected
                };
                AdapterError::new(kind, format!("GET {path}: {}", tls::chain(&e)))
            })?;
        let status = response.status();
        let body = response.bytes().await.map_err(|e| {
            let kind = if e.is_timeout() {
                AdapterErrorKind::Timeout
            } else {
                AdapterErrorKind::Disconnected
            };
            AdapterError::new(kind, format!("GET {path}: {e}"))
        })?;
        if !status.is_success() {
            return Err(AdapterError::new(
                AdapterErrorKind::DeviceFailure,
                format!("GET {path} answered {status}"),
            ));
        }
        serde_json::from_slice(&body).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("GET {path}: not JSON: {e}"),
            )
        })
    }

    /// The JSON body that carries `command`.
    fn request(
        &self,
        command: &AdapterCommand,
    ) -> Result<(&WriteRule, serde_json::Value), AdapterError> {
        let not_sent = |m: String| AdapterError::new(AdapterErrorKind::NotSent, m);
        let (_, ep) = self.profile.get(&command.capability).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no capability '{}'", command.capability),
            )
        })?;
        let mut vars: BTreeMap<String, serde_json::Value> = [(
            "command_id".to_owned(),
            serde_json::json!(command.command_id),
        )]
        .into();
        let rule = match (command.operation, &command.value) {
            (Operation::Write, Some(v)) => {
                vars.insert("value".into(), codec::to_json(v));
                ep.write.as_ref()
            }
            (Operation::Invoke, _) => {
                vars.extend(
                    command
                        .params
                        .iter()
                        .map(|(k, v)| (k.clone(), codec::to_json(v))),
                );
                ep.invoke.as_ref()
            }
            _ => None,
        }
        .ok_or_else(|| {
            not_sent(format!(
                "cannot {:?} '{}'",
                command.operation, command.capability
            ))
        })?;
        let body = template::render_json(&rule.body, &vars).map_err(not_sent)?;
        Ok((rule, body))
    }
}

/// What a command's HTTP answer means.
fn judge(
    rule: Option<&ResponseRule>,
    status: reqwest::StatusCode,
    body: &[u8],
) -> Result<Result<(), String>, AdapterError> {
    let uncertain = |kind, m: String| Err(AdapterError::new(kind, m));
    let text = || {
        String::from_utf8_lossy(body)
            .chars()
            .take(200)
            .collect::<String>()
    };
    let json: Option<serde_json::Value> = serde_json::from_slice(body).ok();
    let reason = |default: String| -> String {
        rule.and_then(|r| r.error_pointer.as_deref())
            .and_then(|p| json.as_ref()?.pointer(p)?.as_str().map(str::to_owned))
            .unwrap_or(default)
    };
    match status.as_u16() {
        200..=299 => match rule {
            None => Ok(Ok(())),
            Some(r) => match json
                .as_ref()
                .and_then(|j| j.pointer(&r.ok_pointer))
                .and_then(serde_json::Value::as_bool)
            {
                Some(true) => Ok(Ok(())),
                Some(false) => Ok(Err(reason(format!("{status}: refused")))),
                None => uncertain(
                    AdapterErrorKind::MalformedResponse,
                    format!(
                        "{status} without a boolean at '{}': {:?}",
                        r.ok_pointer,
                        text()
                    ),
                ),
            },
        },
        408 | 504 => uncertain(AdapterErrorKind::Timeout, format!("{status}: {}", text())),
        // Not performed: the request was refused or the service is down.
        400..=499 | 501 | 503 => Ok(Err(reason(format!("{status}: {}", text())))),
        _ => uncertain(
            AdapterErrorKind::MalformedResponse,
            format!(
                "{status}: the device failed and may or may not have acted: {}",
                text()
            ),
        ),
    }
}

/// When a reading was measured, from the document if the profile says where.
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
impl HardwareAdapter for HttpAdapter {
    fn adapter_id(&self) -> &str {
        &self.profile.descriptor.adapter.id
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        Ok(self.profile.descriptor_json.clone())
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        let probe = self.profile.connection.health.clone().or_else(|| {
            self.profile
                .descriptor
                .capabilities
                .iter()
                .find(|c| c.is_readable_property())
                .map(|c| c.name.clone())
        });
        match probe {
            Some(name) => self.read(&name).await.map(|_| ()),
            None => Ok(()),
        }
    }

    async fn health(&self) -> Health {
        match self.connect().await {
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
        let (cap, ep) = self.profile.get(capability).ok_or_else(unsupported)?;
        let (Some(rule), Some(spec)) = (&ep.read, &cap.value) else {
            return Err(unsupported());
        };
        let doc = self.get(&rule.path).await?;
        let malformed = |m: String| {
            AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("GET {}: {m}", rule.path),
            )
        };
        let raw = at(&doc, &rule.pointer)
            .ok_or_else(|| malformed(format!("nothing at '{}'", rule.pointer)))?;
        let value = codec::from_json(spec, raw)
            .map_err(|e| malformed(format!("'{}': {e}", rule.pointer)))?;
        let timestamp = measured_at(&doc, rule.timestamp_pointer.as_deref())
            .map_err(malformed)?
            .unwrap_or_else(|| self.clock.now());
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
        self.allowed()?;
        let (rule, body) = self.request(&command)?;
        let what = format!("{} {}", rule.method.as_str(), rule.path);
        // Sent once, never redirected. Whatever happens next is reported, never retried.
        let response = self
            .client
            .request(rule.method.reqwest(), self.url(&rule.path))
            .headers(self.headers.clone())
            .timeout(self.timeout())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                let kind = if e.is_connect() {
                    AdapterErrorKind::NotSent
                } else if e.is_timeout() {
                    AdapterErrorKind::Timeout
                } else {
                    AdapterErrorKind::Disconnected
                };
                AdapterError::new(kind, format!("{what}: {}", tls::chain(&e)))
            })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|e| {
            let kind = if e.is_timeout() {
                AdapterErrorKind::Timeout
            } else {
                AdapterErrorKind::Disconnected
            };
            AdapterError::new(kind, format!("{what}: answered {status}, then {e}"))
        })?;
        match judge(self.profile.connection.response.as_ref(), status, &bytes)? {
            Ok(()) => {
                progress.accepted();
                Ok(ExecuteReport::Applied {
                    result: BTreeMap::new(),
                })
            }
            Err(reason) => Ok(ExecuteReport::Rejected { reason }),
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
    fn plaintext_is_refused_beyond_loopback_unless_allowed() {
        assert_eq!(refusal("https://device.lab", false), None);
        assert_eq!(refusal("http://127.0.0.1:8080", false), None);
        assert_eq!(refusal("http://localhost", false), None);
        let why = refusal("http://192.168.1.60", false).unwrap();
        assert!(why.contains("allow_plaintext"), "{why}");
        assert_eq!(refusal("http://192.168.1.60", true), None);
        assert!(refusal("ftp://device.lab", true).is_some());
    }

    fn rule() -> ResponseRule {
        ResponseRule {
            ok_pointer: "/ok".into(),
            error_pointer: Some("/error".into()),
        }
    }

    #[test]
    fn answers_are_judged_by_status_and_body() {
        let s = |n| reqwest::StatusCode::from_u16(n).unwrap();
        assert_eq!(
            judge(Some(&rule()), s(200), br#"{"ok":true}"#).unwrap(),
            Ok(())
        );
        assert_eq!(
            judge(
                Some(&rule()),
                s(200),
                br#"{"ok":false,"error":"too bright"}"#
            )
            .unwrap(),
            Err("too bright".into())
        );
        assert_eq!(
            judge(Some(&rule()), s(409), br#"{"ok":false,"error":"busy"}"#).unwrap(),
            Err("busy".into())
        );
        assert_eq!(judge(None, s(204), b"").unwrap(), Ok(()));
        let kind = |r: Result<Result<(), String>, AdapterError>| r.unwrap_err().kind;
        assert_eq!(
            kind(judge(Some(&rule()), s(200), b"\x01#?K")),
            AdapterErrorKind::MalformedResponse
        );
        assert_eq!(
            kind(judge(None, s(500), b"oops")),
            AdapterErrorKind::MalformedResponse
        );
        assert_eq!(kind(judge(None, s(504), b"")), AdapterErrorKind::Timeout);
        assert!(judge(None, s(503), b"").unwrap().is_err());
    }

    #[test]
    fn the_example_profile_loads_and_bad_mappings_are_named() {
        load(include_str!("../profiles/led-controller.json")).unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../profiles/led-controller.json")).unwrap();
        v["mapping"]["led.brightness"]["write"]["body"] = serde_json::json!({"b": "{{level}}"});
        v["mapping"]["led.blink"]["invoke"]["path"] = serde_json::json!("api/blink");
        let err = load(&v.to_string()).unwrap_err().to_string();
        assert!(
            err.contains("/mapping/led.brightness: the 'write' body must use {{value}}"),
            "{err}"
        );
        assert!(
            err.contains("/mapping/led.blink: path 'api/blink' must start with '/'"),
            "{err}"
        );
    }
}
