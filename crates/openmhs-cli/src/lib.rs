//! The `openmhs` CLI and the `openmhsd` gateway binary (docs/cli.md).
//!
//! Every command except `serve` and `demo` is a client of a running gateway; the CLI
//! never touches devices, policy or the journal directly (ADR-0001).

mod demo;
mod output;
mod values;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use openmhs_api::runtime::default_home;
use openmhs_api::{Client, ClientError, Role};
use openmhs_gateway::{DEFAULT_OPERATOR, DEFAULT_PORT, GatewayConfig};
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::{CapabilityKind, Operation, SideEffect};
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
}

#[derive(Args, Clone)]
pub struct ServeArgs {
    /// Register the virtual lab: temperature sensor, power supply, linear stage.
    #[arg(long)]
    sim: bool,
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
    /// Create a token for a principal. The secret is shown once.
    Create {
        principal: String,
        #[arg(long, default_value = "agent", value_parser = parse_enum::<Role>)]
        role: Role,
    },
    List,
    Revoke {
        id: String,
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
    let config = GatewayConfig {
        home,
        listen: SocketAddr::new(args.listen, args.port),
        allow_remote: args.allow_remote,
        operator: args.operator,
        simulation: args.sim,
        seed: args.seed,
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
        println!("  devices  none (use --sim for the virtual lab; hardware adapters arrive in M6)");
    } else {
        for d in &devices {
            println!("  device   {} ({})", d.device.id, d.adapter.id);
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
    match gateway.shutdown().await {
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

async fn run(cli: Cli) -> u8 {
    let home = cli.home.clone().unwrap_or_else(default_home);
    match cli.command {
        Cmd::Serve(args) => return serve(home, args).await,
        Cmd::Demo(args) => return demo::run(args.keep).await,
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
        Cmd::Serve(_) | Cmd::Demo(_) => unreachable!("handled before connecting"),
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
            TokensCmd::Create { principal, role } => {
                let t = client.create_token(&principal, role).await?;
                show(json, &t, |t| {
                    println!(
                        "created {} for {} ({})",
                        t.info.id,
                        t.info.principal,
                        output::name(&t.info.role)
                    );
                    println!("token: {}", t.token);
                    println!("This is shown once. Pass it with --token or OPENMHS_TOKEN.");
                });
            }
            TokensCmd::List => show(json, &client.tokens().await?, |t| output::tokens(t)),
            TokensCmd::Revoke { id } => {
                let t = client.revoke_token(&id).await?;
                show(json, &t, |t| println!("revoked {} ({})", t.id, t.principal));
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
