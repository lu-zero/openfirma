//! Config-masking scenarios prove a TOCTOU-adjacent claim — that a compromised agent cannot read
//! or poison `firma.toml`/policy state via symlink swaps, mount aliases, or precedence tricks —
//! against **every** structural backend's own masking implementation, not just one. `bwrap` and
//! `hakoniwa` each compute their own mount plan independently (`linux_bwrap/mount.rs` vs.
//! `hakoniwa/mount.rs` — "a deliberately separate, duplicated implementation... not a shared
//! refactor," per `hakoniwa-backend-plan.md`'s Slice 2 notes), so a symlink-swap or alias
//! resistance proven against one says nothing about the other. Every scenario below therefore
//! loops over [`Backend::Bwrap`] and [`Backend::Hakoniwa`], skipping the `hakoniwa` iteration
//! (not the whole test) when `firma-hakoniwa-runner` isn't built, matching
//! `hakoniwa_backend.rs`'s own skip convention.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::harness::{Backend, ProcessOutput, TestWorld, hakoniwa_runner_path};

const MASK_TEST_SENTINEL: &str = "firma-mask-adversarial-sentinel";
const MASK_TEST_RAN_MARKER: &str = "FIRMA-MASK-ADVERSARIAL-RAN";
const BACKENDS: [Backend; 2] = [Backend::Bwrap, Backend::Hakoniwa];

#[test]
fn masks_firma_config_under_workspace_mount() {
    const SENTINEL: &str = "firma-mask-structural-sentinel";
    const RAN_MARKER: &str = "STRUCTURAL-SANDBOX-RAN";

    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(backend, "masks_firma_config_under_workspace_mount");
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let state_dir = world.state_path();
        world.scaffold_config("codex", &config_dir, &state_dir, None, &workspace);

        let config_path = config_dir.join("firma.toml");
        let generated = std::fs::read_to_string(&config_path).expect("read generated config");
        std::fs::write(&config_path, format!("{generated}\n# {SENTINEL}\n"))
            .expect("plant sentinel in config");
        patch_backend(&config_path, "codex", backend);

        let shell = format!(
            "cat {config} 2>/dev/null; echo {RAN_MARKER}",
            config = shell_quote(&config_path),
        );
        let borrowed_env: Vec<(&str, &str)> = extra_env
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let output = world.run_firma_with_env(
            "codex",
            Some(&config_path),
            &workspace,
            &[],
            &borrowed_env,
            "sh",
            ["-c", &shell],
        );
        assert!(
            output.success(),
            "[{backend:?}] firma run failed:\n{output}"
        );
        assert!(
            output.stdout.contains(RAN_MARKER),
            "[{backend:?}] sandboxed command did not run:\n{output}"
        );
        assert!(
            !output.stdout.contains(SENTINEL),
            "[{backend:?}] config mask leaked under the codex workspace mount — the agent read \
             firma.toml.\n{output}"
        );
    }
}

#[test]
fn missing_nearer_firma_candidate_cannot_be_planted() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(backend, "missing_nearer_firma_candidate_cannot_be_planted");
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let run_cwd = workspace.join("service");
        let config_dir = workspace.join(".firma");
        std::fs::create_dir_all(&run_cwd).expect("mkdir run cwd");
        let config_file = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);

        let planted_config = run_cwd.join(".firma/firma.toml");
        assert!(
            !planted_config.exists(),
            "precondition: nearer config candidate must be absent"
        );
        let shell = format!(
            "mkdir -p {candidate_dir} && printf '%s\\n' '# planted by sandbox' > {candidate}; \
             echo {ran}",
            candidate_dir = shell_quote(planted_config.parent().expect("candidate parent")),
            candidate = shell_quote(&planted_config),
            ran = MASK_TEST_RAN_MARKER,
        );

        let output = run_structural_shell(&world, None, &extra_env, &run_cwd, &shell);
        assert_mask_test_ran(backend, &output);
        assert!(
            !planted_config.exists(),
            "[{backend:?}] sandbox created a higher-precedence host config at {}",
            planted_config.display()
        );
    }
}

