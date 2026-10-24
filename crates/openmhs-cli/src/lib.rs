//! The `openmhs` CLI and the `openmhsd` gateway binary (docs/cli.md).
//!
//! Every command except `serve` and `demo` is a client of a running gateway; the CLI
//! never touches devices, policy or the journal directly (ADR-0001).

mod demo;
mod output;
mod profiles;
pub mod scaffold;
mod values;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use openmhs_adapter::HardwareAdapter;
use openmhs_api::runtime::default_home;
use openmhs_api::{Client, ClientError, CreateTokenParams, Role};
use openmhs_gateway::{DEFAULT_OPERATOR, DEFAULT_PORT, GatewayConfig};
use openmhs_policy::{Effect, Grant, Scope};
use openmhs_types::descriptor::{CapabilityKind, Operation, SideEffect};
use openmhs_types::receipt::Decision;
use openmhs_types::{CommandState, Outcome, Receipt};
use serde::Serialize;

/// Exit codes (docs/cli.md). Scripts and agents can branch on these.
pub mod exit {
    pub const OK: u8 = 0;
    pub const ERROR: u8 = 1;
    pub const DENIED: u8 = 2;
    pub const FAILED: u8 = 3;
    pub const UNCERTAIN: u8 = 4;
    pub const PENDING: u8 = 5;
    pub const UNVERIFIED: u8 = 6;
    pub const NOT_EXECUTED: u8 = 7;
}

const ABOUT: &str = "OpenMHS: a supervisory gateway between AI agents and programmable hardware.\n\
Independent open-source project; not affiliated with Anthropic.";

#[derive(Parser)]
#[command(name = "openmhs", version, about = ABOUT)]
struct Cli {
    /// Gateway home directory (journal, policy, tokens, runtime file).
    #[arg(long, global = true, env = "OPENMHS_HOME")]
    home: Option<PathBuf>,
    /// Print the raw API result as JSON.
    #[arg(long, global = true)]
    json: bool,
    /// API token to use instead of the operator token from the runtime file.
    #[arg(long, global = true, env = "OPENMHS_TOKEN", hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the gateway in the foreground (same as `openmhsd`).
    Serve(ServeArgs),
    /// Start a throwaway gateway with the virtual lab and walk through a guided scenario.
    Demo(DemoArgs),
    /// Show who you are to the gateway and what it is running.
    Info,
    /// List or inspect registered devices.
    Devices {
        #[command(subcommand)]
        command: DevicesCmd,
    },
    /// Read a property from the device (authorized and audited).
    Read { device: String, capability: String },
    /// Submit a write (properties) or invoke (actions) and wait for its receipt.
    Exec(ExecArgs),
    /// Inspect and manage commands.
    Commands {
        #[command(subcommand)]
        command: CommandsCmd,
    },
    /// Approve a command waiting in AWAITING_APPROVAL (operator).
    Approve { command_id: String },
    /// Manage authorization grants (operator).
    Grants {
        #[command(subcommand)]
        command: GrantsCmd,
    },
    /// Manage API tokens (operator).
    Tokens {
        #[command(subcommand)]
        command: TokensCmd,
    },
    /// Inspect and verify the audit log (operator).
    Audit {
        #[command(subcommand)]
        command: AuditCmd,
    },
    /// Fault injection for simulated devices (operator, `serve --sim` only).
    Sim {
        #[command(subcommand)]
        command: SimCmd,
    },
    /// Serve the Model Context Protocol on stdin/stdout for an AI agent (docs/mcp.md).
    Mcp(McpArgs),
    /// Run the conformance suite against the built-in simulators, or against a protocol
    /// profile's simulator (spec/conformance.md).
    Conformance(ConformanceArgs),
    /// Protocol profiles: Modbus TCP, MQTT, SCPI and HTTP devices (docs/adapters/profiles.md).
    Profile {
        #[command(subcommand)]
        command: ProfileCmd,
    },
    /// Create adapters (docs/adapters/).
    Adapter {
        #[command(subcommand)]
        command: AdapterCmd,
    },
}

