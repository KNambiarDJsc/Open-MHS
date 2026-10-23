//! An OpenMHS adapter for Modbus TCP devices (PLCs, controllers, meters, drives), driven
//! by a **register-map profile**: which coil or register holds each property, how its
//! raw value is scaled, and what each action writes.
//!
//! Each command is one Modbus write, sent once. An exception reply means the device
//! refused it (`REJECTED`); no reply, a dropped connection, or a reply that does not
//! match the request leave the outcome uncertain (`UNKNOWN_OUTCOME`), and the gateway
//! reconciles by reading back. See `docs/adapters/modbus.md`.

pub mod register;
pub mod sim;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use openmhs_adapter::{
    AdapterCommand, AdapterError, AdapterErrorKind, ExecuteReport, HardwareAdapter, Health,
    Progress,
};
use openmhs_profile::{Loaded, ProfileError, codec};
use openmhs_types::descriptor::Operation;
use openmhs_types::{Clock, Quality, Source, StateValue, Value};
use serde::Deserialize;
use tokio_modbus::client::{Client, Context};
use tokio_modbus::{ExceptionCode, Request, Response, Slave};

pub use register::{Register, Table};

/// The `protocol` of Modbus TCP profiles.
pub const PROTOCOL: &str = "modbus-tcp";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    /// `host:port`; Modbus TCP uses port 502.
    pub address: String,
    /// The unit identifier; 1 for most devices, or the downstream device behind a gateway.
    #[serde(default = "default_unit")]
    pub unit_id: u8,
    /// How long to wait for any one reply.
    #[serde(default = "default_reply_timeout")]
    pub reply_timeout_ms: u64,
}

fn default_unit() -> u8 {
    1
}
fn default_reply_timeout() -> u64 {
    1000
}

pub type Profile = Loaded<Connection, Register>;

/// Parse and validate a Modbus TCP profile.
pub fn load(json: &str) -> Result<Profile, ProfileError> {
    openmhs_profile::load(json, PROTOCOL)
}

/// How one Modbus transaction failed.
#[derive(Debug)]
enum Failure {
    /// Could not connect: nothing was sent.
    Down(String),
    /// The connection broke after the request may have left.
    Lost(String),
    Timeout,
    /// An answer that does not fit the request.
    Garbled(String),
    /// The device answered with an exception.
    Exception(ExceptionCode),
}

pub struct ModbusAdapter {
    profile: Profile,
    address: String,
    source: Source,
    clock: Arc<dyn Clock>,
    conn: tokio::sync::Mutex<Option<Context>>,
}

impl ModbusAdapter {
    /// An adapter for `profile`, at the profile's address.
    pub fn new(profile: Profile, clock: Arc<dyn Clock>) -> Self {
        let address = profile.connection.address.clone();
        Self {
            profile,
            address,
            source: Source::Device,
            clock,
            conn: tokio::sync::Mutex::new(None),
        }
    }

    /// Connect somewhere else than the profile says, e.g. a simulator.
    #[must_use]
    pub fn at(mut self, address: impl Into<String>) -> Self {
        self.address = address.into();
        self
    }

