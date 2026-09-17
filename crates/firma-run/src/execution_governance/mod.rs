//! Pluggable governance for whether `sidecar_local_exec.allowed_executables`
//! reaches a sandboxed process's *descendants*, not just the root command.
//!
//! `mediator::enforce_local_command_governance` already checks the root
//! command against `allowed_executables` regardless of which strategy is
//! selected here — this module is strictly additive to that, extending
//! (or not, for [`ExecutionGovernanceStrategy::Inherited`]) the same
//! restriction to processes the root command itself spawns. See
//! `docs/architecture/selectable-execution-governance-plan.md` (this
//! module's own design) and
//! `docs/architecture/ptrace-seccomp-exec-gate-plan.md`
//! ([`ExecutionGovernanceStrategy::PtraceSeccompExec`]'s implementation).

mod inherited;
#[cfg(target_os = "linux")]
pub mod ptrace_seccomp;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Child;

pub use inherited::InheritedGovernor;
#[cfg(target_os = "linux")]
pub use ptrace_seccomp::PtraceSeccompGovernor;

use crate::backend::BackendKind;
use crate::config::ExecutionGovernanceStrategy;
use crate::error::RunError;

/// Executables the sandboxed process (and, depending on the selected
/// strategy, its descendants) may run.
///
/// A distinct type over a raw `BTreeSet<PathBuf>`: it marks that the set has
/// already been through `resolve_profile`'s own
/// `canonicalize_allowed_executables`, so a governor implementation cannot
/// accidentally compare an un-canonicalized path against it (a governor
/// receives this type, never the raw config value).
#[derive(Debug, Clone, Default)]
pub struct AllowedExecutables(BTreeSet<PathBuf>);

impl AllowedExecutables {
    #[must_use]
    pub fn new(paths: BTreeSet<PathBuf>) -> Self {
        Self(paths)
    }

    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.0.contains(path)
    }

    /// Iterates the allowed paths themselves.
    ///
    /// Used by [`ptrace_seccomp`]'s post-exec identity re-verification,
    /// which needs to `stat` each allowed path (not just look one up by
    /// exact path string) to compare by device+inode rather than by path.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().map(PathBuf::as_path)
    }
}

/// State threaded from a governor's `rewrite_launch` to its own `supervise`.
///
/// Each strategy that needs state gets its own variant; strategies with
/// none use [`Self::None`].
pub enum GovernanceHandle {
    None,
    #[cfg(target_os = "linux")]
    PtraceSeccompExec(ptrace_seccomp::Handle),
}

/// A pluggable mechanism for extending `allowed_executables` enforcement to
/// a sandboxed process's descendants.
///
/// `rewrite_launch` runs before the backend spawns the child (an
/// opportunity to rewrite the command that will actually be exec'd, e.g. to
/// insert a wrapping shim); `supervise` replaces
/// `supervisor::wait_with_signal_forwarding` once the child exists, so a
/// strategy that needs to observe the child's own syscalls can own the wait
/// loop instead of racing a second one.
pub trait ExecutionGovernor: Send + Sync {
    /// Called before the backend spawns the child. May rewrite `executable`/
    /// `args` in place (e.g. to wrap them in a shim binary) and returns a
    /// handle carrying whatever state `supervise` needs.
    ///
    /// `sandbox_runtime_dir` is the prepared sandbox's own
    /// `SandboxHandle::runtime_dir` — already bind-mounted into the
    /// sandbox's mount namespace by the backend (the same directory
    /// `egress_guard`'s own handshake socket lives in), unlike an
    /// independently created host temp directory, which a strategy that
    /// needs a filesystem path reachable from *inside* the sandbox must not
    /// use instead.
    ///
    /// # Errors
    ///
    /// Returns an error when the strategy's own launch-time setup (e.g.
    /// installing a filter, creating a handshake socket) fails.
    fn rewrite_launch(
        &self,
        allowed: &AllowedExecutables,
        sandbox_runtime_dir: &Path,
        executable: &mut String,
        args: &mut Vec<String>,
    ) -> Result<GovernanceHandle, RunError>;

    /// Replaces `wait_with_signal_forwarding` once the backend has spawned
    /// `child`. Owns waiting for it to exit, forwarding SIGINT/SIGTERM/
    /// SIGWINCH the same way `wait_with_signal_forwarding` does, and (for
    /// strategies that need it) descendant-exec enforcement. Returns the
    /// same exit-code convention `wait_with_signal_forwarding` already
    /// establishes (`supervisor::exit_code_from_outcome`).
    ///
    /// # Errors
    ///
    /// Returns an error when the wait loop itself fails (not when the
    /// wrapped command exits non-zero — that is reported via the returned
    /// exit code).
    fn supervise(
        &self,
        handle: GovernanceHandle,
        child: Child,
        backend: BackendKind,
    ) -> Result<i32, RunError>;
}

/// Construct the governor for a resolved strategy.
///
/// `PtraceSeccompExec` is only ever resolved on Linux in practice —
/// `config::validate_execution_governance_preconditions` requires
/// `BackendKind::Bwrap`, itself only ever selectable on Linux
/// (`backend_supported_on_host`) — so the non-Linux arm below is genuinely
/// unreachable through `resolve_profile`, not a silent downgrade.
#[must_use]
pub fn build_governor(strategy: ExecutionGovernanceStrategy) -> Box<dyn ExecutionGovernor> {
    match strategy {
        ExecutionGovernanceStrategy::Inherited => Box::new(InheritedGovernor),
        #[cfg(target_os = "linux")]
        ExecutionGovernanceStrategy::PtraceSeccompExec => Box::new(PtraceSeccompGovernor),
        #[cfg(not(target_os = "linux"))]
        ExecutionGovernanceStrategy::PtraceSeccompExec => unreachable!(
            "PtraceSeccompExec requires BackendKind::Bwrap, which \
             backend_supported_on_host never selects outside Linux — \
             config resolution rejects this combination before build_governor \
             is ever called"
        ),
    }
}