#[derive(Args)]
struct ConformanceArgs {
    /// Simulator to test, repeatable. Default: all of them, unless --profile or
    /// --adapter-manifest is given.
    #[arg(long = "device")]
    devices: Vec<String>,
    /// Test a protocol profile's adapter against the profile's simulator, over real
    /// loopback sockets. Repeatable. Never touches the real device.
    #[arg(long = "profile")]
    profiles: Vec<PathBuf>,
    /// Test an out-of-process adapter from its permission manifest, in fixture mode:
    /// the adapter drives its own simulator. Refused if the adapter offers no fixture.
    /// Repeatable (docs/architecture/adapter-host.md).
    #[arg(long = "adapter-manifest")]
    manifests: Vec<PathBuf>,
    /// Write each report as <device>.json into this directory.
    #[arg(long)]
    report_dir: Option<PathBuf>,
    /// Seed for simulator noise.
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Validate profiles without connecting to anything.
    Check {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

#[derive(Subcommand)]
enum AdapterCmd {
    /// Create an adapter crate from the template: a working example to make your own.
    New {
        /// Crate and adapter name: lowercase letters, digits and hyphens, e.g. bench-psu.
        name: String,
        /// Directory to create (default: ./<name>). Must not exist, or be empty.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Use the OpenMHS crates from this local checkout instead of the repository's
        /// main branch.
        #[arg(long)]
        openmhs_path: Option<PathBuf>,
    },
    /// Validate adapter permission manifests without starting anything
    /// (spec/permission-manifest.md).
    Check {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

#[derive(Args)]
struct McpArgs {
    /// Gateway URL. Defaults to the one in the running gateway's runtime file.
    #[arg(long)]
    url: Option<String>,
    /// Allow running with an operator token, which can approve commands and change
    /// policy. Never give a model that power unless you understand the risk.
    #[arg(long)]
    allow_operator: bool,
}

#[derive(Args, Clone)]
pub struct ServeArgs {
    /// Register the virtual lab: temperature sensor, power supply, linear stage.
    #[arg(long)]
    sim: bool,
    /// Register the device a protocol profile describes. This connects to the real
    /// device at the profile's address. Repeatable (docs/adapters/profiles.md).
    #[arg(long = "profile")]
    profiles: Vec<PathBuf>,
    /// Register a protocol profile's simulated device instead of the real one: its
    /// simulator runs in this process on a loopback port. Repeatable.
    #[arg(long = "profile-sim")]
    profile_sims: Vec<PathBuf>,
    /// Run an adapter as a separate process from its permission manifest. It connects
    /// to whatever its manifest configures. Repeatable (docs/architecture/adapter-host.md).
    #[arg(long = "adapter-manifest")]
    manifests: Vec<PathBuf>,
    /// Port to listen on (0 picks a free one; the runtime file records it).
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Address to listen on. Anything but loopback also needs --allow-remote.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    listen: IpAddr,
    /// Allow a non-loopback listen address.
    #[arg(long)]
    allow_remote: bool,
    /// Principal of the operator token.
    #[arg(long, default_value = DEFAULT_OPERATOR)]
    operator: String,
    /// Seed for simulator noise.
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

#[derive(Args)]
struct DemoArgs {
    /// Keep the demo's home directory (journal, policy, audit) instead of deleting it.
    #[arg(long)]
    keep: bool,
}

#[derive(Subcommand)]
enum DevicesCmd {
    List,
    Inspect { device: String },
}

#[derive(Args)]
struct ExecArgs {
    device: String,
    capability: String,
    /// Value for a write, e.g. `5V`, `5 --unit V`, `true`.
    #[arg(long, allow_hyphen_values = true)]
    value: Option<String>,
    /// Unit for --value.
    #[arg(long)]
    unit: Option<String>,
    /// Action parameter, repeatable: `--param position=120mm --param speed=50mm/s`.
    #[arg(long = "param")]
    params: Vec<String>,
    /// Makes retries safe: the same key returns the same command instead of a new one.
    #[arg(long)]
    idempotency_key: Option<String>,
    #[arg(long)]
    correlation_id: Option<String>,
    /// Command deadline in milliseconds (default 60000).
    #[arg(long)]
    deadline_ms: Option<u64>,
    /// Only ask whether it would be allowed right now (preflight). Runs nothing.
    #[arg(long)]
    check: bool,
}

#[derive(Subcommand)]
enum CommandsCmd {
    /// Show a command's full receipt.
    Show { command_id: String },
    /// List recent commands (agents see only their own).
    List {
        /// Filter by state, repeatable, e.g. --state AWAITING_APPROVAL.
        #[arg(long = "state")]
        states: Vec<String>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Approve a command (same as `openmhs approve`).
    Approve { command_id: String },
    /// Cancel a command that has not been dispatched.
    Cancel { command_id: String },
    /// Resolve an UNKNOWN_OUTCOME command by reading the device.
    Reconcile { command_id: String },
}

#[derive(Subcommand)]
enum GrantsCmd {
    List,
    Add(GrantArgs),
    Revoke { id: String },
}

#[derive(Args)]
struct GrantArgs {
    #[arg(long)]
    id: String,
    /// Principal or pattern, e.g. `agent:*`.
    #[arg(long)]
    principal: String,
    /// Device id or pattern.
    #[arg(long)]
    device: String,
    /// Capability name or pattern, e.g. `power.output.*`.
    #[arg(long)]
    capability: String,
    /// Comma-separated: read, write, invoke, subscribe.
    #[arg(long, value_delimiter = ',', value_parser = parse_enum::<Operation>)]
    ops: Vec<Operation>,
    /// allow, require_approval or deny.
    #[arg(long, value_parser = parse_enum::<Effect>)]
    effect: Effect,
    /// Highest side-effect class this grant covers: none, reversible, irreversible, hazardous.
    #[arg(long, value_parser = parse_enum::<SideEffect>)]
    max_side_effect: Option<SideEffect>,
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Subcommand)]
enum TokensCmd {
    /// Create a credential for a principal. The secret is shown once.
    ///
    /// A credential can do nothing outside its scopes, and a scope only lets it reach
    /// what the policy grants also allow. Give at least one `--scope`, or `--any-scope`.
    Create {
        principal: String,
        #[arg(long, default_value = "agent", value_parser = parse_enum::<Role>)]
        role: Role,
        /// `DEVICE:CAPABILITY:OPS`, e.g. `sim.psu-01:power.output.*:read,write`. `*`
        /// matches any run of characters. Repeat for several scopes.
        #[arg(long = "scope", value_parser = parse_scope)]
        scopes: Vec<Scope>,
        /// No limit beyond the policy's grants. Use sparingly.
        #[arg(long)]
        any_scope: bool,
        /// Lifetime, e.g. `30m`, `12h`, `7d`. Without it the credential does not expire.
        #[arg(long, value_parser = parse_duration_ms)]
        ttl: Option<u64>,
    },
    List,
    /// Revoke a credential now. It stays on record, marked revoked.
    Revoke {
        id: String,
    },
    /// Replace a credential: a new secret, same principal and scopes; the old one stops.
    Rotate {
        id: String,
        /// Lifetime of the replacement; default is the old credential's lifetime.
        #[arg(long, value_parser = parse_duration_ms)]
        ttl: Option<u64>,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Verify the audit log's hash chain.
    Verify,
    /// Show the most recent audit records.
    Tail {
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand)]
enum SimCmd {
    /// Inject a fault: `clear`, `link-down`, `stale=5000`, `apply-then-hang`, `crash`, ...
    Fault { device: String, fault: String },
    /// List the fault names.
    Faults,
}

/// `DEVICE:CAPABILITY:read,write`.
fn parse_scope(s: &str) -> Result<Scope, String> {
    let parts: Vec<&str> = s.split(':').collect();
    let [device, capability, ops] = parts.as_slice() else {
        return Err("expected DEVICE:CAPABILITY:OPS, e.g. sim.psu-01:power.*:read".into());
    };
    let operations = ops
        .split(',')
        .map(parse_enum::<Operation>)
        .collect::<Result<Vec<_>, _>>()?;
    let scope = Scope {
        device: (*device).into(),
        capability: (*capability).into(),
        operations,
    };
    if scope.device.is_empty() || scope.capability.is_empty() {
        return Err("device and capability must not be empty".into());
    }
    Ok(scope)
}

/// `30m`, `12h`, `7d`, `90s`, in milliseconds.
fn parse_duration_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("'{s}' is not a duration like 30m, 12h or 7d"))?;
    let unit_ms: u64 = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(format!("'{s}' needs a unit: s, m, h or d")),
    };
    n.checked_mul(unit_ms)
        .ok_or_else(|| format!("'{s}' is too long"))
}

fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, String> {
    serde_json::from_value(serde_json::Value::String(s.trim().replace('-', "_")))
        .map_err(|_| format!("'{s}' is not a valid value"))
}

// ------------------------------------------------------------------ entry points

pub fn openmhs_main() -> ExitCode {
    let cli = Cli::parse();
    ExitCode::from(runtime().map_or(exit::ERROR, |rt| rt.block_on(run(cli))))
}

#[derive(Parser)]
#[command(name = "openmhsd", version, about = ABOUT)]
struct Daemon {
    #[arg(long, env = "OPENMHS_HOME")]
    home: Option<PathBuf>,
    #[command(flatten)]
    serve: ServeArgs,
}

pub fn openmhsd_main() -> ExitCode {
    let d = Daemon::parse();
    let home = d.home.unwrap_or_else(default_home);
    ExitCode::from(runtime().map_or(exit::ERROR, |rt| rt.block_on(serve(home, d.serve))))
}

fn runtime() -> Option<tokio::runtime::Runtime> {
    match tokio::runtime::Runtime::new() {
        Ok(rt) => Some(rt),
        Err(e) => {
            eprintln!("error: cannot start async runtime: {e}");
            None
        }
    }
}

async fn serve(home: PathBuf, args: ServeArgs) -> u8 {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
    let clock: std::sync::Arc<dyn openmhs_types::Clock> =
        std::sync::Arc::new(openmhs_types::SystemClock);
    let mut opened = Vec::new();
    let wanted = args
        .profiles
        .iter()
        .map(|p| (p, false))
        .chain(args.profile_sims.iter().map(|p| (p, true)));
    for (path, simulate) in wanted {
        match profiles::open(path, simulate, clock.clone()).await {
            Ok(o) => opened.push(o),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
    }
    let mut hosted = Vec::new();
    for path in &args.manifests {
        match start_hosted(path, false).await {
            Ok(a) => hosted.push(a),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
    }
    let mut adapters: Vec<std::sync::Arc<dyn HardwareAdapter>> =
        opened.iter().map(|o| o.adapter.clone()).collect();
    adapters.extend(
        hosted
            .iter()
            .map(|a| a.clone() as std::sync::Arc<dyn HardwareAdapter>),
    );
    let config = GatewayConfig {
        home,
        listen: SocketAddr::new(args.listen, args.port),
        allow_remote: args.allow_remote,
        operator: args.operator,
        simulation: args.sim,
        seed: args.seed,
        adapters,
    };
    let gateway = match openmhs_gateway::start(config).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    println!("OpenMHS gateway listening on {}", gateway.url);
    println!("  home     {}", gateway.home().display());
    let devices = gateway.engine().devices();
    if devices.is_empty() {
        println!(
            "  devices  none (use --sim for the virtual lab, --profile or --adapter-manifest for a device)"
        );
    } else {
        for d in &devices {
            match opened.iter().find(|o| o.device_id == d.device.id) {
                Some(o) if o.fixture.is_some() => println!(
                    "  device   {} ({}, simulated {} at {})",
                    d.device.id, d.adapter.id, o.protocol, o.target
                ),
                Some(o) => println!(
                    "  device   {} ({}, {} at {})",
                    d.device.id, d.adapter.id, o.protocol, o.target
                ),
                None if hosted.iter().any(|a| a.adapter_id() == d.adapter.id) => println!(
                    "  device   {} ({}, adapter process)",
                    d.device.id, d.adapter.id
                ),
                None => println!("  device   {} ({})", d.device.id, d.adapter.id),
            }
        }
    }
    let r = &gateway.recovery;
    if !r.unknown_outcome.is_empty() {
        println!(
            "  recovery {} command(s) were in flight at the last stop and are now UNKNOWN_OUTCOME; run `openmhs commands list --state UNKNOWN_OUTCOME`",
            r.unknown_outcome.len()
        );
    }
    println!("Press Ctrl+C to stop.");
    if let Err(e) = tokio::signal::ctrl_c().await {
        eprintln!("error: waiting for Ctrl+C: {e}");
    }
    let stopped = gateway.shutdown().await;
    drop(opened);
    drop(hosted);
    match stopped {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error during shutdown: {e}");
            exit::ERROR
        }
    }
}

// ------------------------------------------------------------------ client commands

/// Print a result in the chosen format.
fn show<T: Serialize>(json: bool, v: &T, human: impl FnOnce(&T)) {
    if json {
        output::json(v);
    } else {
        human(v);
    }
}

pub fn receipt_exit(r: &Receipt) -> u8 {
    match r.outcome {
        Outcome::Succeeded => exit::OK,
        Outcome::Denied => exit::DENIED,
        Outcome::Failed => exit::FAILED,
        Outcome::Uncertain => exit::UNCERTAIN,
        Outcome::Pending => exit::PENDING,
        Outcome::Unverified => exit::UNVERIFIED,
        Outcome::NotExecuted => exit::NOT_EXECUTED,
    }
}

fn report_error(json: bool, e: &ClientError) -> u8 {
    if json {
        let v = match e {
            ClientError::Rpc(rpc) => serde_json::json!({ "error": rpc }),
            other => serde_json::json!({ "error": { "message": other.to_string() } }),
        };
        output::json(&v);
    } else {
        match e.code() {
            Some(code) => eprintln!("error [{code}]: {e}"),
            None => eprintln!("error: {e}"),
        }
        if let ClientError::Rpc(rpc) = e
            && let Some(data) = &rpc.data
        {
            for err in &data.errors {
                if err.message != rpc.message {
                    eprintln!("  {}: {}", err.code, err.message);
                }
            }
        }
    }
    match e.code() {
        Some(c) if c.starts_with("policy.") || c == "gateway.forbidden" => exit::DENIED,
        _ => exit::ERROR,
    }
}

/// Validate profiles; connects to nothing.
fn profile_check(json: bool, files: &[PathBuf]) -> u8 {
    let results: Vec<(&Path, Result<profiles::Checked, String>)> = files
        .iter()
        .map(|f| (f.as_path(), profiles::check(f)))
        .collect();
    if json {
        let v: Vec<serde_json::Value> = results
            .iter()
            .map(|(f, r)| match r {
                Ok(c) => serde_json::json!({
                    "file": f.display().to_string(), "valid": true,
                    "protocol": c.protocol, "device": c.device_id, "capabilities": c.capabilities,
                }),
                Err(e) => serde_json::json!({
                    "file": f.display().to_string(), "valid": false, "error": e,
                }),
            })
            .collect();
        output::json(&v);
    } else {
        for (f, r) in &results {
            match r {
                Ok(c) => println!(
                    "ok       {}: {} device {}, {} capabilities",
                    f.display(),
                    c.protocol,
                    c.device_id,
                    c.capabilities
                ),
                Err(e) => println!("invalid  {e}"),
            }
        }
    }
    if results.iter().all(|(_, r)| r.is_ok()) {
        exit::OK
    } else {
        exit::ERROR
    }
}

/// Start an adapter process from its manifest. With `fixture`, for the conformance suite:
/// refused unless the adapter offers a fixture, so test commands never reach a device.
async fn start_hosted(
    path: &Path,
    fixture: bool,
) -> Result<std::sync::Arc<openmhs_host::ProcessAdapter>, String> {
    let resolved = openmhs_host::Resolved::load(path)?;
    let adapter =
        openmhs_host::ProcessAdapter::start(resolved, openmhs_host::HostOptions { fixture })
            .await
            .map_err(|e| format!("{}: {e}", path.display()))?;
    if fixture && adapter.fixture().is_none() {
        let _ = adapter.shutdown().await;
        return Err(format!(
            "{}: the adapter offers no conformance fixture; refusing to send test commands to whatever it is connected to",
            path.display()
        ));
    }
    Ok(std::sync::Arc::new(adapter))
}

/// Validate adapter manifests; starts nothing.
fn manifest_check(json: bool, files: &[PathBuf]) -> u8 {
    let results: Vec<(&Path, Result<openmhs_host::Resolved, String>)> = files
        .iter()
        .map(|f| (f.as_path(), openmhs_host::Resolved::load(f)))
        .collect();
    if json {
        let v: Vec<serde_json::Value> = results
            .iter()
            .map(|(f, r)| match r {
                Ok(r) => serde_json::json!({
                    "file": f.display().to_string(), "valid": true,
                    "adapter": r.manifest.adapter.id, "command": r.manifest.runtime.command,
                }),
                Err(e) => serde_json::json!({
                    "file": f.display().to_string(), "valid": false, "error": e,
                }),
            })
            .collect();
        output::json(&v);
    } else {
        for (f, r) in &results {
            match r {
                Ok(r) => println!(
                    "ok       {}: adapter {}, runs {}",
                    f.display(),
                    r.manifest.adapter.id,
                    r.manifest.runtime.command.join(" ")
                ),
                Err(e) => println!("invalid  {e}"),
            }
        }
    }
    if results.iter().all(|(_, r)| r.is_ok()) {
        exit::OK
    } else {
        exit::ERROR
    }
}

/// Run the suite against the virtual lab, in this process: no gateway needed.
async fn conformance(json: bool, args: ConformanceArgs) -> u8 {
    let clock: std::sync::Arc<dyn openmhs_types::Clock> =
        std::sync::Arc::new(openmhs_types::SystemClock);
    let lab = openmhs_sim::virtual_lab(clock.clone(), args.seed);
    let known: Vec<String> = lab
        .iter()
        .map(|d| d.control.device_id().to_owned())
        .collect();
    if let Some(unknown) = args.devices.iter().find(|d| !known.contains(d)) {
        eprintln!(
            "error: no simulator '{unknown}'; known: {}",
            known.join(", ")
        );
        return exit::ERROR;
    }
    let all = args.devices.is_empty() && args.profiles.is_empty() && args.manifests.is_empty();
    let chosen: Vec<_> = lab
        .into_iter()
        .filter(|d| all || args.devices.iter().any(|x| x == d.control.device_id()))
        .collect();
    let mut opened = Vec::new();
    for path in &args.profiles {
        match profiles::open(path, true, clock.clone()).await {
            Ok(o) => opened.push(o),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
    }
    let mut hosted = Vec::new();
    for path in &args.manifests {
        match start_hosted(path, true).await {
            Ok(a) => hosted.push(a),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
    }
    if !json {
        eprintln!(
            "Testing {} simulator(s). The suite sends real commands and injects faults; this takes about 15 seconds.",
            chosen.len() + opened.len() + hosted.len()
        );
    }
    let mut runs: Vec<_> = chosen
        .into_iter()
        .map(|device| {
            let options = openmhs_conformance::Options::new(clock.clone());
            tokio::spawn(async move { openmhs_conformance::sim::run(&device, options).await })
        })
        .collect();
    for o in &opened {
        let Some(fixture) = o.fixture.clone() else {
            continue;
        };
        let adapter = o.adapter.clone();
        let options = openmhs_conformance::Options::new(clock.clone());
        runs.push(tokio::spawn(async move {
            openmhs_conformance::run(adapter, fixture, options).await
        }));
    }
    for adapter in &hosted {
        let Some(fixture) = adapter.fixture() else {
            continue;
        };
        let adapter = adapter.clone();
        let options = openmhs_conformance::Options::new(clock.clone());
        runs.push(tokio::spawn(async move {
            let report =
                openmhs_conformance::run(adapter.clone(), std::sync::Arc::new(fixture), options)
                    .await;
            let _ = adapter.shutdown().await;
            report
        }));
    }
    let mut reports = Vec::new();
    for run in runs {
        match run.await {
            Ok(r) => reports.push(r),
            Err(e) => {
                eprintln!("error: a conformance run failed: {e}");
                return exit::ERROR;
            }
        }
    }
    if let Some(dir) = &args.report_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("error: cannot create {}: {e}", dir.display());
            return exit::ERROR;
        }
        for r in &reports {
            let name = r
                .subject
                .device
                .clone()
                .unwrap_or_else(|| r.subject.adapter.clone());
            let path = dir.join(format!("{name}.json"));
            let written = serde_json::to_string_pretty(r)
                .map_err(|e| e.to_string())
                .and_then(|text| std::fs::write(&path, text + "\n").map_err(|e| e.to_string()));
            if let Err(e) = written {
                eprintln!("error: cannot write {}: {e}", path.display());
                return exit::ERROR;
            }
        }
        if !json {
            eprintln!("Reports written to {}", dir.display());
        }
    }
    if json {
        output::json(&reports);
    } else {
        for r in &reports {
            println!("{}", openmhs_conformance::render(r));
        }
        println!("A conformance level is not a safety certification.");
    }
    if reports.iter().all(|r| r.passed) {
        exit::OK
    } else {
        exit::FAILED
    }
}

fn adapter_new(name: &str, path: Option<PathBuf>, openmhs_path: Option<PathBuf>) -> u8 {
    let dir = path.unwrap_or_else(|| PathBuf::from(name));
    let source = match openmhs_path {
        Some(p) => scaffold::Source::Path(std::path::absolute(&p).unwrap_or(p)),
        None => scaffold::Source::Git,
    };
    match scaffold::create(&dir, name, &source) {
        Ok(files) => {
            println!(
                "Created {} ({} files): an adapter for an example lamp, ready to make your own.",
                dir.display(),
                files.len()
            );
            if matches!(source, scaffold::Source::Git) {
                println!(
                    "It uses the OpenMHS crates from the repository's main branch; pass --openmhs-path <checkout> to use a local copy."
                );
            }
            println!();
            println!("  cd {}", dir.display());
            println!("  cargo test     # the conformance suite, against the fake lamp");
            println!();
            println!("Then follow docs/adapters/README.md.");
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            exit::ERROR
        }
    }
}

/// stdout is the MCP transport, so everything human-readable here goes to stderr.
async fn mcp(home: &std::path::Path, token: Option<&str>, args: McpArgs) -> u8 {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
    if token.is_none() && !args.allow_operator {
        eprintln!(
            "error: `openmhs mcp` needs an agent token (--token or OPENMHS_TOKEN). Create one \
             with `openmhs tokens create agent:<name> --scope DEVICE:CAPABILITY:OPS`; without it the server would act with \
             the operator token."
        );
        return exit::ERROR;
    }
    let client = match (&args.url, token) {
        (Some(url), Some(t)) => Client::new(url, t),
        (Some(_), None) => {
            eprintln!("error: --url needs --token");
            return exit::ERROR;
        }
        (None, _) => Client::from_home(home, token),
    };
    let client = match client {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    match openmhs_mcp::serve_stdio(openmhs_mcp::OpenMhsServer::new(client, args.allow_operator))
        .await
    {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: MCP session ended: {e}");
            exit::ERROR
        }
    }
}

async fn run(cli: Cli) -> u8 {
    let home = cli.home.clone().unwrap_or_else(default_home);
    match cli.command {
        Cmd::Serve(args) => return serve(home, args).await,
        Cmd::Demo(args) => return demo::run(args.keep).await,
        Cmd::Mcp(args) => return mcp(&home, cli.token.as_deref(), args).await,
        Cmd::Conformance(args) => return conformance(cli.json, args).await,
        Cmd::Profile {
            command: ProfileCmd::Check { files },
        } => return profile_check(cli.json, &files),
        Cmd::Adapter {
            command:
                AdapterCmd::New {
                    name,
                    path,
                    openmhs_path,
                },
        } => return adapter_new(&name, path, openmhs_path),
        Cmd::Adapter {
            command: AdapterCmd::Check { files },
        } => return manifest_check(cli.json, &files),
        Cmd::Sim {
            command: SimCmd::Faults,
        } => {
            println!("generic: {}", openmhs_sim::GENERIC_FAULTS.join(", "));
            println!("sim.psu-01: trip   sim.axis-01: jam   sim.thermo-01: overheat");
            return exit::OK;
        }
        _ => {}
    }
    let client = match Client::from_home(&home, cli.token.as_deref()) {
        Ok(c) => c,
        Err(e) => return report_error(cli.json, &e),
    };
    match client_command(&client, cli.json, cli.command).await {
        Ok(code) => code,
        Err(e) => report_error(cli.json, &e),
    }
}

async fn client_command(client: &Client, json: bool, cmd: Cmd) -> Result<u8, ClientError> {
    match cmd {
        Cmd::Serve(_)
        | Cmd::Demo(_)
        | Cmd::Mcp(_)
        | Cmd::Conformance(_)
        | Cmd::Profile { .. }
        | Cmd::Adapter { .. } => {
            unreachable!("handled before connecting")
        }
        Cmd::Info => {
            let info = client.info().await?;
            show(json, &info, |i| {
                println!("OpenMHS gateway {} (spec {})", i.version, i.spec_version);
                println!("you are    {} ({})", i.principal, output::name(&i.role));
                println!("devices    {}", i.devices);
                println!(
                    "mode       {}",
                    if i.simulation {
                        "simulation"
                    } else {
                        "hardware"
                    }
                );
            });
        }
        Cmd::Devices { command } => match command {
            DevicesCmd::List => show(json, &client.devices().await?, |d| output::devices(d)),
            DevicesCmd::Inspect { device } => {
                show(json, &client.describe(&device).await?, output::descriptor);
            }
        },
        Cmd::Read { device, capability } => {
            let r = client.read(&device, &capability).await?;
            show(json, &r, output::read_result);
        }
        Cmd::Exec(args) => {
            let request = match build_request(client, &args).await {
                Ok(r) => r,
                Err(m) => {
                    eprintln!("error: {m}");
                    return Ok(exit::ERROR);
                }
            };
            if args.check {
                let p = client.preflight(request).await?;
                show(json, &p, output::preflight);
                return Ok(match p.decision {
                    Decision::Allow => exit::OK,
                    Decision::Deny => exit::DENIED,
                    Decision::RequireApproval => exit::PENDING,
                });
            }
            let receipt = client.submit(request).await?;
            show(json, &receipt, output::receipt);
            return Ok(receipt_exit(&receipt));
        }
        Cmd::Commands { command } => match command {
            CommandsCmd::Show { command_id } => {
                let r = client.command(&command_id).await?;
                show(json, &r, output::receipt);
            }
            CommandsCmd::List { states, limit } => {
                let mut parsed: Vec<CommandState> = Vec::new();
                for s in states {
                    let upper = s.to_uppercase().replace('-', "_");
                    match parse_enum::<CommandState>(&upper) {
                        Ok(st) => parsed.push(st),
                        Err(m) => {
                            eprintln!("error: {m}");
                            return Ok(exit::ERROR);
                        }
                    }
                }
                let list = client.commands(parsed, limit).await?;
                show(json, &list, |l| {
                    if l.is_empty() {
                        println!("no commands");
                    }
                    l.iter().for_each(output::receipt_line);
                });
            }
            CommandsCmd::Approve { command_id } => return approve(client, json, &command_id).await,
            CommandsCmd::Cancel { command_id } => {
                let r = client.cancel(&command_id).await?;
                show(json, &r, output::receipt);
                return Ok(receipt_exit(&r));
            }
            CommandsCmd::Reconcile { command_id } => {
                let r = client.reconcile(&command_id).await?;
                show(json, &r, output::receipt);
                return Ok(receipt_exit(&r));
            }
        },
        Cmd::Approve { command_id } => return approve(client, json, &command_id).await,
        Cmd::Grants { command } => match command {
            GrantsCmd::List => show(json, &client.grants().await?, |g| output::grants(g)),
            GrantsCmd::Add(a) => {
                let grant = Grant {
                    id: a.id,
                    principal: a.principal,
                    device: a.device,
                    capability: a.capability,
                    operations: a.ops,
                    effect: a.effect,
                    max_side_effect: a.max_side_effect,
                    reason: a.reason,
                };
                let g = client.add_grant(grant).await?;
                show(json, &g, |g| {
                    print!("added ");
                    output::grants(std::slice::from_ref(g));
                });
            }
            GrantsCmd::Revoke { id } => {
                let g = client.revoke_grant(&id).await?;
                show(json, &g, |g| println!("revoked grant {}", g.id));
            }
        },
        Cmd::Tokens { command } => match command {
            TokensCmd::Create {
                principal,
                role,
                mut scopes,
                any_scope,
                ttl,
            } => {
                if any_scope {
                    scopes.push(Scope::any());
                }
                if scopes.is_empty() {
                    eprintln!(
                        "error: a credential with no scope can do nothing; give --scope DEVICE:CAPABILITY:OPS (repeatable) or --any-scope"
                    );
                    return Ok(exit::ERROR);
                }
                let params = CreateTokenParams {
                    principal,
                    role,
                    scopes,
                    ttl_ms: ttl,
                };
                let t = client.create_token(params).await?;
                show(json, &t, output::created_token);
            }
            TokensCmd::List => show(json, &client.tokens().await?, |t| output::tokens(t)),
            TokensCmd::Revoke { id } => {
                let t = client.revoke_token(&id).await?;
                show(json, &t, |t| println!("revoked {} ({})", t.id, t.principal));
            }
            TokensCmd::Rotate { id, ttl } => {
                let t = client.rotate_token(&id, ttl).await?;
                show(json, &t, |t| {
                    println!("{id} is revoked and replaced:");
                    output::created_token(t);
                });
            }
        },
        Cmd::Audit { command } => match command {
            AuditCmd::Verify => {
                let v = client.verify_audit().await?;
                show(json, &v, output::audit_verify);
                if !v.valid {
                    return Ok(exit::FAILED);
                }
            }
            AuditCmd::Tail { limit } => {
                show(json, &client.audit_tail(limit).await?, |e| {
                    output::audit_entries(e);
                });
            }
        },
        Cmd::Sim { command } => match command {
            SimCmd::Fault { device, fault } => {
                let r = client.sim_fault(&device, &fault).await?;
                show(json, &r, |r| println!("{}", r.message));
            }
            SimCmd::Faults => unreachable!("handled before connecting"),
        },
    }
    Ok(exit::OK)
}

async fn approve(client: &Client, json: bool, command_id: &str) -> Result<u8, ClientError> {
    let r = client.approve(command_id).await?;
    show(json, &r, output::receipt);
    Ok(receipt_exit(&r))
}

/// Build a command request. Writes for properties, invokes for actions, decided from the
/// device descriptor.
async fn build_request(client: &Client, a: &ExecArgs) -> Result<serde_json::Value, String> {
    let descriptor = client
        .describe(&a.device)
        .await
        .map_err(|e| e.to_string())?;
    let kind = descriptor
        .capability(&a.capability)
        .map(|c| c.kind)
        .ok_or_else(|| format!("'{}' has no capability '{}'", a.device, a.capability))?;
    let mut req = serde_json::json!({
        "target": { "device": a.device, "capability": a.capability },
    });
    match kind {
        CapabilityKind::Action => {
            req["operation"] = "invoke".into();
            if a.value.is_some() {
                return Err("actions take --param, not --value".into());
            }
            let mut params = serde_json::Map::new();
            for p in &a.params {
                let (k, v) = values::parse_param(p)?;
                params.insert(k, v);
            }
            if !params.is_empty() {
                req["params"] = params.into();
            }
        }
        _ => {
            req["operation"] = "write".into();
            let raw = a
                .value
                .as_deref()
                .ok_or("a write needs --value (e.g. --value 5V)")?;
            req["value"] = values::parse_value(raw, a.unit.as_deref())?;
            if !a.params.is_empty() {
                return Err("properties take --value, not --param".into());
            }
        }
    }
    if let Some(k) = &a.idempotency_key {
        req["idempotency_key"] = k.as_str().into();
    }
    if let Some(c) = &a.correlation_id {
        req["correlation_id"] = c.as_str().into();
    }
    if let Some(d) = a.deadline_ms {
        req["deadline_ms"] = d.into();
    }
    Ok(req)
}
