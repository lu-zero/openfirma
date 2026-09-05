# Selectable execution-governance strategy for `firma-run`

## Artifact metadata

- Status: Accepted (independent plan review complete, all findings corrected — see "Plan-review findings and dispositions")
- Durable locator: `docs/architecture/selectable-execution-governance-plan.md` (this file, in-repo)
- Repository revision researched: `9d761b2b36afa69c32eb1a5cc66e8b9ba45dc34a`
- Task or requirement source: `~/Sources/openfirma-notes/requirements.md` (Workstream 2), user request to make governance mechanisms selectable at runtime from one binary so Workstream 2 candidates can be benchmarked side by side
- Supersedes: Not applicable (new plan; updates but does not replace `docs/adr/FIR-60-sandbox-backend-selection-for-firma-run.md`, which owns backend selection, a separate axis — see `DEC-001`)

## Goal and acceptance outcomes

- Goal: make the mechanism that decides "may this exec happen" independently selectable at runtime from a single `firma` binary, alongside today's namespace/seccomp baseline, so Workstream 2 can gather comparable evidence (correctness under the required test matrix, measured overhead) across candidates before recommending one.
- Observable acceptance outcomes:
  - A profile can select `execution_governance = "inherited" | "landlock_execute" | "ptrace_seccomp_exec"` in `firma.toml`, with `"inherited"` as the default (today's behavior, zero change for existing users).
  - Under `"landlock_execute"` and `"ptrace_seccomp_exec"`, the existing FIR-366 regression test (`tests/e2e/scenarios/child_process_governance/execution.rs`) passes — a denied tool cannot run as a child of an allowed root.
  - Under `"inherited"`, that same test continues to fail, serving as the experiment's control.
  - The three existing passing tests in the same module (`network.rs`, `filesystem.rs`, `http.rs`) pass unchanged regardless of `execution_governance`, since they exercise properties bwrap itself owns (see `DEC-001`).
  - A benchmark harness reports per-mechanism exec-latency overhead, giving Workstream 2's finding real measured numbers instead of estimates.

## Scope

- In scope: a new selectable execution-governance axis in `firma-run`; three strategies (`Inherited` as control, `LandlockExecute`, `PtraceSeccompExec`); config/CLI plumbing; e2e coverage parametrized across strategies; a benchmark harness.
- Out of scope:
  - Replacing or duplicating bwrap's namespace/mount/network confinement — that stays single-owned by bwrap in every strategy (`DEC-001`).
  - Derek's cgroup v2 + namespace + lineage design — undocumented outside Slack; tracked separately in `openfirma-notes/todo/pending.md`, not part of this plan's mechanism set.
  - Re-litigating seccomp-unotify as a primary mechanism — already excluded per FIR-111 with evidence in hand (`openfirma-notes/notes/`); not reopened here.
  - Full syscall-table ptrace mediation (gVisor-Sentry-style) — rejected in favor of a `SECCOMP_RET_TRACE`-scoped design; see `DEC-005` and Appendix "Alternatives".
  - macOS (`vz`) and Windows (`wsl2`) backends — this plan is Linux/bwrap-only, consistent with `requirements.md` §7 ("Out of scope: macOS and Windows").
  - Closing FIR-442/FIR-444 — tracked separately under Workstream 1 in `openfirma-notes/todo/pending.md`; unrelated to this axis.
- Assumptions:
  - `mediator.allowed_executables` (resolved once at root launch, `crates/firma-run/src/runtime/mod.rs`) is an adequate, already-existing enforcement source to extend to descendants for all three strategies — no new policy-resolution mechanism is introduced (`DEC-002`).
  - The host's `firma-run` process (which already spawns `bwrap` and blocks on it for the sandbox's whole lifetime, per Appendix "Semantic call traces" `TRACE-003`) remains the ptrace attacher for `PtraceSeccompExec` — no separate daemon process. This is required for `ptrace_scope=1` compatibility without `CAP_SYS_PTRACE` (see `INV-002`).
  - RHEL/CentOS-family Yama support is presently **Unknown** (prior research could not confirm whether Yama ships there at all) — `PtraceSeccompExec` must fail closed with an actionable error, not silently degrade, when ptrace attach fails for any reason, on any distro.
- Open decisions: exact `firma.toml` field name (`execution_governance` used throughout this plan as a placeholder — confirm against existing naming conventions during Slice 1 review); exact new-subcommand names (`firma __landlock-guarded-run`, `firma __exec-guarded-run` used as placeholders, mirroring `firma __egress-guarded-run`).
- Cohesion and split assessment: kept as one plan rather than three per-mechanism plans because all three strategies share one invariant owner (`INV-001`), one config/dispatch seam (Slice 1), and one proof obligation (the parametrized e2e suite in Slice 4) — splitting would obscure that shared contract. Slices 2 and 3 (Landlock, ptrace) are however independently implementable and shippable once Slice 1 lands; either could be dropped without invalidating the other (see per-slice "Dependencies").
- Deferred child plans: Not applicable.

## Routing

- Mode: Full
- Trigger evidence: this change touches (1) a security/trust boundary — the execution-governance decision itself (multiple Full triggers apply independently, so no single one is load-bearing): (2) an externally observable, documented config/CLI surface (`--backend`/`backend =` and its new sibling are documented in `docs-site/src/content/docs/concepts/sandbox.md`, which CLAUDE.md's API-stability section treats as a stable boundary regardless of the crate's `publish = false`); (3) the owner of the "execution policy propagates to all descendants" invariant, currently undocumented as owned by anything (see `INV-001`); (5) multiple crates (`firma-run`, `firma-config-schema`, `firma`, `tests/e2e`) with genuine design uncertainty (RHEL/Yama support, exact ptrace sequencing); (6) multiple viable designs with materially different tradeoffs (the whole point of the exercise).
- Higher-mode triggers checked: no additional triggers beyond Full apply (no distributed/cross-service ordering concern beyond what's already covered by trigger 4).
- Downgrade evidence and reason: Not applicable — Full is required and multiple independent triggers apply, so no downgrade is considered.

## Current behavior and problem

- Owners and entry points: `crates/firma-run/src/runtime/mod.rs::execute_run` (mod.rs:98) is the closest thing to an orchestrator, but delegates: `resolve_governed_executable` (mod.rs:513-534) canonicalizes and checks `mediator.allowed_executables`; `enforce_local_command_governance` (`crates/firma-run/src/mediator.rs:64-129`) round-trips the Sidecar's `local.exec` protocol. Both run exactly once, at `runtime/mod.rs:311-314`, gated entirely on `if let Some(mediator) = &profile.sidecar_local_exec` (i.e. **governance is opt-in** — absent config, no check runs even for the root). `backend.start_agent(...)` (mod.rs:336) is invoked strictly after, with no hook anywhere that re-enters governance for anything the root spawns.
- Current success and failure outcomes: for the root process, an allowed executable proceeds; a denied one fails closed (`RunError`) before `start_agent` is ever called. For any descendant the root spawns, there is no decision point at all — it inherits bwrap's namespace/mount/network confinement (which does propagate, see `TRACE-001`) but not the executable allow-list.
- Evidence: `tests/e2e/scenarios/child_process_governance/execution.rs` (`child_process_escapes_run_governance`, `#[ignore]`d, FIR-366) is a committed, currently-failing regression proving exactly this gap; `docs/architecture/linux-local-command-enforcement.md`'s "Non-Cooperative Anti-Bypass Guarantees" section (lines 177-194) is the current canonical doc stating the limitation ("Arbitrary child processes spawned after initial launch are constrained primarily by sandbox/seccomp/namespace boundaries... Sidecar governance is not itself a full containment boundary for non-cooperative process trees").

## Key decisions and tradeoffs

### `DEC-001`: The selectable axis is an execution-governance _strategy_ layered onto bwrap, not a new `BackendKind`

- Choice: introduce a new, orthogonal `ExecutionGovernanceStrategy` axis (own module, own config field) rather than adding `Landlock`/`PtraceSeccompExec` as `BackendKind` variants or wholesale alternate backends.
- Rationale and evidence: `tests/e2e/scenarios/child_process_governance/{network,filesystem,http}.rs` all pass today by relying on properties bwrap itself provides (inherited seccomp filter surviving `execve` for network; mount-namespace masking for filesystem; env-var-inherited `HTTP_PROXY` routing for L7) — none of that needs reimplementing per mechanism. Only `execution.rs` exercises the property this plan changes. `secret_providers` vs. `http_secret_providers` (`crates/firma-config-schema/src/secret_provider/`) is existing precedent in this codebase for two independently-configured mechanisms toward one goal living as a parallel axis rather than a backend variant.
- **Insertion point, corrected after plan review (`PLAN-002`)**: `SandboxBackend::start_agent` takes `launch: &LaunchSpec` **immutably** (`crates/firma-run/src/backend/mod.rs:386-391`), and by `linux_bwrap/mod.rs:281` the executable/args have already been read into the bwrap `Command` three lines earlier — there is no mutable `LaunchSpec` available at that call site, and changing `SandboxBackend`'s shared signature would ripple into `firecracker.rs`/`windows_wsl2.rs`/`macos_vz.rs`, contradicting this plan's own Linux/bwrap-only scope. The governor's launch-rewriting phase therefore runs in `runtime::execute_run`, **before** `LaunchSpec` is constructed (before `runtime/mod.rs:321`) — the same place `resolve_governed_executable`/`enforce_local_command_governance` already run today — rewriting the executable/args that go into the `LaunchSpec` builder so it points at the appropriate `firma __*-guarded-run` shim with the real target passed through. `SandboxBackend::start_agent` and `linux_bwrap/mod.rs` receive an already-rewritten `LaunchSpec` and need **no changes** for this phase. The ptrace mechanism's host-side supervision phase is a separate concern — see `DEC-007`.
- Consequences and rejected alternatives: rejected "new `BackendKind::Landlock`/`BackendKind::PtraceSandbox` variants" — would require duplicating namespace/mount/network plumbing bwrap already owns, tripling the surface area for no benefit, and would force `network.rs`/`filesystem.rs`/`http.rs` to grow real per-backend variants they don't need today. Rejected "dispatch inside `BwrapBackend::start_agent`" (the plan's own original position) once plan review showed it doesn't type-check against the actual trait signature.

