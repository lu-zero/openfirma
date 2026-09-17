//! Verifies Slice 3b of `docs/architecture/ptrace-seccomp-exec-gate-plan.md`:
//! the host-side `PtraceSeccompGovernor` actually attaches to a real `bwrap`
//! sandbox via `ptrace::seize` and supervises it end to end.
//!
//! Slice 3b implements no allow/deny decision yet — every
//! `PTRACE_EVENT_SECCOMP` stop is unconditionally `PTRACE_CONT`'d. So this is
//! not a governance-enforcement test (that is `execution.rs`'s job once
//! Slice 3c lands); it proves the new attach/wait-loop mechanism itself does
//! not break the sandboxed command: the root command and a nested descendant
//! both still exec successfully under a live tracer, exit-code and
//! signal-death reporting still match `wait_with_signal_forwarding`'s
//! existing observable contract, and the root command is still governed
//! exactly the same way it is under the default `Inherited` strategy.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::harness::TestWorld;

use super::support::{
    FORBIDDEN_MARKER, assert_only_root_governed, first_existing, patch_local_exec_allowlist,
    shell_quote, spawn_allow_all_endpoint, write_forbidden_tool,
};

/// Rewrites the scaffolded `[run.profiles.generic]` section to select
/// `execution_governance = "ptrace_seccomp_exec"`.
///
/// Runs after [`patch_local_exec_allowlist`], which already leaves the
/// profile's `sidecar_local_exec` satisfying this strategy's own config-time
/// precondition (`enforce_known_executables = true` with a non-empty
/// `allowed_executables`).
fn select_ptrace_seccomp_exec_governance(config_path: &std::path::Path) {
    let original = std::fs::read_to_string(config_path).expect("read patched firma.toml");
    let anchor = "backend = \"bwrap\"\n";
    assert!(
        original.contains(anchor),
        "expected the bwrap backend line patch_local_exec_allowlist leaves in place:\n{original}"
    );
    let patched = original.replacen(
        anchor,
        "backend = \"bwrap\"\nexecution_governance = \"ptrace_seccomp_exec\"\n",
        1,
    );
    std::fs::write(config_path, patched).expect("write execution_governance selection");
}

#[test]
fn ptrace_seccomp_governor_attaches_and_supervises_real_bwrap_sandbox() {
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));
    let bash_canonical = std::fs::canonicalize(&bash)
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", bash.display()));

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();
    let socket_dir = world.path("sockets");
    std::fs::create_dir_all(&socket_dir).expect("create socket directory");

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    let governance_sock = socket_dir.join("local-exec.sock");
    let traffic_sock = socket_dir.join("traffic.sock");
    patch_local_exec_allowlist(
        &config_path,
        &traffic_sock,
        &governance_sock,
        &bash_canonical,
    );
    select_ptrace_seccomp_exec_governance(&config_path);

    let governed = Arc::new(Mutex::new(Vec::<String>::new()));
    spawn_allow_all_endpoint(&governance_sock, Arc::clone(&governed));

    // Root command execs, then a nested descendant execs too — proving the
    // atomic PTRACE_SEIZE option set's PTRACE_O_TRACEFORK/CLONE/VFORK keeps
    // the descendant traced, and its own execve trap is PTRACE_CONT'd rather
    // than left to hang or fail closed. Exits non-zero so the second
    // assertion below proves exit-code propagation is untouched by the new
    // wait loop.
    let bash_script = format!(
        "echo root-ran; {nested} -c 'echo nested-child-ran'; echo \"nested-exit=$?\"; exit 5",
        nested = shell_quote(&bash),
    );
    let scenario = world.run_firma(
        "generic",
        Some(&config_path),
        &workspace,
        &["--sidecar", "local", "--authority", "local"],
        &bash,
        ["-c", &bash_script],
    );
    assert!(
        scenario.stdout.contains("root-ran"),
        "root command did not run under the ptrace-attached sandbox:\n{scenario}"
    );
    assert!(
        scenario.stdout.contains("nested-child-ran"),
        "a descendant exec did not complete under a live ptrace tracer — the seize/attach or the \
         unconditional PTRACE_EVENT_SECCOMP continue is not working:\n{scenario}"
    );
    assert!(
        scenario.stdout.contains("nested-exit=0"),
        "the nested descendant's own exit code was not 0 as expected:\n{scenario}"
    );
    assert!(
        !scenario.success(),
        "expected the root command's exit code 5 to propagate through the ptrace wait loop, but \
         `firma run` reported success:\n{scenario}"
    );

    let governed = governed.lock().expect("lock governance log").clone();
    assert_only_root_governed(&governed, &bash_canonical);

    // A second run, killed by a signal rather than exiting normally, proving
    // the wait loop's WaitStatus::Signaled branch (not just Exited) still
    // reaches exit_code_from_outcome correctly and the run completes
    // promptly rather than hanging until run_firma's own bounding deadline.
    let start = Instant::now();
    let signal_script = "echo about-to-self-signal; kill -TERM $$; sleep 5; echo unreachable";
    let signalled = world.run_firma(
        "generic",
        Some(&config_path),
        &workspace,
        &["--sidecar", "local", "--authority", "local"],
        &bash,
        ["-c", signal_script],
    );
    let elapsed = start.elapsed();
    assert!(
        signalled.stdout.contains("about-to-self-signal"),
        "the signalled scenario did not even start running:\n{signalled}"
    );
    assert!(
        !signalled.stdout.contains("unreachable"),
        "the process kept running past its own self-signal instead of dying to it:\n{signalled}"
    );
    assert!(
        !signalled.success(),
        "expected signal-death to propagate as a non-zero exit, not success:\n{signalled}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "signal-death reporting took {elapsed:?} — the ptrace wait loop likely hung instead of \
         promptly observing WaitStatus::Signaled"
    );
}

