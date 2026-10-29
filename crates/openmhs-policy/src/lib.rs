//! Deterministic policy for OpenMHS (spec/authorization.md, ADR-0003).
//!
//! - [`authz`]: who may perform which operation on which capability.
//! - [`constraints`]: whether a request is physically acceptable right now.
//!
//! Everything here is a pure function of its inputs. No I/O, no clocks, no models.

pub mod auth;
pub mod authz;
pub mod constraints;
mod glob;

/// `*` wildcard matching, as grant and scope patterns use it.
pub use glob::matches as glob_matches;

pub use auth::{AuthMethod, Caller, CallerRecord, PrincipalKind, Scope};
pub use authz::{AuthzDecision, Effect, Grant, Policy};
pub use constraints::{
    Expectation, Live, Mutation, Prepared, check_command, check_read, condition_holds,
    readback_matches, state_needed,
};
