//! `backend = "hakoniwa"` — Slices 1, 2, and 5 of `docs/architecture/hakoniwa-backend-plan.md`.
//!
//! `hakoniwa_backend_blocks_network_like_bwrap` proves `PROOF-001`'s bare-network-namespace case:
//! a root command launched under the experimental Hakoniwa backend cannot reach a service outside
//! its own sandbox, while the same command reaches it fine when run unsandboxed. There is no
//! DNS-stub/egress-guard bootstrap yet (Slice 3), so this only proves namespace-level confinement
//! — not the descendant-inheritance property `child_process_governance`'s tests prove for `bwrap`.
//! `hakoniwa_backend_denies_filesystem_delete_via_seccomp` and
//! `hakoniwa_backend_restricts_descendant_exec_via_landlock` prove Slice 5's seccomp/Landlock
//! wiring. Since Slice 2, the run's working directory is the isolated test workspace itself (mount
//! translation makes it visible inside the sandbox); earlier slices ran from `/tmp` as a
//! bare-rootfs workaround.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::harness::{TestWorld, run_bounded};
use crate::upstream::{HttpProbe, ProbeBehavior};

/// Shell logic shared by the control (unsandboxed) and blocked (sandboxed) runs: attempt one GET
/// against `$1` via `/dev/tcp`, reporting attempt/outcome markers this test asserts on.
const NETWORK_CHECK_SCRIPT: &str = r#"
echo "CHILD NETWORK ATTEMPTED url=$1"
target="${1#http://}"
host_port="${target%%/*}"
request_path="/${target#*/}"
host="${host_port%:*}"
port="${host_port##*:}"
if exec 3<>"/dev/tcp/$host/$port"; then
  printf 'GET %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n\r\n' "$request_path" "$host_port" >&3
  cat <&3
  exit 0
fi
echo "CHILD NETWORK BLOCKED"
exit 23
"#;

/// Shell logic shared by the control (unsandboxed) and denied (sandboxed) runs in the seccomp
/// test: create a file, then try to delete it, reporting the delete's own exit code.
const DELETE_CHECK_SCRIPT: &str = "touch f && echo TOUCH_OK; rm f; echo RM_EXIT=$?";

/// Shell logic for the Landlock test: run a binary that is deliberately never allow-listed,
/// reporting its own exit code.
const DESCENDANT_EXEC_SCRIPT: &str = "/bin/true; echo TRUE_EXIT=$?";

/// Shell logic for the watchdog test: print its own `FIRMA_RUN_*` env-var count once, then tick
/// once a second for up to 30s — long enough for the test to find and kill the proxy bridge well
/// before it would finish naturally.
const WATCHDOG_SCRIPT: &str = "env | grep -c '^FIRMA_RUN_' || true; \
     i=0; while [ $i -lt 30 ]; do echo tick=$i; sleep 1; i=$((i+1)); done; echo FINISHED_ALL_TICKS";

/// Locates the `firma-hakoniwa-runner` binary built alongside `firma` in this workspace's target
/// directory.
///
/// `env!("CARGO_BIN_EXE_<name>")` only covers binaries in the *same* Cargo package as this test
/// target (`firma`); `firma-hakoniwa-runner` is a separate workspace member, so its path is
/// derived from `firma`'s own sibling directory instead — the same directory Cargo places every
/// workspace binary into. Returns `None` (rather than panicking) when it hasn't been built, so a
/// narrowly-scoped test invocation that never builds this crate skips cleanly instead of failing
/// for an unrelated reason.
fn hakoniwa_runner_path() -> Option<PathBuf> {
    let firma = PathBuf::from(env!("CARGO_BIN_EXE_firma"));
    let runner = firma.with_file_name("firma-hakoniwa-runner");
    runner.is_file().then_some(runner)
}

/// Returns the first existing path from an ordered list of platform-specific candidates.
fn first_existing(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
}

