//! Args for `firma mapping-rules`.

use clap::{Args, Subcommand};

/// Arguments for `firma mapping-rules`.
#[derive(Debug, Args)]
pub struct MappingRulesArgs {
    #[command(subcommand)]
    pub command: MappingRulesCommand,
}

/// Subcommands of `firma mapping-rules`.
#[derive(Debug, Subcommand)]
pub enum MappingRulesCommand {
    /// Check the resolved mapping-rules configuration offline, before
    /// deploying it. Reports two things: unreachable rules (a rule fully
    /// shadowed by higher-priority rules sharing its exact host/path — a
    /// hard error, the same fail-closed contract Sidecar startup already
    /// enforces) and registry action classes no mapping rule or Composio
    /// catalog entry ever produces (a warning; may include classes governed
    /// solely by `firma-run`'s local execution config, which this check
    /// cannot see — see `docs/architecture/mapping-rules-prover-plan.md`).
    /// Uses the same `firma.toml` resolution as every other config-consuming
    /// subcommand (`-c`/`--config`/`FIRMA_CONFIG`).
    Validate,
}
