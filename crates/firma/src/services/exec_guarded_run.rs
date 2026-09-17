//! Runner for `firma __exec-guarded-run`.
//!
//! Runs inside the sandbox as the agent's launcher: installs the
//! `execve`/`execveat`-only `SECCOMP_RET_TRACE` filter, performs the
//! readiness handshake with the host supervisor at `--handshake-socket`,
//! then `execve`s the wrapped command. On any failure it returns an error
//! so the wrapped command never starts — fail closed.

use std::process::ExitCode;

use crate::args::run::ExecGuardedRunArgs;

/// Install the `PtraceSeccompExec` exec-gate filter and exec the wrapped
/// command.
///
/// # Errors
///
/// Returns an error when the filter cannot be installed, the handshake with
/// the host supervisor fails, or `exec` fails. On success this never returns
/// (the process image is replaced by the wrapped command).
#[cfg(target_os = "linux")]
pub fn run(args: ExecGuardedRunArgs) -> anyhow::Result<ExitCode> {
    let ExecGuardedRunArgs {
        handshake_socket,
        command,
    } = args;
    let never = firma_run::execution_governance::ptrace_seccomp::install_and_wait_for_ready(
        &handshake_socket,
        &command,
    )?;
    match never {}
}

/// The `PtraceSeccompExec` exec gate has no non-Linux implementation. The
/// `__exec-guarded-run` wrapper is only ever spawned by that strategy, which
/// is itself Linux-only, so on other targets it fails closed rather than
/// silently running the wrapped command unguarded.
///
/// # Errors
///
/// Always returns an error: the exec gate is Linux-only.
#[cfg(not(target_os = "linux"))]
pub fn run(_args: ExecGuardedRunArgs) -> anyhow::Result<ExitCode> {
    anyhow::bail!("the ptrace/seccomp exec gate is only supported on Linux")
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use std::path::PathBuf;

    use super::run;
    use crate::args::run::ExecGuardedRunArgs;

    #[test]
    fn run_fails_closed_when_handshake_socket_is_unreachable() {
        let args = ExecGuardedRunArgs {
            handshake_socket: PathBuf::from("/nonexistent/firma-exec-guard.sock"),
            command: vec!["/bin/true".to_string()],
        };
        assert!(run(args).is_err());
    }
}
