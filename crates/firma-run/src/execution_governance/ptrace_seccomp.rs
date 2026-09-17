//! `PtraceSeccompExec`: descendant-process exec governance via ptrace.
//!
//! A host-side `ptrace(2)` attach plus a `SECCOMP_RET_TRACE` filter scoped
//! to `execve`/`execveat` traps every exec in the sandboxed process's whole
//! subtree, checking each one against `allowed_executables` before allowing
//! it to proceed. See `docs/architecture/ptrace-seccomp-exec-gate-plan.md`.
//!
//! ## Two-process design (mirrors [`crate::egress_guard`]'s own, for the
//! same reason: seccomp state is installed *inside* the sandbox, on the
//! process that will become the wrapped command, but the tracer must be a
//! process *outside* the filter's scope)
//!
//! 1. [`install_and_wait_for_ready`] runs as a thin wrapper inside the
//!    sandbox (invoked by the entrypoint as
//!    `firma __exec-guarded-run --handshake-socket <path> -- <agent> <args...>`).
//!    It installs [`EXEC_TRACE_PROG`] (unlike `egress_guard`'s filter, this
//!    returns `SECCOMP_RET_TRACE`, not `SECCOMP_RET_USER_NOTIF` — no listener
//!    fd is produced; a `ptrace(2)`-attached tracer is notified instead),
//!    then connects to the host's handshake socket, sends one readiness
//!    byte, and blocks reading one byte back before `execve`ing the wrapped
//!    command. The filter survives the exec.
//! 2. The host side (Slice 3b) seizes this process via `ptrace(2)` while
//!    step 1 is blocked waiting for the go-ahead byte — guaranteeing the
//!    tracer is already attached before the wrapped command's own `execve`
//!    can fire — then writes that byte.
//!
//! Landed in slices: this file starts with just the shim-side filter
//! install (Slice 3a). Slice 3b adds the host-side attach and unified wait
//! loop; Slice 3c adds the actual allow/deny decision logic.

#![cfg(target_os = "linux")]
#![expect(
    unsafe_code,
    reason = "seccomp filter install has no safe wrapper in nix or libc; the \
              one unsafe call here is a thin, checked FFI shim, mirroring \
              crate::egress_guard's own install_connect_notifier"
)]

use std::io::{Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nix::sys::ptrace::{self, Options};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

use super::{AllowedExecutables, ExecutionGovernor, GovernanceHandle};
use crate::backend::BackendKind;
use crate::error::RunError;
use crate::supervisor::{exit_code_from_outcome, forward_signal};

// ── seccomp / BPF constants ─────────────────────────────────────────────────
//
// Duplicated from `crate::egress_guard`'s own copies rather than shared:
// these are fixed Linux ABI constants (unlikely to ever change) and the two
// modules have no other dependency relationship: mirrors this crate's
// existing convention of duplicating small, stable wire-format constants
// across independent modules rather than introducing a shared-constants
// module for a handful of `u16`/`u32` values.

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_RET_K: u16 = 0x06;

const SECCOMP_DATA_NR_OFFSET: u32 = 0;
const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;

const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

#[cfg(target_arch = "x86_64")]
const NATIVE_AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const NATIVE_AUDIT_ARCH: u32 = 0xC000_00B7;

// `libc::SYS_execve`/`SYS_execveat` are `c_long`; the BPF filter's `k` field
// is `u32`. Cast once here and statically assert the cast is lossless, so a
// filter whose numbers silently truncated (impossible in practice — real
// syscall numbers are small positive integers — but checked rather than
// assumed) would fail to build instead of installing a broken filter.
#[expect(
    clippy::cast_possible_truncation,
    reason = "checked immediately below by a const assert comparing the cast-back value \
              against the original c_long, not merely assumed lossless"
)]
const SYS_EXECVE_NR: u32 = libc::SYS_execve as u32;
#[expect(
    clippy::cast_possible_truncation,
    reason = "checked immediately below by a const assert comparing the cast-back value \
              against the original c_long, not merely assumed lossless"
)]
const SYS_EXECVEAT_NR: u32 = libc::SYS_execveat as u32;
const _: () = assert!(SYS_EXECVE_NR as libc::c_long == libc::SYS_execve);
const _: () = assert!(SYS_EXECVEAT_NR as libc::c_long == libc::SYS_execveat);

