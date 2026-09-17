//! `firma __exec-guarded-run` — Slice 3a of
//! `docs/architecture/ptrace-seccomp-exec-gate-plan.md`.
//!
//! Tests the shim binary directly (not through `firma run`, since the
//! host-side `PtraceSeccompExec` governor that spawns it doesn't exist yet
//! — Slices 3b/3c land that). This is Slice 3a's own acceptance proof: the
//! filter installs and the handshake completes, but with no `ptrace(2)`
//! tracer actually attached (this test simulates only the byte handshake,
//! not a real `seize`), `execve` fails with `ENOSYS` rather than running
//! the wrapped command unconfined.
//!
//! **Empirically corrects an assumption from the plan's own research**: the
//! plan's design assumed (uncontested in both review rounds) that
//! `SECCOMP_RET_TRACE` with no tracer attached behaves like
//! `SECCOMP_RET_ALLOW`. Measured directly here, that is false on this
//! kernel: with no tracer, the traced syscall returns `-ENOSYS` and does not
//! execute at all (`man 2 seccomp`'s own description of `SECCOMP_RET_TRACE`
//! says exactly this — the plan's assumption was simply wrong, not a
//! kernel-version quirk). This is actually the *safer* of the two possible
//! behaviors for this design (fail closed, not fail open) — Slice 3b's own
//! host-side attach must complete before the shim's handshake byte is sent,
//! which this test also proves is necessary: skipping the real `seize` and
//! only performing the byte handshake is not sufficient for the wrapped
//! command to run at all, let alone unconfined.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::Command;

/// Runs `firma __exec-guarded-run` against a handshake socket that
/// completes the byte handshake but never actually `ptrace(2)`-seizes the
/// shim (Slice 3b is what performs a real seize), and asserts the wrapped
/// command's `execve` fails with `ENOSYS` — proving the filter is genuinely
/// active and fails closed absent a real tracer, not merely installed and
/// ignored.
#[cfg(target_os = "linux")]
#[test]
fn exec_guarded_run_fails_closed_without_a_real_ptrace_tracer() {
    let socket_dir = tempfile::tempdir().expect("create temp dir for handshake socket");
    let socket_path: PathBuf = socket_dir.path().join("handshake.sock");

    let listener = UnixListener::bind(&socket_path).expect("bind handshake socket");
    let acceptor = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().expect("accept shim connection");
        let mut ready = [0_u8; 1];
        conn.read_exact(&mut ready).expect("read shim's ready byte");
        // No real ptrace(2) seize happens here — this thread only ever
        // completes the byte handshake, deliberately, to isolate exactly
        // what Slice 3a's own filter+handshake logic does on its own.
        conn.write_all(&[0_u8]).expect("send go-ahead byte");
    });

    let output = Command::new(env!("CARGO_BIN_EXE_firma"))
        .args(["__exec-guarded-run", "--handshake-socket"])
        .arg(&socket_path)
        .args(["--", "/bin/echo", "should-not-print"])
        .output()
        .expect("spawn firma __exec-guarded-run");

    acceptor.join().expect("join acceptor thread");

    assert!(
        !output.status.success(),
        "exec-guarded-run unexpectedly succeeded without a real ptrace tracer attached:\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "the wrapped command must never have run at all; got stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Function not implemented") || stderr.contains("ENOSYS"),
        "expected the exec to fail with ENOSYS (SECCOMP_RET_TRACE with no tracer attached), \
         got: {stderr}"
    );
}