    /// Mark readings as coming from a simulator rather than hardware.
    #[must_use]
    pub fn simulated(mut self) -> Self {
        self.source = Source::Simulator;
        self
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.profile.connection.reply_timeout_ms)
    }

    /// One transaction. The connection is dropped after anything unexpected, so a late
    /// or stray answer can never be mistaken for the next one's.
    async fn call(&self, request: Request<'static>) -> Result<Response, Failure> {
        let mut guard = self.conn.lock().await;
        if guard.is_none() {
            let unit = Slave(self.profile.connection.unit_id);
            let connect = async {
                let addr = tokio::net::lookup_host(&self.address)
                    .await?
                    .next()
                    .ok_or_else(|| {
                        std::io::Error::other(format!("'{}' does not resolve", self.address))
                    })?;
                tokio_modbus::client::tcp::connect_slave(addr, unit).await
            };
            let ctx = tokio::time::timeout(self.timeout(), connect)
                .await
                .map_err(|_| Failure::Down(format!("connecting to {} timed out", self.address)))?
                .map_err(|e| Failure::Down(format!("cannot connect to {}: {e}", self.address)))?;
            *guard = Some(ctx);
        }
        let Some(ctx) = guard.as_mut() else {
            return Err(Failure::Down("not connected".into()));
        };
        let result = tokio::time::timeout(self.timeout(), ctx.call(request)).await;
        let failure = match result {
            Ok(Ok(Ok(response))) => return Ok(response),
            Ok(Ok(Err(exception))) => return Err(Failure::Exception(exception)),
            Err(_) => Failure::Timeout,
            Ok(Err(tokio_modbus::Error::Protocol(e))) => Failure::Garbled(e.to_string()),
            Ok(Err(tokio_modbus::Error::Transport(e)))
                if e.kind() == std::io::ErrorKind::InvalidData =>
            {
                Failure::Garbled(e.to_string())
            }
            Ok(Err(tokio_modbus::Error::Transport(e))) => Failure::Lost(e.to_string()),
        };
        *guard = None;
        Err(failure)
    }

    async fn read_value(
        &self,
        reg: &Register,
        spec: &openmhs_types::descriptor::ValueSpec,
    ) -> Result<Value, AdapterError> {
        let n = reg.width();
        let request = match reg.table {
            Table::Coil => Request::ReadCoils(reg.address, n),
            Table::DiscreteInput => Request::ReadDiscreteInputs(reg.address, n),
            Table::Input => Request::ReadInputRegisters(reg.address, n),
            Table::Holding => Request::ReadHoldingRegisters(reg.address, n),
        };
        let malformed = |m: String| AdapterError::new(AdapterErrorKind::MalformedResponse, m);
        let response = self.call(request).await.map_err(|f| match f {
            Failure::Down(m) | Failure::Lost(m) => {
                AdapterError::new(AdapterErrorKind::Disconnected, m)
            }
            Failure::Timeout => AdapterError::new(AdapterErrorKind::Timeout, "no reply in time"),
            Failure::Garbled(m) => malformed(m),
            Failure::Exception(e) => AdapterError::new(
                AdapterErrorKind::DeviceFailure,
                format!("the device answered with exception {e:?}"),
            ),
        })?;
        let x = match response {
            Response::ReadCoils(bits) | Response::ReadDiscreteInputs(bits) => match bits.first() {
                Some(b) => f64::from(u8::from(*b)),
                None => return Err(malformed("no bits in the reply".into())),
            },
            Response::ReadInputRegisters(words) | Response::ReadHoldingRegisters(words) => {
                reg.decode(&words).map_err(malformed)?
            }
            other => return Err(malformed(format!("unexpected reply {other:?}"))),
        };
        codec::from_number(spec, x).map_err(|e| malformed(format!("register {}: {e}", reg.address)))
    }

    /// The single write that carries `command`.
    fn write_request(
        &self,
        command: &AdapterCommand,
    ) -> Result<(Register, Request<'static>), AdapterError> {
        let not_sent = |m: String| AdapterError::new(AdapterErrorKind::NotSent, m);
        let (cap, reg) = self.profile.get(&command.capability).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Unsupported,
                format!("no capability '{}'", command.capability),
            )
        })?;
        let x = match (command.operation, &command.value, &reg.write, &reg.param) {
            (Operation::Write, Some(v), _, _) => {
                let spec = reg
                    .spec(cap)
                    .ok_or_else(|| not_sent("no value spec".into()))?;
                codec::to_number(spec, v).map_err(not_sent)?
            }
            (Operation::Invoke, _, Some(fixed), _) => {
                if let Some(b) = fixed.as_bool() {
                    f64::from(u8::from(b))
                } else {
                    // A raw value: written as-is, without scale or offset.
                    let raw = fixed
                        .as_f64()
                        .ok_or_else(|| not_sent("bad fixed value".into()))?;
                    raw * reg.scale + reg.offset
                }
            }
            (Operation::Invoke, _, None, Some(p)) => {
                let v = command
                    .params
                    .get(p)
                    .ok_or_else(|| not_sent(format!("missing parameter '{p}'")))?;
                let spec = reg
                    .spec(cap)
                    .ok_or_else(|| not_sent("no parameter spec".into()))?;
                codec::to_number(spec, v).map_err(not_sent)?
            }
            _ => {
                return Err(not_sent(format!(
                    "cannot {:?} '{}'",
                    command.operation, command.capability
                )));
            }
        };
        let request = match reg.table {
            Table::Coil => Request::WriteSingleCoil(reg.address, x != 0.0),
            Table::Holding => {
                let words = reg.encode(x).map_err(not_sent)?;
                match words.as_slice() {
                    [w] => Request::WriteSingleRegister(reg.address, *w),
                    _ => Request::WriteMultipleRegisters(reg.address, Cow::Owned(words)),
                }
            }
            Table::DiscreteInput | Table::Input => return Err(not_sent("read-only table".into())),
        };
        Ok((reg.clone(), request))
    }
}

/// Whether a write reply echoes the request, as Modbus requires.
fn echoes(request: &Request<'_>, response: &Response) -> bool {
    match (request, response) {
        (Request::WriteSingleCoil(a, v), Response::WriteSingleCoil(b, w)) => a == b && v == w,
        (Request::WriteSingleRegister(a, v), Response::WriteSingleRegister(b, w)) => {
            a == b && v == w
        }
        (Request::WriteMultipleRegisters(a, ws), Response::WriteMultipleRegisters(b, n)) => {
            a == b && usize::from(*n) == ws.len()
        }
        _ => false,
    }
}