/// Traps only `execve`/`execveat` on the native architecture, with
/// `SECCOMP_RET_TRACE`; everything else (including non-native-arch calls,
/// matching `egress_guard::CONNECT_NOTIFY_PROG`'s own choice not to police
/// a foreign ABI) is `SECCOMP_RET_ALLOW`.
///
/// Layout (7 instructions):
/// ```text
/// 0: load arch
/// 1: JEQ native_arch   jt=0 (-> 2)         jf=4 (-> 6, ALLOW)
/// 2: load syscall nr
/// 3: JEQ execve_nr     jt=1 (-> 5, TRACE)  jf=0 (-> 4)
/// 4: JEQ execveat_nr   jt=0 (-> 5, TRACE)  jf=1 (-> 6, ALLOW)
/// 5: RET TRACE
/// 6: RET ALLOW
/// ```
const EXEC_TRACE_PROG: [libc::sock_filter; 7] = [
    // 0: load arch
    libc::sock_filter {
        code: BPF_LD_W_ABS,
        jt: 0,
        jf: 0,
        k: SECCOMP_DATA_ARCH_OFFSET,
    },
    // 1: if arch != native -> allow (skip 4 to idx6)
    libc::sock_filter {
        code: BPF_JMP_JEQ_K,
        jt: 0,
        jf: 4,
        k: NATIVE_AUDIT_ARCH,
    },
    // 2: load syscall nr
    libc::sock_filter {
        code: BPF_LD_W_ABS,
        jt: 0,
        jf: 0,
        k: SECCOMP_DATA_NR_OFFSET,
    },
    // 3: if nr == execve -> trace (skip 1 to idx5)
    libc::sock_filter {
        code: BPF_JMP_JEQ_K,
        jt: 1,
        jf: 0,
        k: SYS_EXECVE_NR,
    },
    // 4: if nr == execveat -> trace (fall through to idx5); else -> allow (skip 1 to idx6)
    libc::sock_filter {
        code: BPF_JMP_JEQ_K,
        jt: 0,
        jf: 1,
        k: SYS_EXECVEAT_NR,
    },
    // 5: trace
    libc::sock_filter {
        code: BPF_RET_K,
        jt: 0,
        jf: 0,
        k: libc::SECCOMP_RET_TRACE,
    },
    // 6: allow
    libc::sock_filter {
        code: BPF_RET_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    },
];

/// Installs [`EXEC_TRACE_PROG`] on the current process.
///
/// No listener fd is produced (unlike `egress_guard`'s `SECCOMP_RET_USER_NOTIF`
/// filter): `SECCOMP_RET_TRACE` notifies a `ptrace(2)`-attached tracer
/// directly, via `PTRACE_EVENT_SECCOMP`, rather than a separate fd-based
/// channel.
fn install_trace_filter() -> std::io::Result<()> {
    // Unprivileged seccomp requires no-new-privs (same requirement as any
    // unprivileged filter install, regardless of the filter's own return
    // action).
    nix::sys::prctl::set_no_new_privs().map_err(std::io::Error::from)?;

    // Local mutable copy: `sock_fprog.filter` is `*mut`, though the kernel
    // only reads the program during `SYS_seccomp`.
    let mut program = EXEC_TRACE_PROG;
    let prog = libc::sock_fprog {
        len: u16::try_from(program.len()).unwrap_or(u16::MAX),
        filter: program.as_mut_ptr(),
    };

    // SAFETY: `SYS_seccomp` with `SECCOMP_SET_MODE_FILTER` reads `prog` (a
    // valid `sock_fprog` pointing at `program`, which outlives the call)
    // and returns 0 on success or -1 with errno set. No listener fd is
    // returned for a filter with no `SECCOMP_RET_USER_NOTIF` action, so
    // (unlike `egress_guard::install_connect_notifier`) no
    // `SECCOMP_FILTER_FLAG_NEW_LISTENER` flag is needed or passed.
    let ret = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            0,
            std::ptr::from_ref(&prog).cast::<libc::c_void>(),
        )
    };
    if ret < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// `DEC-018`: an explicit, executable allow-list of confirmed `architecture`
/// values, checked before any `ptrace(2)` call is made — not merely
/// documented — so a host outside it fails closed with an actionable error
/// naming the specific unconfirmed characteristic, rather than silently
/// attempting (and possibly partially completing) an attach on a register
/// layout that was never actually tested.
///
/// Only `aarch64` is confirmed: it is the only architecture this session
/// actually exercised end to end against a real `bwrap` sandbox (the
/// `ptrace_seccomp_exec_denies_forbidden_tool_as_child_of_allowed_bash_root`
/// e2e test). `x86_64`'s register field layout (`DEC-016`: `orig_rax`/`rax`
/// vs. `aarch64`'s `regs[8]`/`regs[0]`) compiles but has never run against
/// real hardware, so per `DEC-016` it must not ship as confirmed yet.
/// The allow-list itself, as a pure predicate over an architecture name —
/// kept separate from [`confirmed_platform`] so its negative path (an
/// unconfirmed architecture) is unit-testable without requiring an actual
/// unconfirmed host, per this plan's own focused-verification requirement
/// for Slice 3c.
fn is_confirmed_architecture(arch: &str) -> bool {
    arch == "aarch64"
}

fn confirmed_platform() -> Result<(), RunError> {
    if is_confirmed_architecture(std::env::consts::ARCH) {
        return Ok(());
    }
    let yama_scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
        .ok()
        .map(|scope| scope.trim().to_string());
    Err(RunError::Internal(format!(
        "execution_governance = \"ptrace_seccomp_exec\" is not yet confirmed on this \
         architecture ({arch}) — only aarch64 has a passing real-hardware register-access \
         test (see DEC-016 in docs/architecture/ptrace-seccomp-exec-gate-plan.md); \
         yama ptrace_scope={yama_scope:?}",
        arch = std::env::consts::ARCH,
    )))
}

