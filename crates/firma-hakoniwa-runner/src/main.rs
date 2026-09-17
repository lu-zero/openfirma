//! Sibling launcher binary embedding the `hakoniwa` sandboxing library.
//!
//! Spawned by `HakoniwaBackend::start_agent` (in `firma-run`) via
//! `std::process::Command`, mirroring `firma-vz-runner`'s role for the macOS
//! VZ backend — see `docs/architecture/hakoniwa-backend-plan.md` (`DEC-001`).
//!
//! Slices 1 (namespace/loopback), 2 (mount translation), and 5
//! (seccomp/Landlock) done. This file's `run_entrypoint_orchestration`
//! reimplements `bwrap_entrypoint.sh`'s DNS-stub/proxy-bridge/watchdog/
//! env-strip sequence natively (Slice 3, `DEC-003`).

use std::collections::BTreeMap;
use std::net::{TcpListener, UdpSocket};
use std::os::fd::AsRawFd as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use hakoniwa::landlock::{CompatMode, FsAccess, Resource, Ruleset};
use hakoniwa::seccomp::{Action, Arch, Filter};
use hakoniwa::{Container, Namespace, Runctl};
use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use serde::{Deserialize, Serialize};

/// Version of the on-disk launch-contract schema this binary understands.
const LAUNCH_CONTRACT_VERSION: u32 = 6;

/// In-namespace uid/gid `IdentityMode::SandboxUser` remaps to via
/// `Container::uidmap`/`gidmap` — the standard "nobody"/"nogroup" ids nearly
/// every Linux host's real `/etc/passwd`/`/etc/group` already define. A
/// genuine kernel-level remap (the process's own `getuid()` reports this,
/// not the real host uid), not a file overlay: `Container::rootfs("/")`
/// reuses the host's real `/etc` wholesale, so replacing `/etc/passwd`
/// itself fails (`touch("etc/group") => Permission denied` — see the plan
/// doc's Slice 2 notes). Mapping to the *real* nobody/nogroup ids instead
/// means `getpwuid`/`getgrgid`-based lookups (`whoami`, etc.) resolve to a
/// real, generic, non-identifying account rather than erroring out or
/// leaking the real host username.
const SANDBOX_IDENTITY_UID: u32 = 65534;
const SANDBOX_IDENTITY_GID: u32 = 65534;

/// `flags` value that makes `landlock_create_ruleset(2)` behave as a pure
/// ABI-version probe: with a null `attr` and zero `size`, the kernel returns
/// the highest Landlock ABI version it supports instead of allocating a
/// ruleset fd (so there is nothing to close on success), or a negative
/// `errno` (`ENOSYS`/`EOPNOTSUPP`) when Landlock isn't supported at all.
/// Verified against `/usr/include/linux/landlock.h`
/// (`LANDLOCK_CREATE_RULESET_VERSION = 1U << 0`) and matches the `landlock`
/// crate's own (deliberately private) internal probe of the same name in
/// `landlock::compat::LandlockStatus::current`.
const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;

/// Prefix stripped from the wrapped command's environment before the final
/// exec (mirrors `bwrap_entrypoint.sh`'s own strip loop) — see
/// [`strip_firma_run_env`].
const FIRMA_RUN_ENV_PREFIX: &str = "FIRMA_RUN_";

/// Command/config directories granted broad *read-only* access when Landlock
/// is active. Deliberately excludes execute: granting execute here would let
/// every binary underneath run regardless of `LaunchContract::allowed_executables`
/// (Landlock's `path_beneath` rules apply recursively to a directory's whole
/// subtree). Execute on these paths is granted narrowly, per allow-listed
/// executable, in [`build_landlock_ruleset`].
const LANDLOCK_READ_ONLY_DIRS: &[&str] = &["/bin", "/sbin", "/etc", "/dev", "/usr"];

/// Library directories granted broad *read+execute* access when Landlock is
/// active.
///
/// Unlike command directories, these must get execute broadly: the kernel's
/// ELF loader checks Landlock's execute right not only on the `execve`d
/// binary itself but also on its ELF interpreter (`PT_INTERP`, e.g.
/// `/lib64/ld-linux-*.so`) and, on this kernel, on every shared library the
/// dynamic linker subsequently `mmap`s with `PROT_EXEC` — confirmed
/// empirically (a plain read-only grant on `/usr` alone made *every* dynamic
/// binary fail to exec with `EACCES`, including ones on the allow-list).
/// These directories hold only libraries, not user-invocable commands, so
/// granting execute broadly here does not undermine the allow-list — unlike
/// doing the same for `/bin` or `/usr/bin`. On a merged-`/usr` host the
/// classic paths below (`/lib`, `/lib64`, ...) are themselves symlinks that
/// canonicalize to the `/usr/lib*` entries; both spellings are listed so
/// non-merged hosts are covered too.
const LANDLOCK_LIBRARY_DIRS: &[&str] = &[
    "/lib",
    "/lib64",
    "/lib32",
    "/usr/lib",
    "/usr/lib64",
    "/usr/lib32",
];

