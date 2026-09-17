//! Firma Run runtime wrapper.
//!
//! Provides the `firma run` command used to wrap agent processes behind a
//! sandbox backend and sidecar routing contract.

pub mod authority;
pub mod backend;
pub mod capability;
pub mod config;
pub mod dns_stub;
#[cfg(target_os = "linux")]
pub mod egress_guard;
pub mod env;
pub mod error;
/// Pluggable descendant-process exec governance.
///
/// The `PtraceSeccompExec` strategy's shim-facing pieces
/// ([`execution_governance::ptrace_seccomp`]) are called from the
/// `crates/firma` `__exec-guarded-run` binary, so this module is `pub`
/// (like [`egress_guard`], for the same reason), even though `firma-run`
/// itself is an internal, `publish = false` crate not subject to semver.
pub mod execution_governance;
pub mod identity;
pub mod log;
pub(crate) mod mediator;
pub(crate) mod profile;
pub mod proxy_bridge;
pub mod routing;
pub mod runtime;
pub mod seccomp;
pub(crate) mod secret;
pub mod sidecar;
pub(crate) mod supervisor;
pub(crate) mod trust;
