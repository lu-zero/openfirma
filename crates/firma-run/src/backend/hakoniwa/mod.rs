//! Linux backend embedding the `hakoniwa` sandboxing library via a sibling
//! launcher binary (`firma-hakoniwa-runner`) instead of shelling out to an
//! external binary, as `BwrapBackend` does for `bwrap`.
//!
//! Experimental (see `docs/architecture/hakoniwa-backend-plan.md`, `DEC-010`)
//! — never a platform default, opt-in only. Slices 1 (network), 2 (mount
//! translation), 3 (DNS-stub/egress-guard bootstrap), 4 (signal-forwarding
//! parity — see `supervisor.rs`'s `hakoniwa_sandbox_root_pid`/
//! `hakoniwa_descendant_pids`), 5 (seccomp/landlock), and `DEC-012` (Slice
//! 7, the DNS-stub port-53 bind fix) done; still no `firma doctor` support
//! (Slice 6).

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::process::{Child, Command};

use firma_identifiers::SandboxId;
use serde::Serialize;

use crate::backend::platform;
use crate::backend::{
    BackendKind, EnforcementProof, LaunchSpec, NetworkConfinement, PrepareRequest, SandboxBackend,
    SandboxHandle, SandboxMount,
};
use crate::config::{ExecutionGovernanceStrategy, NetworkPolicy};
use crate::error::RunError;

mod mount;

use mount::HakoniwaMountOp;

/// Environment variable naming the `firma-hakoniwa-runner` binary to spawn.
///
/// Mirrors `firma-vz-runner`'s `FIRMA_RUN_VZ_GUEST_RUNNER` convention for
/// locating a sibling launcher binary rather than an external one.
const HAKONIWA_RUNNER_ENV: &str = "FIRMA_RUN_HAKONIWA_RUNNER";

/// Version of the on-disk launch-contract schema `firma-hakoniwa-runner`
/// understands. Must match `firma-hakoniwa-runner`'s own constant.
const LAUNCH_CONTRACT_VERSION: u32 = 5;

/// Fixed in-sandbox path `firma` is bind-mounted at for the DNS-stub/
/// proxy-bridge/egress-guarded-run orchestration (`DEC-003`) to exec — see
/// `start_agent`'s own comment for why this can't be the original host path.
const ORCHESTRATION_FIRMA_PATH: &str = "/run/firma-hakoniwa/firma";

/// Linux Hakoniwa backend.
#[derive(Debug, Default)]
pub struct HakoniwaBackend;

impl HakoniwaBackend {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self
    }
}