### `DEC-002`: Reuse `mediator.allowed_executables` as the single enforcement source for all strategies and both root and descendants

- Choice: the executable allow-list resolved once at root launch (`runtime/mod.rs`, feeding today's root-only `resolve_governed_executable` check) is the same input `LandlockExecute` compiles into a ruleset and `PtraceSeccompExec` consults per trapped `execve`. No strategy re-invokes `enforce_local_command_governance`'s Sidecar round-trip (including its async HITL/approval-token flow) per descendant exec.
- Rationale and evidence: re-running the full Sidecar round-trip per descendant exec would (a) reintroduce per-exec latency proportional to a network-adjacent round trip for potentially many execs in one run, and (b) reopen the hot-swap/determinism hazard already documented in `openfirma-notes/notes/seccomp-profile-and-hotswap.md` — a policy bundle swap mid-run could change the decision for a descendant that started under a different bundle version than the root did. Pinning the enforcement source to what was already resolved at root-launch time avoids both, at the cost of not re-checking HITL/approval state per descendant (an intentional, stated limitation — see Slice 2/3 "Intentionally unsupported").
- Consequences and rejected alternatives: rejected "re-run `enforce_local_command_governance` per descendant exec" for the reasons above. Accepted cost: a HITL approval granted/denied for the root's launch is not re-solicited per descendant; if that granularity is later wanted, it is a separate, follow-on design (out of scope here).
- **Precondition, added after plan review (`PLAN-001`)**: this reuse is only sound when `allowed_executables` is actually populated and authoritative. Today, `sidecar_local_exec` defaults to `None` (`config.rs:773`), and even when configured, `enforce_known_executables` defaults to `false` (`config.rs:911`) — `allowed_executables` is only required non-empty when that flag is `true` (`config.rs:197-202`); otherwise root-level governance is delegated entirely to the dynamic Sidecar round-trip, which is not reducible to a static path set. Selecting `LandlockExecute`/`PtraceSeccompExec` while `enforce_known_executables` is `false` (or `sidecar_local_exec` is unset) must therefore be rejected at config-resolution time (see `DEC-003`'s compatibility gate, Slice 1) rather than silently enforcing an empty set — an empty allow-list is ambiguous between "restrict everything" (breaks the sandbox, including the root's own re-exec through the shim) and "restrict nothing" (a false sense of containment, defeating the point of this plan).

### `DEC-003`: Land as a documented, permanent config field with new values marked experimental — not a hidden env var

- Choice: add `execution_governance` to the profile schema (`crates/firma-config-schema`) and document it in `docs-site/src/content/docs/concepts/sandbox.md` from Slice 1, defaulting to `"inherited"`. Document `"landlock_execute"` and `"ptrace_seccomp_exec"` explicitly as experimental, mirroring how `FIRMA_RUN_VZ_STRUCTURAL_NETWORK`/`FIRMA_RUN_VZ_GUEST` are documented as experimental today (`docs-site/src/content/docs/concepts/macos-structural-strategy.md`, ADR FIR-60 lines 168-170).
- Rationale and evidence: the user's own stated intent for this refactor is a permanent, pluggable feature, not a throwaway harness — but the mechanisms themselves are genuinely unproven (that's the point of Workstream 2). The VZ-structural-network precedent shows this repo already has a pattern for "real, documented, but explicitly experimental" capability, which satisfies both constraints simultaneously.
- Consequences and rejected alternatives: rejected "hidden env-var-only gate" (satisfies "permanent" poorly — no docs, no `firma doctor` visibility, contradicts the user's stated intent). Rejected "fully stable, unmarked field from day one" (overstates confidence in mechanisms not yet benchmarked; this repo's own alpha-posture precedent — `#614`/`#619` — shows breaking changes happen without deprecation windows here, so nothing is lost by being honest about experimental status now and tightening later).

### `DEC-004`: Both new strategies install via a `firma __*-guarded-run` self-apply-then-exec subcommand, not a `pre_exec` closure

- Choice: `LandlockExecute` and `PtraceSeccompExec` each ship a new hidden CLI subcommand (placeholders: `firma __landlock-guarded-run`, `firma __exec-guarded-run`) that applies its restriction to itself, then `execve`s into the real wrapped command — mirroring `egress_guard.rs`'s existing `install_and_exec` (`egress_guard.rs:465-497`).
- Rationale and evidence: the alternative — a `std::os::unix::process::CommandExt::pre_exec` closure run in the child before `bwrap`/the agent execs — is an `unsafe fn` at its call site, which this workspace's lints treat as a hard CI failure (`unsafe_code` promoted to `-D warnings` via `just lint`). `egress_guard.rs` already carries a narrowly-scoped, reasoned `#![expect(unsafe_code, ...)]` for its own raw-syscall needs (egress_guard.rs:52-56) rather than using `pre_exec`; the self-apply-then-exec subcommand pattern is the established precedent for adding a new restriction at exec time in this codebase.
- Consequences and rejected alternatives: rejected `pre_exec`-in-parent (fights the lint, no existing precedent). Consequence: each new subcommand needs its own narrowly-scoped `unsafe_code` justification for the specific raw syscalls it needs (`landlock_restrict_self`; for the ptrace shim, seccomp-filter installation and the readiness handshake described in `DEC-008`, not a `PTRACE_TRACEME` call — see `DEC-008`), matching `egress_guard.rs`'s existing model.

### `DEC-005`: `PtraceSeccompExec` performs a clean in-place allow/deny gate — no "twin execution" or result relay

