//! The OpenMHS adapter host: adapters as separate processes, in any language.
//!
//! An adapter process speaks JSON-RPC 2.0 over its stdin and stdout
//! (spec/adapter-protocol.md) and comes with a permission manifest
//! (spec/permission-manifest.md). [`ProcessAdapter`] makes such a process look like any
//! other [`openmhs_adapter::HardwareAdapter`], so the engine, the policy and the
//! conformance suite treat it exactly like an in-process Rust adapter.
//!
//! ```no_run
//! # async fn demo() -> Result<(), String> {
//! use openmhs_host::{HostOptions, ProcessAdapter, Resolved};
//! let manifest = Resolved::load(std::path::Path::new("adapters/python/psu-sim/manifest.json"))?;
//! let adapter = ProcessAdapter::start(manifest, HostOptions::unsigned()).await?;
//! # Ok(()) }
//! ```

pub mod manifest;
mod process;
pub mod protocol;
pub mod trust;

pub use manifest::{MANIFEST_SCHEMA, Manifest, Resolved};
pub use process::{HostOptions, OS_ENV, ProcessAdapter, ProcessFixture, Status, Trust};
pub use trust::{TrustError, TrustStore, TrustedKey};
