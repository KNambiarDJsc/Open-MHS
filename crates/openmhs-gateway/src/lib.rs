//! The OpenMHS gateway (ADR-0001): one trusted process that owns the devices, the policy
//! and the journal. The CLI, and later the MCP server and SDKs, are clients of its local
//! API (ADR-0005, ADR-0011).
//!
//! Home directory layout:
//!
//! | File | Contents |
//! |---|---|
//! | `journal.sqlite` | Commands, receipts and the hash-chained audit log |
//! | `policy.json` | Operator policy: grants and approvers |
//! | `tokens.json` | API tokens (SHA-256 hashes only) |
//! | `gateway.json` | Runtime file: URL and operator token of the running gateway |
//! | `gateway.lock` | Held while a gateway runs; one gateway per home |

mod server;
pub mod store;

use std::collections::BTreeMap;
use std::fs::File;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use openmhs_adapter::HardwareAdapter;
use openmhs_api::runtime::{RuntimeInfo, runtime_path};
use openmhs_engine::journal::sha256_hex;
use openmhs_engine::{Engine, EngineError, RecoveryReport};
use openmhs_sim::SimControl;
use openmhs_types::{Clock, SystemClock};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::server::{Session, Shared};
use crate::store::{PolicyStore, TokenStore, new_token, write_private};

pub const DEFAULT_PORT: u16 = 7400;
pub const DEFAULT_OPERATOR: &str = "operator:local";
pub const JOURNAL_FILE: &str = "journal.sqlite";
pub const LOCK_FILE: &str = "gateway.lock";
/// The reserve below a journal size limit, a starting hypothesis for controlled testing
/// (ADR-0021), not a sizing recommendation.
pub const DEFAULT_JOURNAL_RESERVE_BYTES: u64 = 256 * 1024 * 1024;
/// How often devices are probed for availability by default (ADR-0019).
pub const DEFAULT_HEALTH_INTERVAL_MS: u64 = 5_000;

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration: {0}")]
    Config(String),
    #[error("another gateway is already running with home {0}")]
    AlreadyRunning(PathBuf),
    #[error(
        "refusing to listen on non-loopback address {0}; pass --allow-remote if you really mean it"
    )]
    NonLoopback(SocketAddr),
    #[error("engine: {0}")]
    Engine(#[from] EngineError),
}

#[derive(Clone)]
pub struct GatewayConfig {
    pub home: PathBuf,
    pub listen: SocketAddr,
    /// Required to bind anything other than loopback.
    pub allow_remote: bool,
    /// Principal of the operator token written to the runtime file.
    pub operator: String,
    /// Register the virtual lab (sensor, power supply, linear stage) and allow fault
    /// injection.
    pub simulation: bool,
    pub seed: u64,
    /// Adapters to register at start, for programs that embed the gateway with their own
    /// devices (docs/adapters/). Registered after the virtual lab, if any.
    pub adapters: Vec<Arc<dyn HardwareAdapter>>,
    /// The operator's topology (spec/topology.md), applied once every device is
    /// registered. A topology naming a device that is not registered stops the start.
    pub topology: Option<openmhs_types::Topology>,
    /// Refuse to start unless every device answers. Off by default: a device that does not
    /// answer is registered `OFFLINE` and comes online when it does (ADR-0019).
    pub require_devices: bool,
    /// How often to probe devices for availability, in milliseconds. `0` turns the probes
    /// off; commands to an `OFFLINE` device still probe it before they are refused.
    pub health_interval_ms: u64,
    /// How far behind the gateway's clock a device's reading may be stamped and still count
    /// as taken after a command was sent (ADR-0020). Zero accepts nothing stamped earlier.
    pub readback_skew_ms: u64,
    /// A hard limit on the journal's size in bytes (ADR-0021). `None`: unlimited.
    pub journal_max_bytes: Option<u64>,
    /// With a limit set, the reserve below it kept for critical writes.
    pub journal_reserve_bytes: u64,
}

