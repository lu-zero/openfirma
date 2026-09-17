# `HakoniwaBackend`: an embedded-library Linux sandbox backend

## Artifact metadata

- Status: Accepted (independent plan review complete, all nine findings corrected — see "Plan-review findings and dispositions")
- Durable locator: `docs/architecture/hakoniwa-backend-plan.md` (this file, in-repo)
- Repository revision researched: `9d761b2b36afa69c32eb1a5cc66e8b9ba45dc34a`
- Task or requirement source: `~/Sources/openfirma-notes/todo/pending.md` ("If Hakoniwa is prototyped: ... Scope as a fourth `BackendKind` spike with a written sunset date"); user request to build this as a new `HakoniwaBackend: SandboxBackend` with real interfacing, not a `Container::command()` shortcut
- Supersedes: Not applicable. Relates to but does not supersede `docs/adr/FIR-60-sandbox-backend-selection-for-firma-run.md` (which explains why `bwrap` is external today) or `docs/architecture/selectable-execution-governance-plan.md` (a separate, independent axis layered on top of whichever `SandboxBackend` is selected — see `~/Sources/openfirma-notes/notes/hakoniwa-backend-gap-analysis.md`'s "Sequencing" section for why neither plan blocks the other)

## Goal and acceptance outcomes

- Goal: add a new, additive `HakoniwaBackend` implementing the existing `SandboxBackend` trait, embedding the `hakoniwa` Rust library instead of shelling out to the external `bwrap` binary, without modifying `SandboxBackend`'s method signatures or `BwrapBackend`'s implementation.
- Observable acceptance outcomes:
  - `firma run --backend hakoniwa` (and `backend = "hakoniwa"` in `firma.toml`) launches a sandboxed agent with network, filesystem, and DNS-stub/egress-guard confinement equivalent to `BwrapBackend`'s, proven by running the existing `tests/e2e/scenarios/child_process_governance/{network,filesystem,http}.rs` suite against it unmodified (parametrized, per `DEC-011`).
  - Selecting `hakoniwa` on a host that can't support it (missing kernel unprivileged-userns support, non-Linux) fails closed at config-resolution time with an actionable error, mirroring `backend_supported_on_host`.
  - `firma.toml`/docs mark `hakoniwa` explicitly experimental with a stated sunset condition, per the existing precedent for unproven mechanisms (`FIRMA_RUN_VZ_STRUCTURAL_NETWORK`).

## Scope

- In scope: a new `HakoniwaBackend` struct and its `SandboxBackend` impl; a new sibling launcher binary crate embedding `hakoniwa`; the ~10 exhaustive-match/gating sites a new `BackendKind` variant requires; a minimal loopback bring-up; a Rust reimplementation of the in-sandbox DNS-stub/egress-guard bootstrap; mapping the existing `deny_actions` policy onto `hakoniwa::seccomp`/`hakoniwa::landlock` builders.
- Out of scope:
  - Modifying `SandboxBackend`'s method signatures or any existing backend's implementation (`BwrapBackend`, `VzBackend`, `Wsl2Backend`, `FirecrackerBackend`) — purely additive.
  - Reusing or extending `crates/firma-run/src/seccomp.rs`'s managed-seccomp TOML/BPF-compiler pipeline for this backend — see `DEC-004`.
  - The `cgroups` feature (requires systemd; not needed for parity with bwrap, which has no cgroups dependency today).
  - `Pasta`/`RustSlirp`-based real network egress — firma-run wants no route out except through the Sidecar, not working internet access for the sandbox.
  - The execution-governance plan's `LandlockExecute`/`PtraceSeccompExec` strategies (`docs/architecture/selectable-execution-governance-plan.md`) — a separate, independent axis; this plan only needs `HakoniwaBackend` to exist as _a_ `SandboxBackend`, not to host those strategies (though `DEC-004` notes a plausible future convergence).
  - Making `hakoniwa` the default backend on any platform — it ships opt-in and experimental (`DEC-010`).
- Assumptions:
  - The 14 existing unit tests in `linux_bwrap/mount.rs` validating `.firma` masking order are a sufficient _reference_ for what a Hakoniwa-side equivalent must prove, even though the underlying mount primitives differ (`DEC-006`).
  - `hakoniwa = "1.7.2"` (the version already used to derive this plan's findings, per `~/Sources/hakoniwa/hakoniwa/Cargo.toml`) is pinned explicitly, not left to float, consistent with this repo's general caution about unvetted transitive dependency drift on security-critical code.
  - **Hakoniwa's own license does not block static embedding, unlike bwrap's** (added after plan review, `PLAN-004`, which correctly noted this plan cites ADR FIR-60's LGPL-driven reason _not_ to statically embed bwrap without ever reasoning about the license of the library this plan proposes to statically embed instead). Already verified in a prior session and recorded at `~/Sources/openfirma-notes/notes/hakoniwa-license-verification.md`: `hakoniwa`'s library crate is `LGPL-3.0-only WITH LGPL-3.0-linking-exception` (confirmed directly from `hakoniwa/Cargo.toml`), and that exception exists specifically so linking doesn't trigger LGPL's copyleft obligations on the linking binary — a materially different position from bwrap's plain LGPL-2.0-or-later, which is exactly why FIR-60 is cautious about _bwrap_ specifically. `hakoniwa-cli` (a separate binary, GPL-3.0-only) is not embedded by this plan — only the `hakoniwa` library crate is a dependency of `firma-hakoniwa-runner`.
- Open decisions: exact new-crate name (`firma-hakoniwa-runner` used as a placeholder, mirroring `firma-vz-runner`); exact sunset condition wording for the "experimental" marking (a fixed release count vs. an explicit audit-completion gate — recommend the latter, see `DEC-010`).
- Cohesion and split assessment: kept as one plan because every slice below shares one invariant owner (`INV-001`, structural network confinement) and one integration point (the new `HakoniwaBackend` struct); however, Slices 5-6 (seccomp/landlock builder wiring; full e2e parity + benchmark) have their own observable acceptance outcomes and could be split into a child plan if reviewers want the "does it structurally confine at all" question (Slices 1-4) answered and shipped before committing to the seccomp/landlock convergence question — flagged as a legitimate split point, not forced here.
- Deferred child plans: possible split noted above; not exercised in this artifact.

## Routing

- Mode: Full
- Trigger evidence: (1) security/trust boundary — a new sandbox backend is definitionally the security boundary; (2) externally observable, documented config/CLI surface — `--backend`/`backend =` is documented in `docs-site/src/content/docs/concepts/sandbox.md` per CLAUDE.md's API-stability carve-out; (3) invariant ownership — `INV-001` (structural network confinement) needs a second, independent proof owner; (5) multiple crates with substantial uncertainty (a new sibling binary crate, `firma-run`, `firma-config-schema`, `firma` CLI, `firma`'s doctor); (6) multiple viable designs with materially different tradeoffs (network bring-up approach, seccomp/landlock convergence, mount-translation approach) — all independently sufficient; no single trigger is load-bearing.
- Higher-mode triggers checked: no additional triggers beyond Full apply.
- Downgrade evidence and reason: Not applicable.

## Current behavior and problem

- Owners and entry points: `crates/firma-run/src/backend/mod.rs` defines `BackendKind` (`Bwrap | Vz | Wsl2 | Firecracker`, lines 82-87) and the `SandboxBackend` trait (347-399: `kind`, `prepare`, `enforce_network`, `verify_fail_closed`, `start_agent -> Result<std::process::Child, RunError>`, `teardown`), dispatched via `build_backend` (401-410). Today, Linux structural confinement is `BwrapBackend` only (`linux_bwrap/mod.rs`), which shells out to the external `bwrap` binary (`Command::new("bwrap")`, line 214) — the ADR-recorded, deliberate choice `docs/adr/FIR-60` documents (LGPL-2.0-or-later, avoid static embedding).
- Current success and failure outcomes: `BwrapBackend` provides real structural confinement (`EnforcementProof.structural = true` when `NetworkPolicy.enforce_network_namespace` is set) via `--unshare-net` plus `egress_guard.rs`'s seccomp-notify loopback interceptor and a DNS stub, both bootstrapped in-sandbox via a generated shell entrypoint (`bwrap_entrypoint.sh`). There is currently no backend that provides equivalent confinement without an external, non-vendorable binary dependency.
- Evidence: `~/Sources/openfirma-notes/notes/hakoniwa-backend-gap-analysis.md` (full API-surface and internals comparison, corrected after direct code reading — see its "Correction" section); this plan's own research (revision above) confirms every claim cited below against the actual `hakoniwa` 1.7.2 source and current `firma-run` code.

## Key decisions and tradeoffs

### `DEC-001`: `HakoniwaBackend::start_agent` spawns a new sibling launcher binary, not `hakoniwa::Command` in-process

- Choice: a new `[[bin]]`-only crate (placeholder name `firma-hakoniwa-runner`) depends on `hakoniwa` and does the actual `Container`/`Command::spawn()` call internally; `HakoniwaBackend::start_agent` (in `firma-run`) spawns _that binary_ via `std::process::Command::new(&runner).arg("--launch-contract").arg(&contract_path).spawn()`, mirroring `VzBackend::start_agent` (`macos_vz.rs:717-748`) exactly.
- Rationale and evidence: `SandboxBackend::start_agent`'s return type is fixed to `Result<std::process::Child, RunError>` (`backend/mod.rs:386-391`), a concrete standard-library type with no public constructor from a raw PID and no conversion from `hakoniwa::Child` (an unrelated concrete type `hakoniwa::Command::spawn()` returns, confirmed by reading `child.rs`/`command.rs` in full). The user has explicitly scoped this task to not modify `SandboxBackend`'s signatures, so widening the return type (e.g. to a trait object) is not available. `firma-vz-runner` (`crates/firma-vz-runner`, embeds `objc2-virtualization` directly, no external VM binary) already solves the identical problem for a different embedded library, via exactly this "sibling binary" shape.
- Consequences and rejected alternatives: rejected "call `hakoniwa::Command::spawn()` directly inside `firma-run`'s own `start_agent`" — does not type-check against the fixed trait signature, confirmed not inferred. Consequence: there is still a process hop (firma-run → runner binary → Hakoniwa's own internal double-fork), a similar shape to bwrap's own outer/inner-child relationship, not the simplification an earlier draft of the gap-analysis note incorrectly claimed (see that note's "Correction" section) — do not describe this backend as eliminating bwrap's outer/inner-PID complexity anywhere in docs or code comments.

### `DEC-002`: Network confinement uses a bare namespace unshare plus a minimal custom loopback bring-up — not `Pasta`, not `RustSlirp`

- Choice: the runner binary unshares `Namespace::Network` and performs a small, self-contained loopback-interface-up step (a handful of ioctls) itself, without calling `Container::network(...)` at all.
- Rationale and evidence: `hakoniwa::Network::Pasta` (the crate's documented default) shells out to an external `pasta` binary (`newnet/pasta.rs:59-85`, confirmed by reading the actual `Command::new(cmdline[0])` call) — reintroducing exactly the external-binary dependency this migration exists to remove. `RustSlirp` (feature-gated, off by default) avoids an external binary but sets up a real TUN device and userspace routing (`newnet/rustslirp.rs`) — heavier than firma-run needs, since the sandboxed process only needs to reach its own loopback (the proxy bridge, the DNS stub), never real egress. Confirmed by reading `runc.rs`'s actual setup sequence (not inferred from the builder API alone): omitting `.network(...)` entirely leaves `lo` present but **down** — `bring_up_loopback_interface` (`newnet/rustslirp.rs:168-190`) is the only code in the crate that brings it up, and it only runs inside the `rustslirp` path. So _some_ explicit step is mandatory regardless of which path is chosen; the minimal one is cheaper and has no feature/dependency footprint.
- Consequences and rejected alternatives: rejected `Pasta` (external binary) and `RustSlirp` (unnecessary TUN/routing machinery, and its own feature surface to audit) in favor of new, narrowly-scoped code isolated to the runner binary — small (a handful of ioctls, the same shape `bring_up_loopback_interface` already demonstrates), auditable in one place. **Added after plan review (`PLAN-005`)**: this ioctl code needs `unsafe`, same as hakoniwa's own equivalent. `firma-hakoniwa-runner`'s `Cargo.toml` needs `[lints.rust] unsafe_code = "allow"`, mirroring `crates/firma-vz-runner/Cargo.toml`'s existing identical carve-out for its own raw FFI — stated here explicitly rather than left as an implementation-time surprise.

### `DEC-003`: The runner binary reimplements the bootstrap _orchestration_ natively in Rust; the installer subcommands themselves are reused as-is

- Choice: the new runner binary, after Hakoniwa's own namespace/mount setup and before `execve`-ing the wrapped command, reimplements — in Rust, not shell — the _orchestration_ `bwrap_entrypoint.sh` performs today. It **reuses**, unchanged, the same subcommand binaries bwrap already invokes as subprocesses (`firma __dns-stub --listen ...`, `firma __egress-guarded-run -- <command>`) — these are already backend-agnostic, exported Rust functions (`egress_guard::install_and_exec` is `pub`, unconditional on backend) invoked as ordinary subprocesses, not shell logic to port.
- **What the orchestration must reproduce, corrected after plan review (`PLAN-001`)**: reading `crates/firma-run/src/resources/bwrap_entrypoint.sh` in full (not just its high-level shape) shows three concrete, currently load-bearing behaviors beyond "start subprocess, set env, exec," none of which may be silently dropped:
  1. A **readiness handshake**: poll a ready-file for up to 5s after starting the proxy bridge, fail closed if it never signals ready or dies during startup.
  2. A **background liveness watchdog** that fail-closed-terminates the wrapped command if the proxy bridge process dies _during_ the run, not only at startup.
  3. An explicit **`FIRMA_RUN_*` env-var strip** before the final exec — the script's own comment names the exact threat this defends against: a nested `firma run` inheriting `FIRMA_RUN_SANDBOX_ID` would derive the live session's runtime dir and could get it bind-mounted read-write into the inner sandbox.
- Rationale and evidence: `egress_guard`/the Linux DNS stub are confirmed backend-agnostic _in type_ (gated only on `NetworkPolicy.enforce_network_namespace`, not `BackendKind` — `routing.rs`'s dispatch chain has no `BackendKind` check in this path). The bwrap-specific part is only the _shell-level orchestration_ (readiness polling, the watchdog, env-stripping) — not the installers themselves, and not the DNS stub. Running the existing shell script from a Hakoniwa-embedding binary whose whole point is avoiding an external dependency would be inconsistent, so the orchestration is rewritten in Rust; the subprocess targets it orchestrates are reused unchanged.
- Consequences and rejected alternatives: rejected "shell `bwrap_entrypoint.sh` through `hakoniwa::Command`" (couples this backend to bwrap-specific script conventions it shouldn't need to understand). Rejected "reimplement `install_and_exec`/the DNS stub too" (redundant — they're already reusable as-is). Consequence: the orchestration logic (readiness handshake, watchdog, env-strip) is real new Rust code with no existing equivalent to call into; a shared, backend-agnostic orchestration helper both `BwrapBackend`'s script generator and this runner binary could use is a plausible future extraction but is out of scope here (additive-only).

### `DEC-004`: Do not reuse `seccomp.rs`'s managed-seccomp pipeline; use Hakoniwa's own seccomp/landlock builders

- Choice: `managed_seccomp_applies` (`config.rs:1014-1016`) stays `Bwrap`-exclusive by design — `HakoniwaBackend` does not consume the existing `SeccompPolicyConfig` TOML/BPF-artifact pipeline. Instead, the same underlying policy source (today, the `deny_actions` list; see `~/Sources/openfirma-notes/notes/seccomp-profile-and-hotswap.md`) is translated into `hakoniwa::seccomp::{Filter, Rule, Action}` and, where wanted, `hakoniwa::landlock::{Ruleset, FsRule, ...}` builder calls inside the runner binary.
- Rationale and evidence: `hakoniwa::seccomp::Action` already includes `Notify` and `Trace(u16)` (confirmed, `hakoniwa/src/seccomp/action.rs`) — a strictly more expressive declarative builder than firma's own static-only hand-rolled BPF compiler (`seccomp.rs`, 1,276 lines) or the pending `seccompiler`-based swap (PRs #415/#416). Reimplementing or forking that BPF-compiler effort for a backend that already ships an equivalent, better-featured builder would be wasted work.
- Consequences and rejected alternatives: rejected "port `seccomp.rs`'s BPF compiler output to Hakoniwa" (redundant given Hakoniwa's own builder). Consequence: a new, small translation layer (deny-actions list → `hakoniwa::seccomp::Rule`s) is needed — scoped to Slice 5, not required for Slices 1-4's structural-confinement proof.

### `DEC-005`: Reuse `NetworkConfinement::LinuxNetworkNamespace`, don't add a new enum variant

- Choice: `HakoniwaBackend::enforce_network` reports `NetworkConfinement::LinuxNetworkNamespace` (the same variant `BwrapBackend` uses), not a new `Hakoniwa`-specific variant.
- Rationale and evidence: both backends confine via the identical kernel primitive (a Linux network namespace) — the variant describes the _mechanism_, not the _backend_, and `NetworkConfinement` (`backend/mod.rs:28-42`) has no existing per-backend granularity for its other variants either (`MacosSandboxNetworkDeny`, `MacosVzGuest`, `KvmMicroVm` are all one-per-_mechanism_, and macOS's two modes already share none of this ambiguity since they're mechanically distinct).
- Consequences and rejected alternatives: rejected adding `NetworkConfinement::HakoniwaNetworkNamespace` — would be a distinction without a difference and would require touching every existing exhaustive match over this enum for no behavioral gain.

## Architecture and invariant ownership

- Architecture shape: `crates/firma-run/src/backend/hakoniwa.rs` (or a `linux_hakoniwa/` directory if it grows past a few hundred lines, mirroring `linux_bwrap/`) defines `HakoniwaBackend: SandboxBackend`. `prepare` mirrors `BwrapBackend::prepare`'s shape (host/preflight checks, runtime-dir creation, `SandboxMount` construction) but checks for kernel unprivileged-userns support instead of `command_available("bwrap")`. `enforce_network`/`verify_fail_closed` mirror `BwrapBackend`'s computational (not OS-verifying) shape, reusing `NetworkConfinement::LinuxNetworkNamespace` (`DEC-005`). `start_agent` serializes a launch contract (mirroring `PrepareRequest`/`LaunchSpec` plus resolved mounts, per `VzBackend`'s `--launch-contract` pattern) and spawns the new `firma-hakoniwa-runner` binary via `std::process::Command` (`DEC-001`), which internally builds a `hakoniwa::Container` from that contract, performs the loopback bring-up (`DEC-002`) and DNS-stub/egress-guard bootstrap (`DEC-003`) natively, then `execve`s the wrapped command. `teardown` mirrors `BwrapBackend`'s best-effort runtime-dir cleanup.

### `INV-001`: Structural network confinement — no route out except loopback, when `NetworkPolicy.enforce_network_namespace` is set

- Semantic predicate: for a `HakoniwaBackend`-launched sandbox with `enforce_network_namespace = true`, no process inside the sandbox can reach any address except loopback (and, through the DNS-stub/egress-guard/proxy-bridge chain, the Sidecar) — the same guarantee `BwrapBackend` already provides and the `network.rs`/`http.rs` e2e tests already prove for it.
- Primary owner: `HakoniwaBackend`/the runner binary's namespace+loopback-bringup+DNS-stub bootstrap sequence, independently of `BwrapBackend`'s equivalent.
- Detailed proof: see Appendix "Detailed proof obligations", `PROOF-001`.

### `INV-002`: `.firma`/config masking is at least as robust as `BwrapBackend`'s (symlink-swap and mount-alias-re-leak resistant)

- Semantic predicate: no path inside a `HakoniwaBackend` sandbox can expose `firma.toml`/`.firma/` contents, including via a symlink planted by the agent or via an operator mount whose source contains a masked path.
- Primary owner: the mount-translation layer inside `HakoniwaBackend`/the runner binary (Slice 2).
- Detailed proof: see Appendix "Detailed proof obligations", `PROOF-002`. **Gap**: `linux_bwrap/mount.rs`'s specific defenses (`reject_symlinked_firma_dirs`, `project_mount_aliases`) are bwrap-argument-ordering-specific; this invariant's Hakoniwa-side proof needs its own equivalent reasoning against Hakoniwa's mount-application order (confirmed lexicographic-by-target-path, `container.rs:452-456`, syscall-verified in `runc/unshare.rs`'s `initialize_rootfs` — the _private_ `unshare.rs` under `runc/`, not the public re-export module at `src/unshare.rs`, corrected after `PLAN-006`), not an assumption that "similar primitives ⟹ similar guarantee."

- Compatibility, migration, and failure semantics: `backend = "hakoniwa"` is new, additive, opt-in — no migration for existing configs. Unsupported-host selection fails closed at config-resolution time (mirrors `backend_supported_on_host`, `config.rs:725-733`), with a Hakoniwa-specific check (kernel unprivileged-userns support directly, not `command_available`, since there's no binary to find).
- Durable documentation owner: `docs-site/src/content/docs/concepts/sandbox.md` ("## The four backends" section becomes five, explicitly marked experimental per `DEC-010`); `docs/adr/FIR-60-sandbox-backend-selection-for-firma-run.md` gets a cross-reference, not a rewrite (its bwrap-specific rationale stands; this is a new, additional option).

### `DEC-010`: Ship marked experimental with an audit-completion sunset condition, not a fixed release count

- Choice: document `hakoniwa` as an experimental backend value from day one (mirroring `FIRMA_RUN_VZ_STRUCTURAL_NETWORK`'s precedent), with the stated condition for graduating to non-experimental being "a completed security audit of Hakoniwa's unprivileged-namespace/seccomp/landlock hardening" (per the pending.md item this plan fulfills), not a fixed number of releases.
- Rationale and evidence: `~/Sources/openfirma-notes/notes/hakoniwa-backend-gap-analysis.md` item 8 notes Hakoniwa is actively developed but far less battle-tested than bubblewrap (81 GitHub stars vs. bubblewrap's decade under Flatpak). A release-count sunset could lapse before that audit happens; an explicit audit-gate can't.
- Consequences and rejected alternatives: rejected a fixed "N releases" sunset (arbitrary, disconnected from the actual risk driver).

### `DEC-011`: Parametrize the existing `child_process_governance` e2e tests over `BackendKind`, don't fork them

- Choice: `network.rs`/`filesystem.rs`/`http.rs` under `tests/e2e/scenarios/child_process_governance/` gain a backend parameter (via the same `TestWorld`/`scaffold_config` seam extension already planned in `docs/architecture/selectable-execution-governance-plan.md`'s Slice 1, if that has landed, or a smaller Hakoniwa-specific equivalent otherwise) and run against both `Bwrap` and `Hakoniwa`.
- Rationale and evidence: these three tests assert exactly the guarantees this plan's `INV-001`/`INV-002` need proven; forking them into bwrap-only and Hakoniwa-only copies would let the two drift silently.
- Consequences and rejected alternatives: rejected duplicating the test files per backend (duplication risk); if the execution-governance plan's Slice 1 seam hasn't landed yet when this work starts, add the minimal version of that seam here instead of blocking on the other plan.

## Implementation slices

### Slice 1: `HakoniwaBackend` skeleton + minimal structural network confinement

- Production, types, tests, and docs/config: `BackendKind::Hakoniwa` variant plus every required exhaustive-match arm (`Display`, `FromStr`, `build_backend` in `backend/mod.rs` — **not** `default_for_current_host`, corrected after `PLAN-007`: that function is a `#[cfg(target_os = ...)]` cascade, not a match over `BackendKind`, and must never return `Hakoniwa` since this backend is never a platform default, per Scope; the parallel schema enum in `firma-config-schema/src/run.rs`; `BackendOverride`/`From` in `firma/src/args/run.rs`); new `firma-hakoniwa-runner` bin crate with a minimal `main` that unshares Mount/User/Pid/Network, does the loopback bring-up (`DEC-002`), and `execve`s the target with no mounts beyond a bare rootfs yet; `HakoniwaBackend::prepare`/`enforce_network`/`verify_fail_closed`/`teardown` per the architecture shape above; `backend_supports_structural_network` and `backend_supported_on_host` gain a `Hakoniwa` arm (kernel unprivileged-userns check, not `command_available`).
- Affected decisions and traces: `DEC-001`, `DEC-002`, `DEC-005`; `TRACE-001`.
- Proof obligations: `INV-001` (`PROOF-001`, network-only — no mount/DNS-stub claims yet).
- Focused verification: `tests/e2e/scenarios/hakoniwa_backend.rs`'s `hakoniwa_backend_blocks_network_like_bwrap` — a real `firma run --backend hakoniwa` invocation attempting a host-bound loopback connection is confirmed blocked (a control run of the same check unsandboxed succeeds first, proving the check itself works). Also manually verified directly against the runner binary during implementation: loopback comes up inside the sandbox (`ip addr show lo`), an external connect attempt fails with `ENETUNREACH` (genuine namespace isolation, not a permission error), and a same-netns loopback connect gets `ECONNREFUSED` (proving loopback routing itself works).
- Dependencies: none.
- Intentionally unsupported: no mounts beyond a bare rootfs, no DNS stub, no seccomp/landlock, no signal forwarding beyond direct single-PID kill. **Discovered during implementation**: `LaunchSpec.cwd` is always `firma run`'s own real working directory, which will not exist inside the bare-rootfs sandbox unless it happens to already be one of the mounted paths (e.g. `/`, `/tmp`) — a real `firma run --backend hakoniwa` invocation from an arbitrary directory will fail the wrapped command's `chdir` before it ever execs, until Slice 2 mounts the actual working directory. The e2e test added for this slice works around it by launching from `/tmp`; this is not yet a usable end state for real invocations.

### Slice 2: mount-plan translation

- Production, types, tests, and docs/config: translate `MountSpec`/`SandboxMount` (operator-provided, framework, sandbox-infrastructure authority classes) into `hakoniwa::Container::bindmount_ro`/`bindmount_rw`/`tmpfsmount`/`file`/`dir`/`symlink` calls inside the runner binary; reimplement `.firma`/config masking (`DEC` reference: `INV-002`) using Hakoniwa's lexicographic-by-target mount ordering (confirmed to produce correct overlay stacking, `container.rs:452-456` + the private `runc/unshare.rs`'s `initialize_rootfs`); reimplement `reject_symlinked_firma_dirs`-equivalent preflight and `project_mount_aliases`-equivalent re-projection.
- Affected decisions and traces: `INV-002`.
- Proof obligations: `INV-002` (`PROOF-002`).
- Focused verification: port the intent (not the literal bwrap-argument-order assertions) of `linux_bwrap/mount.rs`'s masking unit tests to the Hakoniwa translation layer; run `tests/e2e/scenarios/child_process_governance/filesystem.rs` against `Hakoniwa` per `DEC-011`.
- Dependencies: Slice 1.
- Intentionally unsupported: no DNS stub / egress guard yet (Slice 3); no CA trust injection port yet if it turns out to need its own handling beyond ordinary file mounts (verify during implementation).

**Implemented.** The mount plan is computed entirely in the trusted `firma-run` process (`crates/firma-run/src/backend/hakoniwa/mount.rs`, new sibling module to `linux_bwrap/mount.rs`, ~600 lines) as a flat `Vec<HakoniwaMountOp>` (`Bind{source,target,read_only}` / `Tmpfs{target}`), serialized into the launch contract; `firma-hakoniwa-runner` replays it verbatim with no masking/authority decisions of its own (`apply_mount_ops`). This is a deliberately separate, duplicated implementation of the masking logic (`validate_mounts`, `mask_firma_dir`, `project_mount_aliases`, `mask_control_plane_runtime`, `reject_symlinked_firma_dirs`, `is_strict_firma_subpath`, etc.) — not a shared refactor with bwrap, consistent with this plan's additive-only scope.

- **The mount-ordering equivalence gap flagged in `INV-002` is real, and needed a new safeguard, not just careful translation.** Confirmed by reading `container.rs`'s `get_mounts` (sorts by target path, a plain `String`, ascending) and `runc/unshare.rs`'s `initialize_rootfs` (applies mounts in exactly that order): a shallower target always mounts before a deeper one, so ordinary Linux mount-shadowing naturally reproduces bwrap's alias-masking guarantee (a mask's target, always computed as `overlay_target.join(relative)`, is by construction deeper than the overlay it must shadow). It does **not** reproduce bwrap's guarantee in the other direction: bwrap's phase-sequenced argument list makes every mask win over every overlay unconditionally, regardless of relative path depth; Hakoniwa's plain path-depth sort would let an overlay/framework mount whose own _target_ is placed inside a masked zone sort _after_ the shallower mask and reopen it. Fixed with a new, explicit validation with no bwrap equivalent — `reject_overlay_targets_inside_masked_zones` — which fails closed instead of depending on sort order for that direction. Exact-target collisions turned out to need no special handling at all: Hakoniwa stores mounts in a `HashMap` keyed by the literal target string, so two operations at the same target simply overwrite (the later `build_mount_ops` call wins), a stronger guarantee than bwrap's own insertion-order dependency for that case.
- **A second, unrelated implementation-time discovery**: binding a mount source that is the host's temp-directory root (`/tmp` in the common case) or a true ancestor of it breaks Hakoniwa's own internal sandbox setup. `Command::spawn_imp` creates its pivot-root staging directory via `tempfile::TempDir::with_prefix`, which resolves under the same `$TMPDIR`/`/tmp`; bind-mounting that root recursively re-exposes the staging directory inside itself, which reproducibly fails Hakoniwa's own cleanup (`rmdir` on its `.oldproc-*` staging path returns `EBUSY`) with no diagnostic surfaced by this runner (see the next finding). This is a Hakoniwa implementation detail, not a masking gap, and is _not_ a realistic production scenario (no real agent working directory is `/tmp` itself) — it was only reachable through Slice 1's now-obsolete `.current_dir("/tmp")` test workaround, which Slice 2 removes (see below). Still fixed with an explicit, fail-closed validation (`reject_mount_sources_containing_runner_staging_dir`) rather than left as a cryptic crash, since a user or a future test could hit it by accident.
- **A third finding, needed to make the masking machinery (specifically config-file masking via `/dev/null` read-only binds) work at all**: Hakoniwa's `remount_rdonly` step (which every bind mount goes through, to convert an initial `MS_BIND` into a read-only one via a second `MS_REMOUNT` syscall) fails with `EPERM` when the mount source lives on a filesystem with kernel-"locked" flags the remount doesn't repeat exactly — `/dev/null` is a common example, since `/dev` is typically mounted `nosuid`/`noexec`/`nodev`. Hakoniwa ships a purpose-built escape hatch for exactly this (`Runctl::MountFallback`, whose own doc comment names this scenario), which was simply never enabled. Fixed by calling `container.runctl(Runctl::MountFallback)` once in `firma-hakoniwa-runner`'s `run()`.
- **A fourth, process-hygiene finding**: `firma-hakoniwa-runner` was returning `status.code` on any Hakoniwa-internal setup failure (mount, unshare, ...) without ever inspecting `hakoniwa::ExitStatus::reason`, which Hakoniwa always populates with a human-readable description — including, critically, on setup failures, where it is the _only_ signal, since these bypass `run()`'s own `RunnerError` path entirely (Hakoniwa maps them to a fixed exit code, `125`, coincidentally the same numeric fallback this runner's own closure used, which cost real time during diagnosis). Fixed: `run()` now checks `status.exit_code` (`None` exactly when the wrapped command never got to run — a genuine setup failure, as opposed to `Some(code)` for its own normal exit or a stripped signal death) and prints `status.reason` in that case.
- **Slice 1's `.current_dir("/tmp")` e2e-test workaround is now removed** (`tests/e2e/scenarios/hakoniwa_backend.rs`, all three tests): Slice 2 makes an arbitrary working directory (the isolated test's real workspace) usable, which is exactly what that workaround was standing in for. Confirms Slice 1's "Discovered during implementation" limitation is now closed for real invocations, not just worked around in tests.
- `HakoniwaBackend::prepare` now threads `request.profile.mounts` through as operator-provided `SandboxMount`s (mirroring `BwrapBackend::prepare`'s first mount-population step only — not its identity-mode-driven passwd/group mounts or DNS-stub-driven resolv.conf mounts, both out of scope here per Slices 4/2 and 3 respectively).
- Focused verification (as implemented, superseding the line above): 8 unit tests in `mount.rs`'s own `#[cfg(test)] mod tests` (mirroring `linux_bwrap/mount.rs`'s existing in-module pattern for this same class of white-box masking logic — not integration tests, since these exercise private functions directly) porting `mask_firma_dir_masks_dir_without_recreating_file`, `mask_firma_dir_masks_all_ancestor_dirs`, and `mask_firma_dir_follows_symlink_swap_to_real_path`'s intents verbatim, plus new coverage for `project_mount_aliases`'s reachable-through-an-operator-mount case and both new validations (positive and negative cases each). The existing `hakoniwa_backend_blocks_network_like_bwrap` e2e test now exercises the real mount plan (workspace cwd) instead of the `/tmp` workaround, and Slice 5's two e2e tests were re-verified end to end against it. Manually smoke-tested directly against the runner binary during diagnosis of the three implementation-time findings above (isolating each with hand-written launch contracts) before the automated tests were written, matching Slices 1 and 5's verification rigor.
- Intentionally still unsupported: `tests/e2e/scenarios/child_process_governance/filesystem.rs` is not yet parametrized over `Hakoniwa` (`DEC-011`'s parametrize-don't-fork approach) — deferred to whichever slice actually needs it exercised (Slice 6 at the latest); no CA trust injection port (unchanged from the original scope note; still unverified whether it needs anything beyond ordinary file mounts).

### Slice 3: DNS-stub / egress-guard in-sandbox bootstrap

- Production, types, tests, and docs/config: native-Rust orchestration (`DEC-003`) inside the runner binary reproducing all three behaviors named there — start `firma __dns-stub`, the readiness handshake, the liveness watchdog, the `FIRMA_RUN_*` env-strip, and routing through `firma __egress-guarded-run` before the final `execve`. **Open design point flagged by plan review (`PLAN-003`), must be resolved during this slice, not deferred**: `egress_guard::SupervisorConfig.socket_path` must be reachable from inside the sandbox's mount namespace. Two ways to satisfy this exist and the implementer must pick one explicitly: (a) bind-mount the guard socket's directory into the sandbox as part of Slice 2's mount translation (making this slice depend on Slice 2 after all), or (b) connect to the guard socket via its real host path _before_ the runner unshares/pivots into the new mount namespace, then carry the already-connected fd across `execve` into the sandboxed process (available to a single Rust binary in a way bwrap's separate-process-after-mount-setup shell script isn't). Whichever is chosen, update this slice's "Dependencies" accordingly and record the choice as a new decision.
- Affected decisions and traces: `DEC-003`; `INV-001` (extends it to the loopback-bypass case, watchdog fail-closure, and env-leak prevention, not just "no external route").
- Proof obligations: `INV-001` (`PROOF-001`, extended — see below), plus new proof obligations for the watchdog (bridge dies mid-run → sandboxed command is terminated, not left running unconfined) and the env-strip (a nested `firma run` inside this sandbox cannot observe the outer session's `FIRMA_RUN_SANDBOX_ID`/runtime-dir).
- Focused verification: run `tests/e2e/scenarios/child_process_governance/network.rs`'s loopback-bypass scenario against `Hakoniwa` per `DEC-011`; new tests for the watchdog and env-strip behaviors, since no existing parametrizable test covers either today.
- Dependencies: Slice 1 (needs the network namespace and loopback up); dependency on Slice 2 is **undetermined** pending the socket-reachability design point above — do not assume independence.
- Intentionally unsupported: none beyond what `BwrapBackend`'s equivalent already leaves unsupported (e.g. AF_UNIX, per FIR-444 — orthogonal to this backend).

**Implemented.** `PLAN-003` resolved as option (a), already satisfied by Slice 2: `egress_guard::start`'s socket path is always `handle.runtime_dir.join("egress-guard.sock")`, and Slice 2's mount plan already bind-mounts `handle.runtime_dir` into the sandbox wholesale — no new mount-translation work was needed. `run_entrypoint_orchestration` in `firma-hakoniwa-runner` reproduces the entrypoint script's sequence (DNS-stub best-effort start, fail-closed proxy-bridge start with a readiness-file handshake, `FIRMA_RUN_*` env-strip, then `firma __egress-guarded-run` or a direct exec), reusing the same `firma __dns-stub`/`firma __proxy-bridge`/`firma __egress-guarded-run` subcommand binaries `BwrapBackend` already invokes, unchanged.

Three implementation-time findings, each with production-shaping consequences (all corrected, not just noted):

- **`FIRMA_RUN_SELF_EXE`'s path is not reachable inside a minimal Hakoniwa rootfs.** Unlike bwrap's default `--bind / /`, `Container::rootfs("/")` only binds OS-standard directories — a dev build's `target/debug/firma` (or any non-standard install prefix) has no route into the sandbox at all, so the orchestration's own `firma __dns-stub`/`__proxy-bridge`/`__egress-guarded-run` execs would fail outright. Fixed: `HakoniwaBackend::start_agent` bind-mounts `FIRMA_RUN_SELF_EXE`'s host path at a fixed in-sandbox path (`/run/firma-hakoniwa/firma`) and rewrites the contract's own `FIRMA_RUN_SELF_EXE` to match, so the orchestration code needs no awareness of the original host path at all.
- **Self-mounting a binary at its own original path is unsafe when that path falls inside another active mount — confirmed via a real `ETXTBSY`.** An earlier version of the fix above bound `firma`/`firma-hakoniwa-runner` at their _own_ host paths (mirroring the sandbox-runtime self-mount used elsewhere). This breaks whenever the binary's path is itself inside `$HOME` or the cwd (true for any dev build under a home directory): the HOME/cwd bind, being shallower, sorts and applies first, already exposing that path, and Hakoniwa's own bind-mount setup then tries to `touch()` the same live, currently-executing file a second time — which the kernel refuses with `ETXTBSY`. This is why the fix above uses a fixed, dedicated path under `/run` instead of a self-mount: nothing else in the mount plan ever targets it, so it can never collide with an existing mount this way.
- **A namespace's PID 1 cannot be terminated by any signal — `SIGKILL` included — from a process inside the same namespace; the watchdog has to run on the host side.** The design as planned spawned the bridge-death watchdog as a child of the process that becomes the wrapped command (necessary, it was assumed, because a thread does not survive the eventual `exec`, so a real subprocess seemed required either way). That in-sandbox watchdog reliably detected the bridge dying but its `kill()` had no effect on the wrapped command, under `SIGTERM` _or_ `SIGKILL` — confirmed directly with a minimal, from-scratch reproduction outside Hakoniwa entirely (`unshare --user --map-root-user --pid --fork`, then `kill -9 1` from a backgrounded sibling process: PID 1 survives). Hakoniwa's `Container` unshares a PID namespace, making the wrapped command PID 1 within it, and the kernel's namespace-init protection is not signal-specific — it blocks every signal from a same-namespace sender, not just ones without a registered handler. The fix moves the watchdog entirely: it is now a thread in `firma-hakoniwa-runner`'s own `run` function (which never itself enters the new PID namespace — only the later-forked "internal process" Hakoniwa creates does), spawned right after `hakoniwa::Command::spawn()` returns a `Child` exposing the sandbox's host-visible pid. The thread discovers the bridge's own host-visible pid by walking `/proc` for a descendant of that pid whose `cmdline` matches `__proxy-bridge`, then polls its liveness and sends `SIGKILL` to the sandbox's own host-visible pid the moment it is gone — which works precisely because the sender is in an ancestor namespace. This also simplified the design: no self-re-exec subcommand, no `runner_path` bind-mount, no Landlock allow-listing for this runner binary's own path — all needed only by the abandoned in-sandbox design.
- Focused verification (as implemented, superseding the line above): a new e2e test, `hakoniwa_backend_watchdog_kills_wrapped_command_when_bridge_dies`, runs a real `firma run --backend hakoniwa` with a long-running wrapped command, locates the proxy bridge among the run's own descendants (scoped that way — not a system-wide `/proc` scan — specifically because nextest runs this file's tests in parallel and an unscoped scan nondeterministically found and killed a _different_, concurrently-running test's bridge during development), kills it, and asserts the wrapped command is terminated within 15s rather than completing its full run; the same test also asserts zero `FIRMA_RUN_*` variables reach the wrapped command's environment, covering the env-strip proof obligation in the same run. `hakoniwa_backend_blocks_network_like_bwrap` (Slice 1's test) continues to pass unchanged with the orchestration now live end-to-end (DNS-stub and proxy-bridge both actually start under it), and was used interactively during development (via direct `firma-hakoniwa-runner` invocations with hand-written launch contracts, and via `firma run` directly) to confirm the DNS stub and proxy bridge are independently reachable from inside the sandbox and that the env-strip removes a canary `FIRMA_RUN_SANDBOX_ID` before automating the coverage above.
- Intentionally unsupported: `tests/e2e/scenarios/child_process_governance/network.rs`'s loopback-bypass scenario is not yet parametrized over `Hakoniwa` (`DEC-011`) — deferred, as in Slice 2, to whichever slice actually needs it (Slice 6 at the latest).

### Slice 4: signal forwarding and process-tree teardown

- Production, types, tests, and docs/config: extend `supervisor.rs::forward_signal` with a `BackendKind::Hakoniwa` arm if the runner binary uses a new session (job-control parity with bwrap's `--new-session`) — likely reusable via the same `/proc`-children-walk approach `sandbox_child_pid`/`parse_first_pid` already implement (already `pub`, not `pub(crate)`, per Slice 0 of the execution-governance plan, specifically so other in-crate consumers could reuse them) rather than writing a second PID-discovery mechanism from scratch.
- Affected decisions and traces: none new — reuses existing `pub` helpers.
- Proof obligations: none new; existing signal-forwarding test shape (`supervisor.rs`'s test module) extended with a `Hakoniwa` case if the reused helpers need it.
- Focused verification: SIGINT/SIGTERM/SIGWINCH forwarding reaches the whole sandboxed process group under `Hakoniwa`, matching `Bwrap`'s existing coverage.
- Dependencies: Slice 1.
- Intentionally unsupported: none identified.

**Implemented.** No `NewSession` runctl was added — confirmed via direct process-tree inspection of a real run that `hakoniwa::Container` never calls `setsid()` on its own, so bwrap's single `kill(-pgid)` approach has no session boundary to exploit here; every process in a Hakoniwa sandbox's tree shares `firma-run`'s own process group by default. `forward_signal`'s new `BackendKind::Hakoniwa` arm instead does an explicit `/proc` walk and signals every discovered pid individually — reaching the wrapped command and any DNS-stub/proxy-bridge orchestration siblings the same way bwrap's group-signal does, without needing a session boundary at all.

Reusing `sandbox_child_pid`/`parse_first_pid` turned out to need one adjustment the plan's "likely reusable... rather than writing a second PID-discovery mechanism" framing didn't anticipate: confirmed by direct process-tree inspection of a real run, `child_pid` (the spawned `firma-hakoniwa-runner` process) is _two_ hops away from the real sandbox root, not one — `hakoniwa::Command::spawn()` forks once internally to run its own setup/reap supervisor (the pid `sandbox_child_pid` alone would return), which forks again to create the process that unshares the PID namespace and ultimately `exec`s into the wrapped command. `sandbox_child_pid` itself needed no changes; a new `hakoniwa_sandbox_root_pid` calls it twice to land on the real root, then a new `hakoniwa_descendant_pids` (a full recursive `/proc/*/stat` walk, since the sandbox root can have multiple children — the wrapped command plus DNS-stub/proxy-bridge — where `sandbox_child_pid`'s single-child lookup would only find one of them) enumerates everything under it for `forward_signal` to signal.

- Focused verification (as implemented, superseding the line above): two new tests in `supervisor.rs`'s own test module (mirroring its existing `sandbox_child_pid_reads_proc_children` style, not a new e2e test, matching the plan's own suggested location) — `hakoniwa_sandbox_root_pid_skips_two_supervisor_levels` (a two-level nested shell process tree, tolerant of `None` since `CONFIG_PROC_CHILDREN` isn't universal, matching the existing bwrap test's own tolerance) and `forward_signal_hakoniwa_reaches_every_sandbox_root_descendant` (a three-level nested tree whose bottom level has two sibling children standing in for the wrapped command and an orchestration process; asserts both are actually killed — confirmed via `ESRCH` after the forwarded `SIGKILL` — not just discovered). Also manually verified end to end against a real `firma run --backend hakoniwa` invocation: a genuine `SIGTERM` sent to the outer `firma run` process correctly terminated the entire sandboxed process tree (visible via `ps --forest` before/after) with no orphaned processes and clean stack teardown, matching bwrap's existing behavior for the same scenario.
- Intentionally unsupported: none beyond what is already listed above.

### Slice 5: seccomp/landlock builder wiring

- Production, types, tests, and docs/config: translate the existing `deny_actions` policy source into `hakoniwa::seccomp::{Filter, Rule, Action}` calls in the runner binary (`DEC-004`); optionally wire `hakoniwa::landlock` if/when a Landlock strategy is wanted here (cross-reference, don't duplicate, `docs/architecture/selectable-execution-governance-plan.md`'s `LandlockExecute` — decide at implementation time whether this backend's own Landlock use and that plan's strategy converge or stay separate; record whichever is chosen).
- Affected decisions and traces: `DEC-004`.
- Proof obligations: new — the deny-actions-to-Hakoniwa-builder translation needs the same kind of CI verification test `openfirma-notes/todo/pending.md` already recommends for the bwrap side (assert the translated ruleset matches the policy source), not assumed-correct by construction.
- Focused verification: the existing `credential.write`-denial behavior (whatever test currently proves this for bwrap) reproduced under `Hakoniwa`.
- Dependencies: Slice 1.
- Intentionally unsupported: none identified; this is the slice most likely to reveal new gaps once attempted, per the "not yet audited" caveat in `DEC-010`.

**Implemented.** Decisions made at implementation time, per the deferrals above:

- **Seccomp**: `crates/firma-run/src/seccomp.rs` gained `resolve_deny_syscall_names(&ResolvedProfile) -> Result<Option<Vec<&'static str>>, RunError>`, sharing `resolve_effective_seccomp`'s policy parsing/validation but stopping short of BPF compilation — it returns syscall names directly. `HakoniwaBackend`'s `LaunchSpec` gained a `deny_syscalls: Option<Vec<String>>` field, resolved once in `runtime::execute_run` alongside the existing `seccomp_filter_path` (both backends' seccomp inputs are resolved unconditionally; only the active backend consumes its own). `firma-hakoniwa-runner` builds `Filter::new(Action::Allow)` with one `add_rule(Action::Errno(libc::EPERM), name)` per denied syscall — a denylist, matching the bwrap backend's own `deny_actions` semantics and `EPERM_ERRNO` exactly, not Hakoniwa's own allowlist-style examples.
- **Config gate relaxed**: `config.rs`'s profile validation rejected `seccomp_policy` for any backend but `bwrap` outright (`"seccomp_policy is only supported with backend 'bwrap'"`). This is a real, previously-unnoticed blocker: without relaxing it, `resolve_deny_syscall_names` could never observe a configured policy for `backend = "hakoniwa"` — the launch contract would always carry `deny_syscalls: None`, silently. Relaxed to accept `Bwrap | Hakoniwa`, which is squarely inside `DEC-004`'s already-reviewed intent (reuse the same policy source through a different mechanism), not a new trust-boundary decision — no new plan review triggered.
- **Landlock — allowed_executables threading**: `LaunchSpec` gained `allowed_executables: Vec<PathBuf>`, populated in `runtime::execute_run` from `profile.sidecar_local_exec.allowed_executables` **only when `enforce_known_executables` is set** (mirrors the existing root-process gate's own precondition — an operator who configured the allow-list but left enforcement off should not get a surprise stricter sandbox). `HakoniwaBackend::start_agent` threads it into the launch contract unchanged; `firma-hakoniwa-runner` only calls `container.landlock_ruleset(...)` when the list is non-empty (an empty list would restrict `Resource::FS` with zero execute grants anywhere, bricking the sandbox — same class of gate `selectable-execution-governance-plan.md`'s PLAN-001 required for `LandlockExecute`).
- **Landlock — the "broad read, narrow execute" design needed a second correction.** The design recorded before implementation (broad `FsAccess::R` on rootfs dirs, `FsAccess::R | FsAccess::X` narrowly on `allowed_executables` entries) was _insufficient_, discovered empirically against this backend's actual `command_from_closure` exec path: granting only read on `/usr`, `/lib*` etc. made **every** dynamically-linked binary fail `execve` with `EACCES`, including ones on the allow-list. Root cause, confirmed by isolated reproduction against the `hakoniwa` crate directly: the kernel's ELF loader checks Landlock's execute right not only on the `execve`d file itself but also on its ELF interpreter (`PT_INTERP`) and on every shared library the dynamic linker subsequently `mmap`s with `PROT_EXEC` — plain read access does not cover that. The corrected design splits the broad grants into two tiers: `LANDLOCK_READ_ONLY_DIRS = ["/bin", "/sbin", "/etc", "/dev", "/usr"]` (read-only — granting execute here would let every binary in the tree run, defeating the allow-list, since Landlock's rules apply recursively) and `LANDLOCK_LIBRARY_DIRS = ["/lib", "/lib64", "/lib32", "/usr/lib", "/usr/lib64", "/usr/lib32"]` (read **and** execute — these hold only libraries, not user-invocable commands, so broad execute there does not undermine the allow-list). A more specific rule on a path already covered by a broader one adds to it rather than replacing it (Landlock unions matching rules; confirmed by reading `hakoniwa`'s `Ruleset::allow_path`, which keys by literal path string, and `runc/landlock.rs`'s `add_rules_fs`, which issues one `path_beneath` rule per stored entry), so a merged-`/usr` host's `/bin → usr/bin` symlink still resolves to the read-only tier and `/lib → usr/lib` to the read+execute tier, without conflict.
- **Verified property**: with this correction, a descendant process the wrapped command spawns (not just the root command itself) is denied `execve` on any binary outside `allowed_executables`, even one living in an otherwise-broadly-readable system directory (e.g. `/bin/true` when only `/bin/bash` is allow-listed) — the property `Inherited` governance's root-only `sidecar_local_exec` check cannot provide, and the one FIR-366 is ultimately about.
- Manual smoke tests ran directly against the compiled `firma-hakoniwa-runner` binary with hand-written launch contracts before the e2e suite was extended (matching Slice 1's verification rigor): seccomp deny confirmed via `unlink`/`unlinkat` under `EPERM`; Landlock confirmed via an allow-listed executable succeeding, a non-allow-listed one in the same broadly-readable directory failing with `EACCES`, and the two mechanisms combined in one contract without interaction.
- Focused verification (as implemented, superseding the line above): two new e2e tests in `tests/e2e/scenarios/hakoniwa_backend.rs` — `hakoniwa_backend_denies_filesystem_delete_via_seccomp` (control run deletes its own file; hakoniwa-backed run with a `[run.profiles.generic.seccomp_policy]` denying `filesystem.delete` does not) and `hakoniwa_backend_restricts_descendant_exec_via_landlock` (hakoniwa-backed run with `sidecar_local_exec.allowed_executables = [bash]` still fails to run `/bin/true` from inside the wrapped `bash`). The Landlock test stands up a minimal allow-all Unix-socket governance endpoint, duplicated locally rather than reusing `child_process_governance::support`'s private helpers (different scenario module, different backend anchor).
- Intentionally still unsupported: no `ldd`-style transitive dependency scoping — the allow-list gates the top-level `execve` target, not a minimized per-executable library closure; a binary living inside `LANDLOCK_LIBRARY_DIRS` itself (unusual, but possible for some packages) would be executable regardless of the allow-list. Not treated as a blocking gap for this slice, since `mediator.allowed_executables` in practice names commands under `/bin`, `/usr/bin`, `/sbin`, `/usr/sbin` (the read-only tier), not library directories — recorded here so Slice 6's parity/audit work does not have to rediscover it.

### Slice 6: full parity proof, `firma doctor` support, and documentation

- Production, types, tests, and docs/config: `firma doctor`'s `Backend::Hakoniwa` arm and its own probe (kernel unprivileged-userns capability check, not `--version`, per `crates/firma/src/doctor/sandbox.rs`'s existing per-backend `Prober` shape); `docs-site/src/content/docs/concepts/sandbox.md` updated per `DEC-010`; the full `child_process_governance` suite (`DEC-011`) green against `Hakoniwa`; a benchmark comparing `Hakoniwa` vs. `Bwrap` launch/exec overhead.
- Affected decisions and traces: `DEC-010`, `DEC-011`.
- Proof obligations: closes the "audit... before it could be trusted at parity" gap from the gap-analysis note, to the extent achievable by this plan (a real security audit against known escape techniques is explicitly **not** claimed as complete by this slice — see Risks).
- Focused verification: full e2e suite; benchmark reproducibility.
- Dependencies: Slices 1-5.
- Intentionally unsupported: this slice does not itself constitute the security audit `DEC-010`'s sunset condition requires — it proves functional parity, not adversarial robustness.

## Risks and gaps

- Existing risks (now implemented — see Slices 2 and 5 for outcomes): seccomp/landlock translation (Slice 5) and mount-masking equivalence (Slice 2, `INV-002`) were the highest-uncertainty pieces, since both required re-deriving security-relevant behavior against a different underlying primitive set, not just API translation. Both surfaced real, previously-unidentified gaps in the initial design (Slice 5's ELF-interpreter/shared-library execute requirement; Slice 2's mount-ordering-direction asymmetry) that were fixed during implementation, not merely confirmed safe — see each slice's "Implemented" notes.
- Planned mitigations: fail-closed compatibility gate at config-resolution (Slice 1); explicit experimental marking with an audit-completion sunset, not a time-boxed one (`DEC-010`); parametrized (not forked) e2e coverage so bwrap and Hakoniwa can't silently drift apart (`DEC-011`).
- Explicit evidence gaps: no independent security audit of `hakoniwa` itself exists anywhere consulted in this research — this plan produces a _functionally equivalent_ backend, not a _security-audited_ one; that gap is named, not closed, by Slice 6.
- Least-confident decisions: `DEC-003`'s claim that a native-Rust bootstrap reimplementation is straightforward is not yet verified against Hakoniwa's exact process-startup ordering (the `traceme()+SIGSTOP` hook point found in `runc.rs` step 7 during research may or may not be usable/necessary here — flagged as an implementation-time detail, not resolved by this plan).
- **Flagged by plan review, now resolved by implementation**: Hakoniwa's own `runc.rs::spawn()` sequence installs `landlock`/`seccomp` (Slice 5) _before_ its final `execve`, confirmed to also run before the closure that hosts Slice 3's DNS-stub/proxy-bridge/`egress-guarded-run` subprocess launches — meaning that orchestration's own `execve`s are subject to whatever seccomp/Landlock policy is active. This is a real dependency, but not a blocking one in practice: no shipped `deny_actions` profile denies `system.execute` (the only mapped action that would affect `fork`/`execve` at all), and `HakoniwaBackend::start_agent` already Landlock-allow-lists `FIRMA_RUN_SELF_EXE`'s fixed in-sandbox path whenever Landlock is active, specifically so the orchestration's own execs are never blocked by an operator's exec allow-list (see Slice 3's own "Implemented" notes). Verified directly: a manual launch contract combining a non-empty `allowed_executables` (activating Landlock) with a configured proxy bridge successfully started both the bridge and (via the then-current in-sandbox design, since superseded) its watchdog, while a non-allow-listed binary invoked by the wrapped command was still correctly denied.

## Plan-review findings and dispositions

Independent review performed by a fresh reviewer with no prior context on this plan, per this
repository's `adversarial-review` → `reviewing-plans` process. Reviewer confirmed the working tree
had moved two commits past the researched revision (`5e0dd567` vs. `9d761b2b36afa69c32eb1a5cc66e8b9ba45dc34a`)
but re-verified the plan's `supervisor.rs` citations against current `HEAD` and found them still
accurate — noted, not raised as a finding. Reviewer independently traced `DEC-001` and `DEC-002`'s
central claims through both codebases and confirmed both correct, and confirmed the additive-only
scope boundary was maintained throughout (no slice silently modifies `BwrapBackend` or `SandboxBackend`).

```yaml
id: PLAN-001
severity: medium
category: security
classification: confirmed-conflict
claim: >
  DEC-003 omits three concrete, currently load-bearing security behaviors of the bootstrap it
  proposes to port: a readiness handshake (poll a ready-file up to 5s, fail closed on timeout/death),
  a background liveness watchdog (fail-closed-terminate the wrapped command if the proxy bridge dies
  mid-run), and an explicit FIRMA_RUN_* env-var strip defending against a nested-run privilege-
  escalation path the script's own comment names.
evidence:
  - crates/firma-run/src/resources/bwrap_entrypoint.sh (readiness poll loop, watch_bridge_and_parent, env-strip comment)
reachability: a native reimplementation following DEC-003's original abbreviated framing ("start
  subprocess, set env, exec") would silently drop all three
invariant_or_boundary: INV-001
impact: bridge-crash-mid-run leaves the sandbox unconfined-but-running; nested-run env inheritance
  reopens a named privilege-escalation path
correction: enumerate all three behaviors in DEC-003 explicitly; add proof obligations for the
  watchdog and env-strip cases
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted. DEC-003 rewritten to enumerate all three behaviors explicitly with citations;
    PROOF-001 extended with watchdog and env-strip stimuli/observable effects/failure cases; Slice 3
    updated to require new tests for both.
  incorporated_at: DEC-003, PROOF-001, Slice 3
  decided_by: planner
```

```yaml
id: PLAN-002
severity: medium
category: design risk / reuse characterization
classification: confirmed-conflict
claim: >
  DEC-003 mischaracterizes egress_guard.rs's install_and_exec as bwrap-specific shell logic needing
  reimplementation, when it is already a pub, backend-agnostic Rust function invoked as an ordinary
  subprocess — the genuinely bwrap-specific part is only the shell-level orchestration (readiness
  polling, watchdog, env-stripping), not the installer or the DNS stub.
evidence:
  - crates/firma-run/src/egress_guard.rs (install_and_exec doc comment and signature, pub, unconditional on backend)
reachability: as originally worded, an implementer could over-port (reimplementing already-reusable
  installer logic) or under-port (missing that only orchestration needs rewriting)
invariant_or_boundary: DEC-003's own scope statement
impact: wasted implementation effort or an incomplete port, depending on which direction the
  ambiguity resolves
correction: clarify which layer is reused (installer subcommands, invoked as subprocesses exactly as
  bwrap does today) versus rebuilt (shell orchestration, in Rust)
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted. DEC-003's "Choice" and "Consequences" text rewritten to state explicitly that the
    installer subcommands (firma __dns-stub, firma __egress-guarded-run) are reused unchanged as
    subprocess targets; only the orchestration around them is new Rust code.
  incorporated_at: DEC-003
  decided_by: planner
```

```yaml
id: PLAN-003
severity: medium
category: design risk / sequencing
classification: design-risk
claim: >
  Slice 3's "does not depend on Slice 2" claim is unverified and likely false: egress_guard's
  SupervisorConfig.socket_path must be reachable from inside the sandbox's mount namespace
  (documented as "must be reachable from inside the sandbox (bind-mounted)"), which is Slice 2's
  territory, unless a connect-before-namespace-entry design (carrying the fd across execve) is used
  instead — a decision the plan never states.
evidence:
  - crates/firma-run/src/egress_guard.rs (SupervisorConfig.socket_path doc comment)
  - Slice 1's "no mounts beyond a bare rootfs" and Slice 2's mount-translation scope
reachability: Slice 3 implemented as originally scoped (independent of Slice 2, no fd-carrying design
  stated) would have no way to reach the guard socket
invariant_or_boundary: Slice dependency graph / INV-001
impact: either the independence claim is wrong (rework needed once discovered) or a load-bearing
  design decision is silently missing
correction: state explicitly whether socket reachability is via connect-before-namespace-entry or a
  Slice-2-dependent bind mount, and fix the dependency graph accordingly
confidence: medium-high
assumptions:
  - the connect-before-namespace-entry resolution is the reviewer's hypothesis, not confirmed in
    either repository
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted as an open design point requiring explicit resolution during implementation rather than
    a silent assumption. Slice 3 rewritten to state both candidate designs, require the implementer
    to choose and record one, and mark the Slice 2 dependency as undetermined rather than false.
  incorporated_at: Slice 3
  decided_by: planner
```

```yaml
id: PLAN-004
severity: medium
category: compliance / evidence gap
classification: unverified-hypothesis
claim: >
  hakoniwa's own license (LGPL-3.0-only WITH LGPL-3.0-linking-exception, read directly from its
  Cargo.toml) is never surfaced or reasoned about, despite this plan citing ADR FIR-60's LGPL-driven
  rationale for NOT statically embedding bwrap as directly relevant background — the plan proposes
  exactly that kind of static embedding for hakoniwa without explaining why its linking exception
  resolves the analogous concern.
evidence:
  - hakoniwa/Cargo.toml (license field)
  - docs/adr/FIR-60-sandbox-backend-selection-for-firma-run.md ("Do not statically embed bubblewrap")
reachability: not a runtime defect; a documentation/compliance-reasoning gap in a plan that otherwise
  treats licensing as decision-relevant
invariant_or_boundary: compliance review completeness
impact: a reader following this plan's own citation of FIR-60 would reasonably expect the analogous
  question answered for hakoniwa and find it silent
correction: state hakoniwa's license and why its linking exception resolves the concern FIR-60 raised
  for bwrap specifically
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted — this exact analysis already existed from a prior session
    (openfirma-notes/notes/hakoniwa-license-verification.md) and should have been cited here
    directly; added as an explicit Assumptions entry.
  incorporated_at: Scope (Assumptions)
  decided_by: planner
```

```yaml
id: PLAN-005
severity: low
category: constructibility / lint policy
classification: design-risk
claim: >
  DEC-002's ioctl-based loopback bring-up needs unsafe, matching hakoniwa's own equivalent
  (bring_up_loopback_interface, which is pub(crate) inside hakoniwa and not reachable externally, so
  new code is genuinely required) — but the plan never states that firma-hakoniwa-runner needs an
  unsafe_code lint carve-out, unlike the directly analogous firma-vz-runner precedent which states
  unsafe_code = "allow" explicitly in its Cargo.toml.
evidence:
  - crates/firma-vz-runner/Cargo.toml ([lints.rust] unsafe_code = "allow")
  - hakoniwa/src/unshare/newnet/rustslirp.rs:168-190 (bring_up_loopback_interface, unsafe ioctls)
reachability: implementation-time surprise when the new crate fails to compile under the workspace's
  default unsafe_code posture
invariant_or_boundary: lint policy consistency with existing precedent
impact: minor — a compile-time speed bump, not a security or correctness issue
correction: state the lint carve-out explicitly, mirroring firma-vz-runner
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Accepted. Added to DEC-002's consequences.
  incorporated_at: DEC-002
  decided_by: planner
```

```yaml
id: PLAN-006
severity: low
category: citation precision
classification: confirmed-conflict
claim: >
  "unshare.rs's initialize_rootfs" is ambiguous/wrong as literally cited — hakoniwa has two files
  named unshare.rs (public src/unshare.rs, a re-export module with no initialize_rootfs; private
  runc/unshare.rs, which actually contains it at line 98). The underlying mount-ordering claim is
  independently verified correct; this is a path-precision defect only.
evidence:
  - hakoniwa/src/unshare.rs (re-exports only)
  - hakoniwa/src/runc/unshare.rs:98 (initialize_rootfs)
reachability: a reader following the citation literally into the public module won't find the function
invariant_or_boundary: n/a (documentation precision)
impact: minor — wastes a reader's time, does not affect the plan's correctness
correction: disambiguate to "the private runc/unshare.rs"
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Accepted. Both citations in INV-002 and Slice 2 disambiguated.
  incorporated_at: INV-002, Slice 2
  decided_by: planner
```

```yaml
id: PLAN-007
severity: low
category: factual accuracy
classification: confirmed-conflict
claim: >
  Slice 1's file-tree list incorrectly includes default_for_current_host as needing a new match arm.
  That function is a #[cfg(target_os = ...)] cascade, not a match over BackendKind, and must NOT gain
  a Hakoniwa arm since this backend is never a platform default per the plan's own Scope.
evidence:
  - crates/firma-run/src/backend/mod.rs:89-115 (default_for_current_host, cfg cascade)
reachability: could mislead an implementer into adding an arm that contradicts the plan's own
  "never a platform default" scope statement
invariant_or_boundary: Scope (non-goal: hakoniwa is never a platform default)
impact: minor if caught during implementation; would be a real scope violation if not caught
correction: remove default_for_current_host from the required-edit-site list, state explicitly it
  must not gain a Hakoniwa arm
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Accepted. Slice 1 and the file-tree diff both corrected.
  incorporated_at: Slice 1, File-tree diff
  decided_by: planner
```

```yaml
id: PLAN-008
severity: low
category: nit / factual accuracy
classification: confirmed-conflict
claim: "linux_bwrap/mount.rs has 14 #[test] functions, not 15+."
evidence:
  - crates/firma-run/src/backend/linux_bwrap/mount.rs (direct count)
reachability: n/a
invariant_or_boundary: n/a
impact: negligible
correction: "15+ -> 14"
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Accepted, trivial fix.
  incorporated_at: Scope (Assumptions), PROOF-002
  decided_by: planner
```

```yaml
id: PLAN-009
severity: low
category: proof-scope precision
classification: confirmed-conflict
claim: >
  PROOF-003's stimulus is framed as "any SandboxBackend method" called with a mismatched
  SandboxHandle, but CW-001's remediation text only proposed guarding start_agent — the other three
  handle-consuming methods (enforce_network, verify_fail_closed, teardown) are equally exposed under
  the same looseness and PROOF-003 would overclaim what the design actually closes if only one method
  were guarded.
evidence:
  - crates/firma-run/src/backend/mod.rs (SandboxBackend trait, all four methods take &SandboxHandle)
reachability: a reviewer checking PROOF-003 against CW-001's stated remedy would find a scope mismatch
invariant_or_boundary: CW-001 / PROOF-003 consistency
impact: minor — would under-implement the defensive guard if left unresolved
correction: broaden the guard to all four handle-consuming methods, or narrow PROOF-003's stated scope
  to match
confidence: medium
assumptions:
  - whether an implementer would in fact guard only start_agent is inference from the plan's original
    prose emphasis, not a stated restriction
```

```yaml
disposition:
  status: corrected
  rationale: Accepted the broader guard (all four methods) as the more defensible choice, since the
    looseness applies equally to all of them and the cost of guarding all four is negligible.
  incorporated_at: CW-001
  decided_by: planner
```

All nine findings are corrected in this artifact. The reviewer's "Residual uncertainty" note (a
possible ordering interaction between Slice 3's subprocess orchestration and Slice 5's seccomp/
landlock installation, not raised as a numbered finding since the reviewer lacked enough evidence to
state a concrete conflict) is recorded above under "Risks and gaps" as an unresolved, flagged item
rather than a disposed finding, since the reviewer explicitly did not assert a conflict — only that
it needs checking during implementation.

## Final verification

- Focused checks: per-slice verification as listed above.
- Workspace checks: `just check` after each slice.
- Post-implementation independent review: required per `adversarial-review` on the actual implemented change, in addition to this plan review.

---

## Technical evidence

### Applicability assessment

| Section                     | Applicability | Reason or evidence                                                                                                                                      |
| --------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Vocabulary                  | Applicable    | New/overloaded terms: "structural" vs. "proxy-only," "runner binary," the mount-authority classes.                                                      |
| Alternatives                | Applicable    | Network bring-up (`Pasta` vs. `RustSlirp` vs. minimal custom), seccomp/landlock convergence, mount-translation approach all have material alternatives. |
| File-tree diff              | Applicable    | New crate, new backend module, ~10 modified match/gate sites across 6 files.                                                                            |
| Type and signature sketches | Applicable    | `HakoniwaBackend`'s shape and the launch-contract serialization boundary need recording; a real constructibility risk exists.                           |
| Semantic call traces        | Applicable    | Behavior crosses a new process boundary (the runner binary) and multiple trust-relevant stages.                                                         |
| Trust analysis              | Applicable    | New sandbox backend — squarely a security-boundary change.                                                                                              |
| Detailed proof obligations  | Applicable    | `INV-001`/`INV-002` need evidence across e2e suites and mount-masking reasoning.                                                                        |

### Conditional: Vocabulary

| Canonical term           | Meaning                                                                                                                                     | Owner/context                                           | Synonyms or terms to avoid                        | Conflict or decision                                                                                  |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------- | ------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Structural (confinement) | `EnforcementProof.structural = true` — the backend independently enforces network isolation, not just relying on cooperative proxy env vars | `SandboxBackend::enforce_network`                       | "hardened," "sandboxed" (too vague — avoid)       | Existing term, reused as-is                                                                           |
| Runner binary            | The new sibling `[[bin]]`-only crate embedding `hakoniwa`, spawned by `HakoniwaBackend::start_agent`                                        | This plan                                               | "launcher," "wrapper" (fine as informal synonyms) | New term for this plan; mirrors `firma-vz-runner`'s existing role without an established generic name |
| Launch contract          | The serialized `PrepareRequest`/`LaunchSpec`-derived data passed from `HakoniwaBackend` to the runner binary via a file path argument       | This plan, modeled on `VzBackend`'s `--launch-contract` | n/a                                               | Reuses `VzBackend`'s existing term                                                                    |
| Mount authority          | `SandboxMountAuthority`: `OperatorProvided \| Framework \| SandboxInfrastructure(kind)`                                                     | `backend/mod.rs`                                        | n/a                                               | Existing term, reused as-is                                                                           |

### Conditional: Alternatives

**`hakoniwa::Network::Pasta`** — shape: shell out to the external `pasta` binary for real userspace networking. Benefits: zero new code, matches Hakoniwa's own documented default. Costs: reintroduces exactly the external-binary dependency this plan exists to remove; gives the sandbox _working_ egress, which firma-run explicitly does not want. Rejected: contradicts this plan's own motivation. See `DEC-002`.

**`hakoniwa::RustSlirp`** — shape: pure-Rust userspace network stack (TUN device + routing), no external binary. Benefits: real network access with no external process. Costs: firma-run doesn't want real network access at all (only loopback), so this is strictly more machinery (and more audit surface — a real TUN device, route management) than the goal requires. Rejected on unnecessary-scope grounds, not a functional defect. See `DEC-002`.

**Reuse `crates/firma-run/src/seccomp.rs`'s BPF pipeline for Hakoniwa, treating Hakoniwa purely as a namespace/mount backend** — shape: keep seccomp entirely bwrap-style (a precompiled static BPF blob handed to the kernel), applied identically regardless of backend. Benefits: no new seccomp-translation code; one seccomp story for the whole codebase. Costs: throws away Hakoniwa's more expressive, already-available `Action::Notify`/`Trace` builder, and couples this new backend to a compiler pipeline (`seccomp.rs`, or its pending `seccompiler` successor) that has its own unrelated migration in flight. Rejected in favor of `DEC-004`, but flagged as the lower-effort fallback if Slice 5's translation proves harder than expected — a legitimate deferred option, not eliminated outright.

**A single shared `SandboxBackend`-adjacent trait for mount-planning, extracted now and implemented by both `BwrapBackend` and `HakoniwaBackend`** — shape: the "shared sub-traits" option the user explicitly declined when scoping this task (see conversation record). Benefits: avoids two parallel mount-authority implementations. Costs: touches `BwrapBackend`'s existing, working code, explicitly out of scope for this additive-only plan. Rejected per direct user instruction, not re-litigated here.

### Conditional: File-tree diff

```diff
 crates/
+├── firma-hakoniwa-runner/          # NEW — [[bin]]-only crate embedding `hakoniwa`
+│   ├── Cargo.toml                  # NEW — depends on `hakoniwa = "=1.7.2"` (pinned, DEC noted in Assumptions)
+│   └── src/main.rs                 # NEW — Container/Command build, loopback bring-up (DEC-002),
+│                                   #       DNS-stub/egress-guard bootstrap (DEC-003), seccomp/landlock (Slice 5)
 firma-run/src/backend/
+├── hakoniwa.rs                     # NEW — HakoniwaBackend: SandboxBackend
~├── mod.rs                          # MODIFIED — BackendKind::Hakoniwa variant + Display/FromStr/build_backend arms (not default_for_current_host — PLAN-007)
 firma-run/src/
~├── config.rs                       # MODIFIED — backend_supports_structural_network, backend_supported_on_host gain a Hakoniwa arm; resolve_backend_for_linux unchanged (bwrap stays default); managed_seccomp_applies deliberately excludes Hakoniwa (DEC-004)
~├── supervisor.rs                   # MODIFIED (Slice 4) — forward_signal gains a Hakoniwa arm if the runner uses a new session, reusing sandbox_child_pid/parse_first_pid (already pub)
 crates/firma-config-schema/src/run.rs
~└── (schema BackendKind)            # MODIFIED — add Hakoniwa variant to the parallel schema enum
 crates/firma/src/args/run.rs
~└── (CLI args)                      # MODIFIED — BackendOverride gains Hakoniwa, From<BackendOverride> for BackendKind updated
 crates/firma/src/doctor/sandbox.rs
~└── (doctor)                        # MODIFIED (Slice 6) — separate Backend::Hakoniwa arm + a kernel-capability probe, not a --version check
 Cargo.toml
~└── (workspace members)             # MODIFIED — add firma-hakoniwa-runner to members
 docs-site/src/content/docs/concepts/sandbox.md
~└── (docs)                          # MODIFIED (Slice 6) — document as experimental per DEC-010
 tests/e2e/scenarios/child_process_governance/
~├── network.rs, filesystem.rs, http.rs   # MODIFIED (DEC-011) — parametrized over BackendKind
```

### Conditional: Types and signatures

```rust
// crates/firma-run/src/backend/hakoniwa.rs

pub struct HakoniwaBackend;

impl SandboxBackend for HakoniwaBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Hakoniwa
    }

    fn prepare(&self, request: &PrepareRequest) -> Result<SandboxHandle, RunError> {
        // Mirrors BwrapBackend::prepare's shape: host/preflight checks (kernel
        // unprivileged-userns support, not command_available), runtime-dir
        // creation, SandboxMount construction. See Slice 1/2.
        todo!()
    }

    fn enforce_network(
        &self,
        handle: &SandboxHandle,
        policy: &NetworkPolicy,
    ) -> Result<EnforcementProof, RunError> {
        // Computational only, like BwrapBackend's — reuses
        // NetworkConfinement::LinuxNetworkNamespace (DEC-005).
        todo!()
    }

    fn verify_fail_closed(
        &self,
        handle: &SandboxHandle,
        proof: &EnforcementProof,
    ) -> Result<(), RunError> {
        todo!()
    }

    fn start_agent(
        &self,
        layout: &firma_runtime_state::RuntimeLayout,
        handle: &SandboxHandle,
        launch: &LaunchSpec,
    ) -> Result<std::process::Child, RunError> {
        // Serializes a launch contract, spawns firma-hakoniwa-runner via
        // std::process::Command (DEC-001) — never calls hakoniwa::Command
        // directly from this crate.
        todo!()
    }

    fn teardown(&self, handle: SandboxHandle) -> Result<(), RunError> {
        todo!()
    }
}
```

**Constructibility attack (`CW-001`)**: does anything stop any `HakoniwaBackend` method from being called with a `SandboxHandle` whose `backend` field is `BackendKind::Bwrap` (a mismatched handle from a different backend)? As sketched, **nothing does** — `SandboxHandle.backend` is a plain field, not a type parameter, and every `SandboxBackend` method takes `&SandboxHandle` generically. This is a **pre-existing** looseness in the trait (not introduced by this plan) — `BwrapBackend` has the identical exposure today, and it is not reachable through the sole production entry point today (`runtime::execute_run` builds one backend instance and threads one matching handle through its own lifecycle). Not fixing the trait itself (out of scope), but recording the cheap insurance: **all four** `HakoniwaBackend` methods that take a `&SandboxHandle` (`enforce_network`, `verify_fail_closed`, `start_agent`, `teardown` — corrected after `PLAN-009`, which noted the original text guarded only `start_agent` while `PROOF-003`'s stimulus claimed "any method") should defensively assert `handle.backend == BackendKind::Hakoniwa` and fail closed (an internal-error `RunError`, not a panic) rather than silently proceeding against a foreign handle. This is a proof obligation (`PROOF-003`), not a type-level fix.

### Conditional: Semantic call traces

| Field                      | `TRACE-001`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Proposed                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Entry and stimulus         | `firma run --backend hakoniwa -- <command>`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Path                       | `runtime::execute_run → build_backend(Hakoniwa) → HakoniwaBackend::prepare → HakoniwaBackend::enforce_network → HakoniwaBackend::verify_fail_closed → resolve_launch_target (unchanged, per selectable-execution-governance-plan Slice 0) → HakoniwaBackend::start_agent → std::process::Command::new(firma-hakoniwa-runner).spawn() → [new process] firma-hakoniwa-runner: build hakoniwa::Container, unshare namespaces, loopback bring-up (DEC-002), mount translation (Slice 2), DNS-stub/egress-guard bootstrap (DEC-003), seccomp/landlock install (Slice 5), execve the real command` |
| Input/output types         | `PrepareRequest` → `SandboxHandle` → `EnforcementProof` → `LaunchSpec` → serialized launch contract (file) → `std::process::Child`                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Validation/trust crossings | Config-resolution-time host-capability check (Slice 1); the launch contract itself crosses a process boundary but originates entirely from firma-run's own already-validated types, not untrusted input                                                                                                                                                                                                                                                                                                                                                                                      |
| Invariant established      | `INV-001` (once Slice 3 lands), `INV-002` (once Slice 2 lands)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Invariant assumed          | Kernel unprivileged-userns support was actually verified at `prepare` time, not just assumed from config                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Success outcome            | Sandboxed process runs with confinement equivalent to `BwrapBackend`'s                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Failure path               | Any stage failure maps to `RunError`, never a panic; unsupported-host selection fails at config-resolution before `prepare` is ever reached                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Evidence                   | This plan's slices; `~/Sources/openfirma-notes/notes/hakoniwa-backend-gap-analysis.md`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Proof boundary             | e2e suite (`DEC-011`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Unknowns                   | Exact launch-contract schema (JSON? bincode? — mirror `VzBackend`'s existing choice, not yet cited in this research)                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |

### Conditional: Trust analysis

- Actors: the sandboxed agent process and everything it spawns (untrusted); the `firma-hakoniwa-runner` process (trusted, part of the TCB, analogous to `bwrap`'s current trust position); `firma-run`'s host process (trusted).
- Supported workloads/deployment modes: Linux only, opt-in via explicit `--backend hakoniwa`/config (`DEC-010`).
- Attacker capabilities: full control over the sandboxed process and its descendants — same threat model `BwrapBackend` already defends against.
- Protected assets: network egress confinement (`INV-001`), `.firma`/config confidentiality (`INV-002`), the identical assets `BwrapBackend` protects today.
- Trust transitions: `firma-run` trusts the `firma-hakoniwa-runner` binary as much as it trusts itself (same binary, same build) — a materially _different_ trust position than trusting an external `bwrap` binary resolved from `$PATH`, which is one motivation for this migration (removes a `$PATH`-resolution trust question, but see `DEC-010`'s point that it does not remove the need for a security audit of the _embedded library's_ own hardening).
- Reachable abuse paths: an adversarial agent attempting namespace escape, mount-mask bypass (`INV-002`), or network exfiltration (`INV-001`) — the same abuse classes `linux_bwrap/mount.rs`'s existing test suite already exercises against bwrap; this plan's `PROOF-001`/`PROOF-002` need the equivalent adversarial framing, not just "does it work" functional tests.

### Conditional: Detailed proof obligations

| Field                  | `PROOF-001` (`INV-001`)                                                                                                                                                                                                                                                                                                                                                    |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kind                   | Runtime / Trust                                                                                                                                                                                                                                                                                                                                                            |
| Owner/proof boundary   | `HakoniwaBackend`/runner binary                                                                                                                                                                                                                                                                                                                                            |
| Suite/boundary         | E2E (Slices 1 and 3)                                                                                                                                                                                                                                                                                                                                                       |
| Stimulus               | Sandboxed process attempts a direct connection to a non-loopback address, and separately, a loopback address that isn't a sanctioned Firma endpoint; separately (added after `PLAN-001`), the proxy-bridge process is killed mid-run, and separately, the sandbox attempts to read `FIRMA_RUN_SANDBOX_ID`/derive the outer session's runtime dir from an inherited env var |
| Observable effects     | Connection attempts fail; sanctioned loopback endpoints (proxy bridge, DNS stub) remain reachable; a mid-run bridge death terminates the wrapped command (watchdog, `DEC-003`) rather than leaving it running unconfined; the sandboxed process observes no `FIRMA_RUN_*` env vars from the outer session (env-strip, `DEC-003`)                                           |
| Controls/substitutions | Same fixture pattern as `tests/e2e/scenarios/child_process_governance/network.rs`, extended with a bridge-kill fixture and an env-inheritance assertion                                                                                                                                                                                                                    |
| Failure cases          | Namespace/loopback-bringup misconfiguration → connectivity either over- or under-permissive; a dropped watchdog → sandboxed command survives an unconfined bridge death; a dropped env-strip → nested-run privilege-escalation path reopens                                                                                                                                |
| Evidence               | New/parametrized e2e test (`DEC-011`)                                                                                                                                                                                                                                                                                                                                      |
| Status                 | Planned                                                                                                                                                                                                                                                                                                                                                                    |
| Slice                  | 1 (bare network-namespace case), 3 (loopback-bypass, watchdog, env-strip cases)                                                                                                                                                                                                                                                                                            |
| Limits                 | Proves the specific stimuli tested; does not constitute the broader security audit `DEC-010` names as the actual sunset condition                                                                                                                                                                                                                                          |

| Field                  | `PROOF-002` (`INV-002`)                                                                                                                              |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kind                   | Runtime / Trust                                                                                                                                      |
| Owner/proof boundary   | Mount-translation layer in `HakoniwaBackend`/runner binary                                                                                           |
| Suite/boundary         | Unit (translation logic) + E2E (Slice 2)                                                                                                             |
| Stimulus               | Agent attempts to read `firma.toml`/`.firma/` directly, via a planted symlink, and via an operator mount whose source contains a masked path         |
| Observable effects     | All three attempts fail to expose the real content, matching `linux_bwrap/mount.rs`'s existing test intent                                           |
| Controls/substitutions | Reuse the _scenarios_ (not the bwrap-argument-order assertions) from `linux_bwrap/mount.rs`'s 14 existing unit tests                                 |
| Failure cases          | Any of the three stimuli succeeding is a confirmed defect                                                                                            |
| Evidence               | New tests, Slice 2                                                                                                                                   |
| Status                 | Planned                                                                                                                                              |
| Slice                  | 2                                                                                                                                                    |
| Limits                 | Proves the specific bypass classes bwrap's own tests already anticipate; does not prove absence of Hakoniwa-specific bypass classes not yet imagined |

| Field                  | `PROOF-003` (`CW-001`)                                                                                                |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Kind                   | Type / Runtime                                                                                                        |
| Owner/proof boundary   | `HakoniwaBackend`'s trait methods                                                                                     |
| Suite/boundary         | Unit                                                                                                                  |
| Stimulus               | Any `SandboxBackend` method called with a `SandboxHandle` whose `backend` field doesn't match `BackendKind::Hakoniwa` |
| Observable effects     | A typed `RunError`, not a panic and not silent success                                                                |
| Controls/substitutions | Construct a mismatched handle directly in a unit test                                                                 |
| Failure cases          | Silent proceed against a foreign handle (the pre-existing looseness `CW-001` names)                                   |
| Evidence               | New test, Slice 1                                                                                                     |
| Status                 | Planned                                                                                                               |
| Slice                  | 1                                                                                                                     |
| Limits                 | Defensive insurance against a currently-unreachable case, not a fix to the underlying trait looseness (out of scope)  |