impl SandboxBackend for HakoniwaBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Hakoniwa
    }

    fn prepare(&self, request: &PrepareRequest) -> Result<SandboxHandle, RunError> {
        if !cfg!(target_os = "linux") {
            return Err(RunError::UnsupportedBackend {
                backend: BackendKind::Hakoniwa.to_string(),
                reason: "hakoniwa backend is only available on Linux hosts".to_string(),
            });
        }

        preflight_host_support(platform::detect_wsl(), platform::userns_restricted())?;

        let runtime_dir = create_hakoniwa_runtime_dir(&request.identity.sandbox_id)?;

        let mounts = request
            .profile
            .mounts
            .iter()
            .cloned()
            .map(SandboxMount::operator_provided)
            .collect::<Vec<_>>();

        Ok(SandboxHandle {
            backend: BackendKind::Hakoniwa,
            runtime_dir,
            identity: request.identity.clone(),
            mounts,
            network_policy: request.profile.network.clone(),
        })
    }

    fn enforce_network(
        &self,
        _handle: &SandboxHandle,
        policy: &NetworkPolicy,
    ) -> Result<EnforcementProof, RunError> {
        let structural = policy.enforce_network_namespace;
        let detail = if structural {
            "network namespace isolation enabled; sandbox-local loopback only, with DNS refused \
             and HTTP/HTTPS relayed through the DNS-stub/proxy-bridge chain to the Sidecar \
             (Slice 3, DEC-012)"
                .to_string()
        } else {
            "network namespace isolation disabled; cooperative routing mode".to_string()
        };
        let network_confinement = if structural {
            // Same OS primitive BwrapBackend uses — the mechanism, not the
            // backend, is what this enum distinguishes. See DEC-005.
            NetworkConfinement::LinuxNetworkNamespace
        } else {
            NetworkConfinement::ProxyOnly
        };

        Ok(EnforcementProof {
            backend: BackendKind::Hakoniwa,
            structural,
            fail_closed: policy.fail_closed,
            detail,
            network_confinement,
        })
    }

    fn verify_fail_closed(
        &self,
        _handle: &SandboxHandle,
        proof: &EnforcementProof,
    ) -> Result<(), RunError> {
        if !proof.fail_closed {
            return Err(RunError::Backend {
                backend: BackendKind::Hakoniwa.to_string(),
                reason: "fail-closed policy is disabled".to_string(),
            });
        }
        Ok(())
    }

    fn start_agent(
        &self,
        runtime_layout: &firma_runtime_state::RuntimeLayout,
        handle: &SandboxHandle,
        launch: &LaunchSpec,
    ) -> Result<Child, RunError> {
        reject_foreign_handle(handle)?;

        mount::reject_symlinked_firma_dirs(launch)?;
        let mut mounts = mount::build_mount_ops(runtime_layout, handle, launch)?;

        let runner = runner_path()?;

        // The DNS-stub/proxy-bridge/egress-guarded-run orchestration
        // (DEC-003) execs `firma` (its hidden subcommands) from *inside* the
        // sandbox. Hakoniwa's `Container::rootfs("/")` only binds
        // OS-standard directories (`/bin`, `/usr`, ...), unlike bwrap's
        // default `--bind / /`, which exposes the whole host — so a
        // dev-build binary (e.g. under `target/debug/`) or any non-standard
        // install prefix would otherwise be unreachable inside the sandbox,
        // breaking the orchestration entirely.
        //
        // **Discovered during implementation**: binding it at its *own* host
        // path (mirroring the sandbox-runtime self-mount elsewhere in this
        // plan) is unsafe here specifically: if the binary's path happens to
        // fall inside `$HOME` (true for any dev build under a home
        // directory) or the working directory, the HOME or cwd bind — which
        // sorts first, being shallower — already exposes it, and Hakoniwa's
        // own bind-mount setup then tries to `touch()` that same live path a
        // second time, which fails with `ETXTBSY` (the kernel refuses to
        // open a currently-executing binary for writing). Fixed by mounting
        // it at a fixed, dedicated in-sandbox path under `/run` instead — a
        // location nothing else in this mount plan ever targets — and
        // overriding `FIRMA_RUN_SELF_EXE` (read by the orchestration to know
        // what to exec) to match. This runner binary itself needs no such
        // mount: unlike an earlier design, its own bridge-death watchdog
        // runs on the host side, in `firma-hakoniwa-runner`'s own `run`
        // function, never inside the sandbox — see that function's docs for
        // why (a namespace's PID 1 is immune to every signal sent by a
        // process inside the same namespace).
        // Mirrors BwrapBackend's `--setenv FIRMA_RUN_RUNTIME_DIR`: the
        // in-sandbox DNS-stub/proxy-bridge orchestration (DEC-003) reads this
        // to know where to write its readiness marker. Injected here, not
        // via the shared `EnvOverrides` mechanism, since it is a
        // backend-owned runtime path, not agent-facing configuration — and
        // it must not leak into the wrapped command's own env, which the
        // orchestration's env-strip step (mirroring the bwrap entrypoint
        // script) enforces alongside every other `FIRMA_RUN_*` variable.
        let mut env: BTreeMap<String, String> = launch
            .env
            .into_iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env.insert(
            "FIRMA_RUN_RUNTIME_DIR".to_string(),
            handle.runtime_dir.display().to_string(),
        );
        if let Some(self_exe) = env.get("FIRMA_RUN_SELF_EXE").cloned() {
            mounts.push(HakoniwaMountOp::Bind {
                source: PathBuf::from(self_exe),
                target: PathBuf::from(ORCHESTRATION_FIRMA_PATH),
                read_only: true,
            });
            env.insert(
                "FIRMA_RUN_SELF_EXE".to_string(),
                ORCHESTRATION_FIRMA_PATH.to_string(),
            );
        }

        // When Landlock will be active, the DNS-stub/proxy-bridge/
        // egress-guarded-run orchestration (DEC-003) needs to `execve`
        // `firma` (its subcommands) regardless of the operator's own
        // `allowed_executables` — Landlock persists across exec to every
        // descendant, so without this the orchestration's own internal
        // execs would be denied the moment an operator configures any
        // exec allow-list at all.
        let mut allowed_executables = launch.allowed_executables.clone();
        if !allowed_executables.is_empty() && env.contains_key("FIRMA_RUN_SELF_EXE") {
            allowed_executables.push(PathBuf::from(ORCHESTRATION_FIRMA_PATH));
        }
        // Same reasoning, for a selected `execution_governance` strategy's
        // own launch-target rewrite: `ExecutionGovernor::rewrite_launch`
        // (e.g. `PtraceSeccompExec`) may replace `launch.executable` with
        // its own shim binary before `start_agent` ever runs — a path that
        // is never itself a member of the operator's own
        // `allowed_executables`. Discovered empirically running
        // `PtraceSeccompExec` against a real Hakoniwa sandbox for the first
        // time: without this, Landlock denied the shim's own exec outright,
        // before it ever got a chance to install its own governance. This
        // is deliberately generic over *which* strategy rewrote the target,
        // not specific to `PtraceSeccompExec` — any non-`Inherited`
        // strategy's rewritten launch target must be exec-able the same
        // way, and any *further* descendant exec is still independently
        // governed by both Landlock (this same allow-list, unchanged
        // otherwise) and whatever the selected strategy itself enforces.
        let alternate_exec_governance_active =
            launch.execution_governance != ExecutionGovernanceStrategy::Inherited;
        if !allowed_executables.is_empty() && alternate_exec_governance_active {
            allowed_executables.push(PathBuf::from(&launch.executable));
        }

        let contract = HakoniwaLaunchContract {
            version: LAUNCH_CONTRACT_VERSION,
            executable: launch.executable.clone(),
            args: launch.args.clone(),
            cwd: launch.cwd.clone(),
            env,
            mounts,
            deny_syscalls: launch.deny_syscalls.clone().unwrap_or_default(),
            allowed_executables,
            landlock_optional: alternate_exec_governance_active,
        };
        let contract_path = write_launch_contract(&handle.runtime_dir, &contract)?;

        Command::new(&runner)
            .arg("--launch-contract")
            .arg(&contract_path)
            .spawn()
            .map_err(|error| {
                RunError::Spawn(format!(
                    "failed to spawn hakoniwa runner {}: {error}",
                    runner.display()
                ))
            })
    }

    fn teardown(&self, handle: SandboxHandle) -> Result<(), RunError> {
        reject_foreign_handle(&handle)?;
        let _ = std::fs::remove_dir_all(&handle.runtime_dir);
        Ok(())
    }
}

