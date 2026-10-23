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

use openmhs_api::Role;
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

#[derive(Debug, Clone)]
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
    let tokens = TokenStore::open(&config.home)?;
    let engine = Engine::open(&config.home.join(JOURNAL_FILE), clock.clone(), policy)?;
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
        operator: Session {
            principal: config.operator.clone(),
            role: Role::Operator,
        },
        operator_token_sha256: sha256_hex(&operator_token),
        tokens: Mutex::new(tokens),
        policy_store: tokio::sync::Mutex::new(policy_store),
        sims,
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
        _lock: lock,
    })
}