/// Installs the exec-trace filter, then performs the readiness handshake
/// with the host supervisor and `execve`s the wrapped command.
///
/// # Errors
///
/// Returns an error when the filter cannot be installed, the handshake
/// socket is unreachable, or `exec` fails. On success this never returns
/// (the process image is replaced by the wrapped command).
pub fn install_and_wait_for_ready(
    handshake_socket: &Path,
    argv: &[String],
) -> Result<std::convert::Infallible, RunError> {
    let (executable, agent_args) = argv
        .split_first()
        .ok_or_else(|| RunError::Internal("exec guard: empty agent argv".to_string()))?;

    install_trace_filter()
        .map_err(|error| RunError::Internal(format!("exec guard: install filter: {error}")))?;

    // Installed *before* connecting (unlike `egress_guard::install_and_exec`,
    // which connects first): `connect(2)` is not one of the two syscalls
    // this filter traps, so installing first does not self-trap the
    // handshake, and it means the filter is already active for the rest of
    // this process's lifetime, including if a future syscall this process
    // itself makes were ever added to the trap set.
    let mut stream = UnixStream::connect(handshake_socket).map_err(|error| {
        RunError::Internal(format!(
            "exec guard: connect handshake socket {}: {error}",
            handshake_socket.display()
        ))
    })?;

    // Signal readiness (filter installed), then block for the host's own
    // go-ahead byte — sent only once the host has seized this process via
    // `ptrace(2)` (Slice 3b), guaranteeing a tracer is attached before the
    // exec below can fire.
    stream
        .write_all(&[0_u8])
        .map_err(|error| RunError::Internal(format!("exec guard: send ready byte: {error}")))?;
    let mut go_ahead = [0_u8; 1];
    stream.read_exact(&mut go_ahead).map_err(|error| {
        RunError::Internal(format!("exec guard: read host go-ahead byte: {error}"))
    })?;
    drop(stream);

    // `exec` keeps the installed seccomp filter; the agent inherits it.
    let error = std::process::Command::new(executable)
        .args(agent_args)
        .exec();
    Err(RunError::Spawn(format!(
        "exec guard: exec {executable}: {error}"
    )))
}

/// State threaded from `rewrite_launch` to `supervise`.
///
/// Holds the handshake listener, already bound before the backend spawns
/// the child (the shim connects to it once it execs). The socket lives
/// under the sandbox's own runtime directory (removed by the backend's own
/// teardown), not a directory this module creates or owns. Also carries
/// the resolved `allowed` set forward, since `supervise` (not
/// `rewrite_launch`) is where each descendant's exec is actually decided.
pub struct Handle {
    listener: UnixListener,
    allowed: AllowedExecutables,
}

pub struct PtraceSeccompGovernor;

impl ExecutionGovernor for PtraceSeccompGovernor {
    fn rewrite_launch(
        &self,
        allowed: &AllowedExecutables,
        sandbox_runtime_dir: &Path,
        executable: &mut String,
        args: &mut Vec<String>,
    ) -> Result<GovernanceHandle, RunError> {
        // `DEC-018`'s "confirmed platforms" gate, checked here rather than
        // at `supervise()` entry as the plan's own Choice paragraph names:
        // rejecting here fails closed strictly earlier — before the
        // sandboxed process is even spawned by the backend, not merely
        // before this governor's own first `ptrace(2)` call — which the
        // same underlying rationale ("fail before any ptrace call is
        // made") supports just as well.
        confirmed_platform()?;

        // Must live under `sandbox_runtime_dir`, not an independent host
        // temp directory: the shim connects to this socket from *inside*
        // the bwrap mount namespace, which only has the sandbox's own
        // runtime directory (and configured mounts) bind-mounted in — a
        // freshly created `tempfile::tempdir()` elsewhere on the host is
        // invisible to it (confirmed empirically: the shim's `connect(2)`
        // failed with ENOENT under a real bwrap sandbox before this fix).
        let socket_path = sandbox_runtime_dir.join("firma-exec-guard.sock");
        let listener = UnixListener::bind(&socket_path).map_err(|error| {
            RunError::Internal(format!(
                "exec guard: bind handshake socket {}: {error}",
                socket_path.display()
            ))
        })?;

        let firma_exe = std::env::current_exe().map_err(|error| {
            RunError::Internal(format!("exec guard: locate current executable: {error}"))
        })?;

        let mut new_args = vec![
            "__exec-guarded-run".to_string(),
            "--handshake-socket".to_string(),
            socket_path.to_string_lossy().into_owned(),
            "--".to_string(),
            std::mem::take(executable),
        ];
        new_args.append(args);

        *executable = firma_exe.to_string_lossy().into_owned();
        *args = new_args;

        Ok(GovernanceHandle::PtraceSeccompExec(Handle {
            listener,
            allowed: allowed.clone(),
        }))
    }

    fn supervise(
        &self,
        handle: GovernanceHandle,
        child: Child,
        backend: BackendKind,
    ) -> Result<i32, RunError> {
        let GovernanceHandle::PtraceSeccompExec(handle) = handle else {
            return Err(RunError::Internal(
                "ptrace_seccomp governor received a foreign GovernanceHandle variant".to_string(),
            ));
        };
        supervise_ptrace_loop(handle, child, backend)
    }
}

/// The atomic `PTRACE_SEIZE` option set (`DEC-013`): stop at `execve`
/// (`PTRACE_O_TRACEEXEC` — without this, every successful exec is an
/// ambiguous plain-`SIGTRAP` stop instead of a distinguishable
/// `PTRACE_EVENT_EXEC`), at `fork`/`vfork`/`clone` (so descendant processes
/// are automatically traced too, not just the root — the entire point of
/// this strategy over `Inherited`'s root-only coverage), and at
/// `SECCOMP_RET_TRACE` triggers (`PTRACE_O_TRACESECCOMP` — the actual
/// exec-gate decision point, Slice 3c).
fn seize_options() -> Options {
    Options::PTRACE_O_TRACESECCOMP
        | Options::PTRACE_O_TRACEEXEC
        | Options::PTRACE_O_TRACEFORK
        | Options::PTRACE_O_TRACECLONE
        | Options::PTRACE_O_TRACEVFORK
}

