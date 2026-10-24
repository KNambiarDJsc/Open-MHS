//! Devices described by protocol profiles (docs/adapters/profiles.md): the adapter for
//! the real device, or the profile's simulator with the adapter pointed at it.

use std::path::Path;
use std::sync::Arc;

use openmhs_adapter::HardwareAdapter;
use openmhs_profile::{ProfileError, VirtualDevice, protocol_of};
use openmhs_types::Clock;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The protocols a profile can name.
pub const PROTOCOLS: [&str; 4] = [
    openmhs_modbus::PROTOCOL,
    openmhs_mqtt::PROTOCOL,
    openmhs_scpi::PROTOCOL,
    openmhs_http::PROTOCOL,
];

/// A device opened from a profile.
pub struct Opened {
    pub adapter: Arc<dyn HardwareAdapter>,
    pub device_id: String,
    pub protocol: String,
    /// Where the adapter connects.
    pub target: String,
    /// The simulated device, when the profile's simulator was started.
    pub fixture: Option<Arc<VirtualDevice>>,
    /// The simulator's server task.
    pub server: Option<JoinHandle<()>>,
}

impl Drop for Opened {
    fn drop(&mut self) {
        if let Some(s) = self.server.take() {
            s.abort();
        }
    }
}

fn invalid(path: &Path, e: &ProfileError) -> String {
    format!("{}: {e}", path.display())
}

/// What a valid profile describes.
pub struct Checked {
    pub protocol: String,
    pub device_id: String,
    pub capabilities: usize,
}

/// Read and validate a profile without opening anything.
pub fn check(path: &Path) -> Result<Checked, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let protocol = protocol_of(&text).map_err(|e| invalid(path, &e))?;
    let descriptor = match protocol.as_str() {
        openmhs_modbus::PROTOCOL => openmhs_modbus::load(&text).map(|p| p.descriptor),
        openmhs_mqtt::PROTOCOL => openmhs_mqtt::load(&text).map(|p| p.descriptor),
        openmhs_scpi::PROTOCOL => openmhs_scpi::load(&text).map(|p| p.descriptor),
        openmhs_http::PROTOCOL => openmhs_http::load(&text).map(|p| p.descriptor),
        other => return Err(unknown(path, other)),
    }
    .map_err(|e| invalid(path, &e))?;
    Ok(Checked {
        protocol,
        device_id: descriptor.device.id,
        capabilities: descriptor.capabilities.len(),
    })
}

fn unknown(path: &Path, protocol: &str) -> String {
    format!(
        "{}: unknown protocol '{protocol}'; known: {}",
        path.display(),
        PROTOCOLS.join(", ")
    )
}

