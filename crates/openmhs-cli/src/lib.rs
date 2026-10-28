//! The `openmhs` CLI and the `openmhsd` gateway binary (docs/cli.md).
//!
//! Every command except `serve` and `demo` is a client of a running gateway; the CLI
//! never touches devices, policy or the journal directly (ADR-0001).

mod bench;
mod demo;
mod discover;
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
    /// Measure a throwaway gateway on this machine, through its real API: read, command
    /// and event latency, concurrent reads, watch timing. Simulated devices only.
    Bench(BenchArgs),
    /// Ask addresses you name for their identity (SCPI `*IDN?`, which changes nothing) and
    /// list what answers. Proposes only: nothing is connected, registered or granted.
    Discover {
        #[command(subcommand)]
        command: DiscoverCmd,
    },
    /// Show who you are to the gateway and what it is running.
    Info,
    /// List or inspect registered devices.
    Devices {
        #[command(subcommand)]
        command: DevicesCmd,
    },
    /// Show the capability graph: relationships, shared resources and frames
    /// (spec/topology.md).
    Graph,
    /// Take, inspect, renew and release leases: exclusive use of devices for a while.
    Leases {
        #[command(subcommand)]
        command: LeasesCmd,
    },
    /// Run several commands as one plan: in order, under one lease, with a declared
    /// response to failure (spec/plans.md).
    Plans {
        #[command(subcommand)]
        command: PlansCmd,
    },
    /// Reviewed, versioned plans with typed parameters (spec/procedures.md).
    Procedures {
        #[command(subcommand)]
        command: ProceduresCmd,
    },
    /// Read a property from the device (authorized and audited).
    Read { device: String, capability: String },
    /// Follow the state fabric (ADR-0023): observations you may subscribe to, availability
    /// changes, and command state changes, as they happen.
    Events(EventsArgs),
    /// Sample a capability that declares `subscribe` and print each reading until stopped.
    Watch {
        device: String,
        capability: String,
        /// How often, e.g. 500ms or 2s (at least 200ms).
        #[arg(long, default_value = "1s", value_parser = parse_duration_ms)]
        every: u64,
        /// How long, e.g. 30s or 5m (at most 1h).
        #[arg(long = "for", default_value = "1m", value_parser = parse_duration_ms)]
        ttl: u64,
    },
    /// Find capabilities by what they can do: quantity, unit, operation, a value in range.
    Find(FindArgs),
    /// What the journal says happened on a device: confirmed values, refusals and the limits
    /// behind them, tracking error, readback latency, unresolved outcomes (ADR-0022).
    /// As an agent it covers your own commands; as the operator, everyone's.
    Insights {
        device: String,
        /// Only this capability.
        capability: Option<String>,
    },
    /// Submit a write (properties) or invoke (actions) and wait for its receipt.
    Exec(ExecArgs),
    /// Inspect and manage commands.
    Commands {
        #[command(subcommand)]
        command: CommandsCmd,
    },
    /// Approve a command waiting in AWAITING_APPROVAL (operator).
    Approve { command_id: String },
    /// Request, inspect, approve, deny and revoke approvals.
    Approvals {
        #[command(subcommand)]
        command: ApprovalsCmd,
    },
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
    /// Choose which signing keys may vouch for adapters (a file in the gateway home).
    Trust {
        #[command(subcommand)]
        command: TrustCmd,
    },
}

