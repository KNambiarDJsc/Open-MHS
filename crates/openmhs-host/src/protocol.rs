//! Wire format of the adapter-host protocol (spec/adapter-protocol.md).
//!
//! JSON-RPC 2.0, one message per line on the adapter's stdin and stdout. The host sends
//! requests; the adapter answers them and may send notifications. Nothing else may
//! appear on the adapter's stdout.

use std::collections::BTreeMap;

use openmhs_adapter::{AdapterCommand, AdapterErrorKind, ExecuteReport, Health};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Timestamp, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Protocol versions this host speaks, most preferred first.
pub const PROTOCOL_VERSIONS: &[&str] = &["0.1"];

/// JSON-RPC error codes. The standard ones, then the protocol's own.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// The adapter's operation failed; `data.kind` classifies it (`adapter.timeout` …).
    pub const ADAPTER_ERROR: i64 = -32000;
    /// A request other than `initialize` or `ping` arrived before `initialize`.
    pub const NOT_INITIALIZED: i64 = -32001;
    /// None of the offered protocol versions is supported; `data.supported` lists them.
    pub const UNSUPPORTED_VERSION: i64 = -32002;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

const KINDS: [AdapterErrorKind; 7] = [
    AdapterErrorKind::Timeout,
    AdapterErrorKind::Disconnected,
    AdapterErrorKind::MalformedResponse,
    AdapterErrorKind::DeviceFailure,
    AdapterErrorKind::NotSent,
    AdapterErrorKind::Unsupported,
    AdapterErrorKind::Internal,
];

pub fn kind_from_code(code: &str) -> Option<AdapterErrorKind> {
    KINDS.into_iter().find(|k| k.code() == code)
}

impl RpcError {
    /// The adapter error class this error carries. Unclassified errors count as
    /// `adapter.internal`, whose outcome is uncertain, except a method the adapter does
    /// not have, which it cannot have carried out.
    pub fn kind(&self) -> AdapterErrorKind {
        let declared = self
            .data
            .as_ref()
            .and_then(|d| d.get("kind"))
            .and_then(|k| k.as_str())
            .and_then(kind_from_code);
        match declared {
            Some(kind) => kind,
            None if self.code == code::METHOD_NOT_FOUND => AdapterErrorKind::Unsupported,
            None => AdapterErrorKind::Internal,
        }
    }
}

/// A message from the adapter.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: Result<serde_json::Value, RpcError>,
    },
    Notification {
        method: String,
        params: serde_json::Value,
    },
    /// Adapters may not send requests; the host answers them with `METHOD_NOT_FOUND`.
    Request { id: serde_json::Value },
}

/// Parse one line from the adapter. An error is a protocol violation.
pub fn parse_incoming(line: &[u8]) -> Result<Incoming, String> {
    let v: serde_json::Value =
        serde_json::from_slice(line).map_err(|e| format!("not JSON: {e}"))?;
    let obj = v.as_object().ok_or("not a JSON object")?;
    if obj.get("jsonrpc").and_then(|j| j.as_str()) != Some("2.0") {
        return Err("missing \"jsonrpc\": \"2.0\"".into());
    }
    let id = obj.get("id");
    if let Some(method) = obj.get("method") {
        let method = method.as_str().ok_or("method is not a string")?;
        return Ok(match id {
            Some(id) => Incoming::Request { id: id.clone() },
            None => Incoming::Notification {
                method: method.to_string(),
                params: obj
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            },
        });
    }
    let id = match id {
        Some(serde_json::Value::Number(n)) => {
            n.as_u64().ok_or("response id is not a request id")?
        }
        Some(serde_json::Value::Null) => {
            let detail = obj
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("no message");
            return Err(format!("adapter could not read a host message: {detail}"));
        }
        _ => return Err("response without a numeric id".into()),
    };
    match (obj.get("result"), obj.get("error")) {
        (Some(result), None) => Ok(Incoming::Response {
            id,
            result: Ok(result.clone()),
        }),
        (None, Some(error)) => {
            let error: RpcError = serde_json::from_value(error.clone())
                .map_err(|e| format!("malformed error object: {e}"))?;
            Ok(Incoming::Response {
                id,
                result: Err(error),
            })
        }
        _ => Err("response must have exactly one of result and error".into()),
    }
}