impl std::fmt::Debug for GatewayConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayConfig")
            .field("home", &self.home)
            .field("listen", &self.listen)
            .field("allow_remote", &self.allow_remote)
            .field("operator", &self.operator)
            .field("simulation", &self.simulation)
            .field("seed", &self.seed)
            .field("require_devices", &self.require_devices)
            .field("health_interval_ms", &self.health_interval_ms)
            .field("readback_skew_ms", &self.readback_skew_ms)
            .field("journal_max_bytes", &self.journal_max_bytes)
            .field(
                "adapters",
                &self
                    .adapters
                    .iter()
                    .map(|a| a.adapter_id())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl GatewayConfig {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
            allow_remote: false,
            operator: DEFAULT_OPERATOR.into(),
            simulation: false,
            seed: 42,
            adapters: Vec::new(),
            topology: None,
            require_devices: false,
            health_interval_ms: DEFAULT_HEALTH_INTERVAL_MS,
            readback_skew_ms: 0,
            journal_max_bytes: None,
            journal_reserve_bytes: DEFAULT_JOURNAL_RESERVE_BYTES,
        }
    }
}

pub struct RunningGateway {
    pub url: String,
    pub addr: SocketAddr,
    pub operator_token: String,
    pub recovery: RecoveryReport,
    engine: Engine,
    home: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server: JoinHandle<std::io::Result<()>>,
    /// The availability probes, if on.
    monitor: Option<JoinHandle<()>>,
    _lock: File,
}

impl RunningGateway {
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Stop serving, shut adapters down and remove the runtime file.
    pub async fn shutdown(mut self) -> Result<(), GatewayError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
        }
        match (&mut self.server).await {
            Ok(r) => r?,
            Err(e) => warn!(error = %e, "server task ended abnormally"),
        }
        self.engine.shutdown_adapters().await;
        let _ = self.engine.journal().append_audit(
            SystemClock.now(),
            "gateway.stopped",
            "gateway",
            &serde_json::json!({}),
        );
        let _ = std::fs::remove_file(runtime_path(&self.home));
        info!("gateway stopped");
        Ok(())
    }
}