/// Fails closed on host environments this backend cannot actually run on,
/// before any runtime-directory or mount work begins. Mirrors
/// `linux_bwrap::preflight_host_support` exactly (same two checks, same
/// reasoning) — found missing here while wiring `firma doctor` support
/// (Slice 6): `HakoniwaBackend::prepare`'s own `cfg!(target_os = "linux")`
/// check is true under WSL (it runs a real Linux kernel), so nothing else
/// in `prepare` caught it. Hakoniwa needs the exact same kernel primitive
/// bwrap does (unprivileged user namespaces), which WSL does not support.
/// A pure function of its two inputs (not reading `/proc` itself) so it's
/// directly testable without a real WSL/restricted host.
fn preflight_host_support(
    wsl_kind: platform::WslKind,
    userns_restriction: Option<String>,
) -> Result<(), RunError> {
    if wsl_kind.is_wsl() {
        return Err(RunError::UnsupportedBackend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: "WSL environment detected; hakoniwa requires unprivileged user \
                     namespaces which are unavailable under WSL. Use a non-hakoniwa \
                     backend on this host or run `firma doctor` for a full sandbox \
                     compatibility report."
                .to_string(),
        });
    }
    if let Some(restriction) = userns_restriction {
        return Err(RunError::UnsupportedBackend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: format!(
                "unprivileged user namespace creation is restricted by {restriction}; \
                 enable it or use a different backend"
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod preflight_tests {
    use super::{RunError, preflight_host_support};
    use crate::backend::platform::WslKind;

    #[test]
    fn preflight_rejects_wsl() {
        let result = preflight_host_support(WslKind::Wsl2, None);
        let err = result.expect_err("WSL must be rejected for hakoniwa");
        let RunError::UnsupportedBackend { reason, .. } = err else {
            panic!("expected UnsupportedBackend, got {err:?}");
        };
        assert!(reason.to_ascii_lowercase().contains("wsl"));
    }

    #[test]
    fn preflight_rejects_userns_restriction() {
        let result = preflight_host_support(
            WslKind::NotWsl,
            Some("/proc/sys/user/max_user_namespaces".to_owned()),
        );
        let err = result.expect_err("a restricted host must be rejected for hakoniwa");
        let RunError::UnsupportedBackend { reason, .. } = err else {
            panic!("expected UnsupportedBackend, got {err:?}");
        };
        assert!(reason.contains("max_user_namespaces"));
    }

    #[test]
    fn preflight_allows_an_ordinary_native_linux_host() {
        preflight_host_support(WslKind::NotWsl, None)
            .expect("a native Linux host with no restriction must be allowed");
    }
}

/// Fails closed when a `SandboxHandle` built by a different backend is
/// passed to a `HakoniwaBackend` method.
///
/// `SandboxHandle.backend` is a plain field, not enforced by the type
/// system (`CW-001` in the design plan) — not reachable through the current
/// production entry point (`runtime::execute_run` builds one backend
/// instance and threads one matching handle through its lifecycle), but
/// cheap insurance against a future refactor making it reachable.
fn reject_foreign_handle(handle: &SandboxHandle) -> Result<(), RunError> {
    if handle.backend == BackendKind::Hakoniwa {
        Ok(())
    } else {
        Err(RunError::Internal(format!(
            "hakoniwa backend received a sandbox handle built by backend '{}'",
            handle.backend
        )))
    }
}

fn runner_path() -> Result<PathBuf, RunError> {
    let raw = env::var(HAKONIWA_RUNNER_ENV).map_err(|_| RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!("{HAKONIWA_RUNNER_ENV} is not set"),
    })?;
    let path = PathBuf::from(raw);
    if !path.is_file() {
        return Err(RunError::Backend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: format!(
                "{HAKONIWA_RUNNER_ENV} does not point to a file: {}",
                path.display()
            ),
        });
    }
    Ok(path)
}

fn create_hakoniwa_runtime_dir(sandbox_id: &SandboxId) -> Result<PathBuf, RunError> {
    let temp_dir = env::temp_dir();
    let runtime_root = temp_dir.join("firma-run");
    firma_fs::create_private_dir_all(&runtime_root).map_err(|error| RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!(
            "failed to create runtime root {}: {error}",
            runtime_root.display()
        ),
    })?;

    let runtime_dir = runtime_root.join(sandbox_id.to_string());
    firma_fs::create_private_dir_all(&runtime_dir).map_err(|error| RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!(
            "failed to create sandbox runtime dir {}: {error}",
            runtime_dir.display()
        ),
    })?;
    Ok(runtime_dir)
}

