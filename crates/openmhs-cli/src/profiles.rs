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

/// Starting points for `openmhs profile new`: the tested example profile of each protocol.
const TEMPLATES: [(&str, &str, &str); 4] = [
    (
        openmhs_modbus::PROTOCOL,
        "climate chamber",
        include_str!("../../../adapters/openmhs-modbus/profiles/climate-chamber.json"),
    ),
    (
        openmhs_mqtt::PROTOCOL,
        "greenhouse node",
        include_str!("../../../adapters/openmhs-mqtt/profiles/greenhouse-node.json"),
    ),
    (
        openmhs_scpi::PROTOCOL,
        "bench power supply",
        include_str!("../../../adapters/openmhs-scpi/profiles/bench-psu.json"),
    ),
    (
        openmhs_http::PROTOCOL,
        "LED controller",
        include_str!("../../../adapters/openmhs-http/profiles/led-controller.json"),
    ),
];

/// A protocol's name as `profile new` accepts it: the profile's own name, or `modbus`.
pub fn protocol_named(name: &str) -> Option<&'static str> {
    let name = if name == "modbus" { "modbus-tcp" } else { name };
    TEMPLATES.iter().find(|t| t.0 == name).map(|t| t.0)
}

/// What `profile new` wrote.
pub struct Created {
    pub checked: Checked,
    /// Which example it was copied from.
    pub example: &'static str,
}

/// Write a new profile at `path` from the protocol's example, with `id` as its device id,
/// and check it. Never overwrites; a file that does not check is removed again.
pub fn create(path: &Path, protocol: &str, id: Option<&str>) -> Result<Created, String> {
    let (protocol, example, template) = TEMPLATES
        .iter()
        .find(|t| Some(t.0) == protocol_named(protocol))
        .copied()
        .ok_or_else(|| {
            format!(
                "unknown protocol '{protocol}'; known: {}",
                PROTOCOLS.join(", ")
            )
        })?;
    let text = match id {
        // The device id is the first "id" in every example (descriptor.device.id).
        Some(id) => {
            let key = "\"id\": ";
            let start = template.find(key).ok_or("the template has no device id")? + key.len();
            let len = template[start + 1..]
                .find('"')
                .ok_or("the template has no device id")?
                + 2;
            let quoted = serde_json::to_string(id).map_err(|e| e.to_string())?;
            format!("{}{quoted}{}", &template[..start], &template[start + len..])
        }
        None => template.to_string(),
    };
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                format!("{} already exists; choose another --out", path.display())
            }
            _ => format!("{}: {e}", path.display()),
        })?;
    f.write_all(text.as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    drop(f);
    match check(path) {
        Ok(checked) => {
            debug_assert_eq!(checked.protocol, protocol);
            Ok(Created { checked, example })
        }
        Err(e) => {
            let _ = std::fs::remove_file(path);
            Err(e)
        }
    }
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
    async fn a_new_profile_checks_carries_the_id_and_runs_against_its_simulator() {
        let dir = tempfile::tempdir().unwrap();
        let clock: Arc<dyn Clock> = Arc::new(openmhs_types::SystemClock);
        for (name, protocol) in [
            ("modbus", "modbus-tcp"),
            ("mqtt", "mqtt"),
            ("scpi", "scpi"),
            ("http", "http"),
        ] {
            let path = dir.path().join(format!("{name}.json"));
            let id = format!("lab.{name}-01");
            let c = create(&path, name, Some(&id)).unwrap();
            assert_eq!(c.checked.protocol, protocol);
            assert_eq!(c.checked.device_id, id);
            let o = open(&path, true, clock.clone()).await.unwrap();
            assert_eq!(o.device_id, id);
            o.adapter.connect().await.unwrap();
            // Writing again never overwrites.
            let e = create(&path, name, None).err().unwrap();
            assert!(e.contains("already exists"), "{e}");
        }
    }

    #[test]
    fn a_new_profile_without_an_id_keeps_the_examples_and_bad_input_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        assert_eq!(
            create(&p, "mqtt", None).unwrap().checked.device_id,
            "mqtt.greenhouse-01"
        );
        assert!(
            create(&dir.path().join("x.json"), "zigbee", None)
                .err()
                .unwrap()
                .contains("unknown protocol")
        );
        // An id the descriptor schema refuses: the file is removed, not left half-valid.
        let bad = dir.path().join("bad.json");
        assert!(create(&bad, "http", Some("Not An Id\"")).is_err());
        assert!(!bad.exists());
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