nix::ioctl_readwrite_bad!(get_iface_flags, libc::SIOCGIFFLAGS, libc::ifreq);
nix::ioctl_readwrite_bad!(set_iface_flags, libc::SIOCSIFFLAGS, libc::ifreq);

#[derive(Debug, Parser)]
struct Cli {
    /// Path to the serialized Hakoniwa launch contract written by
    /// `HakoniwaBackend::start_agent`.
    #[arg(long)]
    launch_contract: PathBuf,
}

/// Mirrors `firma_run::config::SandboxIdentityMode`'s wire shape (JSON,
/// `snake_case`) — duplicated, not shared, since this binary depends only on
/// the launch-contract JSON schema, not on the `firma-run` crate itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IdentityMode {
    SandboxUser,
    HostUser,
}

/// Launch payload written by `HakoniwaBackend::start_agent`.
///
/// `identity_mode` drives a real `Container::uidmap`/`gidmap` remap in
/// `run()` — see [`SANDBOX_IDENTITY_UID`]'s own doc comment for why this is
/// a kernel-level remap, not a `bwrap`-style `/etc/passwd` file overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LaunchContract {
    version: u32,
    executable: String,
    args: Vec<String>,
    cwd: PathBuf,
    identity_mode: IdentityMode,
    env: BTreeMap<String, String>,
    /// Fully resolved, validated filesystem operations computed by
    /// `firma-run`'s `hakoniwa::mount::build_mount_ops`. This binary makes no
    /// masking/authority decisions of its own — it replays these verbatim.
    mounts: Vec<MountOp>,
    deny_syscalls: Vec<String>,
    allowed_executables: Vec<PathBuf>,
    /// Whether Landlock's ruleset may be skipped (rather than hard-failing
    /// the whole sandbox launch) when this host's kernel doesn't support
    /// Landlock at all. Set by `firma-run` exactly when a non-`Inherited`
    /// `execution_governance` strategy is already enforcing
    /// `allowed_executables` independently. See
    /// [`landlock_kernel_support_available`].
    landlock_optional: bool,
}

