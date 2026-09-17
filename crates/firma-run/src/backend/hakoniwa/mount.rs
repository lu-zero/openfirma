//! Validated filesystem planning for the Hakoniwa backend.
//!
//! Mirrors `linux_bwrap/mount.rs`'s role — the single owner of masking and
//! authority decisions — but targets Hakoniwa's own mount-application model
//! instead of bwrap's. This is a deliberately separate, duplicated
//! implementation, not a shared refactor: the two backends' security
//! posture must stay independently reviewable (see
//! `docs/architecture/hakoniwa-backend-plan.md`, Slice 2).
//!
//! # Why this can't just replay bwrap's plan verbatim
//!
//! bwrap's plan is an ordered sequence of CLI arguments applied strictly in
//! insertion order — masks always win because they are emitted in a later
//! *phase*, regardless of how the mask's target path relates to an earlier
//! overlay's target path. Hakoniwa has no phases: `Container::get_mounts`
//! sorts every mount by target path (ascending, plain string order) and
//! applies them in that order at mount time (confirmed by reading
//! `runc/unshare.rs`'s `initialize_rootfs`), so a shallower target always
//! mounts before a deeper one — ordinary Linux mount-shadowing then makes
//! the deeper mount win.
//!
//! That accidentally reproduces bwrap's guarantee for masks whose target is
//! a *descendant* of an overlay's target (exactly the shape
//! `project_mount_aliases`-equivalent below produces: `overlay_target.join(relative)`
//! is always deeper than `overlay_target`). It does **not** protect the
//! reverse case: an overlay/framework mount whose own target is placed
//! *inside* a masked zone would sort after the (shallower) mask and reopen
//! it. bwrap's phase ordering defends that direction unconditionally; here
//! it is defended explicitly instead, by [`reject_overlay_targets_inside_masked_zones`].
//! Exact-target collisions are additionally simpler than bwrap: Hakoniwa
//! stores mounts in a `HashMap` keyed by the literal target string, so two
//! operations at the same target just overwrite rather than needing any
//! ordering argument at all.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::{
    BackendKind, LaunchSpec, SandboxHandle, SandboxInfrastructureKind, SandboxMount,
    SandboxMountAuthority, SandboxMountPlacement,
};
use crate::config::MountSpec;
use crate::error::RunError;
use firma_config_loader::{CONFIG_DIR_NAME, CONFIG_FILE_NAME};

/// One concrete filesystem operation `firma-hakoniwa-runner` replays inside
/// the sandbox.
///
/// All masking and authority decisions are made here, in the trusted
/// `firma-run` process, before serialization into the launch contract — the
/// runner makes none of its own and applies these in the order given by
/// Hakoniwa itself (target-path order), not this `Vec`'s order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HakoniwaMountOp {
    /// Bind-mounts a host source at a sandbox target.
    Bind {
        source: PathBuf,
        target: PathBuf,
        read_only: bool,
    },
    /// Mounts an empty temporary filesystem over a sandbox path.
    Tmpfs { target: PathBuf },
}

/// Prepared mount whose source has passed authority-aware path validation.
struct ValidatedMount {
    spec: MountSpec,
    authority: SandboxMountAuthority,
    placement: SandboxMountPlacement,
}

/// Whether a mask hides a whole `.firma/` directory or a single config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskKind {
    Dir,
    File,
}

/// Builds the complete, validated list of mount operations for one launch.
///
/// # Errors
///
/// Returns an error when a mount source/target fails authority validation,
/// overlay targets collide or overlap a protected framework subpath, or an
/// overlay/framework mount's target lands inside a masked zone (see the
/// module docs).
pub fn build_mount_ops(
    runtime_layout: &firma_runtime_state::RuntimeLayout,
    handle: &SandboxHandle,
    launch: &LaunchSpec,
) -> Result<Vec<HakoniwaMountOp>, RunError> {
    let control_plane_runtime = resolve_path_allow_missing(runtime_layout.root())?;
    let sandbox_runtime = handle
        .runtime_dir
        .canonicalize()
        .map_err(|error| RunError::Backend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: format!(
                "failed to resolve sandbox runtime {} before planning mounts: {error}",
                handle.runtime_dir.display()
            ),
        })?;

    let mounts = validate_mounts(handle, &control_plane_runtime, &sandbox_runtime)?;
    reject_mount_sources_containing_runner_staging_dir(&mounts, launch)?;
    let reconstructing_etc = handle.network_policy.enforce_network_namespace;
    if reconstructing_etc {
        reject_operator_mount_targeting_etc_anchor(&mounts)?;
    }
    let mut ops = Vec::new();

    if reconstructing_etc {
        push_etc_reconstruction_anchor(&mut ops);
    }

    bind_host_home(&mut ops, launch);
    ops.push(HakoniwaMountOp::Bind {
        source: launch.cwd.clone(),
        target: launch.cwd.clone(),
        read_only: false,
    });
    ops.push(HakoniwaMountOp::Bind {
        source: sandbox_runtime.clone(),
        target: sandbox_runtime.clone(),
        read_only: false,
    });

    for mount in &mounts {
        let spec = &mount.spec;
        ops.push(HakoniwaMountOp::Bind {
            source: spec.source.clone(),
            target: spec.target.clone(),
            read_only: spec.read_only,
        });
    }

    let mut masked = BTreeMap::new();
    mask_firma_dir(&mut ops, launch, &mut masked);
    let overlay_specs = mounts
        .iter()
        .filter(|mount| mount.placement == SandboxMountPlacement::Overlay)
        .map(|mount| &mount.spec)
        .collect::<Vec<_>>();
    project_mount_aliases(&mut ops, &overlay_specs, &mut masked);

    mask_control_plane_runtime(
        &mut ops,
        &mounts,
        &control_plane_runtime,
        &sandbox_runtime,
        launch,
    )?;

    reject_overlay_targets_inside_masked_zones(&mounts, &masked)?;

    Ok(ops)
}

