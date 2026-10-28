//! `openmhs bench`: wall-clock measurements of a real gateway, through its real API.
//!
//! A throwaway gateway with the virtual lab runs in this process; every measurement is a
//! client call over loopback HTTP, as any client makes it. The numbers describe this machine,
//! this build profile and simulated devices: they are not a capacity claim for hardware or for
//! a fleet, and the report says what it was run on.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use openmhs_api::{Client, ClientError, CreateTokenParams, EventsPollParams, Role, WatchParams};
use openmhs_gateway::GatewayConfig;
use openmhs_policy::{Effect, Grant};
use openmhs_types::descriptor::Operation;
use openmhs_types::event::EventKind;
use serde::Serialize;
use serde_json::json;

use crate::exit;

const PSU: &str = "sim.psu-01";
const SETPOINT: &str = "power.output.voltage.setpoint";
const MEASURED: &str = "power.output.voltage.measured";

#[derive(Debug, Serialize)]
pub struct Stats {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

fn stats(mut d: Vec<Duration>) -> Stats {
    d.sort_unstable();
    let at = |q: f64| {
        let i = ((d.len() as f64 - 1.0) * q).round() as usize;
        d.get(i).map_or(0.0, |x| x.as_secs_f64() * 1e3)
    };
    Stats {
        samples: d.len(),
        p50_ms: at(0.50),
        p95_ms: at(0.95),
        p99_ms: at(0.99),
        max_ms: at(1.0),
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub openmhs: &'static str,
    pub profile: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub cpus: usize,
    /// One audited `state.read` at a time.
    pub read: Stats,
    /// Reads from `concurrency` clients at once.
    pub concurrent_reads: Concurrent,
    /// One verified write at a time: submit to `VERIFIED`, including the declared 50 ms
    /// settle before readback. Paced under the setpoint's rate limit (5 per second).
    pub command: Stats,
    /// From a read returning to a waiting `events.poll` returning its observation.
    pub event_delivery: Stats,
    /// Samples a 200 ms watch delivered in `watch_seconds`, against the ideal count.
    pub watch: WatchStats,
    pub note: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Concurrent {
    pub concurrency: usize,
    pub reads: usize,
    pub seconds: f64,
    pub reads_per_second: f64,
    pub latency: Stats,
}

#[derive(Debug, Serialize)]
pub struct WatchStats {
    pub interval_ms: u64,
    pub watch_seconds: f64,
    pub samples: usize,
    pub expected: usize,
    /// Gaps between consecutive samples' publish times.
    pub gap: Stats,
}

fn temp_home() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("openmhs-bench-{}-{stamp}", std::process::id()))
}

pub async fn run(json_out: bool, reads: usize, commands: usize, concurrency: usize) -> u8 {
    let home = temp_home();
    let mut config = GatewayConfig::new(&home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.simulation = true;
    let gateway = match openmhs_gateway::start(config).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: cannot start the benchmark gateway: {e}");
            return exit::ERROR;
        }
    };
    let result = measure(
        &gateway.url,
        &gateway.operator_token,
        reads,
        commands,
        concurrency,
    )
    .await;
    let _ = gateway.shutdown().await;
    let _ = std::fs::remove_dir_all(&home);
    match result {
        Ok(report) => {
            if json_out {
                crate::output::json(&report);
            } else {
                print_report(&report);
            }
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            exit::ERROR
        }
    }
}

