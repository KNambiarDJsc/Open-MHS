//! OpenMHS specification types.
//!
//! This crate is the Rust binding of the language-neutral specification in `spec/`:
//! descriptor types, the unit table, the command lifecycle, wire types, and validation.
//! It contains no hardware-specific logic.

pub mod command;
pub mod descriptor;
pub mod error;
pub mod issue;
pub mod lifecycle;
pub mod principal;
pub mod receipt;
pub mod schema;
pub mod state;
pub mod time;
pub mod units;
pub mod validate;
pub mod value;

pub use command::{CommandRequest, RequestOperation, Target};
pub use descriptor::Descriptor;
pub use error::ErrorObject;
pub use issue::Issue;
pub use lifecycle::{CommandState, Outcome};
pub use principal::is_principal;
pub use receipt::Receipt;
pub use state::{Quality, Source, StateValue};
pub use time::{Clock, SystemClock, Timestamp};
pub use validate::validate_descriptor;
pub use value::{Quantity, Value};