#[async_trait]
impl HardwareAdapter for ModbusAdapter {
    fn adapter_id(&self) -> &str {
        &self.profile.descriptor.adapter.id
    }

    async fn describe(&self) -> Result<serde_json::Value, AdapterError> {
        Ok(self.profile.descriptor_json.clone())
    }

    async fn connect(&self) -> Result<(), AdapterError> {
        // Prove the device answers by reading the first readable property.
        match self
            .profile
            .descriptor
            .capabilities
            .iter()
            .find(|c| c.is_readable_property())
        {
            Some(cap) => self.read(&cap.name).await.map(|_| ()),
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
        let (cap, reg) = self.profile.get(capability).ok_or_else(unsupported)?;
        let spec = cap.value.as_ref().ok_or_else(unsupported)?;
        let value = self.read_value(reg, spec).await?;
        Ok(StateValue {
            value,
            timestamp: self.clock.now(),
            source: self.source,
            quality: Quality::Good,
        })
    }

    async fn execute(
        &self,
        command: AdapterCommand,
        progress: Progress,
    ) -> Result<ExecuteReport, AdapterError> {
        let (_, request) = self.write_request(&command)?;
        let sent = request.clone();
        // Sent once. Whatever happens next is reported, never retried.
        match self.call(request).await {
            Ok(response) if echoes(&sent, &response) => {
                progress.accepted();
                Ok(ExecuteReport::Applied {
                    result: BTreeMap::new(),
                })
            }
            Ok(response) => Err(AdapterError::new(
                AdapterErrorKind::MalformedResponse,
                format!("the reply {response:?} does not echo the write"),
            )),
            Err(Failure::Down(m)) => Err(AdapterError::new(AdapterErrorKind::NotSent, m)),
            Err(Failure::Lost(m)) => Err(AdapterError::new(AdapterErrorKind::Disconnected, m)),
            Err(Failure::Timeout) => Err(AdapterError::new(
                AdapterErrorKind::Timeout,
                "no reply in time",
            )),
            Err(Failure::Garbled(m)) => {
                Err(AdapterError::new(AdapterErrorKind::MalformedResponse, m))
            }
            Err(Failure::Exception(e)) => exception(e, &progress),
        }
    }

    async fn shutdown(&self) -> Result<(), AdapterError> {
        if let Some(mut ctx) = self.conn.lock().await.take() {
            let _ = ctx.disconnect().await;
        }
        Ok(())
    }
}

/// What a Modbus exception says about a write.
fn exception(e: ExceptionCode, progress: &Progress) -> Result<ExecuteReport, AdapterError> {
    let rejected = |why: &str| {
        Ok(ExecuteReport::Rejected {
            reason: format!("{why} (exception {e:?})"),
        })
    };
    match e {
        ExceptionCode::IllegalFunction => rejected("the device does not support this write"),
        ExceptionCode::IllegalDataAddress => rejected("the device has no such register"),
        ExceptionCode::IllegalDataValue => rejected("the device refused the value"),
        ExceptionCode::ServerDeviceBusy => rejected("the device is busy"),
        ExceptionCode::GatewayPathUnavailable | ExceptionCode::GatewayTargetDevice => {
            Err(AdapterError::new(
                AdapterErrorKind::NotSent,
                format!("a Modbus gateway could not reach the device (exception {e:?})"),
            ))
        }
        // Accepted and still in progress: readback verification decides.
        ExceptionCode::Acknowledge => {
            progress.accepted();
            Ok(ExecuteReport::Applied {
                result: BTreeMap::new(),
            })
        }
        ExceptionCode::ServerDeviceFailure => Err(AdapterError::new(
            AdapterErrorKind::DeviceFailure,
            "the device reported a failure while carrying out the write (exception ServerDeviceFailure)",
        )),
        other => Err(AdapterError::new(
            AdapterErrorKind::MalformedResponse,
            format!("unexpected exception {other:?}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_profile_loads_and_bad_mappings_are_named() {
        load(include_str!("../profiles/climate-chamber.json")).unwrap();
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../profiles/climate-chamber.json")).unwrap();
        v["mapping"]["heater.setpoint"]["table"] = serde_json::json!("input");
        v["mapping"]["door.closed"]["scale"] = serde_json::json!(2);
        let err = load(&v.to_string()).unwrap_err().to_string();
        assert!(
            err.contains(
                "/mapping/heater.setpoint: only coils and holding registers can be written"
            ),
            "{err}"
        );
        assert!(
            err.contains("/mapping/door.closed: coils and discrete inputs are single bits"),
            "{err}"
        );
    }
}
