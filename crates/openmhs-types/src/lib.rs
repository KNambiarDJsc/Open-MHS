//! OpenMHS specification types.
//!
//! This crate is the Rust binding of the language-neutral specification in `spec/`:
//! descriptor types, the unit table, the command lifecycle, and validation.
//! It contains no hardware-specific logic.

pub mod descriptor;
pub mod issue;
pub mod lifecycle;
pub mod schema;
pub mod units;
pub mod validate;
pub mod value;

pub use descriptor::Descriptor;
pub use issue::Issue;
pub use lifecycle::{CommandState, Outcome};
pub use validate::validate_descriptor;
pub use value::{Quantity, Value};