#[derive(Subcommand)]
enum TrustCmd {
    /// Trust a public key to sign adapter manifests.
    Add {
        /// The key: a `.pub` file written by `adapter keygen`, or 64 hex digits.
        key: String,
        /// Whose key this is, so you can tell later.
        #[arg(long)]
        name: String,
        /// Adapter ids this key may sign (`*` patterns). Default: any.
        #[arg(long = "adapter")]
        adapters: Vec<String>,
    },
    /// List the trusted signing keys.
    List,
    /// Stop trusting a key. Adapters already running keep running until the gateway restarts.
    Revoke { key_id: String },
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
    /// Start a profile for your device from the tested example of a protocol.
    New {
        /// modbus-tcp (or modbus), mqtt, scpi or http.
        protocol: String,
        /// Your device's id, e.g. lab.psu-01 (default: the example's).
        #[arg(long)]
        id: Option<String>,
        /// File to write (default: <id>.json). Never overwritten.
        #[arg(long)]
        out: Option<PathBuf>,
    },
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
    /// Make a signing key: writes <OUT>.key (secret, keep it private) and <OUT>.pub.
    Keygen { out: PathBuf },
    /// Sign a manifest, binding it to the files that make up the adapter.
    Sign {
        manifest: PathBuf,
        /// The secret key file from `adapter keygen`.
        #[arg(long)]
        key: PathBuf,
        /// A file or directory of the adapter, relative to the manifest. Repeatable. Every
        /// file the launch command runs must be covered.
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
        /// Write here instead of overwriting the manifest.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Args)]
struct EventsArgs {
    /// Only these devices (`*` patterns). Repeatable.
    #[arg(long = "device")]
    devices: Vec<String>,
    /// Only these kinds: observation, availability, command. Repeatable.
    #[arg(long = "kind", value_parser = parse_enum::<openmhs_types::event::EventKind>)]
    kinds: Vec<openmhs_types::event::EventKind>,
    /// Start from the oldest event still kept instead of from now.
    #[arg(long)]
    from_start: bool,
    /// Keep waiting for new events instead of printing what is there and stopping.
    #[arg(long)]
    follow: bool,
}

#[derive(Args)]
struct FindArgs {
    /// Capability name, `*` patterns allowed.
    #[arg(long)]
    name: Option<String>,
    /// Physical quantity, e.g. voltage, temperature.
    #[arg(long)]
    quantity: Option<String>,
    /// Unit, e.g. V, Cel.
    #[arg(long)]
    unit: Option<String>,
    /// read, write, subscribe or invoke.
    #[arg(long = "op", value_parser = parse_enum::<Operation>)]
    operation: Option<Operation>,
    /// A value it must accept (inside its declared range).
    #[arg(long)]
    value: Option<f64>,
    /// Only devices that are not OFFLINE.
    #[arg(long)]
    available: bool,
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
    /// Run a throwaway gateway with the virtual lab inside this process and serve MCP
    /// for it: no separate `serve`, grants or token. The agent may read every simulated
    /// device and write the power supply; nothing persists and no real hardware is
    /// touched. One config line is enough to try OpenMHS from an MCP host.
    #[arg(long, conflicts_with_all = ["url", "allow_operator"])]
    sim: bool,
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
    /// The manifest must be signed by a key added with `openmhs trust add`.
    #[arg(long = "adapter-manifest")]
    manifests: Vec<PathBuf>,
    /// Development only: launch adapters whose manifests are unsigned or unverified.
    #[arg(long)]
    allow_unsigned_adapters: bool,
    /// The installation's topology: how registered devices relate (spec/topology.md).
    /// Applied after every device is registered; a topology naming an unknown device
    /// stops the start.
    #[arg(long)]
    topology: Option<PathBuf>,
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
    /// Refuse to start unless every device answers. By default a device that does not
    /// answer is registered OFFLINE and used once it does.
    #[arg(long)]
    require_devices: bool,
    /// How often to check that devices answer, in milliseconds; 0 turns it off.
    #[arg(long, default_value_t = openmhs_gateway::DEFAULT_HEALTH_INTERVAL_MS)]
    health_interval_ms: u64,
    /// How far behind this machine's clock a device may stamp a reading and still have it
    /// count as measured after a command was sent. A command is only verified or reconciled
    /// by readings taken after it; set this to your device clocks' offset if they run
    /// behind (default 0).
    #[arg(long, default_value_t = 0)]
    readback_skew_ms: u64,
    /// Hard limit on the journal's size in MiB. Within the reserve of it, new commands and
    /// audited reads are refused and only stops and recovery may write (ADR-0021).
    /// Default: no limit.
    #[arg(long)]
    journal_max_mib: Option<u64>,
    /// With --journal-max-mib, the reserve below the limit kept for critical writes, in
    /// MiB. A starting point to test, not a sizing recommendation.
    #[arg(long, default_value_t = openmhs_gateway::DEFAULT_JOURNAL_RESERVE_BYTES / (1024 * 1024))]
    journal_reserve_mib: u64,
}

#[derive(Subcommand)]
enum DiscoverCmd {
    /// SCPI instruments over TCP (LXI, bench supplies, meters).
    Scpi {
        /// Hosts: 192.168.1.20, 192.168.1.20:5555, or a range 192.168.1.20-40 (at most 256).
        #[arg(required = true)]
        targets: Vec<String>,
        /// Port for targets that name none.
        #[arg(long, default_value_t = 5025)]
        port: u16,
        /// How long to wait for each answer.
        #[arg(long, default_value_t = 800)]
        timeout_ms: u64,
    },
}

#[derive(Args)]
struct BenchArgs {
    /// Sequential reads to time (also the total for the concurrent run).
    #[arg(long, default_value_t = 500)]
    reads: usize,
    /// Verified writes to time (paced at under 5 a second, the setpoint's rate limit).
    #[arg(long, default_value_t = 20)]
    commands: usize,
    /// Clients reading at once in the concurrent run.
    #[arg(long, default_value_t = 8)]
    concurrency: usize,
}

#[derive(Args)]
struct DemoArgs {
    /// Keep the demo's home directory (journal, policy, audit) instead of deleting it.
    #[arg(long)]
    keep: bool,
}

#[derive(Subcommand)]
enum DevicesCmd {
    /// List the devices this gateway has registered.
    List,
    /// Show a device's descriptor: its capabilities, units, ranges and limits.
    Inspect { device: String },
    /// Show whether devices answer, are busy or leased. Probes any device not checked in
    /// the last second first.
    Status { device: Option<String> },
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
    /// Spend an approval that was requested and approved ahead of this exact command.
    #[arg(long)]
    approval: Option<String>,
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
    /// Operator only: record whether an UNKNOWN_OUTCOME command took effect, for one
    /// that cannot be read back. Nothing is sent to the device.
    #[command(group(clap::ArgGroup::new("outcome").required(true).args(["applied", "not_applied"])))]
    Resolve {
        command_id: String,
        /// It took effect.
        #[arg(long)]
        applied: bool,
        /// It did not take effect.
        #[arg(long)]
        not_applied: bool,
        /// How you know, e.g. "front panel shows 5.00 V". Recorded in the receipt and audit log.
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum GrantsCmd {
    /// List the grants: who may do what, on which devices.
    List,
    /// Allow (or deny, or require approval for) operations by a principal on devices.
    Add(GrantArgs),
    /// Remove a grant by its id.
    Revoke { id: String },
}

#[derive(Args)]
struct GrantArgs {
    /// A name for this grant, used to list and revoke it.
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
    /// Why this grant exists, recorded in the audit log.
    #[arg(long)]
    reason: Option<String>,
}

#[derive(Subcommand)]
enum ApprovalsCmd {
    /// Ask for approval of a command before submitting it (same arguments as `exec`).
    /// Prints the approval id; once approved, `exec ... --approval <id>` spends it.
    Request {
        #[command(flatten)]
        exec: ExecArgs,
        /// How long the approval stays valid once approved, e.g. `15m`, `2h`.
        #[arg(long, value_parser = parse_duration_ms)]
        ttl: Option<u64>,
    },
    /// Show one approval.
    Show { id: String },
    /// List approvals (requesters see their own; approvers see all).
    List {
        /// Only these statuses: pending, approved, denied, expired, revoked, consumed.
        #[arg(long = "status", value_parser = parse_enum::<openmhs_types::ApprovalStatus>)]
        statuses: Vec<openmhs_types::ApprovalStatus>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Approve (operator who is an approver). Not the requester.
    Approve { id: String },
    /// Deny a pending approval.
    Deny {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Withdraw an approval that has not been used: the requester's own, or any as approver.
    Revoke {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProceduresCmd {
    /// Every stored procedure, proposed and approved.
    List,
    /// One procedure (the newest version unless --version).
    Show {
        name: String,
        #[arg(long)]
        version: Option<u32>,
    },
    /// Store a procedure (a JSON file) as proposed; it cannot run until approved.
    Propose { file: PathBuf },
    /// Store a procedure as approved (operator).
    Add { file: PathBuf },
    /// Approve a proposed version (operator).
    Approve { name: String, version: u32 },
    /// Remove a version (operator).
    Remove { name: String, version: u32 },
    /// Dry-run an approved procedure against twins.
    Check(RunProcedureArgs),
    /// Run an approved procedure: an ordinary plan, as you.
    Run(RunProcedureArgs),
}

#[derive(Args)]
struct RunProcedureArgs {
    name: String,
    #[arg(long)]
    version: Option<u32>,
    /// name=value, e.g. voltage=2V. Repeatable.
    #[arg(long = "param", value_parser = values::parse_param)]
    params: Vec<(String, serde_json::Value)>,
}

impl From<RunProcedureArgs> for openmhs_api::RunProcedureParams {
    fn from(a: RunProcedureArgs) -> Self {
        Self {
            name: a.name,
            version: a.version,
            params: a.params.into_iter().collect(),
        }
    }
}

#[derive(Subcommand)]
enum PlansCmd {
    /// Submit a plan (a JSON file); returns when it ends or waits.
    Submit { file: PathBuf },
    /// Dry-run a plan against twins of its devices; changes nothing real.
    Preflight { file: PathBuf },
    /// Show one plan.
    Show { id: String },
    /// List plans, newest first.
    List {
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Decide how a waiting plan continues (a human approver).
    Decide {
        id: String,
        /// `resume`, `retry`, `compensate` or `abort`.
        #[arg(long, value_parser = parse_decision)]
        decision: openmhs_types::plan::PlanDecision,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Cancel a plan: its submitter, or an approver.
    Cancel {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
enum LeasesCmd {
    /// Lease devices (and what they contain or share, spec/topology.md). Others' writes
    /// and invokes on them are refused until the lease ends; stops never are.
    Acquire {
        #[arg(long = "device")]
        devices: Vec<String>,
        /// A shared resource: claims every device that uses it.
        #[arg(long = "resource")]
        resources: Vec<String>,
        /// How long, e.g. `10m`, `2h` (at most 24h).
        #[arg(long, value_parser = parse_duration_ms)]
        ttl: u64,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show one lease.
    Show { id: String },
    /// List leases, newest first.
    List {
        /// Only leases that hold something now.
        #[arg(long)]
        active: bool,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Move a lease's deadline to `--ttl` from now (holder only).
    Renew {
        id: String,
        #[arg(long, value_parser = parse_duration_ms)]
        ttl: u64,
    },
    /// End a lease: yours, or any as an approver.
    Release {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
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
    /// List credentials (ids, principals, scopes and expiry; never secrets).
    List,
    /// Revoke a credential now. It stays on record, marked revoked.
    Revoke { id: String },
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
        "ms" => 1,
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
    let trust = if args.allow_unsigned_adapters {
        if !args.manifests.is_empty() {
            eprintln!(
                "warning: --allow-unsigned-adapters: adapter manifests are NOT verified. Use this for development only."
            );
        }
        openmhs_host::Trust::AllowUnsigned
    } else {
        match openmhs_host::TrustStore::load(&home.join(openmhs_host::trust::TRUSTED_KEYS_FILE)) {
            Ok(store) => openmhs_host::Trust::Require(store),
            Err(e) => {
                eprintln!("error: {e}");
                return exit::ERROR;
            }
        }
    };
    for path in &args.manifests {
        match start_hosted(path, false, trust.clone()).await {
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
    let topology = match args.topology.as_deref().map(read_topology).transpose() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    let config = GatewayConfig {
        home,
        listen: SocketAddr::new(args.listen, args.port),
        allow_remote: args.allow_remote,
        operator: args.operator,
        simulation: args.sim,
        seed: args.seed,
        adapters,
        topology,
        require_devices: args.require_devices,
        health_interval_ms: args.health_interval_ms,
        readback_skew_ms: args.readback_skew_ms,
        journal_max_bytes: args.journal_max_mib.map(|m| m.saturating_mul(1024 * 1024)),
        journal_reserve_bytes: args.journal_reserve_mib.saturating_mul(1024 * 1024),
    };
    let gateway = match openmhs_gateway::start(config).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            // With --require-devices the gateway connects to every device before it
            // listens; say which real devices it was reaching and how to go on without one.
            let real: Vec<_> = args.profiles.iter().zip(&opened).collect();
            if !real.is_empty() && matches!(e, openmhs_gateway::GatewayError::Engine(_)) {
                eprintln!("  the gateway connects to each device before it starts:");
                for (path, o) in &real {
                    eprintln!(
                        "    {} ({} at {}, from {})",
                        o.device_id,
                        o.protocol,
                        o.target,
                        path.display()
                    );
                }
                eprintln!(
                    "  check \"connection\" in the profile and that the device is on and reachable;"
                );
                eprintln!(
                    "  to try the profile without the device: openmhs serve --profile-sim {}",
                    real[0].0.display()
                );
            }
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
        for s in gateway.engine().device_statuses().unwrap_or_default() {
            if s.availability == openmhs_types::Availability::Offline {
                println!(
                    "  offline  {} does not answer ({}); commands to it are refused until it does (`openmhs devices status`)",
                    s.device,
                    s.reason.as_deref().unwrap_or("no reason given")
                );
            }
        }
        println!(
            "  access   deny by default; `openmhs grants list` shows what is allowed (docs/connect-a-device.md)"
        );
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

fn parse_decision(s: &str) -> Result<openmhs_types::plan::PlanDecision, String> {
    serde_json::from_value(serde_json::Value::String(s.into()))
        .map_err(|_| format!("'{s}' is not resume, retry, compensate or abort"))
}

fn read_procedure(path: &Path) -> Result<openmhs_types::procedure::ProcedureSpec, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_plan(path: &Path) -> Result<openmhs_types::plan::PlanSpec, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// How a plan's state maps onto the command exit codes.
pub fn plan_exit(p: &openmhs_types::plan::Plan) -> u8 {
    use openmhs_types::plan::PlanState;
    match p.state {
        PlanState::Succeeded => exit::OK,
        PlanState::Halted | PlanState::Compensated => exit::FAILED,
        PlanState::AwaitingDecision => exit::UNCERTAIN,
        PlanState::Running | PlanState::WaitingApproval | PlanState::Compensating => exit::PENDING,
        PlanState::Aborted | PlanState::Cancelled => exit::NOT_EXECUTED,
    }
}

/// Preflight: 0 when every step would run, 5 when some need approval, 2 otherwise.
fn plan_preflight_exit(p: &openmhs_types::plan::PlanPreflight) -> u8 {
    use openmhs_types::plan::StepVerdict;
    if !p.ok {
        exit::DENIED
    } else if p
        .steps
        .iter()
        .any(|s| s.verdict == StepVerdict::NeedsApproval)
    {
        exit::PENDING
    } else {
        exit::OK
    }
}

fn read_topology(path: &Path) -> Result<openmhs_types::Topology, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    openmhs_types::Topology::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write a starter profile; connects to nothing.
fn profile_new(json: bool, protocol: &str, id: Option<&str>, out: Option<PathBuf>) -> u8 {
    let path = out.unwrap_or_else(|| {
        let stem = match id {
            Some(id) => id.to_string(),
            None => format!(
                "{}-device",
                profiles::protocol_named(protocol).unwrap_or(protocol)
            ),
        };
        PathBuf::from(format!("{stem}.json"))
    });
    let created = match profiles::create(&path, protocol, id) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    let c = &created.checked;
    if json {
        output::json(&serde_json::json!({
            "file": path.display().to_string(), "protocol": c.protocol,
            "device": c.device_id, "capabilities": c.capabilities, "example": created.example,
        }));
        return exit::OK;
    }
    let f = path.display();
    println!(
        "Created {f}: {} profile for device {}, copied from the {} example ({} capabilities).",
        c.protocol, c.device_id, created.example, c.capabilities
    );
    println!();
    println!("Next:");
    println!(
        "  1. Edit it: \"connection\" is where your device is; \"descriptor\".\"capabilities\" and \"mapping\""
    );
    println!(
        "     are what it can do and how. Reference: docs/adapters/{}.md",
        if c.protocol == "modbus-tcp" {
            "modbus"
        } else {
            &c.protocol
        }
    );
    println!("  2. openmhs profile check {f}");
    println!("  3. openmhs serve --profile-sim {f}    try it against the profile's simulator");
    println!("  4. openmhs serve --profile {f}        connect to the real device");
    println!("  Then grant access and connect an agent: docs/connect-a-device.md");
    exit::OK
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
    trust: openmhs_host::Trust,
) -> Result<std::sync::Arc<openmhs_host::ProcessAdapter>, String> {
    let resolved = openmhs_host::Resolved::load(path)?;
    let adapter =
        openmhs_host::ProcessAdapter::start(resolved, openmhs_host::HostOptions { fixture, trust })
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
fn adapter_keygen(out: &Path) -> u8 {
    let (key_path, pub_path) = (out.with_extension("key"), out.with_extension("pub"));
    for p in [&key_path, &pub_path] {
        if p.exists() {
            eprintln!(
                "error: {} already exists; refusing to overwrite a key",
                p.display()
            );
            return exit::ERROR;
        }
    }
    let pair = match openmhs_host::trust::generate_keypair() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::ERROR;
        }
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&key_path).and_then(|mut f| {
        use std::io::Write;
        writeln!(f, "{}", pair.seed)
    });
    if let Err(e) =
        written.and_then(|()| std::fs::write(&pub_path, format!("{}\n", pair.public_key)))
    {
        eprintln!("error: {e}");
        return exit::ERROR;
    }
    println!(
        "secret key  {} (keep private; anyone with it can sign as you)",
        key_path.display()
    );
    println!("public key  {}", pub_path.display());
    println!("key id      {}", pair.key_id);
    println!(
        "Next: openmhs trust add {} --name <who>",
        pub_path.display()
    );
    exit::OK
}

fn adapter_sign(manifest: &Path, key: &Path, artifacts: &[String], out: Option<&Path>) -> u8 {
    let run = || -> Result<(), String> {
        let text = std::fs::read_to_string(manifest)
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        let seed = std::fs::read_to_string(key).map_err(|e| format!("{}: {e}", key.display()))?;
        let dir = manifest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let signed = openmhs_host::trust::sign_manifest(&text, dir, &seed, artifacts)?;
        let target = out.unwrap_or(manifest);
        std::fs::write(target, signed).map_err(|e| format!("{}: {e}", target.display()))?;
        println!("signed {}", target.display());
        Ok(())
    };
    match run() {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::ERROR
        }
    }
}

fn trust_cmd(json: bool, home: &Path, command: TrustCmd) -> u8 {
    let path = home.join(openmhs_host::trust::TRUSTED_KEYS_FILE);
    let now = openmhs_types::Clock::now(&openmhs_types::SystemClock).millis();
    let run = || -> Result<(), String> {
        let mut store = openmhs_host::TrustStore::load(&path)?;
        match command {
            TrustCmd::Add {
                key,
                name,
                adapters,
            } => {
                let hex = std::fs::read_to_string(&key)
                    .unwrap_or(key)
                    .trim()
                    .to_owned();
                std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
                let added = store.add(&hex, &name, adapters, now)?;
                store.save(&path)?;
                println!(
                    "trusting {} ({}) to sign {}",
                    added.key_id,
                    added.name,
                    added.adapters.join(", ")
                );
            }
            TrustCmd::Revoke { key_id } => {
                if store.revoke(&key_id, now)? {
                    store.save(&path)?;
                    println!("revoked {key_id}; adapters it signed are refused at the next start");
                } else {
                    println!("{key_id} was already revoked");
                }
            }
            TrustCmd::List => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(store.keys()).map_err(|e| e.to_string())?
                    );
                } else if store.keys().is_empty() {
                    println!("no trusted signing keys; signed adapters will be refused");
                }
                if !json {
                    for k in store.keys() {
                        let status = if k.revoked_at_ms.is_some() {
                            "revoked"
                        } else {
                            "trusted"
                        };
                        println!(
                            "{}  {:<8} {:<20} signs {}",
                            k.key_id,
                            status,
                            k.name,
                            k.adapters.join(", ")
                        );
                    }
                }
            }
        }
        Ok(())
    };
    match run() {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::ERROR
        }
    }
}

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
        // Fixture mode talks only to the adapter's own simulator, so a signature is not
        // required here; `serve` is where it is.
        match start_hosted(path, true, openmhs_host::Trust::AllowUnsigned).await {
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
    if args.sim {
        return mcp_sim().await;
    }
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

/// `openmhs mcp --sim`: the whole gateway in one process, for trying OpenMHS from an MCP
/// host without any setup. The MCP server gets an agent token minted here, scoped to the
/// virtual lab; the operator token never leaves this function, so the model cannot
/// approve its own commands or change policy (the same boundary as `openmhs mcp`).
async fn mcp_sim() -> u8 {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let home = std::env::temp_dir().join(format!("openmhs-mcp-sim-{}-{stamp}", std::process::id()));
    let mut config = GatewayConfig::new(&home);
    config.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.simulation = true;
    let gateway = match openmhs_gateway::start(config).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: cannot start the virtual lab: {e}");
            return exit::ERROR;
        }
    };
    let code = match mcp_sim_session(&gateway.url, &gateway.operator_token).await {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::ERROR
        }
    };
    if let Err(e) = gateway.shutdown().await {
        eprintln!("warning: gateway shutdown: {e}");
    }
    let _ = std::fs::remove_dir_all(&home);
    code
}

async fn mcp_sim_session(url: &str, operator_token: &str) -> Result<(), String> {
    let operator = Client::new(url, operator_token).map_err(|e| e.to_string())?;
    let read = Grant {
        id: "sim-read".into(),
        principal: "agent:*".into(),
        device: "sim.*".into(),
        capability: "*".into(),
        operations: vec![Operation::Read],
        effect: Effect::Allow,
        max_side_effect: None,
        reason: None,
    };
    let psu = Grant {
        id: "sim-psu".into(),
        principal: "agent:*".into(),
        device: "sim.psu-01".into(),
        capability: "power.output.*".into(),
        operations: vec![Operation::Write],
        effect: Effect::Allow,
        max_side_effect: Some(SideEffect::Reversible),
        reason: None,
    };
    for g in [read, psu] {
        operator.add_grant(g).await.map_err(|e| e.to_string())?;
    }
    let scopes = ["sim.*:*:read", "sim.psu-01:*:write"]
        .map(|s| parse_scope(s).expect("static scope"))
        .to_vec();
    let token = operator
        .create_token(CreateTokenParams::scoped("agent:sim", Role::Agent, scopes))
        .await
        .map_err(|e| e.to_string())?;
    let agent = Client::new(url, &token.token).map_err(|e| e.to_string())?;
    eprintln!(
        "openmhs mcp --sim: virtual lab only (sim.thermo-01, sim.psu-01, sim.axis-01);          the agent can read all and write the power supply; nothing persists."
    );
    openmhs_mcp::serve_stdio(openmhs_mcp::OpenMhsServer::new(agent, false))
        .await
        .map_err(|e| format!("MCP session ended: {e}"))
}

async fn run(cli: Cli) -> u8 {
    let home = cli.home.clone().unwrap_or_else(default_home);
    match cli.command {
        Cmd::Serve(args) => return serve(home, args).await,
        Cmd::Demo(args) => return demo::run(args.keep).await,
        Cmd::Discover {
            command:
                DiscoverCmd::Scpi {
                    targets,
                    port,
                    timeout_ms,
                },
        } => return discover::run(cli.json, &targets, port, timeout_ms).await,
        Cmd::Bench(args) => {
            return bench::run(cli.json, args.reads, args.commands, args.concurrency).await;
        }
        Cmd::Mcp(args) => return mcp(&home, cli.token.as_deref(), args).await,
        Cmd::Conformance(args) => return conformance(cli.json, args).await,
        Cmd::Profile {
            command: ProfileCmd::Check { files },
        } => return profile_check(cli.json, &files),
        Cmd::Profile {
            command: ProfileCmd::New { protocol, id, out },
        } => return profile_new(cli.json, &protocol, id.as_deref(), out),
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
        Cmd::Adapter {
            command: AdapterCmd::Keygen { out },
        } => return adapter_keygen(&out),
        Cmd::Adapter {
            command:
                AdapterCmd::Sign {
                    manifest,
                    key,
                    artifacts,
                    out,
                },
        } => return adapter_sign(&manifest, &key, &artifacts, out.as_deref()),
        Cmd::Trust { command } => return trust_cmd(cli.json, &home, command),
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
        | Cmd::Bench(_)
        | Cmd::Discover { .. }
        | Cmd::Mcp(_)
        | Cmd::Conformance(_)
        | Cmd::Profile { .. }
        | Cmd::Adapter { .. }
        | Cmd::Trust { .. } => {
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
            DevicesCmd::Status { device } => show(
                json,
                &client.device_status(device.as_deref(), true).await?,
                |s| output::device_statuses(s),
            ),
        },
        Cmd::Graph => show(json, &client.graph().await?, output::graph),
        Cmd::Leases { command } => match command {
            LeasesCmd::Acquire {
                devices,
                resources,
                ttl,
                reason,
            } => {
                let params = openmhs_api::AcquireLeaseParams {
                    devices,
                    resources,
                    ttl_ms: ttl,
                    reason,
                };
                show(json, &client.acquire_lease(params).await?, output::lease);
            }
            LeasesCmd::Show { id } => show(json, &client.lease(&id).await?, output::lease),
            LeasesCmd::List { active, limit } => {
                show(json, &client.leases(active, limit).await?, |l| {
                    output::leases(l)
                });
            }
            LeasesCmd::Renew { id, ttl } => {
                show(json, &client.renew_lease(&id, ttl).await?, output::lease);
            }
            LeasesCmd::Release { id, reason } => {
                show(
                    json,
                    &client.release_lease(&id, reason).await?,
                    output::lease,
                );
            }
        },
        Cmd::Plans { command } => match command {
            PlansCmd::Submit { file } => {
                let spec = match read_plan(&file) {
                    Ok(s) => s,
                    Err(m) => {
                        eprintln!("error: {m}");
                        return Ok(exit::ERROR);
                    }
                };
                let p = client.submit_plan(spec).await?;
                show(json, &p, output::plan);
                return Ok(plan_exit(&p));
            }
            PlansCmd::Preflight { file } => {
                let spec = match read_plan(&file) {
                    Ok(s) => s,
                    Err(m) => {
                        eprintln!("error: {m}");
                        return Ok(exit::ERROR);
                    }
                };
                let p = client.preflight_plan(spec).await?;
                show(json, &p, output::plan_preflight);
                return Ok(plan_preflight_exit(&p));
            }
            PlansCmd::Show { id } => show(json, &client.plan(&id).await?, output::plan),
            PlansCmd::List { limit } => {
                show(json, &client.plans(limit).await?, |l| output::plans(l));
            }
            PlansCmd::Decide {
                id,
                decision,
                reason,
            } => {
                let p = client.decide_plan(&id, decision, reason).await?;
                show(json, &p, output::plan);
                return Ok(plan_exit(&p));
            }
            PlansCmd::Cancel { id, reason } => {
                show(json, &client.cancel_plan(&id, reason).await?, output::plan);
            }
        },
        Cmd::Procedures { command } => match command {
            ProceduresCmd::List => {
                show(json, &client.procedures().await?, |l| output::procedures(l))
            }
            ProceduresCmd::Show { name, version } => {
                show(json, &client.procedure(&name, version).await?, |p| {
                    output::procedures(std::slice::from_ref(p))
                });
            }
            ProceduresCmd::Propose { file } => {
                let spec = match read_procedure(&file) {
                    Ok(s) => s,
                    Err(m) => {
                        eprintln!("error: {m}");
                        return Ok(exit::ERROR);
                    }
                };
                show(json, &client.propose_procedure(spec).await?, |p| {
                    output::procedures(std::slice::from_ref(p))
                });
            }
            ProceduresCmd::Add { file } => {
                let spec = match read_procedure(&file) {
                    Ok(s) => s,
                    Err(m) => {
                        eprintln!("error: {m}");
                        return Ok(exit::ERROR);
                    }
                };
                show(json, &client.add_procedure(spec).await?, |p| {
                    output::procedures(std::slice::from_ref(p))
                });
            }
            ProceduresCmd::Approve { name, version } => {
                show(
                    json,
                    &client.approve_procedure(&name, version).await?,
                    |p| output::procedures(std::slice::from_ref(p)),
                );
            }
            ProceduresCmd::Remove { name, version } => {
                show(json, &client.remove_procedure(&name, version).await?, |p| {
                    output::procedures(std::slice::from_ref(p))
                });
            }
            ProceduresCmd::Check(args) => {
                let p = client.check_procedure(args.into()).await?;
                show(json, &p, output::plan_preflight);
                return Ok(plan_preflight_exit(&p));
            }
            ProceduresCmd::Run(args) => {
                let p = client.run_procedure(args.into()).await?;
                show(json, &p, output::plan);
                return Ok(plan_exit(&p));
            }
        },
        Cmd::Read { device, capability } => {
            let r = client.read(&device, &capability).await?;
            show(json, &r, output::read_result);
        }
        Cmd::Events(args) => {
            let mut params = openmhs_api::EventsPollParams {
                after: args.from_start.then_some(0),
                devices: args.devices,
                kinds: args.kinds,
                limit: Some(500),
                ..Default::default()
            };
            loop {
                let b = client.poll_events(params.clone()).await?;
                if b.missed > 0 {
                    eprintln!("({} event(s) dropped before they could be read)", b.missed);
                }
                for e in &b.events {
                    if json {
                        output::json_line(e);
                    } else {
                        output::event(e);
                    }
                }
                if !args.follow {
                    break;
                }
                params.stream = Some(b.stream);
                params.after = Some(b.next);
                params.wait_ms = 25_000;
            }
        }
        Cmd::Watch {
            device,
            capability,
            every,
            ttl,
        } => {
            let start = client
                .poll_events(openmhs_api::EventsPollParams::default())
                .await?;
            let w = client
                .create_watch(openmhs_api::WatchParams {
                    device: device.clone(),
                    capability: capability.clone(),
                    interval_ms: every,
                    ttl_ms: Some(ttl),
                })
                .await?;
            eprintln!(
                "watching {device} {capability} every {every} ms until {} (watch {})",
                w.expires_at, w.id
            );
            let mut params = openmhs_api::EventsPollParams {
                stream: Some(start.stream),
                after: Some(start.next),
                devices: vec![device.clone()],
                kinds: vec![openmhs_types::event::EventKind::Observation],
                limit: Some(500),
                wait_ms: 25_000,
            };
            let ends = std::time::Instant::now() + std::time::Duration::from_millis(ttl);
            while let Some(left) = ends.checked_duration_since(std::time::Instant::now()) {
                params.wait_ms = u64::try_from(left.as_millis())
                    .unwrap_or(25_000)
                    .min(25_000);
                let b = client.poll_events(params.clone()).await?;
                for e in &b.events {
                    if let openmhs_types::event::EventBody::Observation { capability: c, .. } =
                        &e.body
                        && *c == capability
                    {
                        if json {
                            output::json_line(e);
                        } else {
                            output::event(e);
                        }
                    }
                }
                params.after = Some(b.next);
            }
        }
        Cmd::Find(args) => {
            let q = openmhs_api::selection::CapabilityQuery {
                name: args.name,
                quantity: args.quantity,
                unit: args.unit,
                operation: args.operation,
                value: args.value,
                max_side_effect: None,
                available_only: args.available,
            };
            show(
                json,
                &client.find_capabilities(&q).await?,
                output::selection,
            );
        }
        Cmd::Insights { device, capability } => {
            let i = client.insights(&device, capability.as_deref()).await?;
            show(json, &i, output::insights);
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
            let receipt = match &args.approval {
                Some(id) => client.submit_approved(request, id).await?,
                None => client.submit(request).await?,
            };
            show(json, &receipt, output::receipt);
            return Ok(receipt_exit(&receipt));
        }
        Cmd::Approvals { command } => match command {
            ApprovalsCmd::Request { exec, ttl } => {
                let request = match build_request(client, &exec).await {
                    Ok(r) => r,
                    Err(m) => {
                        eprintln!("error: {m}");
                        return Ok(exit::ERROR);
                    }
                };
                let a = client.request_approval(request, ttl).await?;
                show(json, &a, output::approval);
            }
            ApprovalsCmd::Show { id } => show(json, &client.approval(&id).await?, output::approval),
            ApprovalsCmd::List { statuses, limit } => {
                let list = client.approvals(statuses, limit).await?;
                show(json, &list, |l| output::approvals(l));
            }
            ApprovalsCmd::Approve { id } => {
                show(json, &client.approve_approval(&id).await?, output::approval);
            }
            ApprovalsCmd::Deny { id, reason } => {
                show(
                    json,
                    &client.deny_approval(&id, reason).await?,
                    output::approval,
                );
            }
            ApprovalsCmd::Revoke { id, reason } => {
                show(
                    json,
                    &client.revoke_approval(&id, reason).await?,
                    output::approval,
                );
            }
        },
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
            CommandsCmd::Resolve {
                command_id,
                applied,
                reason,
                ..
            } => {
                let r = client.resolve(&command_id, applied, &reason).await?;
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