- Choice: on a trapped `execve`/`execveat` (via `SECCOMP_RET_TRACE` + `PTRACE_O_TRACESECCOMP`, not full-syscall-table mediation), the attacher either `PTRACE_CONT`s the syscall unmodified (allow) or neutralizes it in place (deny — e.g. rewriting the syscall number so it fails, the same observable failure shape as today's static seccomp deny). No daemon-performs-the-real-exec-elsewhere-and-relays-a-substituted-result design.
- Rationale and evidence: `execve` replaces the calling process's image on success — there is no syscall return value to substitute afterward for a syscall that succeeds by destroying the caller's own image, and seccomp-notify's `CONTINUE` only means "re-run the real syscall in the same task," not "redirect and report back" (research finding, confirmed against `egress_guard.rs`'s own connect-notify design where this distinction doesn't arise because `connect()` has an ordinary return value). gVisor's actual approach to something like this requires reimplementing the syscall's OS-level behavior inside the tracer (a full userspace-kernel emulation layer) — categorically out of scope for a governance gate that only needs to answer allow/deny. A plain gate achieves the FIR-366 requirement (a denied tool cannot run as a child of an allowed root) without that complexity.
- Consequences and rejected alternatives: rejected "daemon runs the real exec in a twin, relays stdout/exit status back" (the originally proposed shape) once analysis showed it doesn't need to exist for a governance decision, and that seccomp-notify structurally cannot do it for `execve` regardless. This also keeps the mechanism's overhead profile close to the original ~3x-virtio-comparable estimate (see `DEC-006` and Appendix "Alternatives" for why full-table mediation was rejected on overhead grounds specifically).

### `DEC-006`: Trap scope is `execve`/`execveat` only, via `SECCOMP_RET_TRACE`, not full syscall-table mediation

- Choice: the agent's seccomp filter returns `SECCOMP_RET_ALLOW` for the overwhelming majority of syscalls (zero ptrace overhead) and `SECCOMP_RET_TRACE` only for `execve`/`execveat`, so the ptracer is invoked only for the syscalls governance actually cares about.
- Rationale and evidence: gVisor's own documentation calls its full-table ptrace platform "the highest structural costs by far" among its platforms, and academic benchmarking (arXiv 2406.07429) measures raw per-syscall ptrace cost at 10-70x heavier than lighter mechanisms, with real-workload overhead from ~40% up past 300% for syscall/signal-heavy patterns — none of which is close to a ~3x-virtio target. None of that literature covers the scoped case; trapping only `execve`/`execveat` (a low-frequency syscall relative to read/write/mmap) is a categorically lighter workload, consistent with the original overhead estimate.
- Consequences and rejected alternatives: rejected "trap every syscall" (the originally-described "broad syscall set full mediation" framing) specifically on overhead grounds — see Appendix "Alternatives" for the full comparison.

### `DEC-007`: The ptrace strategy's supervision phase owns the wait loop outright; it does not run alongside the existing reaper

- Choice: for `PtraceSeccompExec`, `ExecutionGovernor::supervise` (see Appendix "Types and signatures") takes full responsibility for waiting on the root process — including the signal-forwarding and terminal-exit reaping `supervisor::wait_with_signal_forwarding` performs today — rather than that function and a new, independent ptrace-wait loop both calling `waitpid` on the same pid from the same process.
- Rationale and evidence: added after plan review (`PLAN-003`). `wait_with_signal_forwarding`'s own doc comment (`supervisor.rs:11-24`) states reaping happens via "a plain blocking `Child::wait` on this thread — no `SIGCHLD` handling," and calls `child.wait()` directly (`supervisor.rs:66-69`). Linux ptrace-stop notification delivery is tied to the specific tracing thread; a second, independent plain-`wait()` loop on the same pid in the same process is a known source of missed or racing wait-status reaping. `Inherited` and `LandlockExecute` need no host-side supervision at all and delegate straight through to today's unchanged function.
- Consequences and rejected alternatives: rejected "run the ptrace-wait loop alongside `wait_with_signal_forwarding` unchanged" (the plan's original, unstated assumption) — both threads/loops calling `waitpid` on the same pid is a race, not a design. Consequence: `supervisor.rs` must be touched by Slice 3 (added to the file-tree diff) to either become strategy-aware or be fully superseded by the ptrace governor's own wait loop for this one strategy; the existing signal-forwarding behavior (SIGINT/SIGTERM/SIGWINCH relay, `--new-session`/pgid handling) must be preserved inside that unified loop, not dropped. **Mechanism, confirmed after further investigation**: a raw-`waitpid`-based loop (via `nix::sys::wait`, not `std::process::Child::wait`) observes the tracee's terminal exit (`WIFEXITED`/`WIFSIGNALED`) as just another loop outcome, alongside `PTRACE_EVENT_SECCOMP` stops — it fully subsumes `child.wait()`'s job rather than needing to run beside it. `exit_code()` (`supervisor.rs:82-89`) is currently written against `std::process::ExitStatus` specifically; generalizing it to a plain `(exit_code: Option<i32>, term_signal: Option<i32>)` shape lets both the existing path and this new loop share the same `128+signum` mapping (see Slice 0).

### `DEC-008`: The ptrace strategy attaches via `PTRACE_SEIZE` from the firma-run host process, not `PTRACE_TRACEME` from inside the shim

- Choice: `firma-run`'s host process issues `PTRACE_SEIZE` (with `PTRACE_O_TRACESECCOMP | PTRACE_O_TRACEEXEC | PTRACE_O_TRACEFORK | PTRACE_O_TRACECLONE | PTRACE_O_TRACEVFORK` set atomically) on the discovered inner-sandbox PID, from outside, after `backend.start_agent` returns. The `firma __exec-guarded-run` shim installs its `SECCOMP_RET_TRACE`-on-`execve` filter and then blocks on a readiness handshake (e.g. reading one byte from a pipe/socket the host writes to only after `PTRACE_SEIZE` succeeds) before `execve`-ing into the real command — it does **not** call `PTRACE_TRACEME`.
- Rationale and evidence: found while stress-testing this mechanism against the actual current code, not during the earlier independent review. `PTRACE_TRACEME` makes the _calling process's direct parent_ the tracer. `crates/firma-run/src/supervisor.rs`'s existing `sandbox_child_pid`/`parse_first_pid` helpers (`supervisor.rs:184-205`) exist specifically because **bwrap forks an inner child that becomes the actual sandboxed process** — the direct parent of that inner process is bwrap, not the firma-run host process. A `PTRACE_TRACEME` call from inside `firma __exec-guarded-run` would therefore make **bwrap** the tracer, not firma-run, silently defeating `INV-002`: the Yama research (`TRACE-003`) specifically validated `PTRACE_ATTACH`/`PTRACE_SEIZE` from a real ancestor at any depth (firma-run qualifies as a grandparent through bwrap), not `PTRACE_TRACEME`'s direct-parent-only semantics.
- Consequences and rejected alternatives: rejected `PTRACE_TRACEME` (wrong tracer, would silently break `INV-002` without any error signal — the attach would simply succeed against the wrong process, which is worse than a loud failure). Rejected relying on `sandbox_child_pid`'s existing best-effort `/proc` polling alone to time the attach — that function's own doc comment already accepts a race as "harmless but silently dropped" for a forwarded `SIGWINCH`; a missed or mistimed ptrace attach means zero enforcement for that run, not a cosmetic gap, so it needs the explicit readiness handshake instead of polling. Reusing `sandbox_child_pid`/`parse_first_pid` themselves (made `pub`, effectively crate-scoped since the owning module is `pub(crate)` — see Slice 0) to _locate_ the PID once the handshake confirms it exists is still appropriate — only the _timing_ source changes, not the PID-discovery mechanism.

## Architecture and invariant ownership

- Architecture shape: a new `crates/firma-run/src/execution_governance/` module defines `ExecutionGovernanceStrategy` (an enum: `Inherited | LandlockExecute | PtraceSeccompExec`) and an `ExecutionGovernor` trait with two dispatch points, both in `runtime::execute_run`, not inside any `SandboxBackend` implementation (`DEC-001`): `rewrite_launch`, called before `LaunchSpec` is constructed (before `runtime/mod.rs:321`), and `supervise`, called immediately after `backend.start_agent` returns its `Child`. `Inherited` is a no-op for both (today's behavior — nothing added, `SandboxBackend`/`linux_bwrap/mod.rs` untouched). `LandlockExecute` rewrites the launch to point at its `firma __landlock-guarded-run` shim (`DEC-004`) and delegates `supervise` straight through to today's unchanged `wait_with_signal_forwarding`. `PtraceSeccompExec` does the same rewrite for its own shim and, per `DEC-007`, its `supervise` takes over the wait loop entirely — unifying ptrace-stop servicing with the signal-forwarding/reaping duties `crates/firma-run/src/supervisor.rs` performs today, rather than running a second independent wait loop alongside it.

### `INV-001`: Execution policy applies to the initial process and every process it launches, directly or indirectly

- Semantic predicate: for a given `ExecutionGovernanceStrategy`, if executable `E` is denied for the root process, no process anywhere in the root's descendant tree may successfully `execve(E)`.
- Primary owner: the selected `ExecutionGovernor` implementation (not `runtime/mod.rs`, which only owns the root-launch decision today). `Inherited` explicitly does **not** satisfy this invariant — that is its documented, intentional gap, matching current behavior and the existing "Current limits" doc.
- Detailed proof: see Appendix "Detailed proof obligations", `PROOF-001`/`PROOF-002`.

### `INV-002`: `PtraceSeccompExec` must never require elevated privilege beyond what `firma-run` already has

- Semantic predicate: the ptrace attach in `PtraceSeccompExec` must succeed under Yama `ptrace_scope=1` (Ubuntu/Debian default) without `CAP_SYS_PTRACE`.
- Primary owner: the `PtraceSeccompExec` `ExecutionGovernor` implementation, specifically by remaining the real-process-tree ancestor of the traced agent (never delegating the attach to a separate, unrelated daemon process).
- Detailed proof: see Appendix "Semantic call traces" `TRACE-003` and "Detailed proof obligations" `PROOF-003`. **Gap**: RHEL/CentOS-family Yama support is Unknown; `PROOF-003`'s failure path must be exercised there specifically before this strategy is documented as supported on that distro family (see `compatibility.md`'s existing RHEL-Landlock-ABI caveat for the analogous kind of "don't assume from `uname -r`" gotcha).

- Compatibility, migration, and failure semantics: `execution_governance` defaults to `"inherited"` — no migration needed for existing configs. Selecting `"landlock_execute"` or `"ptrace_seccomp_exec"` must fail closed at config-resolution time, mirroring `backend_supported_on_host`'s existing pattern (`crates/firma-run/src/config.rs:725-733`), in at least two independent cases — never silently falling back to `Inherited`'s weaker guarantee in either: (1) host/backend incompatibility (kernel <5.13 for Landlock; ptrace attach failure for any reason for `PtraceSeccompExec`; either strategy on non-Linux `BackendKind`s); (2) **added after plan review (`PLAN-001`)** — `sidecar_local_exec` unset, or set with `enforce_known_executables != true`, or an empty `allowed_executables` (see `DEC-002`'s precondition) — since neither new strategy has anything authoritative to enforce in that configuration.
- Durable documentation owner: `docs/architecture/linux-local-command-enforcement.md`'s "Non-Cooperative Anti-Bypass Guarantees" section, updated per strategy once implemented; `docs-site/src/content/docs/concepts/sandbox.md` for the user-facing field documentation.

## Implementation slices

### Slice 0: preparatory refactors, behavior-preserving, no new config surface

Added after stress-testing the plan's insertion points against the actual current code (not part of
the independent review — found while validating the design before implementation). Each item is a
small, local, behavior-preserving change with no contract or invariant change on its own — normal
implementation/verification guidance applies, not a separate planning pass — but each directly
de-risks Slices 1-3.

- Production, types, tests, and docs/config:
  - Extract the executable/args rewrite chain in `runtime::execute_run` (currently inline:
    `maybe_apply_executable_policy` → `maybe_apply_claude_settings` → the VS Code shim, `runtime/
    mod.rs:285-320`) into an explicit, ordered sequence, so adding `ExecutionGovernor::rewrite_launch`
    as one more step (Slice 1) is mechanical. The VS Code shim already establishes the exact pattern
    needed — wrap the executable, pass the real target through — so this is consolidation, not new
    design. Confirms the sequencing point: the new step belongs **after** the existing governance
    check (`runtime/mod.rs:311-314`, which must keep evaluating the real target) and **before**
    `LaunchSpec` construction (`runtime/mod.rs:321`).
  - Generalize `supervisor.rs::exit_code()` (`supervisor.rs:82-89`) from taking a
    `std::process::ExitStatus` to a plain `(exit_code: Option<i32>, term_signal: Option<i32>)` shape,
    so both today's `Child`-based path and Slice 3's raw-`waitpid`-based ptrace loop share the same
    `128+signum` mapping instead of duplicating it.
  - Make `sandbox_child_pid`/`parse_first_pid` (`supervisor.rs:184-205`, currently private) visible as
    `pub` (effectively crate-scoped, since the owning module is `pub(crate)`), since Slice 3's ptrace attach needs to locate the same inner-sandbox PID the
    signal-forwarder already knows how to find. Do **not** change their current best-effort semantics
    here — see `DEC-008` for why Slice 3 needs an additional readiness handshake on top, not a change
    to this function's existing (signal-forwarding-appropriate) behavior.
- Affected decisions and traces: `DEC-001` (validates the corrected insertion point against real code), `DEC-007`, `DEC-008`.
- Proof obligations: none new — behavior-preserving by construction.
- Focused verification: full existing test suite passes unchanged, including `supervisor.rs`'s existing signal-forwarding tests against the generalized `exit_code()`.
- Dependencies: none — can land independently of and before Slice 1.
- Intentionally unsupported: no new governance behavior; this is groundwork only.

### Slice 1: config/CLI seam, no new enforcement behavior

- Production, types, tests, and docs/config: add `ExecutionGovernanceStrategy` enum and `ExecutionGovernor` trait (`crates/firma-run/src/execution_governance/mod.rs`) with only the `Inherited` (no-op) implementation; add the `execution_governance` field to the profile schema (`crates/firma-config-schema`), defaulting to `Inherited`; add the compatibility gate (mirrors `backend_supported_on_host`) rejecting, at config-resolution time, both (a) unsupported strategy/backend combinations and (b) strategy selection without a populated, authoritative `allowed_executables` (`DEC-002`'s precondition, `PLAN-001`); extend `firma config`'s template generation and `TestWorld::scaffold_config` (`tests/e2e/harness.rs:103-133`) to accept a strategy parameter, closing the B2 gap (today there is no seam at all — confirmed absent). Document the new field (marked "inherited only is stable; others land in later slices") in `docs-site/src/content/docs/concepts/sandbox.md`.
- Affected decisions and traces: `DEC-001`, `DEC-002`, `DEC-003`.
- Proof obligations: none new (`Inherited` is behavior-preserving by construction).
- Focused verification: full existing e2e suite passes unchanged; unit tests asserting the compatibility gate rejects both (a) `execution_governance = "landlock_execute"` with `backend = "vz"`, and (b) `execution_governance = "landlock_execute"` with `sidecar_local_exec` unset or `enforce_known_executables = false`.
- Dependencies: none strictly, but Slice 0 should land first so this slice's `rewrite_launch` insertion lands in the already-clarified rewrite chain rather than the original inline code.
- Intentionally unsupported: no new governance behavior yet.

### Slice 2: `LandlockExecute`

- Production, types, tests, and docs/config: new `firma __landlock-guarded-run` subcommand (`DEC-004`) resolving `mediator.allowed_executables` to canonical paths, building a Landlock ruleset restricting `LANDLOCK_ACCESS_FS_EXECUTE` to those paths (deny-by-default elsewhere), `PR_SET_NO_NEW_PRIVS` + `landlock_restrict_self`, then `execve` into the real command; wired via `ExecutionGovernor::rewrite_launch` in `runtime::execute_run`, per `DEC-001`'s corrected insertion point — no `SandboxBackend`/`linux_bwrap/mod.rs` change needed; add the `landlock` crate dependency (version `0.4.4`+, per `~/Sources/hakoniwa`'s known-good pin) to `firma-run`; runtime ABI probe (not kernel-version string matching, per `openfirma-notes/compatibility.md`) to detect support and fail closed with an actionable error otherwise.
- Affected decisions and traces: `DEC-001`, `DEC-002`, `DEC-004`; `TRACE-002`.
- Proof obligations: `INV-001` (`PROOF-001`).
- Focused verification: new e2e test — `execution.rs`'s scenario, run under `LandlockExecute`, now asserting the denied child **cannot** execute (kernel-level `EACCES`/`EPERM`), where today it asserts the opposite. Skip (not fail) on kernel <5.13 or when the runtime ABI probe reports Landlock unsupported.
- Dependencies: Slice 1.
- Intentionally unsupported: path-based only — cannot distinguish `git status` from `git push` (same binary, different argv); that remains a Sidecar/Cedar-level concern, not this strategy's job.

### Slice 3: `PtraceSeccompExec`

- Production, types, tests, and docs/config: new `firma __exec-guarded-run` subcommand installing an additional stacked seccomp filter returning `SECCOMP_RET_TRACE` for `execve`/`execveat` (`DEC-006`), then blocking on a readiness handshake before executing the real command (`DEC-008` — not `PTRACE_TRACEME`); wired via `ExecutionGovernor::rewrite_launch` (the shim-wrapping half) exactly like Slice 2. Host-side, a new sibling module to `egress_guard.rs` (placeholder: `exec_guard.rs`) running in the same `firma-run` process (`INV-002`): after `backend.start_agent` returns, locates the inner sandbox PID via `sandbox_child_pid` (made `pub` in Slice 0 — effectively crate-scoped, since the owning module is `pub(crate)`), issues `PTRACE_SEIZE` on it with `PTRACE_O_TRACESECCOMP | PTRACE_O_TRACEEXEC | PTRACE_O_TRACEFORK | PTRACE_O_TRACECLONE | PTRACE_O_TRACEVFORK` set (`DEC-008`), signals the shim's readiness handshake to proceed, then runs a `waitpid`-based loop reading trapped `execve` argv via `process_vm_readv` (reusing the primitive already in `egress_guard.rs:219-236`), checking against `mediator.allowed_executables`, and either `PTRACE_CONT`-ing (allow) or neutralizing the syscall (deny) per `DEC-005`. Per `DEC-007`, this same loop detects the tracee's terminal exit itself and maps it via the generalized `exit_code()` from Slice 0 — it **replaces** `supervisor::wait_with_signal_forwarding` for this strategy only, rather than running a second, independent `waitpid` caller on the same pid; the existing signal-forwarding behavior (SIGINT/SIGTERM/SIGWINCH relay) is reimplemented inside this loop, not dropped. Use `nix::sys::ptrace` (enable the `ptrace` feature, already a workspace dependency at a different feature set) and/or `pete` (actively maintained, safe `PTRACE_SYSCALL` loop) rather than hand-rolled raw `ptrace(2)` calls where their safe APIs suffice.
- Affected decisions and traces: `DEC-002`, `DEC-004`, `DEC-005`, `DEC-006`, `DEC-007`; `TRACE-003`.
- Proof obligations: `INV-001` (`PROOF-002`), `INV-002` (`PROOF-003`), `PROOF-004` (wait-loop unification).
- Focused verification: same e2e scenario as Slice 2, now green under `PtraceSeccompExec`; a dedicated Yama-compatibility test/doctor check exercising the failure path when ptrace attach is denied (`ptrace_scope=2`/`3`, or RHEL if Yama turns out absent there) to confirm fail-closed behavior per `INV-002`; a test confirming SIGINT/SIGTERM/SIGWINCH forwarding still works under this strategy (i.e. `DEC-007`'s unification didn't silently drop existing signal-relay behavior).
- Dependencies: Slice 1. Independent of Slice 2 — either can ship without the other.
- Intentionally unsupported: RHEL-family support is unconfirmed pending the Yama-presence Unknown; ship gated behind an explicit "confirmed platforms" allow-list until verified there, rather than assuming success.

### Slice 4: parametrized coverage and Workstream 2 evidence

- Production, types, tests, and docs/config: generalize `execution.rs` into a table-driven or per-strategy-module test run against all three strategies (`Inherited` stays the red control); add a benchmark harness measuring per-strategy exec-latency overhead; produce the Workstream 2 technical finding comparing `Inherited`/`LandlockExecute`/`PtraceSeccompExec` against `requirements.md` §2's evaluation criteria using this slice's real measurements.
- Affected decisions and traces: none new — this slice consumes Slices 1-3's outputs.
- Proof obligations: closes the "measured overhead" gap in `requirements.md`'s Workstream 2 deliverable list.
- Focused verification: benchmark reproducibility (same profile, repeated runs, bounded variance); the parametrized suite itself is the acceptance check.
- Dependencies: Slices 1-3 (though can run against however many of Slices 2/3 actually landed — the harness should not hard-require all three to exist).
- Intentionally unsupported: does not itself decide a winner — per `requirements.md`, that recommendation is a human/team decision informed by this evidence, not automated by this slice.

## Risks and gaps

- Existing risks: `PtraceSeccompExec` is the highest-complexity, highest-risk slice (new syscall-interception sequencing, an Unknown on RHEL/Yama support, no existing crate providing the full interception loop — confirmed via research, `nix`/`pete` give primitives only). It is sequenced so it can be dropped without invalidating Slices 1-2's value.
- Planned mitigations: fail-closed compatibility gates at config-resolution time (Slice 1); explicit "confirmed platforms" allow-list for `PtraceSeccompExec` until RHEL/Yama is verified (Slice 3); each new `unsafe_code` surface scoped and reasoned per `egress_guard.rs`'s existing precedent (`DEC-004`).
- Explicit evidence gaps: RHEL/CentOS-family Yama LSM presence (Unknown — needs direct verification on a RHEL host, not assumed); exact bubblewrap-internal fork behavior when `--unshare-pid` is absent (Inferred only, not confirmed against `bubblewrap.c` — relevant to whether any additional untracked hop could break the ancestor chain `INV-002` relies on, though current evidence suggests no such hop exists for the documented launch path).
- Least-confident decisions: `DEC-006`'s overhead estimate (scoped `execve`-only trapping being close to ~3x-virtio) is not yet measured — Slice 4 exists specifically to convert this from an estimate into evidence; treat it as provisional until then.

## Plan-review findings and dispositions

Independent review performed by a fresh reviewer with no prior context on this plan, per this
repository's `adversarial-review` → `reviewing-plans` process (the planner is involved in producing
this plan and therefore did not self-review). Reviewer confirmed the working tree matched the
researched revision (`9d761b2b36afa69c32eb1a5cc66e8b9ba45dc34a`) and spot-checked every specific file/
line/symbol citation in the candidate plan against the actual repository before reporting findings.

```yaml
id: PLAN-001
severity: critical
category: constructibility / security
classification: confirmed-conflict
claim: >
  DEC-002 and the CW-001 constructibility analysis both miss a reachable, ordinary config
  combination that leaves the new strategies with nothing to enforce: sidecar_local_exec defaults
  to None (config.rs:773), and even when configured, enforce_known_executables defaults to false
  (config.rs:911) with allowed_executables only required non-empty when that flag is true
  (config.rs:197-202). Selecting LandlockExecute/PtraceSeccompExec without also setting
  enforce_known_executables=true and populating allowed_executables is legal today and produces
  either silent no-op governance or (if Landlock's deny-by-default is taken literally) denies
  everything including the root's own re-exec through the shim.
evidence:
  - crates/firma-run/src/config.rs:773 (sidecar_local_exec default None)
  - crates/firma-run/src/config.rs:911 (enforce_known_executables default false)
  - crates/firma-run/src/config.rs:197-202 (allowed_executables required only when enforce_known_executables=true)
  - docs/architecture/linux-local-command-enforcement.md:183 ("Optional executable allowlist")
reachability: >
  operator sets execution_governance = "landlock_execute" without also flipping
  enforce_known_executables = true and populating allowed_executables → config resolves
  successfully today → strategy has no authoritative set to enforce
invariant_or_boundary: INV-001; "never silently fall back to Inherited's weaker guarantee" clause
impact: false sense of containment (silent no-op) or a broken sandbox (deny-everything), for the
  exact property this plan exists to close
correction: Slice 1's compatibility gate must also reject this combination
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted as a genuine gap. Added as an explicit precondition to DEC-002, a second failure case
    in "Compatibility, migration, and failure semantics", a second constructibility attack (CW-002),
    a new proof obligation (PROOF-000), and an explicit unit-test requirement in Slice 1's focused
    verification.
  incorporated_at: DEC-002, Architecture and invariant ownership (compatibility semantics), CW-002, PROOF-000, Slice 1
  decided_by: planner
```

```yaml
id: PLAN-002
severity: major
category: architecture / type-signature mismatch
classification: confirmed-conflict
claim: >
  The sketched ExecutionGovernor::prepare_launch(&AllowedExecutables, &mut LaunchSpec) cannot be
  called from the integration point DEC-001/architecture text names (BwrapBackend::start_agent,
  immediately before command.spawn() at linux_bwrap/mod.rs:281). SandboxBackend::start_agent takes
  launch: &LaunchSpec immutably (backend/mod.rs:386-391), and by line 281 the executable/args have
  already been read into the bwrap Command three lines earlier. No &mut LaunchSpec is available at
  the cited call site.
evidence:
  - crates/firma-run/src/backend/mod.rs:386-391 (SandboxBackend::start_agent signature, &LaunchSpec)
  - crates/firma-run/src/backend/linux_bwrap/mod.rs:278-281 (executable/args already consumed before the cited insertion point)
reachability: any implementer wiring the trait to its stated call site per the original plan text
invariant_or_boundary: DEC-001's insertion-point claim; the file-tree diff's linux_bwrap/mod.rs entry
impact: unplanned design fork or compile error the first time Slice 1/2 is implemented as originally
  written; the alternative of changing SandboxBackend's shared signature would ripple into
  firecracker.rs/windows_wsl2.rs/macos_vz.rs, contradicting the plan's stated Linux/bwrap-only scope
correction: move the rewrite phase to runtime::execute_run, before LaunchSpec construction, operating
  on the executable/args that will become the LaunchSpec rather than mutating it after construction
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted. DEC-001's insertion-point claim rewritten; ExecutionGovernor's type sketch changed
    from prepare_launch(&mut LaunchSpec) to rewrite_launch(&mut PathBuf, &mut Vec<String>) called
    before LaunchSpec construction; architecture-shape paragraph, Slice 2/3 descriptions, TRACE-002/
    TRACE-003, and the file-tree diff (linux_bwrap/mod.rs removed from the modified-files list;
    runtime/mod.rs's entry expanded) all updated to match.
  incorporated_at: DEC-001, Architecture and invariant ownership, Types and signatures, Slice 2, Slice 3, TRACE-002, TRACE-003, File-tree diff
  decided_by: planner
```

```yaml
id: PLAN-003
severity: major
category: architecture / lifecycle ownership
classification: design-risk
claim: >
  PtraceSeccompExec's required ptrace supervision loop and the existing wait_with_signal_forwarding
  reaper are not shown to coexist safely on the same root_pid. wait_with_signal_forwarding's doc
  comment states reaping happens via "a plain blocking Child::wait on this thread" (supervisor.rs:
  11-24), calling child.wait() directly (supervisor.rs:66-69). Ptrace-stop notification delivery is
  thread-affine on Linux; a second, independent plain-wait() loop on the same pid in the same process
  is a known source of missed or racing wait-status reaping. supervisor.rs is absent from the plan's
  file-tree diff.
evidence:
  - crates/firma-run/src/supervisor.rs:11-24 (doc comment: reaping via plain blocking Child::wait, no SIGCHLD handling)
  - crates/firma-run/src/supervisor.rs:66-69 (child.wait() call site)
reachability: PtraceSeccompExec selected → both the existing reaper and a new ptrace-wait loop would
  independently call waitpid on the same pid from the same process, absent a stated resolution
invariant_or_boundary: process lifecycle ownership for the sandboxed root process
impact: missed or racing wait-status reaping; unclear which loop actually observes a given ptrace stop
  or the terminal exit
correction: give supervise() full ownership of the wait loop for this strategy, unifying or
  superseding wait_with_signal_forwarding rather than running alongside it; add supervisor.rs to the
  Slice 3 file-tree diff
confidence: medium
assumptions:
  - based on documented Linux ptrace/waitpid thread-affinity behavior and the explicit "reaped on
    this thread" code comment, not verified against a running prototype
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted as a design risk requiring an explicit resolution direction (full closure deferred to
    implementation, per the reviewer's own confidence caveat). Added DEC-007 stating supervise()
    takes over the wait loop entirely for PtraceSeccompExec rather than running alongside the
    existing reaper; added supervisor.rs to the Slice 3 file-tree diff and to Slice 3's production
    description; added PROOF-004 covering signal-forwarding/reaping continuity under the unified loop.
  incorporated_at: DEC-007, Architecture and invariant ownership, Slice 3, File-tree diff, PROOF-004
  decided_by: planner
```

All three findings are corrected in this artifact (not merely acknowledged) — see the cited sections.
No finding was rejected or deferred; none required a user decision, since each had one clearly
correct-per-evidence resolution direction rather than a product tradeoff. The reviewer's "Other
observations" (the `AllowedExecutables` raw-type imprecision; the benchmark-harness placement
question) were not independently actionable per the reviewer's own framing and are left as
implementation-time detail, not tracked as separate findings.