#[test]
fn hakoniwa_backend_blocks_network_like_bwrap() {
    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_blocks_network_like_bwrap: firma-hakoniwa-runner was not \
             built (run `cargo build --workspace` or `cargo nextest run` without `-p` to build it)"
        );
        return;
    };
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

    // Control: bash run directly (no sandbox) must reach a responding probe.
    let control_probe = HttpProbe::start(
        "hakoniwa-network-control",
        ProbeBehavior::Respond("CONTROL-REACHED"),
    );
    let control_url = control_probe.url();
    let mut control_command = world.isolated_command_in(&bash, &workspace);
    control_command
        .arg("-c")
        .arg(NETWORK_CHECK_SCRIPT)
        .arg("bash")
        .arg(&control_url);
    let control = run_bounded(&mut control_command, Duration::from_secs(10));
    assert!(
        control.success(),
        "network-check control failed:\n{control}"
    );
    assert!(
        control.stdout.contains("CHILD NETWORK ATTEMPTED"),
        "network-check control did not attempt the connection:\n{control}"
    );
    control_probe
        .finish()
        .expect("network-check control must reach the HTTP probe");

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    patch_backend_to_hakoniwa(&config_path);

    // Blocked: the same check run through `firma run --backend hakoniwa` must not reach a
    // host-bound probe at all — Slice 1's bare network namespace has no route to the host's own
    // loopback (a different netns entirely), let alone the outside world.
    let blocked_probe = HttpProbe::start("hakoniwa-network-blocked", ProbeBehavior::MustNotConnect);
    let blocked_url = blocked_probe.url();

    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(NETWORK_CHECK_SCRIPT)
        .arg("bash")
        .arg(&blocked_url);
    let output = run_bounded(&mut command, Duration::from_mins(2));

    // The script exits 23 when the connection is blocked, and firma run propagates the wrapped
    // command's exit code, so `!success()` is the *expected* outcome here — not a run failure.
    assert!(
        !output.success(),
        "hakoniwa-backed run unexpectedly succeeded (the check should exit 23 when blocked):\n{output}"
    );
    assert!(
        output.stdout.contains("CHILD NETWORK BLOCKED"),
        "the hakoniwa-backed run did not report a blocked connection:\n{output}"
    );
    assert!(
        blocked_probe.finish().is_none(),
        "the hakoniwa-backed run reached the forbidden loopback destination"
    );
}

/// Sends one well-formed DNS query over UDP to `127.0.0.1:53` and reports its
/// outcome — `CHILD DNS RESPONSE RCODE=<n> TXN_ID_OK=<bool>` on a reply
/// received within 5s, `CHILD DNS NO RESPONSE` on a timeout. Exits 0 only
/// when the reply is a `REFUSED` (RCODE 5) response to this exact query
/// (transaction ID echoed back) — this is `DEC-012`'s own observable
/// capability: `Slice 3`'s DNS-stub bootstrap silently failed to bind port
/// 53 until `Slice 7` closed that gap.
const DNS_RESOLUTION_SCRIPT: &str = r#"
python3 - <<'PYEOF'
import socket
import sys

query = bytes([0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]) \
    + bytes([7]) + b"example" + bytes([3]) + b"com" + bytes([0, 0, 1, 0, 1])

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.settimeout(5)
sock.sendto(query, ("127.0.0.1", 53))
try:
    data, _ = sock.recvfrom(512)
except socket.timeout:
    print("CHILD DNS NO RESPONSE")
    sys.exit(1)

rcode = data[3] & 0x0F
txn_id_ok = data[0:2] == query[0:2]
print(f"CHILD DNS RESPONSE RCODE={rcode} TXN_ID_OK={txn_id_ok}")
sys.exit(0 if rcode == 5 and txn_id_ok else 2)
PYEOF
"#;

/// `DEC-012`/Slice 7 of `docs/architecture/hakoniwa-backend-plan.md`.
///
/// Proves the sandbox's DNS stub actually answers real queries end to end
/// under a real `firma run --backend hakoniwa` invocation — the first test
/// anywhere to assert this for `HakoniwaBackend` (Slice 3's own
/// `hakoniwa_backend_blocks_network_like_bwrap` only proves namespace-level
/// network confinement, not that the sanctioned loopback DNS-stub route
/// itself works). Before `DEC-012`, this same query would time out (`CHILD
/// DNS NO RESPONSE`): the stub silently failed to bind `127.0.0.1:53`
/// inside the sandbox's own network namespace.
#[test]
fn hakoniwa_backend_dns_stub_answers_real_queries() {
    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_dns_stub_answers_real_queries: firma-hakoniwa-runner was \
             not built (run `cargo build --workspace` or `cargo nextest run` without `-p` to \
             build it)"
        );
        return;
    };
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));
    let python3 = first_existing(&["/usr/bin/python3", "/bin/python3"]);
    if python3.is_none() {
        eprintln!(
            "skipping hakoniwa_backend_dns_stub_answers_real_queries: python3 was not found on \
             this host"
        );
        return;
    }

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    patch_backend_to_hakoniwa(&config_path);

    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(DNS_RESOLUTION_SCRIPT);
    let output = run_bounded(&mut command, Duration::from_mins(2));

    assert!(
        output.success(),
        "hakoniwa-backed DNS query did not succeed:\n{output}"
    );
    assert!(
        output
            .stdout
            .contains("CHILD DNS RESPONSE RCODE=5 TXN_ID_OK=True"),
        "hakoniwa-backed run did not observe a REFUSED response to its own query:\n{output}"
    );
}