/// Launch payload handed to `firma-hakoniwa-runner`.
///
/// Must stay in sync with `firma-hakoniwa-runner`'s own `LaunchContract`.
/// Still no identity-mode support (`BwrapBackend`'s sandbox-user remap,
/// passwd/group mounts) — unlike signal-forwarding (Slice 4) and mount-plan
/// translation (Slice 2), both now implemented without adding it, this is a
/// genuinely open, not-yet-scoped-into-any-slice gap, not upcoming work.
#[derive(Debug, Serialize)]
struct HakoniwaLaunchContract {
    version: u32,
    executable: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
    /// Fully resolved, validated filesystem operations for the sandbox,
    /// computed by [`mount::build_mount_ops`]. The runner replays these
    /// verbatim; it makes no masking/authority decisions of its own.
    mounts: Vec<HakoniwaMountOp>,
    /// Syscall names to deny (`Action::Errno(EPERM)`) via a
    /// `hakoniwa::seccomp::Filter`. Empty means no seccomp filter is loaded
    /// at all — see `DEC-004`.
    deny_syscalls: Vec<String>,
    /// Executables allowed to run inside the sandbox. Empty means Landlock's
    /// `Resource::FS` is never restricted (an empty allow-list would brick
    /// the sandbox once FS is restricted at all — see `runc/landlock.rs`'s
    /// `handle_access_fs`, which always handles the *full* read/write/execute
    /// access set together, not just the modes actually used in `allow_path`
    /// calls).
    allowed_executables: Vec<PathBuf>,
    /// Whether the runner may skip building the Landlock ruleset entirely
    /// (rather than hard-failing the whole sandbox launch) when the host
    /// kernel doesn't support Landlock at all.
    ///
    /// `true` exactly when a non-`Inherited` `execution_governance` strategy
    /// is active: that strategy already enforces `allowed_executables`
    /// independently (e.g. `PtraceSeccompExec`'s own ptrace-based exec gate),
    /// so Landlock is redundant defense-in-depth on a kernel that has it and
    /// a safe-to-drop mechanism (not the only enforcement) on one that
    /// doesn't. Under the default `Inherited` strategy, Landlock is the
    /// *only* enforcement of `allowed_executables`, so this stays `false` and
    /// the runner preserves today's behavior: hard-fail closed if Landlock
    /// is unsupported. See `docs/architecture/ptrace-seccomp-exec-gate-plan.md`
    /// `DEC-021`.
    landlock_optional: bool,
}

fn write_launch_contract(
    runtime_dir: &std::path::Path,
    contract: &HakoniwaLaunchContract,
) -> Result<PathBuf, RunError> {
    let contract_path = runtime_dir.join("hakoniwa-launch-contract.json");
    let json = serde_json::to_vec_pretty(contract).map_err(|error| {
        RunError::Internal(format!(
            "failed to serialize hakoniwa launch contract: {error}"
        ))
    })?;
    firma_fs::write_private_file(&contract_path, &json).map_err(|error| RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!(
            "failed to write hakoniwa launch contract {}: {error}",
            contract_path.display()
        ),
    })?;
    Ok(contract_path)
}