/// Refuses discoverable `.firma/` symlinks before launch, mirroring
/// `linux_bwrap/mount.rs::reject_symlinked_firma_dirs`.
///
/// A symlinked `.firma` entry inside a writable workspace can be unlinked
/// and replaced with a real directory, planting a higher-precedence config
/// for the next run. Fail closed rather than trust a resolved-away symlink.
pub fn reject_symlinked_firma_dirs(launch: &LaunchSpec) -> Result<(), RunError> {
    let mut checked = std::collections::BTreeSet::new();
    for candidate in firma_config_loader::FirmaConfigCandidateAncestors::new(&launch.cwd, None) {
        reject_symlinked_firma_dir(&candidate.config_dir, &mut checked)?;
    }

    if let Some(home_firma) = host_home_firma_dir(launch) {
        reject_symlinked_firma_dir(&home_firma, &mut checked)?;
    }

    if let Some(config_file) = &launch.config_file {
        let config_file = if config_file.is_absolute() {
            config_file.clone()
        } else {
            launch.cwd.join(config_file)
        };
        if let Some(parent) = config_file
            .parent()
            .filter(|parent| parent.file_name().and_then(OsStr::to_str) == Some(CONFIG_DIR_NAME))
        {
            reject_symlinked_firma_dir(parent, &mut checked)?;
        }
    }

    Ok(())
}

fn reject_symlinked_firma_dir(
    dir: &Path,
    checked: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<(), RunError> {
    if dir.file_name().and_then(OsStr::to_str) != Some(CONFIG_DIR_NAME) {
        return Ok(());
    }
    if !checked.insert(dir.to_path_buf()) {
        return Ok(());
    }
    match std::fs::symlink_metadata(dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(RunError::Backend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: format!(
                "refusing to launch hakoniwa sandbox because discoverable config directory {} is a symlink; use a real .firma directory or pass an explicit config file outside .firma",
                dir.display()
            ),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RunError::Backend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: format!(
                "failed to inspect discoverable config directory {} before masking: {error}",
                dir.display()
            ),
        }),
    }
}

/// Rebinds real `$HOME` read-write, mirroring `linux_bwrap/mount.rs::bind_host_home`.
///
/// Hakoniwa's `Container::rootfs("/")` only covers OS-foundation directories
/// (`/bin`, `/etc`, `/lib*`, `/sbin`, `/usr`) — unlike bwrap's `--bind / /`,
/// it never touches `$HOME` at all, so this bind is required unconditionally,
/// not just as a hardening-mode fallback.
fn bind_host_home(ops: &mut Vec<HakoniwaMountOp>, launch: &LaunchSpec) {
    if let Some(home) = resolved_home(launch) {
        ops.push(HakoniwaMountOp::Bind {
            source: PathBuf::from(&home),
            target: PathBuf::from(&home),
            read_only: false,
        });
    }
}

fn resolved_home(launch: &LaunchSpec) -> Option<String> {
    let home = launch
        .env
        .get("HOME")
        .cloned()
        .or_else(|| std::env::var("HOME").ok())?;
    (!home.is_empty() && home.starts_with('/')).then_some(home)
}

fn host_home_firma_dir(launch: &LaunchSpec) -> Option<PathBuf> {
    resolved_home(launch).map(|home| Path::new(&home).join(CONFIG_DIR_NAME))
}