/// Tries to bind `127.0.0.1:80` (a privileged port unrelated to the DNS
/// stub) and reports the outcome — `CHILD BIND OK` or `CHILD BIND
/// FAILED: <errno>`.
const UNRELATED_PRIVILEGED_PORT_BIND_SCRIPT: &str = r#"
python3 -c "
import socket
import sys

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
try:
    sock.bind(('127.0.0.1', 80))
    print('CHILD BIND OK')
    sys.exit(0)
except OSError as e:
    print(f'CHILD BIND FAILED: {e}')
    sys.exit(1)
"
"#;

/// `DEC-012`'s own design point, required by Slice 7's proof-obligation
/// list: the accepted fd-inheritance fix must not widen the sandbox's own
/// network namespace configuration for anything *other* than the two
/// sockets it explicitly binds and hands over — unlike the earlier,
/// rejected design (lowering `ip_unprivileged_port_start` to 0 for the
/// whole netns), which would have let the wrapped command bind *any*
/// privileged port, not just port 53. Asserts the wrapped command still
/// cannot bind an unrelated privileged port (80) — proving that rejected
/// widening did not happen.
#[test]
fn hakoniwa_backend_wrapped_command_cannot_bind_an_unrelated_privileged_port() {
    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_wrapped_command_cannot_bind_an_unrelated_privileged_port: \
             firma-hakoniwa-runner was not built (run `cargo build --workspace` or `cargo \
             nextest run` without `-p` to build it)"
        );
        return;
    };
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));
    let python3 = first_existing(&["/usr/bin/python3", "/bin/python3"]);
    if python3.is_none() {
        eprintln!(
            "skipping hakoniwa_backend_wrapped_command_cannot_bind_an_unrelated_privileged_port: \
             python3 was not found on this host"
        );
        return;
    }

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    patch_backend_to_hakoniwa(&config_path);

    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(UNRELATED_PRIVILEGED_PORT_BIND_SCRIPT);
    let output = run_bounded(&mut command, Duration::from_mins(2));

    assert!(
        !output.success(),
        "the wrapped command unexpectedly bound an unrelated privileged port (80) -- the \
         DEC-012 fix must not widen the sandbox's own network namespace configuration:\n{output}"
    );
    assert!(
        output.stdout.contains("CHILD BIND FAILED"),
        "the wrapped command did not report the expected bind failure:\n{output}"
    );
}

/// Patches a scaffolded `firma.toml`'s generic profile to use the experimental Hakoniwa backend.
///
/// Fails loudly if the generated profile no longer has the expected anchor, rather than silently
/// patching the wrong section — mirrors
/// `child_process_governance::support::patch_local_exec_allowlist`.
fn patch_backend_to_hakoniwa(config_path: &Path) {
    let original = std::fs::read_to_string(config_path).expect("read generated firma.toml");
    let anchor = "[run.profiles.generic]\nbackend = \"bwrap\"\n";
    assert!(
        original.contains(anchor),
        "generated firma.toml did not contain the expected generic profile anchor:\n{original}"
    );
    let patched = original.replacen(
        anchor,
        "[run.profiles.generic]\nbackend = \"hakoniwa\"\n",
        1,
    );
    std::fs::write(config_path, patched).expect("write patched firma.toml");
}

