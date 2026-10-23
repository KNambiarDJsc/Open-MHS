//! Serve the adapter in a gateway, with the fake lamp standing in for the device.
//!
//! ```sh
//! cargo run --example serve -- --home ./lamp-home
//! openmhs --home ./lamp-home devices list          # in another terminal
//! ```
//!
//! To drive real hardware, replace `FakeLamp` with a `TcpLink` to the device's address.
//! Only do that with the operator's approval, and with the device's own safety systems
//! in place: OpenMHS is not one.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use openmhs_adapter_template::LampAdapter;
use openmhs_adapter_template::fixture::FakeLamp;
use openmhs_gateway::GatewayConfig;
use openmhs_types::SystemClock;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let home = match (args.next().as_deref(), args.next()) {
        (Some("--home"), Some(dir)) => dir,
        _ => {
            eprintln!("usage: serve --home <dir>");
            std::process::exit(2);
        }
    };

    let lamp = FakeLamp::new();
    let adapter = LampAdapter::new(lamp, Arc::new(SystemClock));

    let mut config = GatewayConfig::new(&home);
    config.listen = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    config.adapters.push(Arc::new(adapter));
    let gateway = match openmhs_gateway::start(config).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    println!("gateway at {} (home {home}); Ctrl-C to stop", gateway.url);
    let _ = tokio::signal::ctrl_c().await;
    if let Err(e) = gateway.shutdown().await {
        eprintln!("error during shutdown: {e}");
    }
}
