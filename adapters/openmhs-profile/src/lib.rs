//! Device profiles for OpenMHS protocol adapters.
//!
//! A **profile** is one JSON file describing one device instance: its descriptor (what
//! it is and can do), a protocol **mapping** (how each capability is read and commanded
//! over Modbus, MQTT, SCPI or HTTP) and the **connection** (where it is). The protocol
//! adapters (`openmhs-modbus`, `openmhs-mqtt`, `openmhs-scpi`, `openmhs-http`) load
//! profiles through [`load`], which validates the descriptor and checks that every
//! capability is mapped consistently, so a bad profile fails before anything connects.
//!
//! The crate also holds what every protocol needs: value codecs ([`codec`]), command
//! templates ([`template`]), and the [`VirtualDevice`] that the in-repo protocol
//! simulators serve, with conformance faults on demand.

pub mod codec;
mod device;
mod profile;
pub mod template;

pub use device::{Outcome, VirtualDevice};
pub use profile::{FORMAT_VERSION, Loaded, Mapping, ProfileError, SimSettings, load, protocol_of};