/// `deny_actions = ["filesystem.delete"]` — Slice 5 of `docs/architecture/hakoniwa-backend-plan.md`.
///
/// Proves `HakoniwaBackend` denies syscalls from the profile's managed `seccomp_policy` the same
/// way the bwrap backend does, via `hakoniwa::seccomp::Filter` instead of a compiled BPF artifact
/// (`DEC-004`). Confirmed by hand against the runner binary directly before this test was written
/// (see the plan's Slice 5 section): `unlink`/`unlinkat` denied with `EPERM`.
#[test]
fn hakoniwa_backend_denies_filesystem_delete_via_seccomp() {
    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_denies_filesystem_delete_via_seccomp: \
             firma-hakoniwa-runner was not built"
        );
        return;
    };
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

    // Control: the same script run directly (no sandbox) must delete the file it created.
    let mut control_command = world.isolated_command_in(&bash, &workspace);
    control_command.arg("-c").arg(DELETE_CHECK_SCRIPT);
    let control = run_bounded(&mut control_command, Duration::from_secs(10));
    assert!(control.success(), "delete-check control failed:\n{control}");
    assert!(
        control.stdout.contains("RM_EXIT=0"),
        "delete-check control did not delete its own file:\n{control}"
    );

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    patch_backend_to_hakoniwa(&config_path);
    let policy_path = patch_seccomp_deny_filesystem_delete(&config_path, &state_dir);

    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(DELETE_CHECK_SCRIPT);
    let output = run_bounded(&mut command, Duration::from_mins(2));

    assert!(
        output.stdout.contains("TOUCH_OK"),
        "the hakoniwa-backed run did not create the file to delete:\n{output}"
    );
    assert!(
        !output.stdout.contains("RM_EXIT=0"),
        "the hakoniwa-backed run deleted the file despite the seccomp deny policy:\n{output}"
    );
    // Keeps the temp policy artifact directory alive for the duration of the run above.
    drop(policy_path);
}

/// Patches a scaffolded `firma.toml`'s generic profile to load a managed seccomp policy denying
/// `filesystem.delete`, and returns the policy source file (kept alive so its path stays valid).
fn patch_seccomp_deny_filesystem_delete(config_path: &Path, state_dir: &Path) -> PathBuf {
    std::fs::create_dir_all(state_dir).expect("create state dir for seccomp policy");
    let policy_path = state_dir.join("hakoniwa-deny-filesystem-delete.toml");
    std::fs::write(
        &policy_path,
        "policy_id = \"e2e-hakoniwa-delete\"\npolicy_version = \"v1\"\ndefault_action = \"allow\"\ndeny_actions = [\"filesystem.delete\"]\n",
    )
    .expect("write seccomp policy source");
    let artifact_dir = state_dir.join("seccomp-artifacts");

    let original = std::fs::read_to_string(config_path).expect("read patched firma.toml");
    let anchor = "[run.profiles.generic]\nbackend = \"hakoniwa\"\n";
    assert!(
        original.contains(anchor),
        "firma.toml did not contain the expected hakoniwa profile anchor:\n{original}"
    );
    let injected = format!(
        "{anchor}\n[run.profiles.generic.seccomp_policy]\nsource_policy_path = '{}'\nartifact_dir = '{}'\n",
        policy_path.display(),
        artifact_dir.display(),
    );
    std::fs::write(config_path, original.replacen(anchor, &injected, 1))
        .expect("write seccomp-patched firma.toml");
    policy_path
}

/// `sidecar_local_exec.allowed_executables` — Slice 5 of
/// `docs/architecture/hakoniwa-backend-plan.md`.
///
/// Proves the property `Inherited` governance (today's bwrap-default, root-only mediator check)
/// cannot provide: a command the root command spawns — not just the root command itself — is
/// denied when it is not in `allowed_executables`, because `HakoniwaBackend` scopes Landlock's
/// execute right to that same set (see `DEC-004` and the plan's Slice 5 design notes on
/// `LANDLOCK_LIBRARY_DIRS`). `/bin/true` is deliberately never added to the allow-list, only
/// `bash` itself is, so this isolates descendant-level enforcement from the pre-existing
/// root-level `sidecar_local_exec` check.
#[test]
fn hakoniwa_backend_restricts_descendant_exec_via_landlock() {
    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_restricts_descendant_exec_via_landlock: \
             firma-hakoniwa-runner was not built"
        );
        return;
    };
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));
    let bash_canonical =
        std::fs::canonicalize(&bash).expect("canonicalize bash for allowed_executables");

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    patch_backend_to_hakoniwa(&config_path);

    let traffic_sock = world.path("state/hakoniwa-landlock-sidecar.sock");
    let governance_sock = world.path("state/hakoniwa-landlock-governance.sock");
    patch_local_exec_allowlist(
        &config_path,
        &traffic_sock,
        &governance_sock,
        &bash_canonical,
    );
    spawn_allow_all_endpoint(&governance_sock, Arc::new(Mutex::new(Vec::new())));

    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(DESCENDANT_EXEC_SCRIPT);
    let output = run_bounded(&mut command, Duration::from_mins(2));

    assert!(
        !output.stdout.contains("TRUE_EXIT=0"),
        "the hakoniwa-backed run let a non-allow-listed descendant executable run:\n{output}"
    );
}