## Final verification

- Focused checks: per-slice verification as listed above.
- Workspace checks: `just check` (fmt, lint, hawk, test, build) after each slice.
- Post-implementation independent review: required per `adversarial-review` on the actual implemented change, in addition to this plan review.

---

## Technical evidence

### Applicability assessment

| Section                     | Applicability | Reason or evidence                                                                                                                                           |
| --------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Vocabulary                  | Applicable    | "execution governance," "mechanism"/"strategy," and per-mechanism names are new or overloaded terms this plan introduces.                                    |
| Alternatives                | Applicable    | Multiple viable designs were considered and rejected with material tradeoffs (full ptrace mediation, cgroup+lineage, seccomp-unotify, twin-execution relay). |
| File-tree diff              | Applicable    | New modules and files are added across four crates.                                                                                                          |
| Type and signature sketches | Applicable    | `ExecutionGovernanceStrategy`/`ExecutionGovernor` shape and a real constructibility risk (mechanism/backend mismatch) need recording.                        |
| Semantic call traces        | Applicable    | Behavior crosses `runtime/mod.rs`, `linux_bwrap/mod.rs`, and new host-side supervision, at a trust boundary.                                                 |
| Trust analysis              | Applicable    | This is squarely a security-boundary change with an adversarial actor (the sandboxed agent).                                                                 |
| Detailed proof obligations  | Applicable    | `INV-001`/`INV-002` need evidence across the e2e suite and platform-specific failure paths.                                                                  |