/// Attaches to the sandboxed process, completes the handshake, then runs
/// the unified wait loop until the *root* traced process (not any
/// descendant) exits.
///
/// Slice 3b: every `PTRACE_EVENT_SECCOMP` stop is unconditionally
/// continued — no allow/deny decision yet (Slice 3c adds it). This slice's
/// own job is proving trap delivery reaches descendants, signal forwarding
/// still works, and exit/signal-death reporting matches
/// `wait_with_signal_forwarding`'s existing behavior.
///
/// # Errors
///
/// Returns an error when the handshake, the `ptrace(2)` seize, or the wait
/// loop itself fails — not when the wrapped command exits non-zero (that
/// is reported via the returned exit code).
fn supervise_ptrace_loop(
    handle: Handle,
    mut child: Child,
    backend: BackendKind,
) -> Result<i32, RunError> {
    let Handle { listener, allowed } = handle;
    let bwrap_pid = child.id();

    // Blocks until the shim (having already execed, deep inside bwrap's own
    // process tree) connects and completes step 1 of its own handshake.
    let (mut conn, _) = listener
        .accept()
        .map_err(|error| RunError::Internal(format!("exec guard: accept handshake: {error}")))?;
    let mut ready = [0_u8; 1];
    conn.read_exact(&mut ready).map_err(|error| {
        RunError::Internal(format!("exec guard: read shim ready byte: {error}"))
    })?;

    // `SO_PEERCRED` gives the connecting process's pid as the kernel
    // translates it into *this* (the accepting) process's own pid
    // namespace — not `sandbox_child_pid`'s `/proc/<bwrap_pid>/.../children`
    // guess, which can name an intermediate bwrap setup process rather than
    // the actual shim that is about to `execve` (confirmed empirically: that
    // approach seized the wrong pid, so the real shim's exec still hit
    // `SECCOMP_RET_TRACE` with no tracer attached and failed `ENOSYS`,
    // exactly as it would with no tracer at all). The peer of this
    // connection is, by construction, the shim itself — no polling or
    // guessing required.
    let credentials =
        nix::sys::socket::getsockopt(&conn, nix::sys::socket::sockopt::PeerCredentials).map_err(
            |error| RunError::Internal(format!("exec guard: read shim peer credentials: {error}")),
        )?;
    let traced_pid = Pid::from_raw(credentials.pid());

    // One atomic PTRACE_SEIZE call with every option set together — not a
    // separate attach-then-setoptions pair, closing the window where the
    // tracee could otherwise run briefly unobserved between the two calls.
    ptrace::seize(traced_pid, seize_options()).map_err(|error| {
        RunError::Internal(format!("exec guard: ptrace(2) seize {traced_pid}: {error}"))
    })?;

    // Only after the seize completes: the shim is still blocked reading
    // this byte, so the tracer is guaranteed attached before the wrapped
    // command's own `execve` can fire.
    conn.write_all(&[0_u8])
        .map_err(|error| RunError::Internal(format!("exec guard: send go-ahead byte: {error}")))?;
    drop(conn);

    let forwarder_shutdown = Arc::new(AtomicBool::new(false));
    let forwarder = {
        let shutdown = Arc::clone(&forwarder_shutdown);
        std::thread::spawn(move || forward_signals_until(bwrap_pid, backend, &shutdown))
    };

    let result = ptrace_wait_loop(traced_pid, &allowed);

    forwarder_shutdown.store(true, Ordering::SeqCst);
    let _ = forwarder.join();
    // The backend's own Child still needs reaping; the root traced process
    // and `bwrap_pid` are not the same pid (see `sandbox_child_pid`'s own
    // doc comment), so this wait is independent of `ptrace_wait_loop`'s own
    // `waitpid` calls, which never observe `bwrap_pid` itself (bwrap is
    // never a tracee).
    let _ = child.wait();

    result
}

