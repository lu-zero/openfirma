//! Host-platform probes used by sandbox backend selection and preflight checks.
//!
//! All probes are read-only and side-effect-free. On non-Linux targets the
//! functions return safe "not applicable" values so callers don't need
//! per-platform gating.

/// Characterises the WSL environment, if any, detected at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WslKind {
    /// Not running inside WSL; native Linux or another OS.
    NotWsl,
    /// Running inside WSL (version indeterminate or WSL 1).
    Wsl,
    /// Running inside WSL 2 specifically.
    Wsl2,
}

impl WslKind {
    /// Returns `true` for any WSL environment.
    #[must_use]
    pub fn is_wsl(self) -> bool {
        !matches!(self, Self::NotWsl)
    }
}

/// Detect whether the process is running inside WSL by reading
/// `/proc/sys/kernel/osrelease`.
///
/// Fails open: if the file cannot be read the function returns [`WslKind::NotWsl`]
/// so non-Linux or restricted environments do not produce false positives.
#[must_use]
pub fn detect_wsl() -> WslKind {
    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    classify_osrelease(&osrelease)
}

/// Pure inner function so tests can supply arbitrary osrelease strings.
fn classify_osrelease(osrelease: &str) -> WslKind {
    let lower = osrelease.to_ascii_lowercase();
    if lower.contains("microsoft") || lower.contains("wsl") {
        if lower.contains("wsl2") {
            WslKind::Wsl2
        } else {
            WslKind::Wsl
        }
    } else {
        WslKind::NotWsl
    }
}

/// Check whether unprivileged user namespace creation is blocked, by sysctl
/// or by any other mechanism.
///
/// Notably catches Ubuntu 23.10+/24.04+'s `AppArmor`
/// `kernel.apparmor_restrict_unprivileged_userns` restriction, which is
/// enforced via an `AppArmor` profile decision on the `unshare(2)`/`clone(2)`
/// call itself, not through either sysctl below.
///
/// Returns `Some(description)` — a specific sysctl path when one of the two
/// known knobs is the cause, or a generic description when a functional
/// probe fails without either knob being set — when user namespace creation
/// is blocked; `None` when it appears to be available or every check is
/// inconclusive (fails open, matching this module's own established
/// convention: an environment this function can't positively confirm as
/// restricted is treated as unrestricted, never the reverse).
///
/// Two sysctls are probed first, in order (cheap, no subprocess):
/// - `/proc/sys/kernel/unprivileged_userns_clone` — Debian/Ubuntu explicit
///   disable flag; value `"0"` means disabled.
/// - `/proc/sys/user/max_user_namespaces` — generic Linux (≥ 4.15); value
///   `"0"` means disabled.
///
/// If neither sysctl indicates a restriction, a functional probe follows:
/// actually attempt unprivileged user namespace creation (via the `unshare`
/// utility, a plain, single-level probe — deliberately not `bwrap`, since
/// this function is also called from `HakoniwaBackend`'s own preflight,
/// which has no dependency on `bwrap` being installed at all) and observe
/// whether it succeeds. This is what catches AppArmor-restricted hosts:
/// their sysctls read as "unrestricted" (neither knob is set), yet the
/// kernel still denies the underlying `unshare(2)` call. Mirrors
/// [`nested_userns_restricted`]'s own "actually try it and see" approach,
/// adapted for the top-level (non-nested) case and for a probe tool neither
/// backend actually depends on.
/// The current process's real uid/gid.
///
/// Shared by `BwrapBackend` and `HakoniwaBackend`'s `SandboxIdentityMode::SandboxUser` handling
/// (the fake `/etc/passwd`/`/etc/group` entries and `USER`/`LOGNAME` env vars presenting a
/// cosmetic "firma-user" identity inside the sandbox — the sandbox does not actually remap the
/// underlying uid). A plain `getuid(2)`/`getgid(2)` read, not a shell-out to `id`: infallible, no
/// subprocess, no dependency on `id` being on `$PATH`.
#[must_use]
pub fn host_uid_gid() -> (u32, u32) {
    (
        nix::unistd::Uid::current().as_raw(),
        nix::unistd::Gid::current().as_raw(),
    )
}

/// Resolve `/etc/resolv.conf` to its canonical on-disk path, following all symlinks.
///
/// On hosts where `/etc/resolv.conf` is a managed symlink (e.g. WSL, systemd-resolved),
/// `mount(2)` with `MS_BIND` follows the symlink to the final target. Both `BwrapBackend` and
/// `HakoniwaBackend` pre-resolve the chain here so they can mount their own stub-pointing content
/// at the explicit canonical target too, preventing bind-mount failures when the symlink points
/// into a managed location the sandbox rootfs cannot otherwise resolve.
///
/// Falls back to `/etc/resolv.conf` itself when:
/// - the path is not a symlink (nothing to resolve)
/// - `canonicalize` fails (broken symlink, permission error)
#[must_use]
pub fn resolve_resolv_conf_target() -> std::path::PathBuf {
    resolve_resolv_conf_target_from(std::path::Path::new("/etc/resolv.conf"))
}