/// Configures the generic profile to allow one root executable through a local governance
/// endpoint, mirroring `child_process_governance::support::patch_local_exec_allowlist` (not
/// reused directly: that helper is private to its own scenario module and anchors on
/// `backend = "bwrap"`).
fn patch_local_exec_allowlist(
    config_path: &Path,
    traffic_sock: &Path,
    governance_sock: &Path,
    bash_canonical: &Path,
) {
    let original = std::fs::read_to_string(config_path).expect("read patched firma.toml");
    let anchor = "[run.profiles.generic]\nbackend = \"hakoniwa\"\n";
    assert!(
        original.contains(anchor),
        "firma.toml did not contain the expected hakoniwa profile anchor:\n{original}"
    );
    let injected = format!(
        "{anchor}sidecar_endpoint = \"unix://{traffic}\"\n\n\
         [run.profiles.generic.sidecar_local_exec]\n\
         endpoint = \"unix://{governance}\"\n\
         timeout = \"2s\"\n\
         enforce_known_executables = true\n\
         allowed_executables = [\"{bash}\"]\n",
        traffic = traffic_sock.display(),
        governance = governance_sock.display(),
        bash = bash_canonical.display(),
    );
    std::fs::write(config_path, original.replacen(anchor, &injected, 1))
        .expect("write patched firma.toml");
}

/// Starts a detached Unix-socket endpoint that records requests and replies with `allow`,
/// mirroring `child_process_governance::support::spawn_allow_all_endpoint` (see
/// `patch_local_exec_allowlist`'s doc comment for why it is not reused directly).
fn spawn_allow_all_endpoint(sock_path: &Path, log: Arc<Mutex<Vec<String>>>) {
    use std::os::unix::net::UnixListener;

    let _ = std::fs::remove_file(sock_path);
    let listener = UnixListener::bind(sock_path)
        .unwrap_or_else(|e| panic!("bind allow-all endpoint at {}: {e}", sock_path.display()));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let mut reader =
                BufReader::new(stream.try_clone().expect("clone allow-all endpoint stream"));
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok() {
                log.lock()
                    .expect("lock governance log")
                    .push(line.trim().to_string());
            }
            let _ = stream.write_all(b"{\"decision\":\"allow\"}\n");
        }
    });
}

/// Shell logic for the shared forbidden-tool probe: prints a marker to stdout and touches a marker
/// file if it ever runs.
const FORBIDDEN_MARKER: &str = "FORBIDDEN-TOOL EXECUTED";

/// Writes an executable script to `path` that prints [`FORBIDDEN_MARKER`] and touches `marker` if
/// it ever runs. Mirrors `child_process_governance::support::write_forbidden_tool` (not reused
/// directly — see `patch_local_exec_allowlist`'s own doc comment for why this file keeps its own
/// copies of these small test-only helpers rather than sharing a private sibling module).
fn write_forbidden_tool(path: &Path, marker: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let script = format!(
        "#!/bin/sh\necho \"{FORBIDDEN_MARKER} pid=$$ argv=$*\"\n: > '{}'\n",
        marker.display(),
    );
    std::fs::write(path, script).expect("write forbidden-tool");
    let mut permissions = std::fs::metadata(path)
        .expect("stat forbidden-tool")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod forbidden-tool");
}

/// Rewrites the scaffolded hakoniwa profile to also select
/// `execution_governance = "ptrace_seccomp_exec"`.
///
/// Runs after [`patch_local_exec_allowlist`], which already leaves the profile's
/// `sidecar_local_exec` satisfying this strategy's own config-time precondition
/// (`enforce_known_executables = true` with a non-empty `allowed_executables`).
fn select_ptrace_seccomp_exec_governance(config_path: &Path) {
    let original = std::fs::read_to_string(config_path).expect("read patched firma.toml");
    let anchor = "backend = \"hakoniwa\"\n";
    assert!(
        original.contains(anchor),
        "expected the hakoniwa backend line patch_local_exec_allowlist leaves in place:\n{original}"
    );
    let patched = original.replacen(
        anchor,
        "backend = \"hakoniwa\"\nexecution_governance = \"ptrace_seccomp_exec\"\n",
        1,
    );
    std::fs::write(config_path, patched).expect("write execution_governance selection");
}