/// Forwards SIGINT/SIGTERM/SIGWINCH into the sandbox via
/// [`forward_signal`] — the same signal-forwarding behavior and
/// escalate-to-`SIGKILL`-on-repeat semantics `wait_with_signal_forwarding`
/// already establishes, reused rather than duplicated (`DEC-014`).
/// `bwrap`'s own process-group signaling (already correct for this
/// strategy, which is bwrap-only per the config-resolution compatibility
/// gate) reaches the traced process the same way it reaches every other
/// process in the sandbox, so no ptrace-specific signal-delivery path is
/// needed here.
fn forward_signals_until(bwrap_pid: u32, backend: BackendKind, shutdown: &AtomicBool) {
    use signal_hook::consts::{SIGINT, SIGTERM, SIGWINCH};
    use signal_hook::iterator::Signals;

    let Ok(mut signals) = Signals::new([SIGINT, SIGTERM, SIGWINCH]) else {
        return;
    };
    let handle = signals.handle();
    let mut termination_requested = false;

    // `forever()` blocks; poll `shutdown` between signals rather than
    // relying solely on `handle.close()` racing a delivery, mirroring the
    // bounded-wait shape `wait_with_signal_forwarding` itself avoids
    // needing only because it has just one waiter (this loop has two:
    // shutdown and the next signal).
    while !shutdown.load(Ordering::SeqCst) {
        for raw in signals.pending() {
            let Ok(signal) = nix::sys::signal::Signal::try_from(raw) else {
                continue;
            };
            match signal {
                nix::sys::signal::Signal::SIGWINCH => {
                    forward_signal(bwrap_pid, backend, nix::sys::signal::Signal::SIGWINCH);
                }
                nix::sys::signal::Signal::SIGINT | nix::sys::signal::Signal::SIGTERM => {
                    let forwarded = if termination_requested {
                        nix::sys::signal::Signal::SIGKILL
                    } else {
                        termination_requested = true;
                        signal
                    };
                    forward_signal(bwrap_pid, backend, forwarded);
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    handle.close();
}

/// The unified wait loop: multiplexes ptrace events (seccomp traps, exec/
/// fork/clone/vfork notifications) with ordinary process-exit reporting
/// for every tracee, until `root` itself exits. Descendants exiting along
/// the way are reaped (so they don't accumulate as zombies) but do not end
/// the loop.
///
/// `WaitPidFlag::__WALL` is required, not optional: `firma-run` is not the
/// traced processes' biological parent (`bwrap` is — see
/// `sandbox_child_pid`'s own doc comment), and without it `waitpid` can
/// silently fail to report a non-biological tracee's stops on this kernel
/// (confirmed empirically for this implementation, not assumed from the
/// `wait(2)` man page alone — the plan this implements left this exact
/// question open).
fn ptrace_wait_loop(root: Pid, allowed: &AllowedExecutables) -> Result<i32, RunError> {
    loop {
        let status = waitpid(None, Some(WaitPidFlag::__WALL))
            .map_err(|error| RunError::Wait(format!("exec guard: waitpid: {error}")))?;

        match status {
            WaitStatus::Exited(pid, code) if pid == root => {
                return Ok(exit_code_from_outcome(Some(code), None));
            }
            WaitStatus::Signaled(pid, signal, _core_dumped) if pid == root => {
                return Ok(exit_code_from_outcome(None, Some(signal as i32)));
            }
            // A non-root descendant exited (`Exited`/`Signaled`), or a
            // `Continued`/`StillAlive` report that should not occur (no
            // `WCONTINUED`/`WNOHANG` was requested): nothing to do but keep
            // waiting for the root either way.
            WaitStatus::Exited(..)
            | WaitStatus::Signaled(..)
            | WaitStatus::Continued(_)
            | WaitStatus::StillAlive => {}
            WaitStatus::PtraceEvent(pid, _signal, event) => {
                if event == ptrace::Event::PTRACE_EVENT_SECCOMP as i32 {
                    handle_seccomp_trap(pid, allowed);
                } else {
                    // fork/vfork/clone/exec notifications: nothing to
                    // decide, just let the tracee proceed.
                    let _ = ptrace::cont(pid, None);
                }
            }
            WaitStatus::Stopped(pid, signal) => {
                // An ordinary signal-delivery stop (not a ptrace event):
                // re-inject the signal on continue so the tracee still
                // observes it, matching untraced signal semantics.
                let _ = ptrace::cont(pid, signal);
            }
            WaitStatus::PtraceSyscall(pid) => {
                // Not expected (PTRACE_O_TRACESYSGOOD was not set), but
                // continue defensively rather than getting stuck if it
                // ever occurs.
                let _ = ptrace::cont(pid, None);
            }
        }
    }
}

// ── Slice 3c: the seccomp-trap allow/deny decision ──────────────────────────

/// Handles one `PTRACE_EVENT_SECCOMP` stop: freezes sibling threads
/// (`DEC-017`) so none of them can race a concurrent rewrite of the
/// pathname buffer this decision is about to read, resolves the pending
/// `execve`/`execveat`'s real target, checks it against `allowed`, and
/// either lets it proceed or denies it (`DEC-015`) — then resumes every
/// thread this call froze, regardless of outcome.
///
/// Errors are logged, not propagated: this runs inline in the wait loop for
/// every traced exec, so one descendant's broken decision must not abort
/// the whole run. `decide_and_continue`'s own error paths already fail
/// closed (deny) before returning an error here, so a logged error means
/// the *logging*, not the decision, is incomplete.
fn handle_seccomp_trap(tid: Pid, allowed: &AllowedExecutables) {
    let frozen = freeze_thread_group_siblings(tid).unwrap_or_default();

    if let Err(error) = decide_and_continue(tid, allowed) {
        tracing::warn!(
            %tid,
            %error,
            "execution governance: descendant exec decision failed"
        );
    }

    for sibling in frozen {
        let _ = ptrace::cont(sibling, None);
    }
}

/// Reads the trapped syscall's registers, resolves its real target, and
/// either continues (allowed) or denies (`DEC-015`) it. Any failure to
/// resolve the target — an unreadable pathname, an unexpected syscall
/// number, a path that does not resolve to a real file — denies rather
/// than propagating: this is a security decision point, so an internal
/// error must fail closed the same way an explicit policy mismatch does.
fn decide_and_continue(tid: Pid, allowed: &AllowedExecutables) -> Result<(), RunError> {
    let regs = ptrace::getregs(tid)
        .map_err(|error| RunError::Internal(format!("exec guard: getregs {tid}: {error}")))?;

    let target = match resolve_traced_exec_target(tid, &regs) {
        Ok(target) => Some(target),
        Err(error) => {
            tracing::warn!(
                %tid,
                %error,
                "execution governance: denying an unresolvable descendant exec"
            );
            None
        }
    };
    let allow = target
        .as_ref()
        .is_some_and(|target| allowed.contains(target));

    let target_display = target.as_deref().map_or_else(
        || "<unresolved>".to_string(),
        |path| path.display().to_string(),
    );
    tracing::info!(
        %tid,
        target = %target_display,
        allow,
        "execution governance: descendant exec decision"
    );

    if !allow {
        deny_traced_exec(tid, regs)?;
    }
    ptrace::cont(tid, None)
        .map_err(|error| RunError::Internal(format!("exec guard: continue {tid}: {error}")))
}

/// Freezes every other thread sharing `trapped`'s address space (`DEC-017`).
///
/// Every thread of this process is already a tracee of ours by
/// construction — the initial `ptrace::seize`'s `PTRACE_O_TRACECLONE`/
/// `TRACEFORK`/`TRACEVFORK` options cover every thread or process this
/// subtree creates after that seize — so this only needs to force each
/// sibling into a ptrace-stop, not attach to it. Returns the sibling tids
/// that were frozen, so the caller can resume them once its decision is
/// made; repeats the enumeration after each pass in case a new thread
/// appeared while others were being frozen, stopping only once a full pass
/// finds nothing new.
fn freeze_thread_group_siblings(trapped: Pid) -> Result<Vec<Pid>, RunError> {
    let tgid = thread_group_id(trapped)?;
    let mut frozen = Vec::new();
    loop {
        let tids = list_task_tids(tgid)?;
        let mut discovered_new = false;
        for tid in tids {
            if tid == trapped || frozen.contains(&tid) {
                continue;
            }
            // PTRACE_INTERRUPT on an already-stopped tracee is documented
            // as a no-op that takes effect on its next continue — safe to
            // call unconditionally rather than first inspecting its own
            // /proc state.
            if ptrace::interrupt(tid).is_ok() && waitpid(tid, Some(WaitPidFlag::__WALL)).is_ok() {
                frozen.push(tid);
                discovered_new = true;
            }
            // An error here means the thread exited between listing and
            // interrupting — not a stabilization failure, just no longer
            // relevant to freeze.
        }
        if !discovered_new {
            return Ok(frozen);
        }
    }
}

/// Reads the thread-group id (`Tgid:`) of `tid` from `/proc/<tid>/status`.
fn thread_group_id(tid: Pid) -> Result<Pid, RunError> {
    let status = std::fs::read_to_string(format!("/proc/{tid}/status")).map_err(|error| {
        RunError::Internal(format!("exec guard: read /proc/{tid}/status: {error}"))
    })?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Tgid:") {
            let group_id: i32 = rest.trim().parse().map_err(|_| {
                RunError::Internal(format!(
                    "exec guard: parse Tgid from /proc/{tid}/status: {rest:?}"
                ))
            })?;
            return Ok(Pid::from_raw(group_id));
        }
    }
    Err(RunError::Internal(format!(
        "exec guard: no Tgid line in /proc/{tid}/status"
    )))
}

/// Lists the thread ids currently listed under `/proc/<tgid>/task/`.
fn list_task_tids(tgid: Pid) -> Result<Vec<Pid>, RunError> {
    let entries = std::fs::read_dir(format!("/proc/{tgid}/task")).map_err(|error| {
        RunError::Internal(format!("exec guard: read /proc/{tgid}/task: {error}"))
    })?;
    let mut tids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            RunError::Internal(format!("exec guard: list /proc/{tgid}/task: {error}"))
        })?;
        if let Some(raw) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        {
            tids.push(Pid::from_raw(raw));
        }
    }
    Ok(tids)
}