/// Start a gateway: lock the home, open the journal and policy, register devices, run
/// crash recovery, then serve the API and publish the runtime file.
pub async fn start(config: GatewayConfig) -> Result<RunningGateway, GatewayError> {
    if !config.listen.ip().is_loopback() {
        if !config.allow_remote {
            return Err(GatewayError::NonLoopback(config.listen));
        }
        warn!(addr = %config.listen, "listening on a non-loopback address; anyone who can reach it and holds a token can control devices");
    }
    if !openmhs_types::is_principal(&config.operator) {
        return Err(GatewayError::Config(format!(
            "operator principal '{}' must be a lowercase name with an optional kind, e.g. operator:local",
            config.operator
        )));
    }
    std::fs::create_dir_all(&config.home)?;
    let lock = File::create(config.home.join(LOCK_FILE))?;
    if lock.try_lock().is_err() {
        return Err(GatewayError::AlreadyRunning(config.home.clone()));
    }

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (policy_store, policy) = PolicyStore::open(&config.home, &config.operator)?;
    let tokens = TokenStore::open(&config.home, &config.operator)?;
    let engine = Engine::with_journal(
        openmhs_engine::Journal::open(&config.home.join(JOURNAL_FILE))
            .map_err(EngineError::from)?,
        clock.clone(),
        policy,
        openmhs_engine::EngineOptions {
            readback_skew_ms: config.readback_skew_ms,
            ..openmhs_engine::EngineOptions::default()
        },
    );
    if let Some(max_bytes) = config.journal_max_bytes {
        engine
            .set_storage_limits(openmhs_engine::StorageLimits {
                max_bytes,
                reserve_bytes: config.journal_reserve_bytes,
            })
            .map_err(|e| GatewayError::Config(format!("journal limits: {e}")))?;
    }
    engine
        .journal()
        .append_audit(
            clock.now(),
            "gateway.started",
            "gateway",
            &serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "simulation": config.simulation,
                "listen": config.listen.to_string(),
            }),
        )
        .map_err(EngineError::from)?;

    let mut sims: BTreeMap<String, Arc<dyn SimControl>> = BTreeMap::new();
    if config.simulation {
        for sim in openmhs_sim::virtual_lab(clock.clone(), config.seed) {
            let d = engine.register(sim.adapter).await?;
            sims.insert(d.device.id, sim.control);
        }
    }
    // A device that is switched off does not keep the others, or crash recovery, from
    // starting (ADR-0019); `require_devices` restores the strict start.
    if config.require_devices {
        for adapter in &config.adapters {
            engine.register(adapter.clone()).await?;
        }
    } else {
        // Each switched-off device costs up to a probe timeout. One at a time that is
        // n timeouts before the gateway listens; a bounded number at a time it is about
        // n / MAX_CONCURRENT_PROBES.
        let limit = Arc::new(tokio::sync::Semaphore::new(
            openmhs_engine::MAX_CONCURRENT_PROBES,
        ));
        let mut registering = tokio::task::JoinSet::new();
        for adapter in config.adapters.iter().cloned() {
            let (engine, limit) = (engine.clone(), limit.clone());
            registering.spawn(async move {
                let _permit = limit.acquire_owned().await;
                engine.register_allow_offline(adapter).await
            });
        }
        while let Some(done) = registering.join_next().await {
            match done {
                Ok(registered) => {
                    registered?;
                }
                Err(join) => {
                    return Err(GatewayError::Config(format!(
                        "registering a device failed: {join}"
                    )));
                }
            }
        }
    }
    if let Some(topology) = config.topology.clone() {
        engine.set_topology(topology).map_err(|e| match e {
            EngineError::InvalidRequest(issues) => GatewayError::Config(format!(
                "topology: {}",
                issues
                    .iter()
                    .map(|i| format!("[{}] {}", i.code, i.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
            other => other.into(),
        })?;
    }
    let recovery = engine.recover().await?;
    if !recovery.unknown_outcome.is_empty() {
        warn!(commands = ?recovery.unknown_outcome, "commands were in flight when the gateway stopped; they are UNKNOWN_OUTCOME and need reconciliation");
    }

    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}");
    let operator_token = new_token()?;
    let shared = Arc::new(Shared {
        engine: engine.clone(),
        clock: clock.clone(),
        simulation: config.simulation,
        operator: Session::operator(&config.operator),
        operator_token_sha256: sha256_hex(&operator_token),
        tokens: Mutex::new(tokens),
        reported: Mutex::new(std::collections::HashSet::new()),
        policy_store: tokio::sync::Mutex::new(policy_store),
        sims,
    });
    let monitor = (config.health_interval_ms > 0).then(|| {
        let engine = engine.clone();
        let every = std::time::Duration::from_millis(config.health_interval_ms);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                // Devices that something else reached recently are not probed again.
                if let Err(e) = engine.sweep_availability(config.health_interval_ms).await {
                    warn!(error = %e, "availability probe failed");
                }
            }
        })
    });
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let app = server::router(shared);
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let runtime = RuntimeInfo {
        url: url.clone(),
        pid: std::process::id(),
        principal: config.operator.clone(),
        token: operator_token.clone(),
        started_at: clock.now(),
    };
    let text =
        serde_json::to_string_pretty(&runtime).map_err(|e| GatewayError::Config(e.to_string()))?;
    write_private(&runtime_path(&config.home), &text)?;
    info!(%url, home = %config.home.display(), "gateway listening");

    Ok(RunningGateway {
        url,
        addr,
        operator_token,
        recovery,
        engine,
        home: config.home,
        shutdown_tx: Some(shutdown_tx),
        server,
        monitor,
        _lock: lock,
    })
}