/// Open the device a profile describes. With `simulate`, start the profile's simulator
/// on a free loopback port and point the adapter at it instead.
pub async fn open(path: &Path, simulate: bool, clock: Arc<dyn Clock>) -> Result<Opened, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let protocol = protocol_of(&text).map_err(|e| invalid(path, &e))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let listener = if simulate {
        let l = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("cannot start a simulator: {e}"))?;
        let addr = l.local_addr().map_err(|e| e.to_string())?;
        Some((l, addr))
    } else {
        None
    };
    let mut fixture = None;
    let mut server = None;
    let (adapter, device_id, target): (Arc<dyn HardwareAdapter>, String, String) =
        match protocol.as_str() {
            openmhs_modbus::PROTOCOL => {
                let p = openmhs_modbus::load(&text)
                    .map_err(|e| invalid(path, &e))?
                    .in_dir(dir);
                let id = p.descriptor.device.id.clone();
                let mut a = openmhs_modbus::ModbusAdapter::new(p.clone(), clock);
                let target = match listener {
                    Some((l, addr)) => {
                        let sim = openmhs_modbus::sim::ModbusSim::new(p);
                        fixture = Some(sim.device());
                        server = Some(tokio::spawn(sim.serve(l)));
                        a = a.at(addr.to_string()).simulated();
                        addr.to_string()
                    }
                    None => p.connection.address.clone(),
                };
                (Arc::new(a), id, target)
            }
            openmhs_mqtt::PROTOCOL => {
                let p = openmhs_mqtt::load(&text)
                    .map_err(|e| invalid(path, &e))?
                    .in_dir(dir);
                let id = p.descriptor.device.id.clone();
                let mut a = openmhs_mqtt::MqttAdapter::new(p.clone(), clock)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let target = match listener {
                    Some((l, addr)) => {
                        let sim = openmhs_mqtt::sim::MqttSim::new(p);
                        fixture = Some(sim.device());
                        server = Some(tokio::spawn(sim.serve(l)));
                        a = a.at(addr.ip().to_string(), addr.port()).simulated();
                        addr.to_string()
                    }
                    None => format!("{}:{}", p.connection.host, p.connection.port),
                };
                (Arc::new(a), id, target)
            }
            openmhs_scpi::PROTOCOL => {
                use openmhs_scpi::link::StreamLink;
                let p = openmhs_scpi::load(&text)
                    .map_err(|e| invalid(path, &e))?
                    .in_dir(dir);
                let id = p.descriptor.device.id.clone();
                let (a, target) = match listener {
                    Some((l, addr)) => {
                        let sim = openmhs_scpi::sim::ScpiSim::new(p.clone());
                        fixture = Some(sim.device());
                        server = Some(tokio::spawn(sim.serve_tcp(l)));
                        let link = Arc::new(StreamLink::tcp(addr.to_string()).simulated());
                        let a = openmhs_scpi::ScpiAdapter::with_link(p, link, clock);
                        (a, addr.to_string())
                    }
                    None => {
                        let target = match &p.connection.transport {
                            openmhs_scpi::Transport::Tcp { address } => address.clone(),
                            openmhs_scpi::Transport::Serial { port, baud } => {
                                format!("{port} at {baud} baud")
                            }
                        };
                        (openmhs_scpi::ScpiAdapter::new(p, clock), target)
                    }
                };
                (Arc::new(a), id, target)
            }
            openmhs_http::PROTOCOL => {
                let p = openmhs_http::load(&text)
                    .map_err(|e| invalid(path, &e))?
                    .in_dir(dir);
                let id = p.descriptor.device.id.clone();
                let mut a = openmhs_http::HttpAdapter::new(p.clone(), clock)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let target = match listener {
                    Some((l, addr)) => {
                        let sim = openmhs_http::sim::HttpSim::new(p);
                        fixture = Some(sim.device());
                        server = Some(tokio::spawn(sim.serve(l)));
                        a = a.at(format!("http://{addr}")).simulated();
                        format!("http://{addr}")
                    }
                    None => p.connection.base_url.clone(),
                };
                (Arc::new(a), id, target)
            }
            other => return Err(unknown(path, other)),
        };
    Ok(Opened {
        adapter,
        device_id,
        protocol,
        target,
        fixture,
        server,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn examples() -> Vec<(PathBuf, &'static str, &'static str)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters");
        [
            (
                "openmhs-modbus/profiles/climate-chamber.json",
                "modbus-tcp",
                "chamber.temperature",
            ),
            (
                "openmhs-mqtt/profiles/greenhouse-node.json",
                "mqtt",
                "air.temperature",
            ),
            (
                "openmhs-scpi/profiles/bench-psu.json",
                "scpi",
                "psu.voltage.setpoint",
            ),
            (
                "openmhs-http/profiles/led-controller.json",
                "http",
                "sensor.temperature",
            ),
        ]
        .into_iter()
        .map(|(f, p, c)| (root.join(f), p, c))
        .collect()
    }

    #[test]
    fn every_example_profile_checks() {
        for (path, protocol, _) in examples() {
            let c = check(&path).unwrap();
            assert_eq!(c.protocol, protocol);
            assert!(c.capabilities > 0);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn every_protocol_opens_against_its_simulator() {
        let clock: Arc<dyn Clock> = Arc::new(openmhs_types::SystemClock);
        for (path, protocol, readable) in examples() {
            let o = open(&path, true, clock.clone()).await.unwrap();
            assert_eq!(o.protocol, protocol);
            assert!(o.fixture.is_some() && o.target.contains("127.0.0.1"));
            o.adapter.connect().await.unwrap();
            let v = o.adapter.read(readable).await.unwrap();
            assert_eq!(v.source, openmhs_types::Source::Simulator, "{protocol}");
        }
    }
}