/// The raw syscall arguments a traced `execve`/`execveat` stop needs to
/// resolve its real target.
enum ExecTarget {
    Execve {
        pathname_ptr: u64,
    },
    Execveat {
        dirfd: i32,
        pathname_ptr: u64,
        flags: i32,
    },
}

/// Decodes which of `execve`/`execveat` trapped, and its arguments, from
/// the trapped syscall's own registers. Returns `None` for any other
/// syscall number — unreachable through [`EXEC_TRACE_PROG`] itself (it
/// only ever traps these two), but not asserted away, since a decode
/// failure here should deny rather than panic.
fn decode_exec_target(regs: &libc::user_regs_struct) -> Option<ExecTarget> {
    let nr = syscall_number(regs);
    if nr == i64::from(SYS_EXECVE_NR) {
        Some(ExecTarget::Execve {
            pathname_ptr: syscall_arg(regs, 0),
        })
    } else if nr == i64::from(SYS_EXECVEAT_NR) {
        // `execveat(int dirfd, ..., int flags)`: each register holds a
        // sign-extended 32-bit `int` in a 64-bit slot. `cast_signed`
        // bit-reinterprets the register as `i64` (always succeeds), and
        // `i32::try_from` then both recovers the original 32-bit value
        // (for a legitimately sign-extended one, including the negative
        // `AT_FDCWD` sentinel) and rejects — denying, via `?` — anything
        // that does not actually fit, rather than truncating it silently.
        let dirfd = i32::try_from(syscall_arg(regs, 0).cast_signed()).ok()?;
        let flags = i32::try_from(syscall_arg(regs, 4).cast_signed()).ok()?;
        Some(ExecTarget::Execveat {
            dirfd,
            pathname_ptr: syscall_arg(regs, 1),
            flags,
        })
    } else {
        None
    }
}