fn validate_mounts(
    handle: &SandboxHandle,
    control_plane_runtime: &Path,
    sandbox_runtime: &Path,
) -> Result<Vec<ValidatedMount>, RunError> {
    let mounts = handle
        .mounts
        .iter()
        .map(|mount| {
            let source = mount.spec().source.canonicalize().map_err(|error| RunError::Backend {
                backend: BackendKind::Hakoniwa.to_string(),
                reason: format!(
                    "failed to resolve mount source {} before planning mounts: {error}",
                    mount.spec().source.display()
                ),
            })?;
            match mount.authority() {
                SandboxMountAuthority::SandboxInfrastructure(kind) => {
                    if !source.starts_with(sandbox_runtime) {
                        return Err(RunError::Backend {
                            backend: BackendKind::Hakoniwa.to_string(),
                            reason: format!(
                                "sandbox-infrastructure mount source {} escapes the private sandbox runtime {}",
                                source.display(),
                                sandbox_runtime.display()
                            ),
                        });
                    }
                    validate_infrastructure_mount(kind, mount.spec())?;
                }
                SandboxMountAuthority::OperatorProvided | SandboxMountAuthority::Framework => {
                    if source.starts_with(control_plane_runtime) {
                        return Err(RunError::Backend {
                            backend: BackendKind::Hakoniwa.to_string(),
                            reason: format!(
                                "refusing mount source {} inside the control-plane runtime {}; wrapped processes must not access FIRMA_STATE_DIR",
                                source.display(),
                                control_plane_runtime.display()
                            ),
                        });
                    }
                }
            }
            if mount.placement() == SandboxMountPlacement::FrameworkProtectedSubpath
                && (mount.authority() != SandboxMountAuthority::Framework
                    || !is_strict_firma_subpath(&mount.spec().target))
            {
                return Err(RunError::Backend {
                    backend: BackendKind::Hakoniwa.to_string(),
                    reason: format!(
                        "framework-protected mount target {} must be strictly inside a .firma directory",
                        mount.spec().target.display()
                    ),
                });
            }

            Ok(ValidatedMount {
                spec: MountSpec {
                    source,
                    target: mount.spec().target.clone(),
                    read_only: mount.spec().read_only,
                },
                authority: mount.authority(),
                placement: mount.placement(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_overlay_destinations(&mounts)?;
    Ok(mounts)
}

fn validate_overlay_destinations(mounts: &[ValidatedMount]) -> Result<(), RunError> {
    let overlays = mounts
        .iter()
        .filter(|mount| {
            mount.placement == SandboxMountPlacement::Overlay
                && !matches!(
                    mount.authority,
                    SandboxMountAuthority::SandboxInfrastructure(_)
                )
        })
        .map(|mount| normalize_absolute_path(&mount.spec.target))
        .collect::<Vec<_>>();

    for (index, target) in overlays.iter().enumerate() {
        for other in overlays.iter().skip(index + 1) {
            if target == other {
                return Err(RunError::Backend {
                    backend: BackendKind::Hakoniwa.to_string(),
                    reason: format!(
                        "duplicate mount targets {} and {} would make sandbox contents depend on mount order",
                        target.display(),
                        other.display()
                    ),
                });
            }
        }
    }

    let protected = mounts
        .iter()
        .filter(|mount| mount.placement == SandboxMountPlacement::FrameworkProtectedSubpath)
        .map(|mount| normalize_absolute_path(&mount.spec.target))
        .collect::<Vec<_>>();
    for (index, target) in protected.iter().enumerate() {
        for other in protected.iter().skip(index + 1) {
            if target.starts_with(other) || other.starts_with(target) {
                return Err(RunError::Backend {
                    backend: BackendKind::Hakoniwa.to_string(),
                    reason: format!(
                        "overlapping protected framework mount targets {} and {} are not permitted",
                        target.display(),
                        other.display()
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_infrastructure_mount(
    kind: SandboxInfrastructureKind,
    spec: &MountSpec,
) -> Result<(), RunError> {
    let valid_target = match kind {
        SandboxInfrastructureKind::Passwd => spec.target == Path::new("/etc/passwd"),
        SandboxInfrastructureKind::Group => spec.target == Path::new("/etc/group"),
        SandboxInfrastructureKind::ResolverConfig => {
            spec.target == Path::new("/etc/resolv.conf")
                || spec.target == crate::backend::platform::resolve_resolv_conf_target()
        }
        SandboxInfrastructureKind::Hosts => spec.target == Path::new("/etc/hosts"),
    };
    if spec.read_only && valid_target {
        return Ok(());
    }
    Err(RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!(
            "invalid {kind:?} sandbox-infrastructure mount at {}; infrastructure files must be read-only and use their designated target",
            spec.target.display()
        ),
    })
}

/// Real host `/etc` paths re-bound read-only onto the reconstructed `/etc`
/// (see [`push_etc_reconstruction_anchor`]) — an allowlist, not "copy
/// everything except a blocklist": an unlisted path is simply absent.
/// Existence-checked, skip-if-absent, mirroring `firma-hakoniwa-runner`'s own
/// `LANDLOCK_READ_ONLY_DIRS`/`LANDLOCK_LIBRARY_DIRS` pattern.
///
/// See `docs/architecture/hakoniwa-etc-reconstruction-plan.md`, `DEC-002`,
/// for the per-path rationale and what is deliberately excluded
/// (`/etc/machine-id`, `/etc/hostname`, `/etc/ssl`/`/etc/pki`).
const PRESERVED_ETC_HOST_PATHS: &[&str] = &[
    "/etc/nsswitch.conf",
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    "/etc/localtime",
    "/etc/passwd",
    "/etc/group",
    "/etc/services",
    "/etc/protocols",
];

/// Builds ordinary framework-authority mounts for [`PRESERVED_ETC_HOST_PATHS`],
/// each read-only from the real host at its own path. Flows through the same
/// `validate_mounts`/`validate_overlay_destinations` pipeline as any operator
/// mount once appended to `handle.mounts` by `HakoniwaBackend::prepare` — a
/// collision with an operator mount targeting the same path is already
/// caught by existing, unmodified validation.
pub(super) fn preserved_etc_host_mounts() -> Vec<SandboxMount> {
    PRESERVED_ETC_HOST_PATHS
        .iter()
        .filter(|path| Path::new(path).exists())
        .map(|path| {
            SandboxMount::framework(MountSpec {
                source: PathBuf::from(path),
                target: PathBuf::from(path),
                read_only: true,
            })
        })
        .collect()
}

/// Replaces `container.rootfs("/")`'s own `/etc` bind mount with a fresh,
/// empty, writable tmpfs — the only op this reconstruction pushes directly
/// into `ops`, bypassing the `SandboxMount` pipeline entirely (`Tmpfs` has no
/// `MountSpec` representation at all), mirroring `mask_firma_dir`'s existing
/// `emit_tmpfs` bypass for the same reason. Hakoniwa's own target-path mount
/// ordering (module docs) then applies every deeper `/etc/*` `Bind` — the
/// preserved paths above, plus the synthesized `resolv.conf`/`hosts` mounts
/// `HakoniwaBackend::prepare` appends — on top of it, each able to `touch()`
/// its own placeholder inside the fresh tmpfs instead of the real, root-owned
/// host file. See `docs/architecture/hakoniwa-etc-reconstruction-plan.md`,
/// `DEC-001`.
fn push_etc_reconstruction_anchor(ops: &mut Vec<HakoniwaMountOp>) {
    ops.push(HakoniwaMountOp::Tmpfs {
        target: PathBuf::from("/etc"),
    });
}

/// Fails closed if any non-infrastructure mount targets exactly `/etc` — the
/// one path [`push_etc_reconstruction_anchor`] pushes outside the
/// `SandboxMount` pipeline, so it is invisible to
/// `validate_overlay_destinations`'s ordinary duplicate-target check.
/// Container's own mount table is a `HashMap` keyed by target (last insert
/// wins, module docs), so without this check an operator mount at `/etc`
/// could silently defeat the whole reconstruction depending on unrelated
/// code ordering, with no diagnostic either way. Deliberately narrower than
/// rejecting every `/etc/*` target: a mount under a *deeper* path (e.g.
/// `/etc/my-app.conf`) is legitimate and lands correctly on the reconstructed
/// tmpfs with no collision at all. See
/// `docs/architecture/hakoniwa-etc-reconstruction-plan.md`, `DEC-008`.
fn reject_operator_mount_targeting_etc_anchor(mounts: &[ValidatedMount]) -> Result<(), RunError> {
    for mount in mounts {
        if matches!(
            mount.authority,
            SandboxMountAuthority::SandboxInfrastructure(_)
        ) {
            continue;
        }
        if normalize_absolute_path(&mount.spec.target) == Path::new("/etc") {
            return Err(RunError::Backend {
                backend: BackendKind::Hakoniwa.to_string(),
                reason: format!(
                    "mount target {} is reserved for hakoniwa's own /etc reconstruction anchor",
                    mount.spec.target.display()
                ),
            });
        }
    }
    Ok(())
}

/// Resolve an absolute mount path through every existing symlink while
/// preserving a suffix that has not been created yet. Mirrors
/// `linux_bwrap/mount.rs::resolve_path_allow_missing`.
fn resolve_path_allow_missing(path: &Path) -> Result<PathBuf, RunError> {
    let absolute = std::path::absolute(path).map_err(|error| RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!(
            "failed to make control-plane runtime path {} absolute: {error}",
            path.display()
        ),
    })?;
    let normalized = normalize_absolute_path(&absolute);
    let mut missing = Vec::<std::ffi::OsString>::new();
    let mut candidate = normalized.as_path();

    loop {
        match candidate.canonicalize() {
            Ok(mut resolved) => {
                while let Some(component) = missing.pop() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = candidate.file_name() else {
                    return Err(RunError::Backend {
                        backend: BackendKind::Hakoniwa.to_string(),
                        reason: format!(
                            "failed to resolve control-plane runtime path {}: no existing ancestor",
                            path.display()
                        ),
                    });
                };
                missing.push(name.to_os_string());
                let Some(parent) = candidate.parent() else {
                    return Err(RunError::Backend {
                        backend: BackendKind::Hakoniwa.to_string(),
                        reason: format!(
                            "failed to resolve control-plane runtime path {}: no parent",
                            path.display()
                        ),
                    });
                };
                candidate = parent;
            }
            Err(error) => {
                return Err(RunError::Backend {
                    backend: BackendKind::Hakoniwa.to_string(),
                    reason: format!(
                        "failed to resolve control-plane runtime path {}: {error}",
                        path.display()
                    ),
                });
            }
        }
    }
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized
                    .components()
                    .next_back()
                    .is_some_and(|part| matches!(part, Component::Normal(_)))
                {
                    normalized.pop();
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn is_strict_firma_subpath(target: &Path) -> bool {
    normalize_absolute_path(target)
        .ancestors()
        .skip(1)
        .any(|ancestor| ancestor.file_name().and_then(OsStr::to_str) == Some(CONFIG_DIR_NAME))
}

/// tmpfs-masks every `.firma/` the agent could discover. Mirrors
/// `linux_bwrap/mount.rs::mask_firma_dir` — see that function's doc comment
/// for the full discovery/fail-closed rationale, which applies identically
/// here.
fn mask_firma_dir(
    ops: &mut Vec<HakoniwaMountOp>,
    launch: &LaunchSpec,
    masked: &mut BTreeMap<PathBuf, MaskKind>,
) {
    for candidate in firma_config_loader::FirmaConfigCandidateAncestors::new(&launch.cwd, None) {
        mask_firma_dir_at(ops, &candidate.config_dir, masked);
    }

    let cwd_firma = launch.cwd.join(CONFIG_DIR_NAME);
    if !cwd_firma.exists() {
        emit_tmpfs(ops, cwd_firma, masked);
    }

    if let Some(home_firma) = host_home_firma_dir(launch) {
        mask_firma_dir_at(ops, &home_firma, masked);
    }

    if let Some(config_file) = &launch.config_file {
        let config_file = if config_file.is_absolute() {
            config_file.clone()
        } else {
            launch.cwd.join(config_file)
        };

        let firma_parent = config_file.parent().filter(|parent| {
            parent.file_name().and_then(|name| name.to_str()) == Some(CONFIG_DIR_NAME)
        });
        if let Some(firma_parent) = firma_parent {
            mask_firma_dir_at(ops, firma_parent, masked);
        } else {
            mask_config_file_at(ops, &config_file, masked);
        }
    }
}

/// Re-applies each mask at the aliased path it acquires under an ordinary
/// overlay. Mirrors `linux_bwrap/mount.rs::project_mount_aliases`.
fn project_mount_aliases(
    ops: &mut Vec<HakoniwaMountOp>,
    mounts: &[&MountSpec],
    masked: &mut BTreeMap<PathBuf, MaskKind>,
) {
    let direct: Vec<(PathBuf, MaskKind)> = masked
        .iter()
        .map(|(path, kind)| (path.clone(), *kind))
        .collect();
    for mount in mounts {
        let Ok(source) = mount.source.canonicalize() else {
            continue;
        };
        for (path, kind) in &direct {
            let Ok(relative) = path.strip_prefix(&source) else {
                continue;
            };
            let alias = if relative.as_os_str().is_empty() {
                mount.target.clone()
            } else {
                mount.target.join(relative)
            };
            match kind {
                MaskKind::Dir => emit_tmpfs(ops, alias, masked),
                MaskKind::File => emit_ro_bind_null(ops, alias, masked),
            }
        }
    }
}

/// Hides host-side Firma runtime state from the wrapped process tree.
/// Mirrors `linux_bwrap/mount.rs::mask_control_plane_runtime`.
fn mask_control_plane_runtime(
    ops: &mut Vec<HakoniwaMountOp>,
    mounts: &[ValidatedMount],
    runtime: &Path,
    sandbox_runtime: &Path,
    launch: &LaunchSpec,
) -> Result<(), RunError> {
    let cwd = launch.cwd.canonicalize().map_err(|error| RunError::Backend {
        backend: BackendKind::Hakoniwa.to_string(),
        reason: format!(
            "failed to resolve sandbox working directory {} before masking control-plane runtime: {error}",
            launch.cwd.display()
        ),
    })?;
    if cwd.starts_with(runtime) {
        return Err(RunError::Backend {
            backend: BackendKind::Hakoniwa.to_string(),
            reason: format!(
                "sandbox working directory {} is inside the control-plane runtime {}; choose a working directory outside FIRMA_STATE_DIR",
                cwd.display(),
                runtime.display()
            ),
        });
    }

    let mut masked = BTreeMap::new();
    emit_tmpfs(ops, runtime.to_path_buf(), &mut masked);
    let specs = mounts.iter().map(|mount| &mount.spec).collect::<Vec<_>>();
    project_mount_aliases(ops, &specs, &mut masked);

    if sandbox_runtime.starts_with(runtime) {
        ops.push(HakoniwaMountOp::Bind {
            source: sandbox_runtime.to_path_buf(),
            target: sandbox_runtime.to_path_buf(),
            read_only: false,
        });
    }
    Ok(())
}

/// Rejects a cwd, `$HOME`, or operator/framework mount whose *source* is the
/// host's temp-directory root or a proper ancestor of it.
///
/// **Discovered during implementation**: Hakoniwa's own `Container` creates
/// an internal staging directory for the sandbox root via
/// `tempfile::TempDir::with_prefix` — which, like `std::env::temp_dir()`,
/// resolves under `$TMPDIR`/`/tmp` — and uses it as the pivot-root target
/// *before* replaying our mount plan. Bind-mounting the host's real temp
/// root (or an ancestor of it) into the sandbox recursively re-exposes that
/// staging directory inside itself, which reproducibly breaks Hakoniwa's own
/// internal cleanup (`rmdir` on its `.oldproc-*` staging path fails with
/// `EBUSY`). This is a Hakoniwa implementation detail, not a masking gap —
/// fail closed with a clear message rather than let the sandbox launch fail
/// deep inside the runner with a cryptic error.
fn reject_mount_sources_containing_runner_staging_dir(
    mounts: &[ValidatedMount],
    launch: &LaunchSpec,
) -> Result<(), RunError> {
    let temp_root = std::env::temp_dir();
    let mut candidates: Vec<(&str, PathBuf)> = vec![("cwd", launch.cwd.clone())];
    if let Some(home) = resolved_home(launch) {
        candidates.push(("$HOME", PathBuf::from(home)));
    }
    for mount in mounts {
        if !matches!(
            mount.authority,
            SandboxMountAuthority::SandboxInfrastructure(_)
        ) {
            candidates.push(("mount source", mount.spec.source.clone()));
        }
    }

    for (description, candidate) in candidates {
        let Ok(candidate) = candidate.canonicalize() else {
            continue;
        };
        if temp_root.starts_with(&candidate) {
            return Err(RunError::Backend {
                backend: BackendKind::Hakoniwa.to_string(),
                reason: format!(
                    "refusing to mount {description} {} because it contains the host's temp \
                     directory {}; hakoniwa's own sandbox-setup staging directory lives there \
                     and cannot be bind-mounted into the sandbox it is still constructing",
                    candidate.display(),
                    temp_root.display()
                ),
            });
        }
    }
    Ok(())
}

/// Rejects an overlay/framework mount whose target lands inside a masked
/// zone (see the module docs for why Hakoniwa needs this explicit check,
/// unlike bwrap).
fn reject_overlay_targets_inside_masked_zones(
    mounts: &[ValidatedMount],
    masked: &BTreeMap<PathBuf, MaskKind>,
) -> Result<(), RunError> {
    for mount in mounts {
        if matches!(
            mount.authority,
            SandboxMountAuthority::SandboxInfrastructure(_)
        ) {
            continue;
        }
        let target = normalize_absolute_path(&mount.spec.target);
        for masked_path in masked.keys() {
            if target.starts_with(masked_path) {
                return Err(RunError::Backend {
                    backend: BackendKind::Hakoniwa.to_string(),
                    reason: format!(
                        "mount target {} is inside protected path {}; hakoniwa applies mounts in target-path order, so a mount placed there would reopen a security mask",
                        target.display(),
                        masked_path.display()
                    ),
                });
            }
        }
    }
    Ok(())
}

fn mask_firma_dir_at(
    ops: &mut Vec<HakoniwaMountOp>,
    dir: &Path,
    masked: &mut BTreeMap<PathBuf, MaskKind>,
) {
    let target = match dir.canonicalize() {
        Ok(canonical) if canonical.file_name().and_then(OsStr::to_str) != Some(CONFIG_DIR_NAME) => {
            mask_config_file_at(ops, &canonical.join(CONFIG_FILE_NAME), masked);
            return;
        }
        Ok(canonical) => canonical,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) if dir.file_name().and_then(OsStr::to_str) != Some(CONFIG_DIR_NAME) => return,
        Err(_) => dir.to_path_buf(),
    };
    let config_file = target.join(CONFIG_FILE_NAME);
    if config_file.is_symlink() {
        mask_config_file_at(ops, &config_file, masked);
    }
    emit_tmpfs(ops, target, masked);
}

fn mask_config_file_at(
    ops: &mut Vec<HakoniwaMountOp>,
    file: &Path,
    masked: &mut BTreeMap<PathBuf, MaskKind>,
) {
    let target = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    emit_ro_bind_null(ops, target, masked);
}

fn emit_tmpfs(
    ops: &mut Vec<HakoniwaMountOp>,
    target: PathBuf,
    masked: &mut BTreeMap<PathBuf, MaskKind>,
) {
    if masked.insert(target.clone(), MaskKind::Dir).is_none() {
        ops.push(HakoniwaMountOp::Tmpfs { target });
    }
}

fn emit_ro_bind_null(
    ops: &mut Vec<HakoniwaMountOp>,
    target: PathBuf,
    masked: &mut BTreeMap<PathBuf, MaskKind>,
) {
    if masked.insert(target.clone(), MaskKind::File).is_none() {
        ops.push(HakoniwaMountOp::Bind {
            source: PathBuf::from("/dev/null"),
            target,
            read_only: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        HakoniwaMountOp, PRESERVED_ETC_HOST_PATHS, SandboxMountAuthority, SandboxMountPlacement,
        ValidatedMount, mask_firma_dir, preserved_etc_host_mounts, project_mount_aliases,
        push_etc_reconstruction_anchor, reject_mount_sources_containing_runner_staging_dir,
        reject_operator_mount_targeting_etc_anchor, reject_overlay_targets_inside_masked_zones,
    };
    use crate::backend::{LaunchSpec, SandboxInfrastructureKind};
    use crate::config::MountSpec;

    /// Pins `HOME` to a non-existent path so `host_home_firma_dir` does not fall
    /// back to the test runner's real `$HOME`, mirroring
    /// `linux_bwrap::mount::tests::launch_with_cwd_and_config`.
    fn launch_with_cwd_and_config(
        cwd: std::path::PathBuf,
        config_file: Option<std::path::PathBuf>,
    ) -> LaunchSpec {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/nonexistent-firma-home".to_string());
        LaunchSpec {
            executable: "/bin/true".to_string(),
            args: vec![],
            cwd,
            env: env.into(),
            sidecar_endpoint: crate::config::SidecarEndpoint::Tcp {
                addr: "127.0.0.1:18080".parse().expect("test sidecar addr"),
            },
            seccomp_filter_path: None,
            deny_syscalls: None,
            allowed_executables: Vec::new(),
            execution_governance: crate::config::ExecutionGovernanceStrategy::Inherited,
            identity_mode: crate::config::SandboxIdentityMode::SandboxUser,
            config_file,
            trust_anchor: None,
        }
    }

    fn tmpfs_targets(ops: &[HakoniwaMountOp]) -> Vec<std::path::PathBuf> {
        ops.iter()
            .filter_map(|op| match op {
                HakoniwaMountOp::Tmpfs { target } => Some(target.clone()),
                HakoniwaMountOp::Bind { .. } => None,
            })
            .collect()
    }

    fn ro_bind_null_targets(ops: &[HakoniwaMountOp]) -> Vec<std::path::PathBuf> {
        ops.iter()
            .filter_map(|op| match op {
                HakoniwaMountOp::Bind {
                    source,
                    target,
                    read_only,
                } if *read_only && source == std::path::Path::new("/dev/null") => {
                    Some(target.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn canonical(path: &std::path::Path) -> std::path::PathBuf {
        path.canonicalize()
            .expect("path should exist for canonicalization")
    }

    fn operator_mount(source: std::path::PathBuf, target: &str) -> ValidatedMount {
        ValidatedMount {
            spec: MountSpec {
                source,
                target: std::path::PathBuf::from(target),
                read_only: false,
            },
            authority: SandboxMountAuthority::OperatorProvided,
            placement: SandboxMountPlacement::Overlay,
        }
    }

    #[test]
    fn mask_firma_dir_masks_dir_without_recreating_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let firma_dir = temp.path().join(".firma");
        std::fs::create_dir_all(&firma_dir).expect("mkdir .firma");
        let config_file = firma_dir.join("firma.toml");
        std::fs::write(&config_file, "").expect("write firma.toml");
        let launch = launch_with_cwd_and_config(temp.path().to_path_buf(), Some(config_file));

        let mut ops = Vec::new();
        let mut masked = BTreeMap::new();
        mask_firma_dir(&mut ops, &launch, &mut masked);

        assert!(tmpfs_targets(&ops).contains(&canonical(&firma_dir)));
        assert!(ro_bind_null_targets(&ops).is_empty());
    }

    #[test]
    fn mask_firma_dir_masks_all_ancestor_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let parent_firma = temp.path().join(".firma");
        let child = temp.path().join("service");
        let child_firma = child.join(".firma");
        std::fs::create_dir_all(&parent_firma).expect("mkdir parent .firma");
        std::fs::create_dir_all(&child_firma).expect("mkdir child .firma");
        let config_file = child_firma.join("firma.toml");
        std::fs::write(&config_file, "").expect("write firma.toml");
        let launch = launch_with_cwd_and_config(child, Some(config_file));

        let mut ops = Vec::new();
        let mut masked = BTreeMap::new();
        mask_firma_dir(&mut ops, &launch, &mut masked);

        let targets = tmpfs_targets(&ops);
        assert!(targets.contains(&canonical(&child_firma)));
        assert!(targets.contains(&canonical(&parent_firma)));
    }

    #[test]
    fn mask_firma_dir_follows_symlink_swap_to_real_path() {
        // A hostile workspace swaps `.firma` for a symlink after discovery. The
        // mask must land on the symlink target's real path, not the link name.
        let temp = tempfile::tempdir().expect("tempdir");
        let real_firma = temp.path().join("real").join(".firma");
        std::fs::create_dir_all(&real_firma).expect("mkdir real .firma");
        let cwd = temp.path().join("workspace");
        std::fs::create_dir_all(&cwd).expect("mkdir workspace");
        let link = cwd.join(".firma");
        std::os::unix::fs::symlink(&real_firma, &link).expect("symlink .firma");

        let launch = launch_with_cwd_and_config(cwd, None);

        let mut ops = Vec::new();
        let mut masked = BTreeMap::new();
        mask_firma_dir(&mut ops, &launch, &mut masked);

        assert!(tmpfs_targets(&ops).contains(&canonical(&real_firma)));
    }

    #[test]
    fn project_mount_aliases_masks_firma_reachable_through_operator_mount() {
        // An operator mount rebinds a tree containing a masked `.firma` at
        // another sandbox target. The alias must be masked too, or the config
        // becomes readable through the operator's own destination.
        let temp = tempfile::tempdir().expect("tempdir");
        let source_tree = temp.path().join("host-tree");
        let firma_dir = source_tree.join(".firma");
        std::fs::create_dir_all(&firma_dir).expect("mkdir .firma");

        let mut masked = BTreeMap::new();
        masked.insert(canonical(&firma_dir), super::MaskKind::Dir);

        let mount = MountSpec {
            source: source_tree,
            target: std::path::PathBuf::from("/workspace"),
            read_only: false,
        };
        let mut ops = Vec::new();
        project_mount_aliases(&mut ops, &[&mount], &mut masked);

        assert!(tmpfs_targets(&ops).contains(&std::path::PathBuf::from("/workspace/.firma")));
    }

    #[test]
    fn reject_overlay_targets_inside_masked_zones_rejects_target_inside_mask() {
        // An operator mount whose *target* (not source) lands inside an
        // already-masked zone must be rejected outright: Hakoniwa applies
        // mounts in target-path order, so a deeper overlay target would sort
        // after the shallower mask and reopen it (see module docs).
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().to_path_buf();
        let mounts = vec![operator_mount(source, "/home/user/.firma/planted")];
        let mut masked = BTreeMap::new();
        masked.insert(
            std::path::PathBuf::from("/home/user/.firma"),
            super::MaskKind::Dir,
        );

        let error = reject_overlay_targets_inside_masked_zones(&mounts, &masked)
            .expect_err("mount inside a masked zone must be rejected");
        assert!(error.to_string().contains("/home/user/.firma"));
    }

    #[test]
    fn reject_overlay_targets_inside_masked_zones_allows_disjoint_targets() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().to_path_buf();
        let mounts = vec![operator_mount(source, "/workspace")];
        let mut masked = BTreeMap::new();
        masked.insert(
            std::path::PathBuf::from("/home/user/.firma"),
            super::MaskKind::Dir,
        );

        reject_overlay_targets_inside_masked_zones(&mounts, &masked)
            .expect("a target unrelated to any masked zone must be allowed");
    }

    #[test]
    fn reject_mount_sources_containing_runner_staging_dir_rejects_temp_root_ancestor() {
        // Discovered during implementation: binding the host's real temp root
        // (or an ancestor of it) recursively re-exposes Hakoniwa's own
        // sandbox-setup staging directory inside itself, which breaks its
        // internal cleanup with a cryptic EBUSY deep inside the runner. Fail
        // closed here instead, with a clear message.
        let temp_root = std::env::temp_dir();
        let launch = launch_with_cwd_and_config(temp_root, None);
        let mounts: Vec<ValidatedMount> = Vec::new();

        let error = reject_mount_sources_containing_runner_staging_dir(&mounts, &launch)
            .expect_err("cwd equal to the host temp root must be rejected");
        assert!(error.to_string().contains("temp directory"));
    }

    #[test]
    fn reject_mount_sources_containing_runner_staging_dir_allows_ordinary_sources() {
        let temp = tempfile::tempdir().expect("tempdir");
        let launch = launch_with_cwd_and_config(temp.path().to_path_buf(), None);
        let mounts: Vec<ValidatedMount> = Vec::new();

        reject_mount_sources_containing_runner_staging_dir(&mounts, &launch)
            .expect("an ordinary workspace directory must be allowed");
    }

    // -- /etc reconstruction (docs/architecture/hakoniwa-etc-reconstruction-plan.md) --

    #[test]
    fn push_etc_reconstruction_anchor_emits_etc_tmpfs() {
        let mut ops = Vec::new();
        push_etc_reconstruction_anchor(&mut ops);
        assert!(tmpfs_targets(&ops).contains(&std::path::PathBuf::from("/etc")));
    }

    #[test]
    fn reject_operator_mount_targeting_etc_anchor_rejects_exact_etc_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mounts = vec![operator_mount(temp.path().to_path_buf(), "/etc")];

        let error = reject_operator_mount_targeting_etc_anchor(&mounts)
            .expect_err("an operator mount targeting exactly /etc must be rejected");
        assert!(error.to_string().contains("/etc"));
    }

    #[test]
    fn reject_operator_mount_targeting_etc_anchor_allows_deeper_etc_subpath() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mounts = vec![operator_mount(
            temp.path().to_path_buf(),
            "/etc/my-app.conf",
        )];

        reject_operator_mount_targeting_etc_anchor(&mounts)
            .expect("an operator mount under a deeper /etc/* path must be allowed");
    }

    #[test]
    fn reject_operator_mount_targeting_etc_anchor_allows_sandbox_infrastructure_at_etc() {
        // The reconstruction's own synthesized mounts (resolv.conf/hosts)
        // never target /etc itself, but this check must not reject
        // SandboxInfrastructure-authority mounts regardless of target,
        // matching the same exemption `reject_overlay_targets_inside_masked_zones`
        // already uses.
        let mounts = vec![ValidatedMount {
            spec: MountSpec {
                source: std::path::PathBuf::from("/dev/null"),
                target: std::path::PathBuf::from("/etc"),
                read_only: true,
            },
            authority: SandboxMountAuthority::SandboxInfrastructure(
                SandboxInfrastructureKind::Hosts,
            ),
            placement: SandboxMountPlacement::Overlay,
        }];

        reject_operator_mount_targeting_etc_anchor(&mounts)
            .expect("a SandboxInfrastructure-authority mount must be exempt from this check");
    }

    #[test]
    fn preserved_etc_host_mounts_only_includes_paths_that_exist() {
        let mounts = preserved_etc_host_mounts();
        // /etc/passwd and /etc/group are as close to universally present on
        // any Linux host capable of running this test suite at all as any
        // path on this list gets — asserting they're included keeps this
        // test from vacuously passing with zero mounts checked on a
        // hypothetical minimal image missing every other preserved path.
        let targets: Vec<&std::path::Path> = mounts
            .iter()
            .map(|mount| mount.spec().target.as_path())
            .collect();
        assert!(
            targets.contains(&std::path::Path::new("/etc/passwd")),
            "expected /etc/passwd among the preserved mounts on this host"
        );
        assert!(
            targets.contains(&std::path::Path::new("/etc/group")),
            "expected /etc/group among the preserved mounts on this host"
        );
        for mount in &mounts {
            let spec = mount.spec();
            assert!(
                PRESERVED_ETC_HOST_PATHS.contains(
                    &spec
                        .target
                        .to_str()
                        .expect("preserved path must be valid utf-8")
                ),
                "unexpected preserved target {}",
                spec.target.display()
            );
            assert!(spec.source.exists(), "{} must exist", spec.source.display());
            assert!(
                spec.read_only,
                "{} must be read-only",
                spec.target.display()
            );
        }
    }
}