#[test]
fn directory_symlink_config_fails_closed_before_agent_launch() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(
                backend,
                "directory_symlink_config_fails_closed_before_agent_launch",
            );
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let external_config_dir = workspace.join("external-config");
        let config_file = scaffold_mask_test_config(&world, &external_config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);

        let lexical_firma = workspace.join(".firma");
        std::os::unix::fs::symlink(&external_config_dir, &lexical_firma)
            .expect("symlink workspace .firma to external config directory");

        let shell = format!(
            "rm {firma_dir} && mkdir {firma_dir} && printf '%s\\n' '# poisoned' > {config}; \
             echo {ran}",
            firma_dir = shell_quote(&lexical_firma),
            config = shell_quote(&workspace.join(".firma/firma.toml")),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output = run_structural_shell(&world, None, &extra_env, &workspace, &shell);
        assert!(
            !output.success(),
            "[{backend:?}] firma run unexpectedly allowed a symlinked .firma directory"
        );
        assert!(
            !output.stdout.contains(MASK_TEST_RAN_MARKER),
            "[{backend:?}] sandboxed command ran even though symlinked .firma should fail closed"
        );
        let metadata = std::fs::symlink_metadata(&lexical_firma).expect("inspect lexical .firma");
        assert!(
            metadata.file_type().is_symlink(),
            "[{backend:?}] sandbox replaced the host .firma symlink"
        );
        assert!(
            std::fs::read_to_string(&config_file)
                .expect("read canonical config")
                .contains(MASK_TEST_SENTINEL),
            "[{backend:?}] canonical config was unexpectedly modified"
        );
    }
}

#[test]
fn file_symlink_config_cannot_be_read_or_modified_via_target() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(
                backend,
                "file_symlink_config_cannot_be_read_or_modified_via_target",
            );
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let lexical_config = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&lexical_config, "generic", backend);
        let canonical_target = workspace.join("firma-target.toml");
        std::fs::rename(&lexical_config, &canonical_target).expect("move config to symlink target");
        std::os::unix::fs::symlink(&canonical_target, &lexical_config)
            .expect("symlink firma.toml to workspace target");
        let original = std::fs::read_to_string(&canonical_target).expect("read pristine target");

        let shell = format!(
            "cat {target} 2>/dev/null; printf '%s\\n' '# modified by sandbox' >> {target}; \
             echo {ran}",
            target = shell_quote(&canonical_target),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output = run_structural_shell(&world, None, &extra_env, &workspace, &shell);
        assert_mask_test_ran(backend, &output);
        assert!(
            !output.stdout.contains(MASK_TEST_SENTINEL),
            "[{backend:?}] sandbox read the selected config through the canonical file-symlink \
             target"
        );
        assert_eq!(
            std::fs::read_to_string(&canonical_target).expect("read target after run"),
            original,
            "[{backend:?}] sandbox modified the selected config through its canonical symlink \
             target"
        );
    }
}

#[test]
fn workspace_mount_alias_does_not_reexpose_firma_config() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(
                backend,
                "workspace_mount_alias_does_not_reexpose_firma_config",
            );
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let mount_alias = world.path("workspace-alias");
        std::fs::create_dir_all(&mount_alias).expect("mkdir mount alias target");
        let config_file = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);
        append_profile_mount(&config_file, &workspace, &mount_alias);

        let aliased_config = mount_alias.join(".firma/firma.toml");
        let shell = format!(
            "cat {config} 2>/dev/null; echo {ran}",
            config = shell_quote(&aliased_config),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output =
            run_structural_shell(&world, Some(&config_file), &extra_env, &workspace, &shell);
        assert_mask_test_ran(backend, &output);
        assert!(
            !output.stdout.contains(MASK_TEST_SENTINEL),
            "[{backend:?}] workspace mount exposed firma.toml through {}",
            aliased_config.display()
        );
    }
}

#[test]
fn mount_targeting_firma_dir_does_not_replace_mask() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(backend, "mount_targeting_firma_dir_does_not_replace_mask");
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let config_file = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);
        append_profile_mount(&config_file, &config_dir, &config_dir);

        let shell = format!(
            "cat {config} 2>/dev/null; echo {ran}",
            config = shell_quote(&config_file),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output =
            run_structural_shell(&world, Some(&config_file), &extra_env, &workspace, &shell);
        assert_mount_inside_mask_neutralized(backend, &output);
    }
}