/// `PtraceSeccompExec` on `HakoniwaBackend` — proves the same ptrace(2)-based descendant-exec
/// governance built for `bwrap` (`docs/architecture/ptrace-seccomp-exec-gate-plan.md`) also works
/// correctly against a real Hakoniwa sandbox, not just as a config-resolution possibility.
/// `hakoniwa_backend_restricts_descendant_exec_via_landlock` already proves Hakoniwa has its own,
/// separate native answer to this same problem (Landlock); this test proves the *alternative*
/// mechanism is real too, e.g. for hosts where Landlock is unavailable (older kernels) but
/// `ptrace(2)` still works.
#[test]
fn hakoniwa_backend_denies_forbidden_tool_via_ptrace_seccomp_exec() {
    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_denies_forbidden_tool_via_ptrace_seccomp_exec: \
             firma-hakoniwa-runner was not built"
        );
        return;
    };
    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));
    let bash_canonical =
        std::fs::canonicalize(&bash).expect("canonicalize bash for allowed_executables");

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

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
    patch_backend_to_hakoniwa(&config_path);

    let traffic_sock = world.path("state/hakoniwa-ptrace-sidecar.sock");
    let governance_sock = world.path("state/hakoniwa-ptrace-governance.sock");
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
        tool = forbidden_tool.display(),
    );
    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(&bash_script);
    let output = run_bounded(&mut command, Duration::from_mins(2));

    assert!(
        output.stdout.contains("bash-done"),
        "the allowed bash root did not run at all under hakoniwa + ptrace_seccomp_exec:\n{output}"
    );
    assert!(
        !forbidden_marker.exists() && !output.stdout.contains(FORBIDDEN_MARKER),
        "FIR-366: the forbidden-tool child executed under hakoniwa + ptrace_seccomp_exec \
         governance:\n{output}"
    );

    let governed = governed.lock().expect("lock governance log").clone();
    assert_eq!(
        governed.len(),
        1,
        "expected exactly one governance request for the root command: {governed:?}"
    );
}