### Conditional: Vocabulary

| Canonical term                | Meaning                                                                   | Owner/context                                    | Synonyms or terms to avoid                                                                                                    | Conflict or decision                                                      |
| ----------------------------- | ------------------------------------------------------------------------- | ------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| Execution governance          | The decision of whether a specific `execve` may proceed                   | `ExecutionGovernor` (new)                        | "command governance," "local-exec governance" (used interchangeably in existing code/tests — keep as synonyms, not new terms) | None — matches existing usage in `mediator.rs`/test module names          |
| Execution-governance strategy | One selectable implementation of the above                                | `execution_governance` config field              | "backend" (avoid — `BackendKind` is a different, existing axis per `DEC-001`)                                                 | New term; must not be confused with `BackendKind` in docs                 |
| Inherited                     | The strategy name for today's unchanged (root-only) behavior              | `ExecutionGovernanceStrategy::Inherited`         | "baseline," "default" (fine as informal synonyms)                                                                             | New term                                                                  |
| Landlock execute-right        | Landlock's `LANDLOCK_ACCESS_FS_EXECUTE` access right, applied path-scoped | `ExecutionGovernanceStrategy::LandlockExecute`   | n/a                                                                                                                           | None                                                                      |
| Ptrace-seccomp exec-gate      | The `SECCOMP_RET_TRACE`-scoped, ptrace-mediated allow/deny design         | `ExecutionGovernanceStrategy::PtraceSeccompExec` | "syscall proxy," "twin execution" (avoid — rejected shape per `DEC-005`)                                                      | Superseded terminology from earlier design discussion; do not reintroduce |