pub fn request(id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn error_response(id: serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// `initialize` result.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: String,
    pub adapter: InitAdapter,
    #[serde(default)]
    pub fixture: Option<InitFixture>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InitAdapter {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InitFixture {
    /// Fault names (`no_response` …); names this host does not know are ignored.
    #[serde(default)]
    pub faults: Vec<String>,
    /// Whether the adapter sends `fixture.deliveries`.
    #[serde(default)]
    pub deliveries: bool,
}

/// An [`AdapterCommand`] as sent in `execute`.
#[derive(Debug, Serialize)]
pub struct WireCommand<'a> {
    pub command_id: &'a str,
    pub capability: &'a str,
    pub operation: Operation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<&'a Value>,
    pub params: &'a BTreeMap<String, Value>,
    pub idempotent: bool,
    pub deadline: Timestamp,
}

impl<'a> From<&'a AdapterCommand> for WireCommand<'a> {
    fn from(c: &'a AdapterCommand) -> Self {
        Self {
            command_id: &c.command_id,
            capability: &c.capability,
            operation: c.operation,
            value: c.value.as_ref(),
            params: &c.params,
            idempotent: c.idempotent,
            deadline: c.deadline,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WireReport {
    Applied {
        #[serde(default)]
        result: BTreeMap<String, Value>,
    },
    Rejected {
        reason: String,
    },
}

pub fn parse_report(v: serde_json::Value) -> Result<ExecuteReport, String> {
    match serde_json::from_value(v).map_err(|e| e.to_string())? {
        WireReport::Applied { result } => Ok(ExecuteReport::Applied { result }),
        WireReport::Rejected { reason } => Ok(ExecuteReport::Rejected { reason }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireHealth {
    status: String,
    #[serde(default)]
    message: Option<String>,
}

pub fn parse_health(v: serde_json::Value) -> Result<Health, String> {
    let h: WireHealth = serde_json::from_value(v).map_err(|e| e.to_string())?;
    let message = h.message.unwrap_or_default();
    match h.status.as_str() {
        "healthy" => Ok(Health::Healthy),
        "degraded" => Ok(Health::Degraded(message)),
        "disconnected" => Ok(Health::Disconnected(message)),
        other => Err(format!("unknown health status {other:?}")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressParams {
    pub command_id: String,
    pub event: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveriesParams {
    pub count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_and_notifications_parse() {
        assert_eq!(
            parse_incoming(br#"{"jsonrpc":"2.0","id":7,"result":null}"#).unwrap(),
            Incoming::Response {
                id: 7,
                result: Ok(serde_json::Value::Null)
            }
        );
        let Incoming::Response { result: Err(e), .. } = parse_incoming(
            br#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"m","data":{"kind":"adapter.not_sent"}}}"#,
        )
        .unwrap() else {
            panic!("expected an error response")
        };
        assert_eq!(e.kind(), AdapterErrorKind::NotSent);
        assert!(matches!(
            parse_incoming(br#"{"jsonrpc":"2.0","method":"progress","params":{}}"#).unwrap(),
            Incoming::Notification { .. }
        ));
        assert!(matches!(
            parse_incoming(br#"{"jsonrpc":"2.0","id":"x","method":"sample"}"#).unwrap(),
            Incoming::Request { .. }
        ));
    }

    #[test]
    fn violations_are_rejected() {
        for line in [
            &b"hello from print()"[..],
            b"[1,2]",
            br#"{"id":1,"result":null}"#,
            br#"{"jsonrpc":"2.0","id":1}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":1,"error":{"code":1,"message":""}}"#,
            br#"{"jsonrpc":"2.0","id":"1","result":null}"#,
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"bad"}}"#,
        ] {
            assert!(
                parse_incoming(line).is_err(),
                "{}",
                String::from_utf8_lossy(line)
            );
        }
    }

    #[test]
    fn unclassified_errors_are_uncertain() {
        let e = RpcError {
            code: -32603,
            message: "boom".into(),
            data: None,
        };
        assert!(e.kind().outcome_uncertain());
        let e = RpcError {
            code: code::METHOD_NOT_FOUND,
            message: "no".into(),
            data: None,
        };
        assert_eq!(e.kind(), AdapterErrorKind::Unsupported);
        let e = RpcError {
            code: code::ADAPTER_ERROR,
            message: "?".into(),
            data: Some(json!({"kind": "adapter.made_up"})),
        };
        assert_eq!(e.kind(), AdapterErrorKind::Internal);
    }

    #[test]
    fn reports_parse() {
        assert_eq!(
            parse_report(json!({"status": "applied"})).unwrap(),
            ExecuteReport::Applied {
                result: BTreeMap::new()
            }
        );
        assert_eq!(
            parse_report(json!({"status": "rejected", "reason": "tripped"})).unwrap(),
            ExecuteReport::Rejected {
                reason: "tripped".into()
            }
        );
        assert!(parse_report(json!({"status": "ok"})).is_err());
        assert!(parse_report(json!({"status": "rejected"})).is_err());
    }
}
