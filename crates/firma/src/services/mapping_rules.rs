//! `firma mapping-rules` implementation.
//!
//! Offline, pre-deploy checking of a resolved mapping-rules configuration —
//! see `docs/architecture/mapping-rules-prover-plan.md`. Resolves `firma.toml`
//! and loads mapping rules exactly like Sidecar startup does (reusing
//! `firma_sidecar::startup::pipeline::load_mapping_rules` directly, `DEC-007`),
//! so a clean report here means Sidecar startup will also succeed against the
//! same configuration.

use std::path::Path;
use std::process::ExitCode;

use crate::args::mapping_rules::{MappingRulesArgs, MappingRulesCommand};

pub fn run(args: &MappingRulesArgs, config: Option<&Path>) -> anyhow::Result<ExitCode> {
    match args.command {
        MappingRulesCommand::Validate => run_validate(config),
    }
}

fn run_validate(config: Option<&Path>) -> anyhow::Result<ExitCode> {
    let Some(resolved) = firma_config_loader::ConfigResolver::default().resolve_config(config)?
    else {
        crate::output::err("could not resolve firma.toml: no config found");
        return Ok(ExitCode::from(1));
    };

    // Uses the crate's typed entry point (`.section`), the same one real
    // Sidecar startup uses (`services::sidecar::read_config`) — not
    // `raw_section` + a second, independent `toml::from_str` pass, which
    // would be a second TOML-deserialization path production doesn't take.
    let schema = resolved
        .config
        .section::<firma_config_schema::sidecar::SidecarConfig>("sidecar")?;
    let mut sidecar_config = firma_sidecar::config::SidecarConfig::try_from(schema)?;
    // Relative resource paths (including `[enforcement.mapping]`'s
    // `rules_path`/`rules_paths`) are relative to the config file's own
    // directory, not the process cwd — mirrors `services::sidecar::run`'s
    // identical rebase before Sidecar startup loads the same config.
    sidecar_config.rebase_defaults(&resolved.config_dir());

    let rules = firma_sidecar::startup::pipeline::load_mapping_rules(&sidecar_config)?;
    let registry = firma_sidecar::pipeline::ActionClassRegistry::v0_1();

    // INV-001: fail-closed shadowing check. Reuses `MappingTable::from_config`
    // verbatim — the exact function Sidecar startup calls — so a rule this
    // rejects would also reject Sidecar startup, not just this CLI report.
    let mut has_error = false;
    match firma_sidecar::pipeline::MappingTable::from_config(
        &rules,
        &registry,
        sidecar_config.mapping_default_protected(),
    ) {
        Ok(_) => crate::output::ok("no unreachable mapping rules found"),
        Err(error) => {
            has_error = true;
            crate::output::err(format!("{error}"));
        }
    }

    // INV-002: advisory registry-reachability check.
    let catalogs = firma_sidecar::composio::ComposioCatalogs::builtin()?;
    let orphaned = firma_sidecar::pipeline::find_orphaned_action_classes(
        &rules,
        catalogs.action_classes(),
        &registry,
    );
    if orphaned.is_empty() {
        crate::output::ok("every registry action class is producible");
    } else {
        for finding in &orphaned {
            crate::output::warn(format!(
                "registry class '{}' is not producible by any mapping rule or Composio catalog \
                 entry — may be governed solely by firma-run's local execution config, which \
                 this check cannot see",
                finding.class
            ));
        }
    }

    Ok(if has_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