### Conditional: Alternatives

**Full syscall-table ptrace mediation (gVisor-Sentry-style)** — shape: trace every syscall, not just `execve`/`execveat`; invariant owner would still be `PtraceSeccompExec`, just with a much larger trapped set. Benefits: closer to true full mediation, could in principle support the originally-proposed "daemon runs the real work, relays result" shape for more than just exec. Costs: gVisor's own docs call this "the highest structural costs by far" among its platforms; academic measurement (arXiv 2406.07429) puts raw per-syscall cost 10-70x above lighter mechanisms, real-workload overhead 40%-300%+. No existing Rust crate implements the required syscall-emulation layer (confirmed via research — `nix`/`pete` give primitives, not a Sentry-equivalent). Rejected: overhead is incompatible with the ~3x-virtio target and the added complexity (full syscall emulation) is not needed to answer a plain allow/deny question. See `DEC-006`.

**Daemon-performs-real-exec-and-relays-substituted-result ("twin execution")** — shape: on a trapped `execve`, the daemon runs the actual command in its own confined child and somehow delivers stdout/exit-status back into the originally-trapped (and now-replaced-or-dead) process. Benefits: was the originally proposed design; would in principle allow output capture/provenance tracking beyond a plain gate. Costs: technically unfounded for `execve` specifically — the syscall replaces the calling process's image on success, and neither seccomp-notify's `CONTINUE` nor ptrace's register/memory access provide a "substitute the whole process's subsequent behavior" primitive; gVisor's actual technique for this class of problem is full OS-level emulation in the tracer (see above alternative), not a lightweight relay. Rejected: unfounded as originally scoped; a plain gate achieves the stated FIR-366 requirement without it. See `DEC-005`.

**Separate ptrace daemon process (not the `firma-run` host process itself)** — shape: a dedicated, standalone process attaches to the sandboxed agent via ptrace. Benefits: cleaner separation of concerns, could be reused across multiple `firma run` invocations. Costs: under Yama `ptrace_scope=1` (Ubuntu/Debian default), an unrelated process is not a permitted attacher without `CAP_SYS_PTRACE` — confirmed via kernel source research (`task_is_descendant` walk in `security/yama/yama_lsm.c`) — since it would not be a real ancestor of the traced process. Rejected: would require running privileged (`CAP_SYS_PTRACE`) or under a weakened `ptrace_scope`, either of which is a strictly worse starting point per `requirements.md` §2's "broader applicability preferred" criterion, when the existing `firma-run` host process already qualifies as an ancestor for free. See `INV-002`, `TRACE-003`.

**Derek's cgroup v2 + namespace + lineage design, as a fourth strategy in this same axis** — shape: unknown in detail (design lives outside the repository). Benefits: per `openfirma-notes/requirements-assessment.md`, likely answers a different half of the problem (always being able to enumerate/kill the whole tree regardless of session tricks — relevant to FIR-442, not FIR-366's execution-allow-list question). Costs: cannot be designed against without the actual write-up. Rejected for _this_ plan specifically because it isn't yet documented anywhere accessible — not rejected as a mechanism; tracked as a separate pending item (`openfirma-notes/todo/pending.md`) to potentially become a fourth strategy in this same `ExecutionGovernanceStrategy` enum once its design exists.

**Plain seccomp-notify allow/deny on `execve` (no ptrace at all)** — shape: use `SECCOMP_RET_USER_NOTIF` directly on `execve`/`execveat` (like `egress_guard.rs` already does for `connect`), skip ptrace entirely. Benefits: reuses the exact existing pattern with zero new primitives (`nix`/`pete` not needed at all). Costs: **Unknown/Inferred** — needs verification during Slice 3 design detail whether `execve` under `SECCOMP_RET_USER_NOTIF` behaves acceptably for a plain deny (unlike the "relay a substituted result" case, a plain deny does not require completing the syscall differently, so this may in fact be sufficient and simpler than the ptrace-based design). **This is flagged as a live open question for Slice 3, not fully resolved by this plan** — worth spiking both variants (seccomp-notify-only vs. `SECCOMP_RET_TRACE`+ptrace) before committing to the heavier one, since the primary reason ptrace was introduced (the "relay a result" idea) has been dropped per `DEC-005`. See "Least-confident decisions."

### Conditional: File-tree diff

```diff
 crates/firma-run/src/
+├── execution_governance/          # NEW — ExecutionGovernanceStrategy, ExecutionGovernor trait, dispatch
+│   ├── mod.rs                     # NEW
+│   ├── inherited.rs               # NEW — no-op (today's behavior)
+│   ├── landlock_execute.rs        # NEW — Slice 2
+│   └── ptrace_seccomp.rs          # NEW — Slice 3
~├── runtime/mod.rs                 # MODIFIED — calls rewrite_launch before LaunchSpec construction (~line 321) and supervise after backend.start_agent returns (~line 336); no linux_bwrap/mod.rs change needed (corrected after PLAN-002)
~├── supervisor.rs                  # MODIFIED (Slice 0: generalize exit_code(), expose sandbox_child_pid/parse_first_pid as pub (effectively crate-scoped; the owning module is pub(crate), so clippy::redundant_pub_crate requires plain pub); Slice 3: wait_with_signal_forwarding superseded per-strategy by PtraceSeccompExec::supervise, DEC-007/DEC-008)
~├── config.rs                      # MODIFIED — add compatibility gate mirroring backend_supported_on_host (lines 725-733), extended to also check enforce_known_executables/allowed_executables (PLAN-001)
 crates/firma-config-schema/src/run.rs
~└── (profile schema)               # MODIFIED — add execution_governance field, default Inherited
 crates/firma/src/args/run.rs
~└── (CLI args)                     # MODIFIED — optional --execution-governance override, mirrors --backend
 tests/e2e/
~├── harness.rs                     # MODIFIED — TestWorld::scaffold_config accepts a strategy parameter
~├── scenarios/child_process_governance/execution.rs   # MODIFIED (Slice 4) — parametrized across strategies
+└── scenarios/child_process_governance/governance_bench.rs   # NEW (Slice 4) — per-strategy overhead benchmark
 docs/adr/
+└── <new ADR>-selectable-execution-governance.md   # NEW — records this decision durably alongside FIR-60
 docs/architecture/
~└── linux-local-command-enforcement.md   # MODIFIED — "Current limits" section updated per strategy
 docs-site/src/content/docs/concepts/sandbox.md
~└── (docs)                         # MODIFIED — document the new field; mark new strategies experimental
```