/// The FIR-366 acceptance proof, under `PtraceSeccompExec` specifically:
/// `execution.rs`'s own `child_process_escapes_run_governance` (the
/// `Inherited`-strategy control, `#[ignore]`d because it fails until
/// child-process governance lands anywhere) proves a forbidden tool run as
/// a child of an allowed bash root executes ungoverned under today's
/// default strategy. Selecting `execution_governance =
/// "ptrace_seccomp_exec"` for the very same scenario must now deny it: the
/// descendant's own `execve` is a `PTRACE_EVENT_SECCOMP` trap this
/// governor decides, not something that only the root command's one-time
/// allowlist check ever sees.
#[test]
fn ptrace_seccomp_exec_denies_forbidden_tool_as_child_of_allowed_bash_root() {
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));
    let bash_canonical = std::fs::canonicalize(&bash)
        .unwrap_or_else(|e| panic!("canonicalize {}: {e}", bash.display()));

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();
    let socket_dir = world.path("sockets");
    std::fs::create_dir_all(&socket_dir).expect("create socket directory");

    let forbidden_tool = workspace.join("forbidden-tool");
    let forbidden_marker = workspace.join("forbidden-ran");
    write_forbidden_tool(&forbidden_tool, &forbidden_marker);

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    let governance_sock = socket_dir.join("local-exec.sock");
    let traffic_sock = socket_dir.join("traffic.sock");
    patch_local_exec_allowlist(
        &config_path,
        &traffic_sock,
        &governance_sock,
        &bash_canonical,
    );
    select_ptrace_seccomp_exec_governance(&config_path);

    let governed = Arc::new(Mutex::new(Vec::<String>::new()));
    spawn_allow_all_endpoint(&governance_sock, Arc::clone(&governed));

    let bash_script = format!(
        "{tool} as-child-of-bash; echo \"bash-done exit=$?\"",
        tool = shell_quote(&forbidden_tool),
    );
    let scenario = world.run_firma(
        "generic",
        Some(&config_path),
        &workspace,
        &["--sidecar", "local", "--authority", "local"],
        &bash,
        ["-c", &bash_script],
    );

    assert!(
        scenario.stdout.contains("bash-done"),
        "the allowed bash root did not run at all — `firma run` could not start the sandbox in \
         this environment:\n{scenario}"
    );
    assert!(
        !forbidden_marker.exists() && !scenario.stdout.contains(FORBIDDEN_MARKER),
        "FIR-366: the forbidden-tool child executed under ptrace_seccomp_exec governance, which \
         is specifically meant to gate descendant execs:\n{scenario}"
    );

    let governed = governed.lock().expect("lock governance log").clone();
    assert_only_root_governed(&governed, &bash_canonical);
}