#[test]
fn operator_mount_inside_firma_does_not_gain_post_seal_placement() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(
                backend,
                "operator_mount_inside_firma_does_not_gain_post_seal_placement",
            );
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let config_file = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);
        let source = world.path("operator-state");
        std::fs::create_dir_all(&source).expect("mkdir operator state source");
        std::fs::write(source.join("marker"), MASK_TEST_SENTINEL).expect("write operator marker");
        let target = config_dir.join("operator-state");
        append_profile_mount(&config_file, &source, &target);

        let shell = format!(
            "cat {marker} 2>/dev/null; echo {ran}",
            marker = shell_quote(&target.join("marker")),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output =
            run_structural_shell(&world, Some(&config_file), &extra_env, &workspace, &shell);
        assert_mount_inside_mask_neutralized(backend, &output);
    }
}

#[test]
fn firma_source_mount_alias_does_not_reexpose_config() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(backend, "firma_source_mount_alias_does_not_reexpose_config");
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let alias = world.path("firma-alias");
        let config_file = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);
        append_profile_mount(&config_file, &config_dir, &alias);

        let aliased_config = alias.join("firma.toml");
        let shell = format!(
            "cat {config} 2>/dev/null; echo {ran}",
            config = shell_quote(&aliased_config),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output =
            run_structural_shell(&world, Some(&config_file), &extra_env, &workspace, &shell);
        assert_mount_inside_mask_neutralized(backend, &output);
    }
}

#[test]
fn normalized_duplicate_mount_target_fails_closed() {
    for backend in BACKENDS {
        let Some(extra_env) = backend_env(backend) else {
            skip(backend, "normalized_duplicate_mount_target_fails_closed");
            continue;
        };

        let world = TestWorld::isolated();
        let workspace = world.workspace_path();
        let config_dir = workspace.join(".firma");
        let config_file = scaffold_mask_test_config(&world, &config_dir, &workspace);
        patch_backend(&config_file, "generic", backend);
        let parent_dir_target = config_dir.join("..");
        append_profile_mount(&config_file, &workspace, &parent_dir_target);

        let shell = format!(
            "cat {config} 2>/dev/null; echo {ran}",
            config = shell_quote(&config_file),
            ran = MASK_TEST_RAN_MARKER,
        );
        let output =
            run_structural_shell(&world, Some(&config_file), &extra_env, &workspace, &shell);
        assert!(
            !output.success(),
            "[{backend:?}] firma run accepted mount targets that normalize to the same \
             destination:\n{output}"
        );
        assert!(
            output.stderr.contains("duplicate mount targets")
                && output.stderr.contains("depend on mount order"),
            "[{backend:?}] firma run did not explain the ambiguous mount rejection:\n{output}"
        );
        assert!(
            !output.stdout.contains(MASK_TEST_RAN_MARKER),
            "[{backend:?}] sandboxed command ran despite ambiguous mount targets:\n{output}"
        );
    }
}

/// `Some(extra_env)` (possibly empty) when `backend` can run on this host; `None` when its
/// prerequisites are missing and the caller should skip this iteration, not fail the test.
fn backend_env(backend: Backend) -> Option<Vec<(&'static str, String)>> {
    match backend {
        Backend::Bwrap => Some(Vec::new()),
        Backend::Hakoniwa => {
            let runner = hakoniwa_runner_path()?;
            Some(vec![(
                "FIRMA_RUN_HAKONIWA_RUNNER",
                runner.to_string_lossy().into_owned(),
            )])
        }
    }
}

fn skip(backend: Backend, test_name: &str) {
    eprintln!(
        "skipping {test_name} for {backend:?}: firma-hakoniwa-runner was not built (run `cargo \
         build --workspace` or `cargo nextest run` without `-p` to build it)"
    );
}

/// Rewrites `profile`'s `backend = "..."` line in the already-scaffolded `config_file` to select
/// `backend`. A no-op for [`Backend::Bwrap`], since the scaffolder's own default already selects
/// it — only `hakoniwa` needs patching in.
fn patch_backend(config_file: &Path, profile: &str, backend: Backend) {
    if backend == Backend::Bwrap {
        return;
    }
    let original = std::fs::read_to_string(config_file).expect("read generated firma.toml");
    let anchor = format!("[run.profiles.{profile}]\nbackend = \"bwrap\"\n");
    assert!(
        original.contains(&anchor),
        "generated firma.toml did not contain the expected {profile} profile anchor:\n{original}"
    );
    let patched = original.replacen(
        &anchor,
        &format!(
            "[run.profiles.{profile}]\nbackend = \"{}\"\n",
            backend.config_value()
        ),
        1,
    );
    std::fs::write(config_file, patched).expect("write patched firma.toml");
}