#[cfg(target_arch = "x86_64")]
fn syscall_number(regs: &libc::user_regs_struct) -> i64 {
    #[expect(
        clippy::cast_possible_wrap,
        reason = "orig_rax holds a small syscall number; wrapping to negative would itself \
                  correctly fail decode_exec_target's equality checks rather than panicking"
    )]
    let nr = regs.orig_rax as i64;
    nr
}
#[cfg(target_arch = "aarch64")]
fn syscall_number(regs: &libc::user_regs_struct) -> i64 {
    // X8 holds the syscall number on entry, per the AArch64 Linux syscall
    // ABI, and (unlike a return value) is not overwritten by the kernel
    // until after the syscall completes — still valid to read here, at a
    // syscall-entry seccomp trap.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "see the x86_64 arm's own reasoning above"
    )]
    let nr = regs.regs[8] as i64;
    nr
}

#[cfg(target_arch = "x86_64")]
fn syscall_arg(regs: &libc::user_regs_struct, index: u8) -> u64 {
    match index {
        0 => regs.rdi,
        1 => regs.rsi,
        2 => regs.rdx,
        3 => regs.r10,
        4 => regs.r8,
        _ => regs.r9,
    }
}
#[cfg(target_arch = "aarch64")]
fn syscall_arg(regs: &libc::user_regs_struct, index: u8) -> u64 {
    // X0-X5 carry syscall arguments on AArch64.
    regs.regs[usize::from(index)]
}

/// Resolves the file a trapped `execve`/`execveat` is about to run, as a
/// plain host-canonical path comparable against [`AllowedExecutables`]
/// (canonicalized the same way, on the host, at config-resolution time).
///
/// Reads the raw pathname (and, for `execveat`, `dirfd`/`flags`) out of
/// `tid`'s own registers and memory, then resolves it through
/// `/proc/<tid>/{root,cwd,fd/<n>}` — magic links the kernel resolves using
/// *`tid`'s own* mount namespace and root, not the caller's, so this works
/// correctly even though `tid` runs inside bwrap's mount namespace and
/// firma-run (the reader) does not.
fn resolve_traced_exec_target(
    tid: Pid,
    regs: &libc::user_regs_struct,
) -> Result<PathBuf, RunError> {
    let target = decode_exec_target(regs).ok_or_else(|| {
        RunError::Internal(format!(
            "exec guard: unexpected syscall number {} at a seccomp trace stop",
            syscall_number(regs)
        ))
    })?;

    let host_path = match target {
        ExecTarget::Execve { pathname_ptr } => {
            let pathname = read_remote_cstring(tid, pathname_ptr)?;
            exec_relative_prefix(tid, &pathname, libc::AT_FDCWD, 0)?
        }
        ExecTarget::Execveat {
            dirfd,
            pathname_ptr,
            flags,
        } => {
            let pathname = read_remote_cstring(tid, pathname_ptr)?;
            exec_relative_prefix(tid, &pathname, dirfd, flags)?
        }
    };

    std::fs::canonicalize(&host_path).map_err(|error| {
        RunError::Internal(format!(
            "exec guard: resolve exec target {}: {error}",
            host_path.display()
        ))
    })
}

/// Builds the host-openable path for a raw exec pathname, honoring
/// `execve`/`execveat`'s own resolution rules — absolute, `cwd`-relative,
/// `dirfd`-relative, or (`execveat` only) `AT_EMPTY_PATH` — routed through
/// the magic `/proc/<tid>/{root,cwd,fd/<n>}` prefix so the kernel resolves
/// it inside `tid`'s own mount namespace rather than firma-run's.
fn exec_relative_prefix(
    tid: Pid,
    raw_pathname: &[u8],
    dirfd: i32,
    flags: i32,
) -> Result<PathBuf, RunError> {
    if raw_pathname.is_empty() {
        if flags & libc::AT_EMPTY_PATH == 0 {
            return Err(RunError::Internal(
                "exec guard: empty exec pathname without AT_EMPTY_PATH".to_string(),
            ));
        }
        // fexecve-style: the target is the open file itself, named by dirfd.
        return Ok(PathBuf::from(format!("/proc/{tid}/fd/{dirfd}")));
    }

    let pathname = std::str::from_utf8(raw_pathname).map_err(|_| {
        RunError::Internal("exec guard: exec pathname is not valid UTF-8".to_string())
    })?;
    let pathname = Path::new(pathname);

    if pathname.is_absolute() {
        return Ok(PathBuf::from(format!(
            "/proc/{tid}/root{}",
            pathname.display()
        )));
    }
    if dirfd == libc::AT_FDCWD {
        return Ok(PathBuf::from(format!("/proc/{tid}/cwd")).join(pathname));
    }
    Ok(PathBuf::from(format!("/proc/{tid}/fd/{dirfd}")).join(pathname))
}

const MAX_EXEC_PATH_LEN: usize = libc::PATH_MAX as usize;
const EXEC_PATH_READ_CHUNK: usize = 256;