/// DNS-stub/proxy-bridge/watchdog/env-strip orchestration — Slice 3 of
/// `docs/architecture/hakoniwa-backend-plan.md` (`DEC-003`).
///
/// Proves two things `PROOF-001`'s extension for this slice names explicitly: killing the proxy
/// bridge mid-run terminates the wrapped command fail-closed (rather than leaving it running
/// unconfined), and no `FIRMA_RUN_*` variable reaches the wrapped command's own environment. Both
/// are exercised in one run since the second is a cheap addition once the first's long-running
/// wrapped command exists.
#[test]
fn hakoniwa_backend_watchdog_kills_wrapped_command_when_bridge_dies() {
    use wait_timeout::ChildExt as _;

    let Some(runner) = hakoniwa_runner_path() else {
        eprintln!(
            "skipping hakoniwa_backend_watchdog_kills_wrapped_command_when_bridge_dies: \
             firma-hakoniwa-runner was not built"
        );
        return;
    };

    let bash = first_existing(&["/usr/bin/bash", "/bin/bash"])
        .unwrap_or_else(|| panic!("bash must be installed in the test environment"));

    let world = TestWorld::isolated();
    let cfg_dir = world.path("config");
    let state_dir = world.state_path();
    let workspace = world.workspace_path();

    world.scaffold_config(
        "generic",
        &cfg_dir,
        &state_dir,
        Some(&workspace),
        &workspace,
    );
    let config_path = cfg_dir.join("firma.toml");
    patch_backend_to_hakoniwa(&config_path);

    let mut command = world.isolated_command_in(env!("CARGO_BIN_EXE_firma"), &workspace);
    command
        .env("FIRMA_RUN_HAKONIWA_RUNNER", &runner)
        .args(["run", "--profile", "generic", "--config"])
        .arg(&config_path)
        .args(["--sidecar", "local", "--authority", "local", "--"])
        .arg(&bash)
        .arg("-c")
        .arg(WATCHDOG_SCRIPT)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    let mut child = command.spawn().expect("spawn hakoniwa-backed firma run");
    let stdout = child.stdout.take().expect("piped stdout");

    // Read in a background thread rather than blocking on this thread until EOF: the DNS stub
    // this run's orchestration deliberately leaves running detached (see
    // `run_entrypoint_orchestration`) holds the write end of this same pipe open for as long as
    // it lives, which is independent of whether the *wrapped command* has been terminated.
    let output = Arc::new(Mutex::new(String::new()));
    let output_reader = Arc::clone(&output);
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buf = [0_u8; 4096];
        let mut stdout = stdout;
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => output_reader
                    .lock()
                    .expect("lock output buffer")
                    .push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
    });

    let root_pid = i32::try_from(child.id()).expect("child pid fits i32");
    let bridge_pid =
        find_pid_by_cmdline_substr(root_pid, "__proxy-bridge", Duration::from_secs(10))
            .unwrap_or_else(|| panic!("proxy bridge did not appear within 10s"));
    // The DNS stub is spawned by the same orchestration and left running detached — it must be
    // reaped explicitly, or it leaks as an orphaned process once this test's own process tree is
    // torn down below.
    let dns_stub_pid = find_pid_by_cmdline_substr(root_pid, "__dns-stub", Duration::from_secs(1));
    kill_pid(bridge_pid);

    let status = child
        .wait_timeout(Duration::from_secs(15))
        .expect("wait for hakoniwa-backed run");

    // Best-effort cleanup regardless of outcome: the wrapped command's own descendants (and, if
    // the watchdog did not work, the wrapped command itself) must not outlive this test.
    for pid in collect_descendant_pids(root_pid) {
        kill_pid(pid);
    }
    if let Some(dns_stub_pid) = dns_stub_pid {
        kill_pid(dns_stub_pid);
    }
    let _ = child.wait();

    let captured = output.lock().expect("lock output buffer").clone();
    let Some(status) = status else {
        panic!(
            "killing the proxy bridge did not terminate the wrapped command within 15s; \
             the watchdog is not working:\n{captured}"
        );
    };
    assert!(
        !status.success(),
        "the wrapped command exited successfully despite the proxy bridge dying:\n{captured}"
    );
    assert!(
        !captured.contains("FINISHED_ALL_TICKS"),
        "the wrapped command ran to completion instead of being terminated fail-closed:\n{captured}"
    );
    assert!(
        captured.trim_start().starts_with('0'),
        "FIRMA_RUN_* variables leaked into the wrapped command's environment:\n{captured}"
    );
}

/// Polls `/proc` for a *descendant of `root_pid`* whose `cmdline` contains `substr`, up to
/// `timeout`. Returns its pid on the first match.
///
/// Scoped to `root_pid`'s own process tree rather than a system-wide `/proc` scan: nextest runs
/// this crate's e2e tests in parallel, and other hakoniwa-backed tests in this same file spawn
/// their own `__proxy-bridge` processes concurrently — a system-wide substring match would
/// nondeterministically kill an unrelated test's bridge instead of this test's own.
fn find_pid_by_cmdline_substr(root_pid: i32, substr: &str, timeout: Duration) -> Option<i32> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let descendants = collect_descendant_pids(root_pid);
        for pid in descendants {
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            if cmdline
                .split(|byte| *byte == 0)
                .any(|arg| String::from_utf8_lossy(arg).contains(substr))
            {
                return Some(pid);
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Returns every pid in `root_pid`'s process subtree (including `root_pid` itself), discovered by
/// scanning `/proc/*/stat` for each process's parent pid.
fn collect_descendant_pids(root_pid: i32) -> Vec<i32> {
    let mut children_of: std::collections::HashMap<i32, Vec<i32>> =
        std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(entry_pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            // Fields after the `(comm)` parenthesized group are space-separated; ppid is the
            // second field overall, i.e. immediately after the `)`.
            let Some(after_comm) = stat.rsplit_once(')') else {
                continue;
            };
            let Some(parent_pid) = after_comm
                .1
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            children_of.entry(parent_pid).or_default().push(entry_pid);
        }
    }

    let mut result = vec![root_pid];
    let mut frontier = vec![root_pid];
    while let Some(pid) = frontier.pop() {
        if let Some(children) = children_of.get(&pid) {
            for &child in children {
                result.push(child);
                frontier.push(child);
            }
        }
    }
    result
}

fn kill_pid(pid: i32) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
}