fn resolve_resolv_conf_target_from(resolv_conf: &std::path::Path) -> std::path::PathBuf {
    if !resolv_conf.is_symlink() {
        return resolv_conf.to_path_buf();
    }
    std::fs::canonicalize(resolv_conf).unwrap_or_else(|_| resolv_conf.to_path_buf())
}

#[must_use]
pub fn userns_restricted() -> Option<String> {
    combine_userns_restriction(
        check_sysctl_blocked(
            "/proc/sys/kernel/unprivileged_userns_clone",
            "/proc/sys/user/max_user_namespaces",
        ),
        userns_creation_probe(),
    )
}

/// Testable inner function combining the sysctl result with the functional
/// probe's result. A specific sysctl match always wins (more precise
/// diagnostic); otherwise a probe that genuinely ran and failed
/// (`Some(false)`) is reported generically; a probe that couldn't run at
/// all (`None`) or succeeded (`Some(true)`) means "not restricted."
fn combine_userns_restriction(
    sysctl_result: Option<String>,
    probe_result: Option<bool>,
) -> Option<String> {
    if sysctl_result.is_some() {
        return sysctl_result;
    }
    if probe_result == Some(false) {
        // A noun phrase, deliberately not a `/proc/sys/...` path — callers
        // that build a message like `"restricted by {sysctl}"` or
        // `"restricted ({sysctl}=0)"` must check `starts_with('/')` before
        // appending any sysctl-specific suffix (see both callers' own
        // handling); this string is never a real path.
        return Some(
            "an AppArmor policy (commonly kernel.apparmor_restrict_unprivileged_userns on \
             Ubuntu 23.10+/24.04+, not surfaced through either the unprivileged_userns_clone \
             or max_user_namespaces sysctl)"
                .to_owned(),
        );
    }
    None
}