### Conditional: Types and signatures

```rust
// crates/firma-run/src/execution_governance/mod.rs

pub enum ExecutionGovernanceStrategy {
    Inherited,
    LandlockExecute,
    PtraceSeccompExec,
}

pub trait ExecutionGovernor: Send + Sync {
    fn strategy(&self) -> ExecutionGovernanceStrategy;

    /// Called in `runtime::execute_run`, before `LaunchSpec` is constructed
    /// (before the immutable value `SandboxBackend::start_agent` receives is
    /// frozen — that trait's `&LaunchSpec` parameter is immutable, and by
    /// `linux_bwrap/mod.rs:281` the executable/args are already consumed, so
    /// this cannot run inside a `SandboxBackend` impl; corrected after
    /// `PLAN-002`). Rewrites the executable/args that will become part of
    /// `LaunchSpec` — e.g. wrapping them to point at a `firma __*-guarded-run`
    /// shim with the real target passed through.
    fn rewrite_launch(
        &self,
        allowed: &AllowedExecutables,
        executable: &mut PathBuf,
        args: &mut Vec<String>,
    ) -> Result<GovernanceHandle, RunError>;

    /// Called in `runtime::execute_run` immediately after `backend.start_agent`
    /// returns its `Child`. For strategies needing host-side supervision
    /// (`PtraceSeccompExec`), this call takes over full responsibility for
    /// waiting on the child — including the signal-forwarding/reaping duties
    /// `supervisor::wait_with_signal_forwarding` performs today — rather than
    /// running a second, independent wait loop alongside it (`DEC-007`,
    /// corrected after `PLAN-003`). `Inherited`/`LandlockExecute` delegate
    /// straight through to that existing function, unchanged.
    fn supervise(&self, handle: GovernanceHandle, child: Child) -> Result<ExitStatus, RunError>;
}
```

**Constructibility attack (`CW-001`)**: does anything stop constructing `execution_governance = "landlock_execute"` together with `backend = "vz"` (macOS) — an illegal combination, since Landlock and this ptrace design are Linux-only? As sketched, **nothing does** — a bare enum plus a bare config field lets this compile and parse. This is why Slice 1 explicitly includes a compatibility gate at config-resolution time (mirroring `backend_supported_on_host`, `config.rs:725-733`) rather than relying on the type system alone to make this state unrepresentable. The type system is not asked to prove this; a runtime validating constructor is the chosen boundary (recorded here rather than overclaiming a type-level guarantee).

**Second constructibility attack (`CW-002`, added after plan review, `PLAN-001`)**: does anything stop constructing `execution_governance = "landlock_execute"` together with `sidecar_local_exec` unset, or set with `enforce_known_executables = false`? Also **nothing does**, and this is a more likely misconfiguration than `CW-001`'s backend mismatch, since it doesn't require picking an unusual backend — just omitting or forgetting one flag on an otherwise-ordinary profile. `DEC-002`'s precondition and the "Compatibility, migration, and failure semantics" line both now require Slice 1's compatibility gate to check this specific cross-field combination, not only strategy/backend compatibility.

**What this design does prove via types**: `ExecutionGovernanceStrategy` is a closed enum (cardinality: exactly three values, no stringly-typed wildcard), and `ExecutionGovernor::rewrite_launch`'s `&AllowedExecutables` parameter (a distinct, already-resolved type, not a raw `Vec<String>`) prevents accidentally passing an un-canonicalized or unresolved executable list to a strategy (semantic role/provenance: the type marks "this has already been through `resolve_governed_executable`'s canonicalization," not just "some list of strings").

### Conditional: Semantic call traces

| Field                      | `TRACE-001`                                                                                                                                                           |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Current                                                                                                                                                               |
| Entry and stimulus         | `firma run` launches a profile with `sidecar_local_exec` configured; root command executes a shell that spawns a denied tool                                          |
| Path                       | `runtime::execute_run → resolve_governed_executable (root only) → enforce_local_command_governance (root only) → backend.start_agent → [no re-entry for descendants]` |
| Input/output types         | `LaunchSpec` (root only) → `Child`                                                                                                                                    |
| Validation/trust crossings | One Sidecar round-trip, root only                                                                                                                                     |
| Invariant established      | None for descendants                                                                                                                                                  |
| Invariant assumed          | None — this is the documented gap                                                                                                                                     |
| Success outcome            | Root allowed/denied correctly; descendant unconditionally allowed regardless of policy                                                                                |
| Failure path               | N/A for descendants — there is no failure path because there is no check                                                                                              |
| Evidence                   | `tests/e2e/scenarios/child_process_governance/execution.rs` (`#[ignore]`d, currently fails as expected)                                                               |
| Proof boundary             | e2e suite                                                                                                                                                             |
| Unknowns                   | None — this trace is fully Observed                                                                                                                                   |

| Field                      | `TRACE-002`                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Proposed (`LandlockExecute`, Slice 2)                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Entry and stimulus         | Same as `TRACE-001`, with `execution_governance = "landlock_execute"`                                                                                                                                                                                                                                                                                                                                                                                     |
| Path                       | `runtime::execute_run → resolve_governed_executable (root) → ExecutionGovernor::rewrite_launch (before LaunchSpec construction; corrected after PLAN-002) → LaunchSpec built pointing at firma __landlock-guarded-run → backend.start_agent (unmodified) → firma __landlock-guarded-run (self-applies Landlock ruleset from AllowedExecutables, execve's real root command) → [descendant execve trapped by inherited Landlock ruleset, kernel-enforced]` |
| Input/output types         | `AllowedExecutables` → `LandlockRuleset` (new) → kernel ruleset state (not a Rust type after installation)                                                                                                                                                                                                                                                                                                                                                |
| Validation/trust crossings | Ruleset installed once, before any untrusted code runs in the sandbox; kernel enforces thereafter — no further userspace trust crossing                                                                                                                                                                                                                                                                                                                   |
| Invariant established      | `INV-001`, for the path-based subset Landlock can express                                                                                                                                                                                                                                                                                                                                                                                                 |
| Invariant assumed          | The resolved `AllowedExecutables` paths are stable for the run's lifetime (matches `DEC-002`)                                                                                                                                                                                                                                                                                                                                                             |
| Success outcome            | Descendant `execve` of a non-allow-listed path fails with `EACCES` at the kernel level                                                                                                                                                                                                                                                                                                                                                                    |
| Failure path               | Ruleset install failure (unsupported kernel, `landlock` crate error) → fail closed, launch aborts with actionable error, never silently falls back to `Inherited`                                                                                                                                                                                                                                                                                         |
| Evidence                   | New e2e test (Slice 2); `openfirma-notes/compatibility.md` for kernel/distro support gating                                                                                                                                                                                                                                                                                                                                                               |
| Proof boundary             | e2e suite + runtime ABI probe                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Unknowns                   | RHEL-family Landlock backport status must be probed at runtime, not assumed from `uname -r` (already documented in `compatibility.md`)                                                                                                                                                                                                                                                                                                                    |

| Field                      | `TRACE-003`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Proposed (`PtraceSeccompExec`, Slice 3) — also documents the _current_, already-Observed ancestor relationship this strategy depends on                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Entry and stimulus         | Same as `TRACE-001`, with `execution_governance = "ptrace_seccomp_exec"`; descendant attempts a disallowed `execve`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Path                       | `runtime::execute_run → ExecutionGovernor::rewrite_launch (before LaunchSpec construction) → LaunchSpec built pointing at firma __exec-guarded-run → backend.start_agent spawns bwrap (unmodified) → bwrap forks the inner sandbox process, which installs a SECCOMP_RET_TRACE-on-execve filter and blocks on the readiness handshake → ExecutionGovernor::supervise (firma-run host process, already a confirmed real-tree ancestor) locates the inner PID via sandbox_child_pid, issues PTRACE_SEIZE on it (DEC-008), signals the handshake, and takes over the wait loop from wait_with_signal_forwarding (DEC-007) → the inner process execve's the real command → [descendant execve] → kernel raises PTRACE_EVENT_SECCOMP → the same host-side supervise loop reads argv via process_vm_readv (reusing egress_guard.rs:219-236's primitive) → checks AllowedExecutables → PTRACE_CONT (allow) or neutralize (deny)` |
| Input/output types         | Raw tracee memory (via `process_vm_readv`) → parsed argv/path → `AllowedExecutables` lookup → ptrace response                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Validation/trust crossings | The daemon trusts nothing from the tracee except as raw bytes to parse defensively (same posture `egress_guard.rs` already takes for `connect()` sockaddrs)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Invariant established      | `INV-001` for every syscall reachable via `execve`/`execveat`; `INV-002` (no elevated privilege) by construction, since the attacher is the same process already proven to be a real ancestor                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Invariant assumed          | `PTRACE_O_TRACEFORK`/`TRACECLONE`/`TRACEVFORK` correctly extends attachment to all descendants, not just the immediate child (needs explicit test coverage, not just assumed from the option names)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Success outcome            | Descendant `execve` of a non-allow-listed path never completes; observable failure matches today's seccomp-deny shape                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Failure path               | Ptrace attach fails (Yama restriction, RHEL absence, or otherwise) → fail closed, launch aborts with actionable error naming the specific cause (never silently degrade to `Inherited`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Evidence                   | Yama kernel-source research (`security/yama/yama_lsm.c`'s `task_is_descendant`); `egress_guard.rs:216`'s existing doc comment on `process_vm_readv`'s ptrace-scope sensitivity, proving this exact process already relies on the same permission today                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Proof boundary             | e2e suite (Slice 3) + a dedicated Yama-failure-path test                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Unknowns                   | RHEL/CentOS-family Yama presence (Unknown); exact byte-level protocol of the readiness handshake in `DEC-008` (pipe vs. socket, timeout on `PTRACE_SEIZE` failure) is implementation-time detail beyond this plan's scope                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |

### Conditional: Trust analysis

- Actors: the sandboxed agent process and everything it spawns (untrusted, potentially adversarial — this is the whole premise of a governance boundary); the `firma-run` host process (trusted, part of the TCB); the Sidecar (trusted, part of the TCB, consulted only at root launch per `DEC-002`).
- Supported workloads/deployment modes: Linux only, `bwrap` structural backend only (`DEC-001` scope).
- Attacker capabilities: full control over what the agent process and any descendant it spawns attempts to execute, including double-forking, `setsid()`, or otherwise trying to escape process-group-based tracking (a separate concern, FIR-442, not this plan's target — but `PTRACE_O_TRACEFORK`/`CLONE`/`VFORK` incidentally also improves descendant tracking, worth cross-referencing when FIR-442 work resumes).
- Protected assets: the set of binaries permitted to run inside the sandbox for the run's lifetime (`AllowedExecutables`).
- Trust transitions: none new for `LandlockExecute` (kernel enforces post-installation, no further userspace involvement). For `PtraceSeccompExec`, every trapped `execve` is a trust transition point where the host-side supervisor (trusted) makes a decision based on data read from the (untrusted) tracee's memory — the same posture `egress_guard.rs` already takes and defends against TOCTOU on sockaddr parsing; the equivalent TOCTOU consideration for `execve` argv (can the tracee race the argv between the trap and `PTRACE_GETREGS`/`process_vm_readv`?) is an explicit proof obligation for Slice 3, not yet resolved by this plan (see `PROOF-002`'s "Limits").
- Reachable abuse paths: distinguish accident-prevention (e.g., a benign script accidentally invoking a disallowed tool) from the actual security boundary (an adversarial agent deliberately trying to run something denied) — both `LandlockExecute` and `PtraceSeccompExec` are designed to hold against the adversarial case, not just the accidental one, since that's the stated requirement in `requirements.md` §1.

### Conditional: Detailed proof obligations

| Field                  | `PROOF-000` (config gate, added after plan review, `PLAN-001`)                                                                                               |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Invariant              | `INV-001`'s precondition — a strategy has nothing to enforce without an authoritative `AllowedExecutables`                                                   |
| Kind                   | Compatibility                                                                                                                                                |
| Owner/proof boundary   | Config resolution (`crates/firma-run/src/config.rs`)                                                                                                         |
| Suite/boundary         | Unit/config                                                                                                                                                  |
| Stimulus               | `execution_governance != Inherited` with `sidecar_local_exec` unset, or `enforce_known_executables = false`, or (defensively) an empty `allowed_executables` |
| Observable effects     | Config resolution fails with an actionable error naming the missing precondition, before any process is launched                                             |
| Controls/substitutions | None needed — pure config-value combination                                                                                                                  |
| Failure cases          | This row is itself the failure case being proven                                                                                                             |
| Evidence               | New unit test, Slice 1                                                                                                                                       |
| Status                 | Planned                                                                                                                                                      |
| Slice                  | 1                                                                                                                                                            |
| Limits                 | Proves the gate rejects the known-bad combination; does not by itself prove the enforcement source is _correct_, only that it's present                      |

| Field                  | `PROOF-001` (`INV-001`, Landlock)                                                                                                                                                                               |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kind                   | Runtime / Trust                                                                                                                                                                                                 |
| Owner/proof boundary   | `LandlockExecute` `ExecutionGovernor`                                                                                                                                                                           |
| Suite/boundary         | E2E (Slice 2)                                                                                                                                                                                                   |
| Stimulus               | Root process (allowed) spawns `bash -c forbidden-tool` where `forbidden-tool` is not in `AllowedExecutables`                                                                                                    |
| Observable effects     | Child `execve` fails with `EACCES`/`EPERM`; no marker file the forbidden tool would have written appears                                                                                                        |
| Controls/substitutions | Same fixture pattern as existing `execution.rs` (`FORBIDDEN_MARKER`, `write_forbidden_tool`)                                                                                                                    |
| Failure cases          | Landlock unsupported on host → launch fails closed at config-resolution, never silently proceeds under `Inherited`                                                                                              |
| Evidence               | New e2e test, Slice 2                                                                                                                                                                                           |
| Status                 | Planned                                                                                                                                                                                                         |
| Slice                  | 2                                                                                                                                                                                                               |
| Limits                 | Proves the path-based case only; does not prove argv/context-based restriction (e.g. "allow `git status`, deny `git push`") — Landlock cannot express that, by design (see Slice 2 "Intentionally unsupported") |

| Field                  | `PROOF-002` (`INV-001`, ptrace)                                                                                                                                                                                                                        |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Kind                   | Runtime / Trust                                                                                                                                                                                                                                        |
| Owner/proof boundary   | `PtraceSeccompExec` `ExecutionGovernor`                                                                                                                                                                                                                |
| Suite/boundary         | E2E (Slice 3)                                                                                                                                                                                                                                          |
| Stimulus               | Same as `PROOF-001`, under `execution_governance = "ptrace_seccomp_exec"`                                                                                                                                                                              |
| Observable effects     | Same as `PROOF-001`                                                                                                                                                                                                                                    |
| Controls/substitutions | Same fixture pattern                                                                                                                                                                                                                                   |
| Failure cases          | Ptrace attach fails → fail closed (see `PROOF-003`)                                                                                                                                                                                                    |
| Evidence               | New e2e test, Slice 3                                                                                                                                                                                                                                  |
| Status                 | Planned                                                                                                                                                                                                                                                |
| Slice                  | 3                                                                                                                                                                                                                                                      |
| Limits                 | Does not prove immunity to a TOCTOU race between the seccomp trap and the supervisor's argv read — that specific race needs its own adversarial test before this proof is considered complete; flagged as an explicit gap, not yet closed by this plan |

| Field                  | `PROOF-003` (`INV-002`)                                                                                                                                           |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kind                   | Trust / Compatibility                                                                                                                                             |
| Owner/proof boundary   | `PtraceSeccompExec` `ExecutionGovernor`                                                                                                                           |
| Suite/boundary         | Platform-specific integration test + `firma doctor`                                                                                                               |
| Stimulus               | Attempt to select `ptrace_seccomp_exec` under `ptrace_scope=2`/`3`, or on a host where Yama is absent but some other restriction applies, or on RHEL specifically |
| Observable effects     | Launch fails closed with an actionable, specific error (not a generic panic, not a silent fallback)                                                               |
| Controls/substitutions | Test harness sets `ptrace_scope` via a scoped sysctl write where CI permits, or mocks the attach failure                                                          |
| Failure cases          | This whole row _is_ the failure case being proven                                                                                                                 |
| Evidence               | New test, Slice 3                                                                                                                                                 |
| Status                 | Gap — not yet designed in detail; RHEL behavior specifically is Unknown pending direct verification                                                               |
| Slice                  | 3                                                                                                                                                                 |
| Limits                 | Proves fail-closed behavior for the failure modes tested; does not prove exhaustive coverage of every possible ptrace-attach failure cause                        |

| Field                  | `PROOF-004` (`DEC-007`, wait-loop unification, added after plan review, `PLAN-003`)                                                                                                                                          |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kind                   | Runtime / Lifecycle                                                                                                                                                                                                          |
| Owner/proof boundary   | `PtraceSeccompExec`'s `supervise` implementation                                                                                                                                                                             |
| Suite/boundary         | Integration (Slice 3)                                                                                                                                                                                                        |
| Stimulus               | Root process under `PtraceSeccompExec` receives SIGINT/SIGTERM/SIGWINCH while a descendant is mid-flight, and separately, the root process exits normally                                                                    |
| Observable effects     | Signals are still forwarded to the sandboxed process group exactly as `wait_with_signal_forwarding` does today; the root's terminal exit status is still correctly reaped; no missed or duplicated `waitpid` on the same pid |
| Controls/substitutions | Reuse existing signal-forwarding test fixtures where possible                                                                                                                                                                |
| Failure cases          | A second, independent wait loop racing the existing reaper (the bug this proof obligation exists to rule out)                                                                                                                |
| Evidence               | New test, Slice 3                                                                                                                                                                                                            |
| Status                 | Gap — design direction stated in `DEC-007`, exact unification mechanism (strategy-aware `wait_with_signal_forwarding` vs. full supersession) left to implementation                                                          |
| Slice                  | 3                                                                                                                                                                                                                            |
| Limits                 | Proves the specific signal/exit scenarios tested; does not prove absence of every possible race under adversarial timing                                                                                                     |