async fn measure(
    url: &str,
    operator_token: &str,
    reads: usize,
    commands: usize,
    concurrency: usize,
) -> Result<Report, ClientError> {
    let operator = Client::new(url, operator_token)?;
    for (id, ops) in [
        ("bench-read", vec![Operation::Read, Operation::Subscribe]),
        ("bench-write", vec![Operation::Write]),
    ] {
        operator
            .add_grant(Grant {
                id: id.into(),
                principal: "agent:*".into(),
                device: PSU.into(),
                capability: "*".into(),
                operations: ops,
                effect: Effect::Allow,
                max_side_effect: None,
                reason: None,
            })
            .await?;
    }
    let token = operator
        .create_token(CreateTokenParams::unrestricted("agent:bench", Role::Agent))
        .await?;
    let agent = Client::new(url, &token.token)?;

    // Warm up connections and code paths; not measured.
    for _ in 0..5 {
        agent.read(PSU, MEASURED).await?;
    }

    let mut read = Vec::with_capacity(reads);
    for _ in 0..reads {
        let t = Instant::now();
        agent.read(PSU, MEASURED).await?;
        read.push(t.elapsed());
    }

    let per_client = (reads / concurrency.max(1)).max(1);
    let started = Instant::now();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..concurrency.max(1) {
        let client = Client::new(url, &token.token)?;
        tasks.spawn(async move {
            let mut lat = Vec::with_capacity(per_client);
            for _ in 0..per_client {
                let t = Instant::now();
                client.read(PSU, MEASURED).await?;
                lat.push(t.elapsed());
            }
            Ok::<_, ClientError>(lat)
        });
    }
    let mut all = Vec::new();
    while let Some(done) = tasks.join_next().await {
        if let Ok(lat) = done {
            all.extend(lat?);
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    let concurrent_reads = Concurrent {
        concurrency: concurrency.max(1),
        reads: all.len(),
        seconds,
        reads_per_second: all.len() as f64 / seconds.max(1e-9),
        latency: stats(all),
    };

    let mut command = Vec::with_capacity(commands);
    for i in 0..commands {
        let v = if i % 2 == 0 { 1.0 } else { 1.2 };
        let request = json!({ "target": { "device": PSU, "capability": SETPOINT },
                              "operation": "write", "value": { "value": v, "unit": "V" } });
        let t = Instant::now();
        let r = agent.submit(request).await?;
        command.push(t.elapsed());
        if r.state != openmhs_types::CommandState::Verified {
            return Err(ClientError::Protocol(format!(
                "a benchmark command ended {:?}, not VERIFIED",
                r.state
            )));
        }
        // The setpoint allows 5 changes a second.
        tokio::time::sleep(Duration::from_millis(210)).await;
    }

    let mut event_delivery = Vec::new();
    let observations = |after| EventsPollParams {
        after: Some(after),
        kinds: vec![EventKind::Observation],
        wait_ms: 5_000,
        limit: Some(10),
        ..EventsPollParams::default()
    };
    for _ in 0..(reads / 10).max(10) {
        let start = agent.poll_events(EventsPollParams::default()).await?;
        let waiter = {
            let client = Client::new(url, &token.token)?;
            let params = observations(start.next);
            tokio::spawn(async move {
                let b = client.poll_events(params).await;
                (Instant::now(), b)
            })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        agent.read(PSU, MEASURED).await?;
        let read_done = Instant::now();
        if let Ok((arrived, Ok(b))) = waiter.await
            && !b.events.is_empty()
        {
            event_delivery.push(arrived.saturating_duration_since(read_done));
        }
    }

    let watch_seconds = 3.0;
    let start = agent.poll_events(EventsPollParams::default()).await?;
    agent
        .create_watch(WatchParams {
            device: PSU.into(),
            capability: MEASURED.into(),
            interval_ms: 200,
            ttl_ms: Some(60_000),
        })
        .await?;
    tokio::time::sleep(Duration::from_secs_f64(watch_seconds)).await;
    let mut cursor = start.next;
    let mut times = Vec::new();
    loop {
        let mut p = observations(cursor);
        p.wait_ms = 0;
        p.limit = Some(500);
        let b = agent.poll_events(p).await?;
        for e in &b.events {
            if matches!(&e.body, openmhs_types::event::EventBody::Observation { cause, .. }
                if *cause == openmhs_types::event::ObservationCause::Watch)
            {
                times.push(e.at.millis());
            }
        }
        if b.events.is_empty() {
            break;
        }
        cursor = b.next;
    }
    let gaps: Vec<Duration> = times
        .windows(2)
        .map(|w| Duration::from_millis(u64::try_from(w[1] - w[0]).unwrap_or(0)))
        .collect();

    Ok(Report {
        openmhs: env!("CARGO_PKG_VERSION"),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        cpus: std::thread::available_parallelism().map_or(1, usize::from),
        read: stats(read),
        concurrent_reads,
        command: stats(command),
        event_delivery: stats(event_delivery),
        watch: WatchStats {
            interval_ms: 200,
            watch_seconds,
            samples: times.len(),
            expected: (watch_seconds * 1000.0 / 200.0) as usize,
            gap: stats(gaps),
        },
        note: "One machine, simulated devices, loopback HTTP. Not a capacity claim for hardware or fleets.",
    })
}

fn line(name: &str, s: &Stats) {
    println!(
        "  {name:<26} p50 {:>7.2} ms   p95 {:>7.2} ms   p99 {:>7.2} ms   max {:>7.2} ms   (n={})",
        s.p50_ms, s.p95_ms, s.p99_ms, s.max_ms, s.samples
    );
}

fn print_report(r: &Report) {
    println!(
        "OpenMHS {} benchmark ({} build, {} {}, {} CPUs). Simulated devices only.",
        r.openmhs, r.profile, r.os, r.arch, r.cpus
    );
    if r.profile == "debug" {
        println!("  (a debug build: run a release build for representative numbers)");
    }
    line("state.read", &r.read);
    line(
        &format!("state.read x{} clients", r.concurrent_reads.concurrency),
        &r.concurrent_reads.latency,
    );
    println!(
        "  {:<26} {:.0} reads/s over {:.2} s",
        "", r.concurrent_reads.reads_per_second, r.concurrent_reads.seconds
    );
    line("write to VERIFIED", &r.command);
    line("event delivery", &r.event_delivery);
    println!(
        "  {:<26} {} of {} samples at {} ms",
        "watch", r.watch.samples, r.watch.expected, r.watch.interval_ms
    );
    line("watch sample gap", &r.watch.gap);
    println!("{}", r.note);
}