/// Actually attempt unprivileged user namespace creation and report whether
/// it succeeded.
///
/// Returns `Some(true)`/`Some(false)` when the probe genuinely ran, `None`
/// when the probe itself couldn't run at all (the `unshare` utility is
/// missing, or this isn't Linux) — an inconclusive result, not a positive
/// restriction finding, so callers fail open on `None` the same way the
/// sysctl checks fail open on an absent file.
#[cfg(target_os = "linux")]
fn userns_creation_probe() -> Option<bool> {
    std::process::Command::new("unshare")
        .args(["--user", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()
        .map(|status| status.success())
}

#[cfg(not(target_os = "linux"))]
fn userns_creation_probe() -> Option<bool> {
    None
}

/// Returns `true` when nested user-namespace creation (bwrap inside bwrap) is blocked.
///
/// Probes by running the exact nesting codex performs inside firma's outer sandbox,
/// catching `AppArmor` sysctls, per-binary profiles, setuid bwrap restrictions, and
/// any other mechanism. Always returns `false` on non-Linux.
#[must_use]
pub(crate) fn nested_userns_restricted() -> bool {
    #[cfg(not(target_os = "linux"))]
    return false;

    #[cfg(target_os = "linux")]
    !std::process::Command::new("bwrap")
        .args([
            "--unshare-user",
            "--ro-bind",
            "/",
            "/",
            "bwrap",
            "--unshare-user",
            "--ro-bind",
            "/",
            "/",
            "true",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Testable inner function that accepts explicit sysctl paths.
fn check_sysctl_blocked(
    unprivileged_clone_path: &str,
    max_namespaces_path: &str,
) -> Option<String> {
    for path in [unprivileged_clone_path, max_namespaces_path] {
        if let Ok(content) = std::fs::read_to_string(path)
            && content.trim() == "0"
        {
            return Some(path.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    // ── resolv.conf target resolution ────────────────────────────────────────

    #[test]
    fn resolv_conf_target_is_itself_when_not_a_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_conf = dir.path().join("resolv.conf");
        std::fs::write(&resolv_conf, "nameserver 1.1.1.1\n").expect("write resolv.conf");

        let resolved = resolve_resolv_conf_target_from(&resolv_conf);
        assert_eq!(resolved, resolv_conf);
    }

    #[test]
    fn resolv_conf_target_follows_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real_file = dir.path().join("real_resolv.conf");
        std::fs::write(&real_file, "nameserver 1.1.1.1\n").expect("write real file");

        let symlink_path = dir.path().join("resolv.conf");
        std::os::unix::fs::symlink(&real_file, &symlink_path).expect("create symlink");

        let resolved = resolve_resolv_conf_target_from(&symlink_path);
        assert_eq!(resolved, real_file.canonicalize().expect("canon real"));
    }

    // ── WSL detection ────────────────────────────────────────────────────────

    #[test]
    fn wsl2_kernel_is_detected() {
        assert_eq!(
            classify_osrelease("5.15.90.1-microsoft-standard-WSL2"),
            WslKind::Wsl2
        );
    }

    #[test]
    fn wsl1_style_kernel_is_detected() {
        assert_eq!(classify_osrelease("4.4.0-22000-Microsoft"), WslKind::Wsl);
    }

    #[test]
    fn generic_wsl_without_version_suffix_is_detected() {
        assert_eq!(
            classify_osrelease("5.10.16.3-microsoft-standard"),
            WslKind::Wsl
        );
    }

    #[test]
    fn native_linux_is_not_wsl() {
        assert_eq!(classify_osrelease("6.8.0-52-generic"), WslKind::NotWsl);
    }

    #[test]
    fn empty_osrelease_is_not_wsl() {
        assert_eq!(classify_osrelease(""), WslKind::NotWsl);
    }

    #[test]
    fn wsl_kind_is_wsl_for_wsl_variants() {
        assert!(WslKind::Wsl.is_wsl());
        assert!(WslKind::Wsl2.is_wsl());
        assert!(!WslKind::NotWsl.is_wsl());
    }

    // ── User namespace restriction ───────────────────────────────────────────

    fn write_temp_sysctl(dir: &tempfile::TempDir, name: &str, value: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).expect("create temp sysctl");
        f.write_all(value.as_bytes()).expect("write temp sysctl");
        path
    }

    #[test]
    fn userns_restricted_when_unprivileged_clone_is_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let clone_path = write_temp_sysctl(&dir, "unprivileged_userns_clone", "0\n");
        let max_path = write_temp_sysctl(&dir, "max_user_namespaces", "7340\n");
        let result = check_sysctl_blocked(
            clone_path.to_str().expect("path utf8"),
            max_path.to_str().expect("path utf8"),
        );
        assert!(result.is_some());
        assert!(result.unwrap().contains("unprivileged_userns_clone"));
    }

    #[test]
    fn userns_restricted_when_max_namespaces_is_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let clone_path = dir.path().join("absent_clone");
        let max_path = write_temp_sysctl(&dir, "max_user_namespaces", "0\n");
        let result = check_sysctl_blocked(
            clone_path.to_str().expect("path utf8"),
            max_path.to_str().expect("path utf8"),
        );
        assert!(result.is_some());
        assert!(result.unwrap().contains("max_user_namespaces"));
    }

    #[test]
    fn userns_not_restricted_when_both_knobs_are_nonzero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let clone_path = write_temp_sysctl(&dir, "unprivileged_userns_clone", "1\n");
        let max_path = write_temp_sysctl(&dir, "max_user_namespaces", "7340\n");
        let result = check_sysctl_blocked(
            clone_path.to_str().expect("path utf8"),
            max_path.to_str().expect("path utf8"),
        );
        assert!(result.is_none());
    }

    #[test]
    fn userns_not_restricted_when_sysctl_files_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = check_sysctl_blocked(
            dir.path().join("absent1").to_str().expect("path utf8"),
            dir.path().join("absent2").to_str().expect("path utf8"),
        );
        assert!(result.is_none());
    }

    #[test]
    fn userns_unprivileged_clone_checked_before_max_namespaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let clone_path = write_temp_sysctl(&dir, "unprivileged_userns_clone", "0\n");
        let max_path = write_temp_sysctl(&dir, "max_user_namespaces", "0\n");
        let result = check_sysctl_blocked(
            clone_path.to_str().expect("path utf8"),
            max_path.to_str().expect("path utf8"),
        );
        // First matching path is returned
        assert!(result.unwrap().contains("unprivileged_userns_clone"));
    }

    // ── Functional-probe fallback (AppArmor restriction, e.g. Ubuntu 24.04) ──

    #[test]
    fn combine_prefers_a_specific_sysctl_match_over_the_probe_result() {
        // Even if the probe (hypothetically) disagreed, a specific sysctl
        // match is the more precise diagnostic and must win.
        let result = combine_userns_restriction(
            Some("/proc/sys/user/max_user_namespaces".to_owned()),
            Some(true),
        );
        assert_eq!(
            result.as_deref(),
            Some("/proc/sys/user/max_user_namespaces")
        );
    }

    #[test]
    fn combine_reports_restricted_when_probe_fails_and_no_sysctl_matched() {
        // This is exactly the Ubuntu 23.10+/24.04+ AppArmor case: neither
        // sysctl is set, but the functional probe still fails.
        let result = combine_userns_restriction(None, Some(false));
        assert!(result.is_some());
        assert!(result.unwrap().contains("AppArmor"));
    }

    #[test]
    fn combine_fails_open_when_the_probe_is_inconclusive() {
        // The `unshare` utility being absent (or non-Linux) must never be
        // mistaken for a positive restriction finding.
        assert!(combine_userns_restriction(None, None).is_none());
    }

    #[test]
    fn combine_is_not_restricted_when_probe_succeeds_and_no_sysctl_matched() {
        assert!(combine_userns_restriction(None, Some(true)).is_none());
    }

    #[test]
    fn userns_creation_probe_runs_without_panicking_on_this_host() {
        // A real, unmocked sanity check that the probe mechanism itself
        // (spawning `unshare --user true`) doesn't panic or hang on a real
        // host — the actual restriction-detection logic is covered above
        // via `combine_userns_restriction`, which is what's actually
        // reachable from CI/test hosts that may or may not have `unshare`
        // installed or user namespaces available.
        let _ = userns_creation_probe();
    }
}