/// Mirrors `firma_run`'s `backend::hakoniwa::mount::HakoniwaMountOp`.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum MountOp {
    Bind {
        source: PathBuf,
        target: PathBuf,
        read_only: bool,
    },
    Tmpfs {
        target: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
enum RunnerError {
    #[error("failed to read launch contract: {0}")]
    Contract(String),
    #[error("failed to prepare sandbox: {0}")]
    Container(String),
    #[error("failed to bring up loopback interface: {0}")]
    Loopback(String),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli.launch_contract) {
        Ok(code) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        Err(error) => {
            eprintln!("firma-hakoniwa-runner: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Reads the launch contract, builds the sandbox, and runs the wrapped
/// command inside it, returning the wrapped command's own exit code.
///
/// # Errors
///
/// Returns an error when the contract cannot be read/parsed or the sandbox
/// itself cannot be prepared — not when the wrapped command exits non-zero,
/// which is reported via the returned exit code instead.
fn run(contract_path: &Path) -> Result<i32, RunnerError> {
    let contract = read_launch_contract(contract_path)?;
    let needs_bridge_watchdog = contract
        .env
        .contains_key("FIRMA_RUN_PROXY_BRIDGE_UPSTREAM_UDS");

    let mut container = Container::new();
    container.unshare(Namespace::Network);
    // `Container::new()` already unshares `Namespace::User` with an identity
    // uid/gid mapping (real uid -> same in-namespace uid) — overriding it
    // here with a distinct value is what actually remaps the reported
    // identity; the mapping stays a single entry, so file-ownership checks
    // against the sandboxed process's own files are unaffected (they are
    // still owned by the same real host uid, now just displayed under the
    // new number). See `SANDBOX_IDENTITY_UID`'s own doc comment.
    if contract.identity_mode == IdentityMode::SandboxUser {
        container.uidmap(SANDBOX_IDENTITY_UID);
        container.gidmap(SANDBOX_IDENTITY_GID);
    }
    // Some bind-mount sources (e.g. `/dev/null`, used to mask config files —
    // see `HakoniwaMountOp`) come from filesystems the host already mounted
    // with locked flags (nosuid/noexec/nodev). Making such a bind read-only
    // requires a second MS_REMOUNT syscall that must repeat those locked
    // flags exactly, which hakoniwa does not query for by default;
    // `MountFallback` retries with the source's actual flags instead of
    // failing the whole launch with EPERM.
    container.runctl(Runctl::MountFallback);
    container
        .rootfs("/")
        .map_err(|error| RunnerError::Container(format!("failed to mount rootfs: {error}")))?;
    container.devfsmount("/dev");
    container.tmpfsmount("/tmp");
    apply_mount_ops(&mut container, &contract.mounts);

    if !contract.deny_syscalls.is_empty() {
        container.seccomp_filter(build_seccomp_filter(&contract.deny_syscalls));
    }
    if !contract.allowed_executables.is_empty() {
        if landlock_kernel_support_available() || !contract.landlock_optional {
            container.landlock_ruleset(build_landlock_ruleset(&contract.allowed_executables));
        } else {
            eprintln!(
                "firma-hakoniwa-runner: Landlock is not supported on this kernel; skipping its \
                 filesystem-confinement ruleset. The configured execution-governance strategy \
                 enforces the executable allow-list independently, but Landlock's own broader \
                 read/write restriction is not active on this host."
            );
        }
    }

    // SAFETY: the closure runs inside the already-unshared, already-mounted
    // sandboxed process (Hakoniwa's "internal process," per its own runc.rs
    // fork sequence), before that process execs the wrapped command. It only
    // brings up loopback (process-local ioctls on a socket it opens itself)
    // and then replaces its own image via `exec` — it never unwinds back
    // across the fork boundary, the same fork-then-exec contract
    // `egress_guard.rs::install_and_exec` already relies on for the bwrap
    // backend.
    let mut command = unsafe {
        container.command_from_closure(move || match bring_up_loopback() {
            Ok(()) => {
                // Bind the DNS-stub's own sockets here, immediately after
                // `bring_up_loopback()` succeeds and before any subsequent
                // `fork`+`exec` — confirmed empirically that this process
                // still holds `CAP_NET_BIND_SERVICE` at this exact point,
                // with no sysctl/capability change needed (`DEC-012`);
                // everything spawned afterward (this same closure's own
                // later `exec`s) does not retain it. `None` on any bind
                // failure — best-effort, matching `spawn_dns_stub`'s own
                // existing non-fatal framing.
                let dns_stub_sockets = contract
                    .env
                    .get("FIRMA_RUN_DNS_STUB_LISTEN_ADDR")
                    .and_then(|addr| bind_dns_stub_sockets(addr));
                run_entrypoint_orchestration(&contract, dns_stub_sockets)
            }
            Err(error) => {
                eprintln!("firma-hakoniwa-runner: {error}");
                125
            }
        })
    };

    let mut child = command.spawn().map_err(|error| {
        RunnerError::Container(format!("failed to spawn sandboxed process: {error}"))
    })?;

    // **Discovered during implementation**: a watchdog spawned *inside* the
    // sandbox (as a child of the process that becomes the wrapped command)
    // cannot terminate it, no matter the signal — Hakoniwa's `Container`
    // unshares a PID namespace, so the wrapped command is PID 1 *within it*,
    // and the kernel protects a namespace's PID 1 from every signal sent by
    // another process *in the same namespace*, SIGKILL included (confirmed
    // directly: `kill -9 1` from a sibling inside a matching `unshare --pid`
    // namespace left PID 1 running). That protection does not apply to a
    // sender in an *ancestor* namespace, which is exactly what this process
    // — still in the host's own PID namespace, since only the later-forked
    // "internal process" Hakoniwa creates ends up inside the new one — is.
    // So the watchdog lives here, as a host-side thread, discovering the
    // bridge's host-visible pid by walking `/proc` for a descendant of the
    // sandbox's own host-visible pid (`child.id()`), rather than as an
    // in-sandbox process working with sandbox-relative pids.
    if needs_bridge_watchdog {
        let sandbox_pid = child.id();
        std::thread::spawn(move || watch_bridge_from_host(sandbox_pid));
    }

    let status = child.wait().map_err(|error| {
        RunnerError::Container(format!("failed to run sandboxed process: {error}"))
    })?;

    // `exit_code` is `None` exactly when the wrapped command never actually
    // ran to completion — a genuine sandbox-setup failure (mount, unshare,
    // seccomp/landlock load, ...) rather than the wrapped command's own exit
    // code or signal death. `reason` is always populated (even on success),
    // so only surface it when it describes that setup failure.
    if status.exit_code.is_none() {
        eprintln!(
            "firma-hakoniwa-runner: hakoniwa sandbox setup failed: {}",
            status.reason
        );
    }
    Ok(status.code)
}

/// Builds a denylist seccomp filter: everything is allowed by default except
/// the syscall names resolved from the profile's `deny_actions` policy, which
/// are denied with `EPERM` — the same errno the bwrap backend's hand-rolled
/// BPF compiler uses for the same policy (see
/// `crates/firma-run/src/seccomp.rs`'s `EPERM_ERRNO`).
/// Replays a fully resolved mount plan against `container`, verbatim.
///
/// All masking/authority decisions were already made by `firma-run`'s
/// `HakoniwaBackend::start_agent` before this contract was serialized; this
/// function makes none of its own. Hakoniwa applies mounts in target-path
/// order regardless of the order they were registered in here (confirmed
/// against `container.rs`/`runc/unshare.rs`), so this loop's order carries
/// no security significance.
fn apply_mount_ops(container: &mut Container, mounts: &[MountOp]) {
    for mount in mounts {
        match mount {
            MountOp::Bind {
                source,
                target,
                read_only,
            } => {
                let source = source.to_string_lossy();
                let target = target.to_string_lossy();
                if *read_only {
                    container.bindmount_ro(&source, &target);
                } else {
                    container.bindmount_rw(&source, &target);
                }
            }
            MountOp::Tmpfs { target } => {
                container.tmpfsmount(&target.to_string_lossy());
            }
        }
    }
}

fn build_seccomp_filter(deny_syscalls: &[String]) -> Filter {
    let mut filter = Filter::new(Action::Allow);
    filter.add_arch(Arch::Native);
    for syscall in deny_syscalls {
        filter.add_rule(Action::Errno(libc::EPERM), syscall);
    }
    filter
}

/// Builds a Landlock ruleset scoping the execute right on command/config
/// directories to `executables`, while leaving ordinary read (and, on
/// `/tmp`, write) access to the rootfs broadly available, and library
/// directories broadly executable (see `LANDLOCK_LIBRARY_DIRS`).
///
/// Hakoniwa's `Resource::FS` restriction always handles the *full*
/// read/write/execute access set together, not just the modes actually used
/// in `allow_path` calls (see `runc/landlock.rs`'s `handle_access_fs`) — so
/// restricting FS at all without granting broad read access here would brick
/// the sandbox's ordinary library/config loading, not just narrow execute.
/// Directories that do not exist on this host (e.g. `/lib32` without 32-bit
/// multiarch support) are skipped — `allow_path` canonicalizes its path when
/// the ruleset loads and hard-fails the whole sandbox launch if that fails.
fn build_landlock_ruleset(executables: &[PathBuf]) -> Ruleset {
    let mut ruleset = Ruleset::default();
    ruleset.restrict(Resource::FS, CompatMode::Enforce);

    for dir in LANDLOCK_READ_ONLY_DIRS {
        if Path::new(dir).is_dir() {
            ruleset.allow_path(dir, FsAccess::R);
        }
    }
    for dir in LANDLOCK_LIBRARY_DIRS {
        if Path::new(dir).is_dir() {
            ruleset.allow_path(dir, FsAccess::R | FsAccess::X);
        }
    }
    ruleset.allow_path("/tmp", FsAccess::R | FsAccess::W);

    for executable in executables {
        ruleset.allow_path(&executable.to_string_lossy(), FsAccess::R | FsAccess::X);
    }
    ruleset
}

/// Probes whether the running kernel supports Landlock at all, independent
/// of any particular ABI version.
///
/// The `landlock` crate deliberately does not expose this itself — its own
/// doc comment on `ABI`/`LandlockStatus` warns that "ABI should not be
/// dynamically created ... to avoid inconsistent behaviors and
/// non-determinism," and its internal probe (`LandlockStatus::current`) is a
/// private fn for exactly that reason. This replicates the same underlying
/// raw syscall directly: `landlock_create_ruleset(NULL, 0,
/// LANDLOCK_CREATE_RULESET_VERSION)` is the kernel's documented "query
/// supported ABI version" form, used here only to answer "supported or not,"
/// not to pick an ABI to build a ruleset against — [`build_landlock_ruleset`]
/// still goes through the `landlock`/`hakoniwa` crates' own ABI negotiation
/// unchanged.
fn landlock_kernel_support_available() -> bool {
    // SAFETY: this is the kernel's documented ABI-version-probe calling
    // form — a null `attr` with `size == 0` is required to be accepted, and
    // the kernel neither reads through `attr` nor allocates an fd in this
    // mode, so there is no memory to account for and nothing to close.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0_usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    version >= 0
}

fn read_launch_contract(path: &Path) -> Result<LaunchContract, RunnerError> {
    let bytes = std::fs::read(path)
        .map_err(|error| RunnerError::Contract(format!("{}: {error}", path.display())))?;
    let contract: LaunchContract = serde_json::from_slice(&bytes)
        .map_err(|error| RunnerError::Contract(format!("{}: {error}", path.display())))?;
    if contract.version != LAUNCH_CONTRACT_VERSION {
        return Err(RunnerError::Contract(format!(
            "unsupported launch contract version {} (expected {LAUNCH_CONTRACT_VERSION})",
            contract.version
        )));
    }
    Ok(contract)
}

/// Brings up the sandbox's own loopback interface.
///
/// Hakoniwa does not do this itself unless a `Network` mode (`Pasta`,
/// `RustSlirp`) is configured — omitting `Container::network(...)` entirely
/// (as this backend does, per `DEC-002`) leaves `lo` present but down. This
/// mirrors the handful of ioctls `hakoniwa`'s own `rustslirp` feature uses
/// for the same purpose, without pulling in that feature's TUN-device and
/// userspace-routing machinery, which this backend does not need.
fn bring_up_loopback() -> Result<(), RunnerError> {
    use nix::sys::socket::{AddressFamily, SockFlag, SockType, socket};

    let fd = socket(
        AddressFamily::Inet,
        SockType::Datagram,
        SockFlag::empty(),
        None,
    )
    .map_err(|error| RunnerError::Loopback(format!("failed to open control socket: {error}")))?;

    // SAFETY: `ifreq` is a plain C struct; zero-initializing it is valid.
    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    for (slot, byte) in ifr.ifr_name.iter_mut().zip(b"lo") {
        *slot = libc::c_char::from(*byte);
    }

    // SAFETY: `fd` is a valid, open socket for the duration of these calls;
    // `ifr` is fully initialized (zeroed, then its name field set) before
    // either ioctl touches it.
    unsafe {
        get_iface_flags(fd.as_raw_fd(), &raw mut ifr)
            .map_err(|error| RunnerError::Loopback(format!("SIOCGIFFLAGS: {error}")))?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "IFF_UP | IFF_RUNNING is a small, fixed constant that always fits in c_short"
        )]
        {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
        }
        set_iface_flags(fd.as_raw_fd(), &raw mut ifr)
            .map_err(|error| RunnerError::Loopback(format!("SIOCSIFFLAGS: {error}")))?;
    }
    Ok(())
}

/// Replaces this process's image with the wrapped command, never returning
/// on success. Returns a process exit code only when `exec` itself fails.
///
/// `Command::exec` reports one combined `io::Error` for both the `chdir`
/// into `contract.cwd` and the `execve` of `contract.executable` — a failure
/// here is not necessarily the executable itself. In particular, Slice 1 has
/// no mount translation yet (Slice 2), so `contract.cwd` (always the
/// invoking `firma run` process's real working directory) will not exist
/// inside this bare-rootfs sandbox unless it happens to be a path the
/// rootfs/`/tmp` mounts already provide.
fn exec_real_command(contract: &LaunchContract, env: &BTreeMap<String, String>) -> i32 {
    let mut command = std::process::Command::new(&contract.executable);
    command.args(&contract.args);
    command.current_dir(&contract.cwd);
    command.env_clear();
    command.envs(env);

    let error = command.exec();
    eprintln!(
        "firma-hakoniwa-runner: failed to exec {} (cwd {}): {error}",
        contract.executable,
        contract.cwd.display()
    );
    126
}

/// Reimplements `bwrap_entrypoint.sh`'s orchestration natively (`DEC-003`),
/// in the order the script itself uses: best-effort DNS-stub startup, a
/// fail-closed proxy-bridge startup with a readiness handshake, an
/// unconditional `FIRMA_RUN_*` env-strip, then either `firma
/// __egress-guarded-run` or a direct exec of the wrapped command.
///
/// All three subprocess targets (`__dns-stub`, `__proxy-bridge`,
/// `__egress-guarded-run`) are the same, already-backend-agnostic binaries
/// `BwrapBackend` invokes — reused unchanged, not reimplemented (`DEC-003`).
///
/// The bridge-death watchdog bwrap's entrypoint script also runs is *not*
/// implemented here — it runs in `run`'s own host-side thread instead. See
/// that function's docs for why: a watchdog spawned from inside this
/// function (i.e. inside the sandbox) cannot terminate the wrapped command,
/// since Hakoniwa's `Container` makes it PID 1 in its own PID namespace.
fn run_entrypoint_orchestration(
    contract: &LaunchContract,
    dns_stub_sockets: Option<(UdpSocket, TcpListener)>,
) -> i32 {
    let self_exe = contract.env.get("FIRMA_RUN_SELF_EXE").cloned();
    let runtime_dir = contract.env.get("FIRMA_RUN_RUNTIME_DIR").cloned();

    if let Some(self_exe) = &self_exe
        && let Some((udp, tcp)) = dns_stub_sockets
    {
        // Best-effort: DNS resolution failing is not fatal to the sandbox,
        // matching `bwrap_entrypoint.sh`'s own non-fatal treatment. The
        // spawned child is intentionally not retained — `Child`'s `Drop`
        // does not kill it, so it keeps running detached in the background
        // for the sandbox's lifetime, same as the shell script's own
        // backgrounded `dns_pid`. `spawn_dns_stub` takes ownership of
        // `udp`/`tcp` and drops them once it returns (`DEC-012`) — after
        // its own `Command::spawn()` has already forked, so the dns-stub
        // child's independently-inherited copies of these fds stay open,
        // but this process's own copies do not survive into any later
        // `exec` it performs itself (the proxy bridge, `egress-guarded-run`,
        // or the final wrapped command) — none of those should ever hold
        // an open handle to the DNS stub's own listening sockets.
        let _ = spawn_dns_stub(self_exe, &contract.env, udp, tcp);
    }

    let bridge = match (
        &self_exe,
        contract.env.get("FIRMA_RUN_PROXY_BRIDGE_UPSTREAM_UDS"),
    ) {
        (Some(self_exe), Some(upstream_uds)) => {
            let Some(runtime_dir) = &runtime_dir else {
                eprintln!(
                    "firma-hakoniwa-runner: FIRMA_RUN_PROXY_BRIDGE_UPSTREAM_UDS is set without \
                     FIRMA_RUN_RUNTIME_DIR"
                );
                return 125;
            };
            let listen_addr = contract
                .env
                .get("FIRMA_RUN_PROXY_LISTEN_ADDR")
                .map_or("127.0.0.1:18080", String::as_str);
            match spawn_and_await_proxy_bridge(
                self_exe,
                &contract.env,
                listen_addr,
                upstream_uds,
                runtime_dir,
            ) {
                Ok(child) => Some(child),
                Err(error) => {
                    eprintln!("firma-hakoniwa-runner: {error}");
                    return 125;
                }
            }
        }
        _ => None,
    };

    // The bridge is deliberately not retained: `run`'s own host-side thread
    // (see its docs) monitors it independently by walking `/proc` for its
    // host-visible pid, since a watchdog running inside the sandbox cannot
    // terminate the sandbox's own PID 1 by any signal. Dropping this handle
    // does not kill the bridge (`Child`'s `Drop` never does), so it keeps
    // running detached for the rest of this run.
    drop(bridge);

    let egress_guard_sock = contract.env.get("FIRMA_RUN_EGRESS_GUARD_SOCK").cloned();
    let stripped_env = strip_firma_run_env(&contract.env);

    if let (Some(self_exe), Some(sock)) = (&self_exe, &egress_guard_sock) {
        let mut command = std::process::Command::new(self_exe);
        command
            .arg("__egress-guarded-run")
            .arg("--supervisor-socket")
            .arg(sock)
            .arg("--")
            .arg(&contract.executable)
            .args(&contract.args)
            .current_dir(&contract.cwd)
            .env_clear()
            .envs(&stripped_env);
        let error = command.exec();
        eprintln!("firma-hakoniwa-runner: failed to exec {self_exe} __egress-guarded-run: {error}");
        return 126;
    }

    exec_real_command(contract, &stripped_env)
}

/// Binds the DNS-stub's UDP/TCP listen sockets directly, before this
/// process's own capabilities are lost at the next `fork`+`exec` boundary
/// (`DEC-012` in `docs/architecture/hakoniwa-backend-plan.md`) — confirmed
/// empirically that a direct bind here succeeds with no sysctl/capability
/// change needed, since this runs before any `exec` crosses that boundary,
/// the same timing `bring_up_loopback` itself already relies on. Clears
/// `FD_CLOEXEC` on both so [`spawn_dns_stub`]'s later `Command::spawn()`
/// can pass them to the `firma __dns-stub` child across `exec`.
///
/// Best-effort: returns `None` (logged) on any bind or `fcntl` failure,
/// matching [`spawn_dns_stub`]'s own established non-fatal framing — a
/// failure here only means the sandboxed process loses DNS resolution
/// through the stub, not that the whole sandbox launch fails.
fn bind_dns_stub_sockets(listen_addr: &str) -> Option<(UdpSocket, TcpListener)> {
    let udp = UdpSocket::bind(listen_addr)
        .inspect_err(|error| {
            eprintln!(
                "firma-hakoniwa-runner: failed to bind DNS UDP stub at {listen_addr}: {error}"
            );
        })
        .ok()?;
    let tcp = TcpListener::bind(listen_addr)
        .inspect_err(|error| {
            eprintln!(
                "firma-hakoniwa-runner: failed to bind DNS TCP stub at {listen_addr}: {error}"
            );
        })
        .ok()?;
    clear_fd_cloexec(&udp)
        .inspect_err(|error| eprintln!("firma-hakoniwa-runner: {error}"))
        .ok()?;
    clear_fd_cloexec(&tcp)
        .inspect_err(|error| eprintln!("firma-hakoniwa-runner: {error}"))
        .ok()?;
    Some((udp, tcp))
}

/// Clears `FD_CLOEXEC` on `fd` so it survives a subsequent `exec`, mirroring
/// `crates/firma-run/src/backend/linux_bwrap/mod.rs`'s own
/// `clear_fd_cloexec` (used there for bwrap's seccomp fd) — duplicated
/// rather than shared, since `firma-hakoniwa-runner` does not depend on
/// `firma-run` (a separate binary crate, `DEC-001`).
fn clear_fd_cloexec<Fd: std::os::fd::AsFd>(fd: &Fd) -> Result<(), RunnerError> {
    let flags = fcntl(fd, FcntlArg::F_GETFD)
        .map_err(|error| RunnerError::Loopback(format!("failed to read fd flags: {error}")))?;
    let mut fd_flags = FdFlag::from_bits_truncate(flags);
    fd_flags.remove(FdFlag::FD_CLOEXEC);
    fcntl(fd, FcntlArg::F_SETFD(fd_flags)).map_err(|error| {
        RunnerError::Loopback(format!("failed to clear CLOEXEC on fd: {error}"))
    })?;
    Ok(())
}

/// Starts `firma __dns-stub --inherited-udp-fd <n> --inherited-tcp-fd <n>`,
/// passing `udp`/`tcp` — already bound at `FIRMA_RUN_DNS_STUB_LISTEN_ADDR`
/// by [`bind_dns_stub_sockets`] before this process's own capability drop
/// — across `exec` rather than having the child bind them itself
/// (`DEC-012`).
///
/// Best-effort, mirroring `bwrap_entrypoint.sh`: a failed or crashed stub
/// does not fail the sandbox launch, since the sandboxed process still has a
/// fail-closed path (no other route out once the network namespace is
/// unshared) — it only loses DNS resolution through the stub. Returns the
/// spawned child on success so the caller can deliberately leak it (see
/// [`run_entrypoint_orchestration`]); returns `None` and logs otherwise.
///
/// Takes `udp`/`tcp` by value and lets them drop at the end of this
/// function, once `Command::spawn()` has already forked — the child's own,
/// independently-inherited copies of these fds stay open regardless; this
/// process's own copies must not survive into whatever it `exec`s into
/// next (see [`run_entrypoint_orchestration`]'s own doc comment).
#[expect(
    clippy::needless_pass_by_value,
    reason = "by-value is deliberate, not incidental: udp/tcp must stay alive (not be dropped by \
              an earlier-returning caller) until after Command::spawn() below has forked, then be \
              dropped by this function itself so this process's own copies don't survive into a \
              later exec (DEC-012) — a reference would let the caller drop them at the wrong time"
)]
fn spawn_dns_stub(
    self_exe: &str,
    env: &BTreeMap<String, String>,
    udp: UdpSocket,
    tcp: TcpListener,
) -> Option<std::process::Child> {
    let mut child = std::process::Command::new(self_exe)
        .arg("__dns-stub")
        .arg("--inherited-udp-fd")
        .arg(udp.as_raw_fd().to_string())
        .arg("--inherited-tcp-fd")
        .arg(tcp.as_raw_fd().to_string())
        .env_clear()
        .envs(env)
        .spawn()
        .inspect_err(|error| {
            eprintln!("firma-hakoniwa-runner: failed to spawn dns stub: {error}");
        })
        .ok()?;

    // Give the stub a brief window to bind before the wrapped command
    // starts, matching `bwrap_entrypoint.sh`'s own fixed 0.2s wait.
    std::thread::sleep(Duration::from_millis(200));
    match child.try_wait() {
        Ok(None) => Some(child),
        Ok(Some(status)) => {
            eprintln!("firma-hakoniwa-runner: dns stub exited during startup: {status}");
            None
        }
        Err(error) => {
            eprintln!("firma-hakoniwa-runner: failed to poll dns stub: {error}");
            None
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ProxyBridgeError {
    #[error("failed to spawn proxy bridge: {0}")]
    Spawn(std::io::Error),
    #[error("proxy bridge exited during startup")]
    ExitedDuringStartup,
    #[error("proxy bridge did not signal readiness within 5 seconds")]
    ReadinessTimeout,
    #[error("failed to poll proxy bridge: {0}")]
    Poll(std::io::Error),
}

/// Starts `firma __proxy-bridge` and waits for its readiness marker file,
/// mirroring `bwrap_entrypoint.sh`'s handshake exactly (50 polls of 100ms —
/// 5s total). Unlike the DNS stub, a failure here is fatal: the agent has no
/// other route to the Sidecar once the network namespace is unshared.
fn spawn_and_await_proxy_bridge(
    self_exe: &str,
    env: &BTreeMap<String, String>,
    listen_addr: &str,
    upstream_uds: &str,
    runtime_dir: &str,
) -> Result<std::process::Child, ProxyBridgeError> {
    let mut child = std::process::Command::new(self_exe)
        .arg("__proxy-bridge")
        .arg("--listen")
        .arg(listen_addr)
        .arg("--upstream-uds")
        .arg(upstream_uds)
        .env_clear()
        .envs(env)
        .spawn()
        .map_err(ProxyBridgeError::Spawn)?;

    let ready_file = Path::new(runtime_dir).join("proxy-bridge-ready");
    for _ in 0..50 {
        if ready_file.is_file() {
            return Ok(child);
        }
        if child.try_wait().map_err(ProxyBridgeError::Poll)?.is_some() {
            return Err(ProxyBridgeError::ExitedDuringStartup);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    Err(ProxyBridgeError::ReadinessTimeout)
}

/// Host-side watchdog thread body: finds the proxy bridge among `sandbox_pid`'s descendants (as
/// seen from *this* process's own, host, PID namespace — see `run`'s docs), then polls its
/// liveness and `SIGKILL`s the whole sandbox the moment it is gone.
///
/// `SIGKILL` from here works precisely because this process is in an ancestor namespace relative
/// to the sandbox: a namespace's PID 1 is immune to every signal — `SIGKILL` included — sent by
/// another process *inside the same namespace*, but not to one sent from outside it.
fn watch_bridge_from_host(sandbox_pid: u32) {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    let Some(bridge_pid) =
        find_descendant_pid_by_cmdline(sandbox_pid, "__proxy-bridge", Duration::from_secs(10))
    else {
        eprintln!(
            "firma-hakoniwa-runner: proxy bridge did not appear among the sandbox's descendants \
             within 10s; its watchdog is not armed"
        );
        return;
    };

    loop {
        if signal::kill(Pid::from_raw(bridge_pid), None).is_err() {
            eprintln!(
                "firma-hakoniwa-runner: proxy bridge (host pid {bridge_pid}) exited \
                 unexpectedly; terminating the sandbox fail-closed"
            );
            let Ok(sandbox_pid) = i32::try_from(sandbox_pid) else {
                return;
            };
            let _ = signal::kill(Pid::from_raw(sandbox_pid), Signal::SIGKILL);
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Polls `/proc` for a descendant of `root_pid` whose `cmdline` contains `substr`, up to
/// `timeout`. Returns its pid (in the caller's own PID namespace) on the first match.
fn find_descendant_pid_by_cmdline(root_pid: u32, substr: &str, timeout: Duration) -> Option<i32> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        for pid in descendant_pids(root_pid) {
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
/// scanning `/proc/*/stat` for each process's parent pid, as seen from the caller's own PID
/// namespace.
fn descendant_pids(root_pid: u32) -> Vec<i32> {
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
            let Some((_, after_comm)) = stat.rsplit_once(')') else {
                continue;
            };
            let Some(parent_pid) = after_comm
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            children_of.entry(parent_pid).or_default().push(entry_pid);
        }
    }

    let Ok(root_pid) = i32::try_from(root_pid) else {
        return Vec::new();
    };
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

/// Strips every `FIRMA_RUN_*` control variable before the final exec,
/// mirroring `bwrap_entrypoint.sh`'s own strip loop: the wrapped command and
/// anything it spawns must not inherit this sandbox's identity or runtime
/// paths — a nested `firma run` that inherited `FIRMA_RUN_SANDBOX_ID` would
/// derive this live session's runtime dir and could get it bind-mounted
/// read-write into the inner sandbox.
fn strip_firma_run_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .filter(|(key, _)| !key.starts_with(FIRMA_RUN_ENV_PREFIX))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
