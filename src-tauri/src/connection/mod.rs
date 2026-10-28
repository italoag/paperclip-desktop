//! Connection model: remote validation, saved profiles, and origin policy.
//!
//! Port of `src/connection/` from the Electron shell.

pub mod health;
pub mod http;
pub mod lifecycle;
pub mod preflight;
pub mod profiles;
pub mod types;
pub mod validate;
pub mod window_policy;