fn scaffold_mask_test_config(world: &TestWorld, config_dir: &Path, workspace: &Path) -> PathBuf {
    world.scaffold_config(
        "generic",
        config_dir,
        &world.state_path(),
        Some(workspace),
        workspace,
    );
    let config_file = config_dir.join("firma.toml");
    TestWorld::disable_host_home_masks(&config_file);
    let generated = std::fs::read_to_string(&config_file).expect("read generated config");
    std::fs::write(
        &config_file,
        format!("{generated}\n# {MASK_TEST_SENTINEL}\n"),
    )
    .expect("plant config sentinel");
    config_file
}

fn append_profile_mount(config_file: &Path, source: &Path, target: &Path) {
    let mut config = std::fs::read_to_string(config_file).expect("read generated config");
    write!(
        config,
        "\n[[run.profiles.generic.mounts]]\n\
         source = \"{}\"\n\
         target = \"{}\"\n\
         read_only = false\n",
        source.display(),
        target.display(),
    )
    .expect("render adversarial profile mount");
    std::fs::write(config_file, config).expect("append adversarial profile mount");
}

fn run_structural_shell(
    world: &TestWorld,
    config_file: Option<&Path>,
    extra_env: &[(&'static str, String)],
    cwd: &Path,
    shell: &str,
) -> ProcessOutput {
    let extra_env: Vec<(&str, &str)> = extra_env
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    world.run_firma_with_env(
        "generic",
        config_file,
        cwd,
        &[],
        &extra_env,
        "sh",
        ["-c", shell],
    )
}

fn assert_mask_test_ran(backend: Backend, output: &ProcessOutput) {
    assert!(
        output.success(),
        "[{backend:?}] firma run failed:\n{output}"
    );
    assert!(
        output.stdout.contains(MASK_TEST_RAN_MARKER),
        "[{backend:?}] sandboxed command did not run:\n{output}"
    );
}

/// Asserts an operator mount whose target lands inside a masked zone is neutralized — however
/// each backend's own, independently-implemented masking enforces it.
///
/// `bwrap`'s phase-sequenced mount ordering (masks applied after ordinary/operator mounts) makes
/// the mount a no-op: `firma run` launches successfully and the mask still wins, so the sentinel
/// never leaks. `hakoniwa` instead rejects the mount outright, before the sandboxed command ever
/// runs at all — `reject_overlay_targets_inside_masked_zones`, a stricter safeguard with no bwrap
/// equivalent, added specifically because Hakoniwa's plain target-path mount ordering does not
/// reproduce bwrap's "every mask wins over every overlay unconditionally" guarantee on its own
/// (see `hakoniwa-backend-plan.md`'s Slice 2 notes). Both close the same leak; this is a real,
/// intentional divergence in *how*, not a bwrap-parity gap to close.
fn assert_mount_inside_mask_neutralized(backend: Backend, output: &ProcessOutput) {
    match backend {
        Backend::Bwrap => {
            assert_mask_test_ran(backend, output);
            assert!(
                !output.stdout.contains(MASK_TEST_SENTINEL),
                "[{backend:?}] mount targeting a masked zone re-exposed the selected config:\n{output}"
            );
        }
        Backend::Hakoniwa => {
            assert!(
                !output.success(),
                "[{backend:?}] firma run unexpectedly allowed a mount targeting a masked \
                 zone:\n{output}"
            );
            assert!(
                output.stderr.contains("is inside protected path")
                    && output.stderr.contains("reopen a security mask"),
                "[{backend:?}] firma run did not explain the masked-zone mount rejection:\n{output}"
            );
            assert!(
                !output.stdout.contains(MASK_TEST_RAN_MARKER),
                "[{backend:?}] sandboxed command ran despite a rejected masked-zone mount:\n{output}"
            );
        }
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}
