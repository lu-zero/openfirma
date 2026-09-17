//! Today's behavior: only the root command is checked against
//! `allowed_executables` (via `mediator::enforce_local_command_governance`,
//! called before this strategy is ever consulted). A descendant process the
//! root spawns is not independently governed — this strategy is a pure
//! passthrough to the existing, unmodified launch/wait path.

use std::path::Path;
use std::process::Child;

use super::{AllowedExecutables, ExecutionGovernor, GovernanceHandle};
use crate::backend::BackendKind;
use crate::error::RunError;
use crate::supervisor::wait_with_signal_forwarding;

pub struct InheritedGovernor;

impl ExecutionGovernor for InheritedGovernor {
    fn rewrite_launch(
        &self,
        _allowed: &AllowedExecutables,
        _sandbox_runtime_dir: &Path,
        _executable: &mut String,
        _args: &mut Vec<String>,
    ) -> Result<GovernanceHandle, RunError> {
        Ok(GovernanceHandle::None)
    }

    fn supervise(
        &self,
        _handle: GovernanceHandle,
        child: Child,
        backend: BackendKind,
    ) -> Result<i32, RunError> {
        wait_with_signal_forwarding(child, backend)
    }
}
