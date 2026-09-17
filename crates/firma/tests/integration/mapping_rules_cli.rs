//! Black-box CLI tests for `firma mapping-rules validate`.
//!
//! Scaffolds a real config via `firma config --yes` into an isolated tempdir
//! (both the config and state directories), then runs
//! `firma mapping-rules validate` against it — full black-box, both
//! invocations go through the compiled `firma` binary, never internal APIs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code: panics are acceptable test failures"
)]

use std::path::Path;
use std::process::Command;

/// Scaffold a config (`--posture dev`, given `--mapping` templates) into an
/// isolated tempdir and return the path to its `firma.toml`.
fn scaffold(dir: &Path, mappings: &[&str]) -> std::path::PathBuf {
    let config_dir = dir.join(".firma");
    let state_dir = dir.join("state");
    let mut args = vec!["config", "--yes", "--posture", "dev", "--output-dir"];
    let config_dir_str = config_dir.to_string_lossy().to_string();
    let state_dir_str = state_dir.to_string_lossy().to_string();
    args.push(&config_dir_str);
    args.push("--state-dir");
    args.push(&state_dir_str);
    for mapping in mappings {
        args.push("--mapping");
        args.push(mapping);
    }

    let out = Command::new(env!("CARGO_BIN_EXE_firma"))
        .args(&args)
        .output()
        .expect("spawn firma config");
    assert!(
        out.status.success(),
        "scaffolding failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    config_dir.join("firma.toml")
}

#[test]
fn validate_clean_config_exits_zero_and_reports_ok() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let firma_toml = scaffold(tmp.path(), &["github"]);

    let out = Command::new(env!("CARGO_BIN_EXE_firma"))
        .args(["mapping-rules", "validate", "--config"])
        .arg(&firma_toml)
        .output()
        .expect("spawn firma mapping-rules validate");

    assert!(
        out.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no unreachable mapping rules found"),
        "stdout missing INV-001 OK line: {stdout}"
    );
}

#[test]
fn validate_reports_orphaned_classes_as_warnings_not_failures() {
    // A single narrow mapping template (github) leaves plenty of registry
    // classes (e.g. every Stripe payment.* class) with no producing rule —
    // INV-002 must report these as warnings on stderr, and they must not
    // affect the exit code (INV-001 has no shadowed rules in this config).
    let tmp = tempfile::tempdir().expect("tempdir");
    let firma_toml = scaffold(tmp.path(), &["github"]);

    let out = Command::new(env!("CARGO_BIN_EXE_firma"))
        .args(["mapping-rules", "validate", "--config"])
        .arg(&firma_toml)
        .output()
        .expect("spawn firma mapping-rules validate");

    assert!(
        out.status.success(),
        "warnings must not fail the exit code, got {:?}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("payment.transfer"),
        "expected an orphaned-class warning for a Stripe-only class not \
         covered by the github mapping template; got stderr:\n{stderr}"
    );
}