/// Reads a NUL-terminated exec pathname out of `pid`'s memory at `ptr`, in
/// bounded chunks, up to `PATH_MAX` bytes.
///
/// Unlike `egress_guard::read_remote_mem`'s own fixed-size "any short read
/// fails" semantics (appropriate there, for a known-size `sockaddr`), a
/// pathname's length is not known up front: a short read that still
/// contains the NUL terminator is the normal, expected way this loop ends,
/// so only a short read that does *not* contain one — meaning it hit an
/// unmapped page boundary before finding the end of the string — is
/// treated as a failure (fail closed rather than acting on a truncated
/// path).
fn read_remote_cstring(pid: Pid, ptr: u64) -> Result<Vec<u8>, RunError> {
    if ptr == 0 {
        return Err(RunError::Internal(
            "exec guard: null exec pathname pointer".to_string(),
        ));
    }
    let mut addr = usize::try_from(ptr).map_err(|_| {
        RunError::Internal("exec guard: exec pathname pointer out of range".to_string())
    })?;
    let mut out = Vec::new();

    while out.len() < MAX_EXEC_PATH_LEN {
        let chunk_len = EXEC_PATH_READ_CHUNK.min(MAX_EXEC_PATH_LEN - out.len());
        let mut buf = vec![0_u8; chunk_len];
        let local_iov = [std::io::IoSliceMut::new(&mut buf)];
        let remote_iov = [nix::sys::uio::RemoteIoVec {
            base: addr,
            len: chunk_len,
        }];
        let mut local_iov = local_iov;
        let n =
            nix::sys::uio::process_vm_readv(pid, &mut local_iov, &remote_iov).map_err(|error| {
                RunError::Internal(format!("exec guard: process_vm_readv: {error}"))
            })?;
        if n == 0 {
            return Err(RunError::Internal(
                "exec guard: exec pathname unreadable".to_string(),
            ));
        }

        if let Some(nul_at) = buf[..n].iter().position(|&byte| byte == 0) {
            out.extend_from_slice(&buf[..nul_at]);
            return Ok(out);
        }
        if n < chunk_len {
            return Err(RunError::Internal(
                "exec guard: exec pathname truncated at an unmapped boundary".to_string(),
            ));
        }
        out.extend_from_slice(&buf[..n]);
        addr += n;
    }
    Err(RunError::Internal(
        "exec guard: exec pathname exceeds PATH_MAX".to_string(),
    ))
}

/// Rewrites `tid`'s pending syscall (`DEC-015`) so continuing it returns
/// `-ENOSYS` without ever running — the same observable failure Slice 3a's
/// filter produces when no tracer is attached at all, so a denied exec and
/// an unattached exec look identical from the traced process's own point
/// of view.
///
/// Sets both the syscall-number register (to an invalid number, so the
/// kernel never dispatches it) and the return-value register (explicitly,
/// to `-ENOSYS`), rather than relying on either architecture's own
/// implicit dispatch-miss behavior for an out-of-range syscall number —
/// `x86_64` and `aarch64` are not guaranteed to populate the return
/// register identically for that case, so both are set explicitly here
/// instead of assumed.
fn deny_traced_exec(tid: Pid, mut regs: libc::user_regs_struct) -> Result<(), RunError> {
    set_syscall_number(&mut regs, -1);
    set_syscall_return_value(&mut regs, -i64::from(libc::ENOSYS));
    ptrace::setregs(tid, regs)
        .map_err(|error| RunError::Internal(format!("exec guard: deny setregs {tid}: {error}")))
}

#[cfg(target_arch = "x86_64")]
fn set_syscall_number(regs: &mut libc::user_regs_struct, value: i64) {
    regs.orig_rax = value.cast_unsigned();
}
#[cfg(target_arch = "aarch64")]
fn set_syscall_number(regs: &mut libc::user_regs_struct, value: i64) {
    regs.regs[8] = value.cast_unsigned();
}

#[cfg(target_arch = "x86_64")]
fn set_syscall_return_value(regs: &mut libc::user_regs_struct, value: i64) {
    regs.rax = value.cast_unsigned();
}
#[cfg(target_arch = "aarch64")]
fn set_syscall_return_value(regs: &mut libc::user_regs_struct, value: i64) {
    regs.regs[0] = value.cast_unsigned();
}

#[cfg(test)]
mod tests {
    use super::{install_and_wait_for_ready, is_confirmed_architecture};

    #[test]
    fn install_and_wait_for_ready_fails_closed_when_handshake_socket_is_unreachable() {
        // The connect happens after installing the filter but before any
        // exec, so a missing socket errors there and the wrapped command
        // never starts.
        let result = install_and_wait_for_ready(
            std::path::Path::new("/nonexistent/firma-exec-guard.sock"),
            &["/bin/true".to_string()],
        );
        assert!(result.is_err());
    }

    /// `DEC-018`'s allow-list negative path, mocked (no real unconfirmed
    /// host is available in any CI or dev environment this crate builds
    /// in) — this session's own real-hardware evidence covers only
    /// `aarch64`; every other architecture name, including `x86_64` itself
    /// pending its own separate real-hardware test, must be rejected.
    #[test]
    fn is_confirmed_architecture_allows_only_aarch64() {
        assert!(is_confirmed_architecture("aarch64"));
        assert!(!is_confirmed_architecture("x86_64"));
        assert!(!is_confirmed_architecture("riscv64"));
        assert!(!is_confirmed_architecture(""));
    }
}
