# Implementation plan: `PtraceSeccompExec` execution-governance strategy

## Artifact metadata

- Status: Implemented (Slices 3a/3b/3c all landed and passing against a real
  `bwrap` sandbox — see each slice's own "implementation findings"
  subsection below). Post-implementation adversarial review obtained and
  found one critical defect (`aarch64`'s deny mechanism did not actually
  work as designed — see Slice 3c's own findings), fixed and independently
  re-verified against real kernel behavior. That review's second finding
  (a cross-process TOCTOU on the checked-vs-executed file) is now
  mitigated by `DEC-019` (kill the process on a post-exec device+inode
  mismatch) — this reduces the exposure window but does not eliminate it;
  see `DEC-019` and Slice 3c's own findings for the residual scope.
  Pre-implementation: two independent review rounds complete — the first
  invalidated by an unauthorized overwrite and restored, then re-verified
  by a second, properly-scoped fresh reviewer that additionally found and
  this document incorporated `PLAN-109` through `PLAN-112`; see the note
  at the end of this section for full provenance)
- Durable locator: `docs/architecture/ptrace-seccomp-exec-gate-plan.md`, in-repo
- Repository revision researched: `5e0dd567` (HEAD of `feat/backend-selection`
  at research time)
- Task or requirement source: user request, this session — "consider the more
  involving backend that uses ptrace + seccomp-notify as we discussed, let's
  try to consider a full implementation plan for it"; elaborates Slice 3 of
  `docs/architecture/selectable-execution-governance-plan.md` (the parent
  plan)
- Supersedes: Not applicable. This is a child plan of the parent plan's
  Slice 3, per that skill's splitting rule (own proof boundary
  `PROOF-002`/`PROOF-003`/`PROOF-004`, independently observable acceptance
  outcome, implementable/shippable once the parent's Slice 1 lands). It does
  not replace the parent; it elaborates Slice 3's already-accepted
  `DEC-005`-`DEC-008` into implementation-ready detail. The design choice to
  retain the full ptrace-based mechanism over the lighter plain-seccomp-notify
  alternative was confirmed by explicit user decision in this session (see
  "Conditional: Alternatives" below for the full comparison presented at
  decision time).
- Provenance note: an earlier draft of this document was produced by an
  autonomous agent that was instructed to perform citation-gathering research
  only. That draft was independently reviewed (findings `PLAN-101` through
  `PLAN-108` below, all corrected) and its content is preserved here. A
  second, separate autonomous process subsequently overwrote this same file
  with a materially different, contradictory replacement (reversing the
  `DEC-013` `PTRACE_O_TRACEEXEC` decision on the basis of an unverified claim
  of having run live ptrace spikes on the host, with no artifact left to
  verify that claim) and silently deleted the critical `PLAN-101` finding
  below in the process. That replacement was discarded and is not
  reflected here. This document was restored to the reviewed draft, then
  put through a second independent review round — this time by a
  deliberately fresh agent with no inherited conversation context and
  explicit, checked constraints (no file writes anywhere, no code execution
  beyond static inspection, no git mutations) — specifically to re-verify
  the first round's findings and check for anything that survived it. That
  round found three further genuine issues (`PLAN-109`-`111`, all major) and
  one citation nit (`PLAN-112`), all independently spot-checked against the
  actual repository by the calling session before being incorporated below.
  Status reflects that this document has now been through two review rounds
  whose findings were independently verified, not merely asserted.

## Goal and acceptance outcomes

- Goal: implement the `PtraceSeccompExec` `ExecutionGovernor` end to end —
  the `firma __exec-guarded-run` shim, the host-side ptrace attach and
  unified wait loop, and the execve allow/deny decision logic — so that
  selecting `execution_governance = "ptrace_seccomp_exec"` closes the FIR-366
  gap for every descendant process, not just the root command.
- Observable acceptance outcomes:
  - `tests/e2e/scenarios/child_process_governance/execution.rs`'s FIR-366
    regression test passes under `ptrace_seccomp_exec` (a denied tool cannot
    run as a child of an allowed root command).
  - The three existing passing tests in that module (network/filesystem/http)
    still pass unchanged under this strategy.
  - SIGINT/SIGTERM/SIGWINCH forwarding and exit-code/signal-death reporting
    match `wait_with_signal_forwarding`'s existing observable behavior
    (`supervisor.rs`), reimplemented inside the unified loop per `DEC-007`.
  - Ptrace attach failure (Yama restriction, unsupported platform) aborts the
    launch with an actionable, specific error before the wrapped command
    starts — never a silent fallback to `Inherited`.
  - A benchmark (parent plan's Slice 4) reports this strategy's per-exec
    overhead; not produced by this plan, but this plan's design must not
    preclude it.

## Scope

- In scope: the `firma __exec-guarded-run` shim binary; the
  `PtraceSeccompExec` `ExecutionGovernor` implementation
  (`crates/firma-run/src/execution_governance/ptrace_seccomp.rs`); the
  readiness handshake; the unified ptrace/wait/signal-forwarding loop; the
  execve-argument read and allow/deny decision; the "confirmed platforms"
  gate; the raw BPF filter that traps only `execve`/`execveat`.
- Out of scope: the parent plan's Slice 1 (`ExecutionGovernanceStrategy` enum,
  `ExecutionGovernor` trait, `execution_governance` config field, the
  `AllowedExecutables` type, the config-resolution compatibility gate) and
  Slice 2 (`LandlockExecute`) — this plan assumes Slice 1 has landed and its
  trait/dispatch points exist as specified in the parent plan's "Conditional:
  Types and signatures". Directly verifying RHEL/CentOS Yama LSM presence is
  out of scope (no RHEL host available to this planning pass) — tracked as an
  explicit gap requiring direct verification before the "confirmed platforms"
  allow-list can include any RHEL-family distribution.
- Assumptions: the parent plan's `DEC-001`-`DEC-008` hold. `firma-run`'s host
  process remains the sole ptrace attacher (no separate daemon, per the
  parent's Yama `ptrace_scope=1` rationale). The sandboxed process tree runs
  under `bwrap`'s PID namespace; the discovered "inner" PID
  (`supervisor::sandbox_child_pid`) is the ptrace-attach target, not bwrap's
  own PID.
- Open decisions: exact allow-list of "confirmed platforms" for first ship
  (needs a compatibility matrix pass beyond this plan, tracked in
  `openfirma-notes/compatibility.md`); whether `x86_64` and `aarch64` both
  ship in the first cut or `aarch64` follows once register-access code is
  written and tested (register layout differs per arch — see `DEC-016`).
- Cohesion and split assessment: kept as one plan because the shim, the
  attach/handshake sequence, the unified wait loop, and the decision logic
  share one invariant owner (`supervise()`) and cannot be verified
  independently of each other — a shim without a working attach handshake
  cannot be tested end-to-end, and the wait loop cannot be tested without a
  real traced process. Implementation is still sliced (3a/3b/3c below) for
  incremental, individually-verifiable progress.
- Deferred child plans: Not applicable.

## Routing

- Mode: Full
- Trigger evidence: (1) security/trust boundary and fail-closed behavior —
  this is the mechanism FIR-366 exists to close; (3) this plan is the primary
  owner of `INV-001`'s proof for every process this strategy governs; (4)
  concurrency/lifecycle — `DEC-007`'s wait-loop unification is a lifecycle
  ownership question with a documented prior near-miss (`PLAN-003`); (5)/(6)
  multiple crates (`firma`, `firma-run`), unresolved architecture-level
  uncertainty (cross-arch register access, syscall-neutralization mechanics),
  and a materially different alternative design that was explicitly
  considered and decided against this session (plain seccomp-notify-only).
- Higher-mode triggers checked: none beyond Full exist in this workflow.
- Downgrade evidence and reason: Not applicable.

## Current behavior and problem

- Owners and entry points: unchanged from the parent plan's "Current behavior
  and problem" (`TRACE-001`) — no code for any of Slice 1/2/3 exists yet.
  Confirmed this session by grep: zero matches for `ExecutionGovernance`,
  `ExecutionGovernor`, `execution_governance`, `rewrite_launch`,
  `PtraceSeccompExec`, or `LandlockExecute` anywhere in the repository.
- What already exists that this plan builds on (Observed, re-verified this
  session against `feat/backend-selection` at `5e0dd567`):
  - `runtime::execute_run`'s `resolve_launch_target` call
    (`runtime/mod.rs:280-287`) returns `ResolvedLaunchTarget{ executable,
    args, .. }` strictly before `LaunchSpec` is constructed
    (`runtime/mod.rs:294-303`) — this is `rewrite_launch`'s insertion point,
    confirming Slice 0 positioned the seam correctly per `PLAN-002`.
  - `backend.start_agent(...)` is called at `runtime/mod.rs:309`;
    `wait_with_signal_forwarding(child, backend.kind())` is called at
    `runtime/mod.rs:320`, immediately after. This is `supervise()`'s
    insertion point.
  - `linux_bwrap/mod.rs:278-281`: the executable/args are appended to the
    `bwrap` `Command` and `command.spawn()` is called. `bwrap` is the process
    `std::process::Command::spawn()` returns a `Child` for; `bwrap` itself
    forks the sandboxed process (`supervisor::sandbox_child_pid`'s doc
    comment and its `/proc/<bwrap_pid>/task/<bwrap_pid>/children` read
    confirm this indirectly: it exists specifically because the returned
    `Child.id()` is bwrap's PID, not the sandboxed process's PID).
  - `supervisor::sandbox_child_pid` and `parse_first_pid`
    (`supervisor.rs:209`, `220`) are already `pub`, with a doc comment
    (`supervisor.rs:201-207`) explicitly anticipating this exact use —
    "e.g. a ptrace-based governance mechanism attaching to the process the
    seccomp filter actually runs in" — and explicitly warning that its
    best-effort/startup-window semantics are _not_ sufficient for a ptrace
    attacher, which "must pair this lookup with its own synchronization."
    This plan's readiness handshake (`DEC-011`, `DEC-012`) is that
    synchronization.
  - `supervisor::forward_signal` (`supervisor.rs:161`) and
    `supervisor::exit_code_from_outcome` (`supervisor.rs:86-88`, already
    shaped to take plain `Option<i32>` facts precisely so a future
    raw-`waitpid` caller can reuse it, per its own doc comment) are **both**
    still private — Slice 0 only exposed `sandbox_child_pid` and
    `parse_first_pid`. `DEC-007`'s "reimplemented inside the unified loop,
    not dropped" / "share... instead of duplicating" requirements need both;
    see `DEC-014`.
  - `egress_guard.rs` is the direct precedent for nearly every mechanical
    piece this plan needs: a host-side supervisor thread bound to a Unix
    socket (`egress_guard.rs:603-642`, `start`), an in-sandbox installer that
    connects before installing any filter (`egress_guard.rs:465-497`,
    `install_and_exec`), a raw hand-authored BPF program
    (`egress_guard.rs:314-357`, `CONNECT_NOTIFY_PROG`), a
    `process_vm_readv`-based remote-memory read (`egress_guard.rs:219-236`,
    `read_remote_mem`), and a TOCTOU re-validation immediately before
    answering a trapped syscall (`egress_guard.rs:845`,
    `notif_id_is_valid`). Every one of these has a direct ptrace-world
    analogue used below.
  - `crates/firma/src/services/egress_guarded_run.rs` is the direct template
    for the new `firma __exec-guarded-run` shim's `firma`-crate wrapper:
    thin `run(args) -> anyhow::Result<ExitCode>` that calls into
    `firma-run`, fails closed on any error, and has a `#[cfg(not(target_os =
    "linux"))]` stub that always errors rather than silently running
    unguarded.
  - `nix` is a workspace dependency at `0.31`, but `firma-run`'s Cargo.toml
    (`crates/firma-run/Cargo.toml:45`) enables only `["process", "signal",
    "socket", "uio"]` — **not** `"ptrace"`. `nix::sys::ptrace` (confirmed by
    reading the vendored `nix-0.31.3` source) provides `seize`, `setoptions`,
    `getevent`, `cont`, `getregs`/`getregset`, and a `nix::sys::wait`
    `WaitStatus::PtraceEvent(Pid, Signal, c_int)` variant carrying the
    ptrace-event code — sufficient primitives; no new crate dependency is
    needed (see `DEC-015`).
- Evidence: file:line citations above, all re-verified this session by direct
  file reads (not carried over from the parent plan's earlier research pass).

## Key decisions and tradeoffs

### `DEC-009`: Filter authoring — raw BPF, loaded without the listener flag

- Choice: author the execve-trap filter as a small, hand-written
  `[libc::sock_filter; N]` array, structurally identical to
  `egress_guard.rs`'s `CONNECT_NOTIFY_PROG` (load arch, compare to native,
  load syscall nr, compare against `execve`'s and `execveat`'s syscall
  numbers, `SECCOMP_RET_TRACE` on match else `SECCOMP_RET_ALLOW`), loaded via
  the plain `SYS_seccomp`/`SECCOMP_SET_MODE_FILTER` path with **no**
  `SECCOMP_FILTER_FLAG_NEW_LISTENER` flag.
- Rationale and evidence: `SECCOMP_RET_TRACE` stops are delivered to a
  ptrace-attached tracer via `waitpid`/`PTRACE_EVENT_SECCOMP`, not via a
  listener fd — the `NEW_LISTENER` flag and its ioctl protocol
  (`egress_guard.rs`'s `notif_recv`/`notif_send`) are specific to
  `SECCOMP_RET_USER_NOTIF` and do not apply here. Reusing the same
  hand-authored-BPF style as `CONNECT_NOTIFY_PROG` keeps one authoring
  convention in the crate rather than introducing a filter-builder
  dependency for a 6-instruction program.
- Consequences and rejected alternatives: a filter-builder crate (`seccompiler`)
  was considered and rejected — not for any capability gap (verified:
  `seccompiler::SeccompAction::Trace(u32)` maps directly to
  `SECCOMP_RET_TRACE`, so it could express this filter), but because this
  filter is fixed (exactly two syscalls, one action, no policy-driven
  variability), so pulling in a policy-compilation dependency for a constant
  ~6-instruction program adds indirection without benefit. `seccompiler` is
  **not** currently a workspace dependency — it exists only in the separate,
  unmerged `seccomp/seccompiler-*` experimental branches, not on
  `feat/backend-selection`; an earlier version of this rationale incorrectly
  claimed `crates/firma-run/src/seccomp.rs` already used it. That file is
  itself hand-rolled BPF emission (`emit_bpf_program`/`emit_stmt`/
  `emit_jump`), which if anything argues the opposite of a
  seccompiler-consistency rationale: the existing convention in this crate is
  hand-authored BPF even for its more complex, policy-driven managed filter,
  not merely a fallback for cases seccompiler can't handle. Revisit only if
  the trap set grows policy-driven enough that hand-authoring becomes the
  actual maintenance burden.

### `DEC-010`: Ptrace primitive layer — `nix::sys::ptrace`, not `pete`

- Choice: enable the `ptrace` feature on `firma-run`'s existing `nix`
  dependency; do not add `pete`.
- Rationale and evidence: `pete`'s safe-loop abstraction assumes it owns the
  wait loop end to end, which conflicts with `DEC-007`'s requirement that
  `supervise()` interleave `PTRACE_EVENT_SECCOMP` handling, fork/clone/vfork
  auto-attach stops, terminal exit, and signal forwarding in one loop this
  plan controls directly. `nix::sys::ptrace` (confirmed present at the
  needed granularity: `seize`, `setoptions`, `getevent`, `cont`, `getregs`)
  gives exactly the primitives needed without an abstraction to work around.
  This also matches the crate's existing precedent (`egress_guard.rs` uses
  raw `libc`/`nix` primitives directly, not a higher-level notify-loop
  crate).
- Consequences and rejected alternatives: raw `libc::ptrace` FFI calls
  (bypassing `nix` entirely) were considered and rejected — `nix::sys::ptrace`
  already wraps the needed calls safely with no abstraction mismatch, so
  hand-rolling the FFI would only reintroduce `unsafe` surface the crate
  doesn't need to own.

### `DEC-011`: Readiness handshake transport — Unix-domain socket, filter installed first

- Choice: the shim installs the `EXEC_TRACE_PROG` filter first, then connects
  to a Unix socket (bind-mounted into the sandbox, analogous to
  `egress_guard.rs`'s `SupervisorConfig.socket_path`), sends a one-byte
  "ready" marker, then blocks reading one byte back from the supervisor. The
  supervisor sends that byte only after `PTRACE_SEIZE`/`PTRACE_SETOPTIONS`
  succeed on the discovered PID. Only after receiving it does the shim
  proceed to `execve`. This ordering — filter, then connect, matching
  `DEC-012`'s sequencing — is the opposite of `egress_guard.rs`'s
  `install_and_exec`, which connects _before_ installing its filter; that
  ordering exists there specifically because the egress guard's filter traps
  `connect`, so its own handshake connect must happen before that filter is
  live. This filter traps only `execve`/`execveat`, so a Unix-socket
  `connect` is never subject to it and the ordering constraint does not
  transfer — installing the filter first is preferred instead because it
  guarantees the tracer's eventual `PTRACE_SEIZE` finds the filter already in
  place with no window where the shim could reach `execve` before the filter
  exists.
- Rationale and evidence: a missed or mistimed attach means the trapped
  `execve` observes `-ENOSYS` with no tracer present to service it
  (`SECCOMP_RET_TRACE` fails closed when unserviced, per documented seccomp
  semantics: "If there is no tracer present, the system call is not executed
  and -ENOSYS is returned" — see the corrected framing below). That
  reliability risk — spurious denial of a legitimate exec during the startup
  window, not a security bypass — is what this handshake exists to
  eliminate: polling-based discovery (`sandbox_child_pid`'s own documented
  startup-window race) could otherwise let the shim reach `execve` before
  the supervisor has attached, spuriously failing that exec. This was
  flagged directly in that function's doc comment (`supervisor.rs:201-207`)
  as needing "its own synchronization." A blocking handshake over a socket
  already bind-mounted for other in-sandbox-to-host coordination (the egress
  guard uses the same shape) is a known-working, already-reviewed pattern in
  this codebase rather than a new primitive.
- Consequences and rejected alternatives: a bare pipe fd (inherited across
  `execve` via `FD_CLOEXEC` clearing, mirroring
  `linux_bwrap/mod.rs:238`'s `clear_fd_cloexec` for the seccomp fd) was
  considered; rejected in favor of the socket for consistency with the
  existing egress-guard bind-mount plumbing and because no fd handoff from
  shim to host is needed here (unlike the egress guard, which hands over a
  listener fd — here the supervisor discovers the PID itself via
  `sandbox_child_pid`, so only the readiness signal is needed).

### `DEC-012`: PID discovery and attach-order — supervisor polls, then attaches (atomically), then signals readiness

- Choice: `supervise()` polls `sandbox_child_pid(bwrap_pid)` (already `pub`)
  until it resolves (bounded retry with a total timeout, matching the
  existing best-effort polling shape used for signal forwarding), then calls
  `ptrace::seize(pid, options)` on that PID with the full `DEC-013` option
  set passed atomically in the same call, and only then writes the readiness
  byte to the connected shim. `nix::sys::ptrace::seize`'s signature
  (`seize(pid: Pid, options: Options) -> Result<()>`) applies the options as
  part of the single `PTRACE_SEIZE` syscall, so there is no intermediate
  attached-but-unconfigured state and no separate `setoptions` call is
  needed.
- Rationale and evidence: `PTRACE_SEIZE` does not require the target to be
  stopped (unlike `PTRACE_ATTACH`), so it can race the shim's own startup
  freely as long as the shim blocks on the handshake before `execve`-ing into
  a state the filter would need to already be watching — which it is,
  because the shim installs the filter _before_ connecting for the
  handshake's second half, per the shim's own sequencing in `DEC-011`. This
  ordering — filter installed, then handshake sent, then block for
  readiness, then `execve` — guarantees the tracer is attached before the
  first trap-worthy syscall the filter could produce.
- Consequences and rejected alternatives: attaching to `bwrap`'s own PID
  instead of the discovered inner PID was considered (it exists earlier,
  avoiding the polling race) and rejected: `bwrap` itself is not the
  sandboxed process and does not carry the installed filter — only its
  forked child (in the new PID namespace) does, so the inner PID is the only
  valid attach target for `PTRACE_EVENT_SECCOMP` to fire against.

### `DEC-013`: Descendant coverage — `PTRACE_O_TRACEFORK`/`TRACECLONE`/`TRACEVFORK`/`TRACEEXEC` at seize time

- Choice: pass `PTRACE_O_TRACESECCOMP | PTRACE_O_TRACEEXEC |
  PTRACE_O_TRACEFORK | PTRACE_O_TRACECLONE | PTRACE_O_TRACEVFORK` together,
  atomically, in the `ptrace::seize` call itself (`DEC-012`). This matches
  the parent plan's `DEC-008` option set exactly, including
  `PTRACE_O_TRACEEXEC` (an earlier draft of this plan omitted it; restored
  here — its absence has an observable effect worth naming explicitly:
  without it, a successful, _allowed_ `execve` delivers a plain
  signal-delivery-stop (`SIGTRAP`) rather than a distinguishable
  `PTRACE_EVENT_EXEC` stop, which Slice 3b's wait loop would then have to
  recognize and swallow as an "ordinary stop" by inference rather than by an
  explicit, named event — `PTRACE_O_TRACEEXEC` avoids that ambiguity by
  giving every successful exec its own explicit, classifiable
  `WaitStatus::PtraceEvent` entry, exactly like the fork/clone/vfork cases
  the loop already has to handle).
- Rationale and evidence: per documented Linux `ptrace(2)` semantics
  (**Inferred** from kernel documentation, not yet exercised against this
  repository's code since none exists — flagged explicitly as `PROOF-005`
  below rather than assumed silently), a process auto-attached via one of
  these options through a traced `fork`/`clone`/`vfork` inherits the same
  trace options as its parent, so a single `setoptions` call at seize time is
  sufficient — no per-child `setoptions` re-issue is required. This is
  exactly the assumption the parent plan's `TRACE-003` already flagged as
  needing "explicit test coverage, not just assumed from the option names";
  this plan does not resolve that Unknown by assumption, only by naming the
  specific test that must establish it (`PROOF-005`).
- Consequences and rejected alternatives: omitting `TRACECLONE`/`TRACEVFORK`
  (tracking only `fork`, the common case) was considered; rejected because a
  process can trivially spawn descendants via `vfork`+`exec` (many shells and
  `posix_spawn` implementations do), and missing that path would silently
  reopen the exact gap FIR-366 exists to close.

### `DEC-014`: Signal forwarding and exit-code mapping — expose `forward_signal` and `exit_code_from_outcome`, reuse both inside the unified loop

- Choice: change both `supervisor::forward_signal` and
  `supervisor::exit_code_from_outcome` from private to `pub(crate)` and call
  them directly from `ptrace_seccomp.rs`'s unified wait loop:
  `forward_signal` for SIGINT/SIGTERM/SIGWINCH (matching
  `wait_with_signal_forwarding`'s existing escalation semantics — first
  SIGINT/SIGTERM forwarded as-is, a second escalates to `SIGKILL` —
  exactly), and `exit_code_from_outcome` to map the loop's own terminal
  `(exit_code: Option<i32>, term_signal: Option<i32>)` facts (obtained from
  `nix::sys::wait::WaitStatus`, not `std::process::ExitStatus`) to the same
  process exit code convention every other backend already reports.
- Rationale and evidence: `DEC-007` (parent plan) requires this behavior
  "reimplemented inside the unified loop, not dropped," and explicitly
  states both today's `Child`-based path and this loop should "share the
  same 128+signum mapping instead of duplicating it." `exit_code_from_outcome`
  (`supervisor.rs:86-88`) is exactly that mapping and was already written,
  during Slice 0, to take plain `Option<i32>` facts specifically so "a
  future raw-`waitpid`-based caller ... can share this mapping instead of
  duplicating it" (its own doc comment) — but Slice 0 only exposed
  `sandbox_child_pid`/`parse_first_pid`, leaving this function itself
  private and therefore unreachable from a new module. Duplicating either
  function's logic would let the two implementations drift silently;
  reusing both existing, already-tested functions guarantees identical
  behavior by construction. This is a small, additive visibility change in
  the same spirit as Slice 0's `sandbox_child_pid`/`parse_first_pid`
  exposure — not a new gap this plan introduces, but a gap Slice 0 left
  unaddressed because Slice 0 was scoped to the executable/args rewrite
  only.
- Consequences and rejected alternatives: free functions duplicating either
  function's logic were rejected for the drift risk above. Moving either
  function into a shared location callable from both `supervisor.rs` and
  `execution_governance/ptrace_seccomp.rs` was considered and rejected as
  unnecessary churn — `pub(crate)` on the existing functions is sufficient
  since both call sites are in the same crate.

### `DEC-015`: Deny mechanism — rewrite the syscall number before `PTRACE_CONT`, not a substituted result

- Choice: on an `execve`/`execveat` trap that resolves to `Block`, before
  `PTRACE_CONT`, overwrite the tracee's syscall-number register (`orig_rax`
  on x86_64; the architecture-appropriate equivalent register elsewhere, see
  `DEC-016`) to an invalid syscall number (e.g. `-1`), then continue. The
  kernel returns `-ENOSYS` to the tracee for the (now-invalid) syscall
  instead of performing the real `execve`.
- Rationale and evidence: this is the standard, minimal ptrace-based
  syscall-denial technique and requires no result substitution or "twin
  execution" — consistent with `DEC-005`'s already-accepted rejection of that
  idea. It is a strict register-write, not a memory-content forgery, so it
  does not touch the TOCTOU surface `PROOF-002` is concerned with (that
  surface is the _read_ of the pathname argument, addressed separately by
  `DEC-017` below).
- Consequences and rejected alternatives: forcing the tracee to take a fatal
  signal (`PTRACE_KILL` or injecting `SIGSYS`) on denial was considered;
  rejected because it kills the _entire process_, not just the one denied
  `execve` call, which is a materially different (and more disruptive)
  observable behavior than `LandlockExecute`'s `EACCES`-style denial
  (`PROOF-001`) or the plain-seccomp-notify alternative's `-EACCES` response
  — parity across strategies (`TRACE-002` vs. `TRACE-003`'s "Success outcome:
  ... observable failure matches today's seccomp-deny shape") requires the
  process to observe a failed `exec`, not to die outright.

### `DEC-016`: Cross-architecture register access — `x86_64` first, `aarch64` gated behind its own proof obligation

- Choice: implement and test register read/write (`DEC-015`) for `x86_64`
  first (`nix::sys::ptrace::getregs`/`setregs`, using the arch-specific
  `user_regs_struct` fields — `orig_rax` for the syscall number). Ship
  `aarch64` only once the equivalent register access has its own passing
  test on real `aarch64` hardware or a CI runner.
- Rationale and evidence: `nix` already exposes the identical `getregs`/
  `setregs` function signatures and a uniform `user_regs_struct` return type
  on both `x86_64` and `aarch64` (on `aarch64`, `getregs`/`setregs`
  internally dispatch through `getregset`/`regset::NT_PRSTATUS` — confirmed
  against the vendored `nix-0.31.3` source's `RegisterSet` implementation —
  but this is an internal detail the call sites in this plan don't need to
  touch directly). The architecture-visible surface this decision actually
  needs to gate is `user_regs_struct`'s _field layout_: `orig_rax` (the
  syscall-number register `DEC-015` rewrites) exists only on `x86_64`;
  `aarch64`'s equivalent is a different field entirely. That field
  difference, not a difference in `nix`'s function surface, is why `x86_64`
  and `aarch64` need independent proof before either ships.
- Consequences and rejected alternatives: shipping both architectures
  simultaneously without separate proof was considered and rejected —
  the "confirmed platforms" gate (`DEC-018`) must be per-architecture, not
  just per-distribution, given this evidence.

### `DEC-017`: Argument read — freeze every tracee thread, bounded-chunk read with in-prefix NUL search

- Choice: on a `PTRACE_EVENT_SECCOMP` stop, before trusting any read of the
  pathname argument: enumerate every thread in the tracee's thread group via
  `/proc/<tgid>/task/`, and for any sibling thread not already in a
  ptrace-stop, issue `PTRACE_INTERRUPT` and `waitpid` for its stop. Then
  re-enumerate `/proc/<tgid>/task/` and repeat the freeze pass against any
  newly-listed thread; continue this enumerate-freeze loop until one full
  pass discovers no thread it hasn't already frozen (a stabilization loop,
  not a single pass — see the corrected rationale below for why a single
  pass is insufficient). Every thread the tracee had at seize time is
  covered because the tracee is provably single-threaded at that point (the
  shim is a thin, single-threaded wrapper blocked on the handshake, and
  `execve` collapses all other threads by kernel guarantee, so seize always
  targets a lone thread); every thread created afterward is auto-attached by
  `PTRACE_O_TRACECLONE` (`DEC-013`) as it's created — but auto-attachment and
  this loop's own freeze pass are not instantaneous with thread creation, so
  the stabilization loop is what actually closes the gap between "a thread
  exists" and "this freeze pass has stopped it." Only once a full pass finds
  no new thread, read the pathname via `process_vm_readv` in bounded
  chunks (a fixed cap, e.g. 4 KiB, one `process_vm_readv` call per chunk up
  to a page boundary), scanning each successfully-read chunk for a NUL
  terminator and stopping as soon as one is found in the data actually
  returned. A short read is a hard failure (`Block`, fail-closed) only when
  it returns _fewer bytes than requested and no NUL was found in what was
  returned_ — a full page successfully read with no NUL simply continues to
  the next chunk, up to the fixed cap, at which point an unterminated
  pathname is treated as `Block`. This differs from `read_remote_mem`
  (`egress_guard.rs:219-236`), which is fixed-length by design (a
  `sockaddr`'s exact `addrlen`) and fails any short read outright — that
  semantics does not transfer to a NUL-terminated string of unknown length,
  where a "short" read relative to the requested chunk size is often just
  "the string ended before the chunk did," not a failure. The per-thread
  freeze established above must be held until the trapping thread's own
  `PTRACE_EVENT_EXEC` stop fires (allow path) or the register rewrite has
  been issued and `PTRACE_CONT` delivered (deny path) — releasing sibling
  threads any earlier would reopen exactly the race this decision exists to
  close, since the kernel does not copy the pathname bytes into its own
  memory until the traced thread is actually resumed into the real `execve`.
- Rationale and evidence: **this decision corrects an overclaim from an
  earlier draft of this plan**, surfaced by independent plan review
  (`PLAN-101`). That earlier draft asserted a `PTRACE_EVENT_SECCOMP` stop
  "halts the entire tracee including all its threads," attributing this to
  "`PTRACE_SEIZE`'s ... group-stop semantics," and used that claim to argue
  the ptrace design's TOCTOU surface was narrower than the parent plan's
  `PROOF-002` already flagged. That claim conflates two distinct, documented
  ptrace-stop categories: a _group-stop_ (triggered by stopping signals,
  which does stop every thread in the process) and a _ptrace-event-stop_
  (`PTRACE_EVENT_FORK`/`CLONE`/`VFORK`/`EXEC`/`SECCOMP`), which is
  documented as per-thread — only the thread that issued the trapping
  syscall stops; sibling threads continue running unless independently
  stopped. Left uncorrected, a multithreaded tracee (a common shape — any
  interpreter, a shell with job control, or an agent runtime using worker
  threads, not a contrived case) could run a helper thread that repeatedly
  overwrites the pathname buffer between the trap and the read, racing the
  supervisor's decision — defeating `INV-EXEC-001` for exactly the property
  this whole strategy exists to guarantee. The explicit per-thread freeze
  above is the standard mitigation for this class of TOCTOU in
  multi-threaded ptrace mediation, and is chosen over the two narrower
  alternatives considered (below) because it preserves the strategy's
  applicability to real, multithreaded agent processes rather than
  restricting them out of scope.

  A second, independent overclaim in that same earlier draft — surfaced by a
  later review round (`PLAN-109`, `PLAN-110`) — asserted that "`PTRACE_SEIZE`
  on a multi-threaded process attaches to the whole thread group," offered as
  the reason sibling threads don't need a separate attach step. This is also
  false: per `ptrace(2)`, attach (via either `PTRACE_ATTACH` or
  `PTRACE_SEIZE`) is per-thread, not per-thread-group — a multi-threaded
  process's siblings are covered only because, at this specific attach point,
  the tracee is provably single-threaded when seized (the shim is a thin,
  single-threaded wrapper blocked on the handshake, and `execve` collapses
  all other threads by kernel guarantee) and every thread created afterward
  is auto-attached by `PTRACE_O_TRACECLONE` (`DEC-013`). That same review
  round also found the freeze mechanism's actual race-closing step — the
  enumerate/freeze stabilization loop now stated explicitly in the Choice
  paragraph above — had previously existed only as a "must be checked" caveat
  in `PROOF-002`'s Failure cases row, not as a mandated part of the design
  itself; a single enumerate-then-freeze pass, as originally specified, does
  not close the race against a thread created concurrently with that pass.
  Both are now corrected in the Choice paragraph above rather than left
  implicit.
- Consequences and rejected alternatives: leaving the strategy's guarantee
  scoped to single-threaded processes at `execve` time (documenting the race
  as an accepted limitation rather than closing it) was considered and
  rejected — many real agent runtimes are multithreaded well before they
  ever call `execve`, so this would silence the gap rather than close it for
  the workloads this strategy is meant to cover. Re-validating via a
  `notif_id_is_valid`-style "is this stop still current" check (mirroring
  `egress_guard.rs:845`) was also considered and rejected: no such ioctl
  exists in the ptrace world (a ptrace-stop does not expire or get recycled
  the way a `SECCOMP_RET_USER_NOTIF` id can — see `PROOF-002` below), so
  that specific mitigation shape doesn't transfer; the per-thread freeze
  addresses the actual (different) race this design has, rather than porting
  a mitigation shaped for a race this design doesn't have.

### `DEC-018`: "Confirmed platforms" gate — allow-list checked at `supervise()` entry, not just documented

- Choice: `PtraceSeccompExec::supervise` (or a preceding check called from
  it before `seize`) consults a small, explicit allow-list of confirmed
  `(distro-family-or-kernel-signal, architecture)` pairs; anything outside it
  fails closed with an actionable error naming the specific host
  characteristic that was unconfirmed, before any `ptrace` call is made.
- Rationale and evidence: the parent plan's `PROOF-003` requires this
  ("Launch fails closed with an actionable, specific error"); this decision
  makes the gate an executable check rather than documentation, so a host
  outside the confirmed set cannot silently attempt (and partially fail)
  attach. Yama `ptrace_scope` value itself should also be read
  (`/proc/sys/kernel/yama/ptrace_scope`, when present) and surfaced in the
  error message when attach fails, to distinguish "this platform was never
  confirmed" from "this platform is confirmed but Yama is configured more
  restrictively than expected on this host" — two different remediation
  paths for an operator.
- Consequences and rejected alternatives: relying solely on `PTRACE_SEIZE`'s
  own failure (`EPERM`) without a pre-check was considered; rejected because
  a bare `EPERM` gives an operator no actionable signal about _why_ (Yama
  scope? RHEL without Yama at all — a different LSM story entirely and
  Unknown per the parent plan? a container without `CAP_SYS_PTRACE` granted
  to `firma-run` itself despite being a real ancestor?) — the explicit gate
  plus a Yama-scope read produces a specific, actionable message instead of
  a bare kernel errno.

### `DEC-019`: Post-exec identity re-verification — kill on a device+inode mismatch against `AllowedExecutables`

- Choice: since `PTRACE_O_TRACEEXEC` is already set at seize time
  (`DEC-013`), every successful `execve`/`execveat` anywhere in the traced
  subtree also produces a `PTRACE_EVENT_EXEC` stop. At that stop,
  additionally to `decide_and_continue`'s own pre-exec check, `stat` the
  file now actually mapped as the process's executable
  (`/proc/<tid>/exe`) and compare its device+inode against every allowed
  path's own device+inode (each `stat`'d directly, from this process's
  ordinary host view — those are real host paths already). If it matches
  none of them, kill the process (`SIGKILL`) rather than continuing it.
- Rationale and evidence: added post-implementation, in response to a
  real adversarial-review finding — `decide_and_continue`'s own check
  approves a target _before_ the real syscall runs, and nothing in this
  design (not even `DEC-017`'s multi-thread freeze, which only covers
  threads of the _same_ tracee) stops an independent second process able
  to write to the resolved path from replacing the file there between
  that approval and the kernel's own later lookup for the real syscall.
  This is a defense-in-depth layer, not a replacement for closing the
  race at its source: it cannot prevent the swapped file from executing
  for the brief window between the exec completing and this stop being
  observed, only from running any further than that. Comparing by
  device+inode rather than path string is deliberate: reading
  `/proc/<tid>/exe`'s target as a string and re-resolving _that_ string
  from the tracer's own filesystem view would reproduce exactly the class
  of bug `resolve_traced_exec_target` was built to avoid elsewhere in
  this same file (see the child plan's own Slice 3b findings on
  `SO_PEERCRED` vs. `/proc`-based pid discovery, and Slice 3c's
  `/proc/<tid>/{root,cwd,fd/<n>}` resolution) — `std::fs::metadata`
  called directly on the magic-link path itself is the one-syscall,
  namespace-correct equivalent. Device+inode comparison also means a
  legitimately bind-mounted alias of an allowed file is still recognized
  correctly, rather than requiring path-string equality.
- Consequences and rejected alternatives: closing the race at its source
  (rewriting the traced syscall to `execveat` against a file descriptor
  the tracer itself opened, verified, and injected into the tracee — the
  only mechanism that would eliminate the window entirely) was considered
  and deferred: it requires either a live `SCM_RIGHTS` channel to _every_
  traced process (this design only maintains one to the root shim, torn
  down after the initial handshake) or syscall injection via ptrace,
  either of which is a materially larger design than this fix — tracked
  as a follow-up, not part of this decision's own scope. Re-verifying via
  the _path string_ rather than device+inode was considered and rejected
  for the reason given above.

### `DEC-020`: Extend `PtraceSeccompExec` to `HakoniwaBackend`, not `bwrap` only

- Choice: widen `validate_execution_governance_preconditions`'s backend
  check from `Bwrap`-only to `Bwrap | Hakoniwa`. No change to
  `ptrace_seccomp.rs`'s own attach/decision logic was needed for pid
  discovery — `HakoniwaBackend`'s runner forks once and that fork's pid is
  already the one that becomes the wrapped command, so the existing
  `SO_PEERCRED`-based discovery (added for `bwrap`, `DEC-012`'s
  implementation finding) already generalizes: it identifies whoever
  connects to the handshake socket, regardless of which backend's process
  tree produced that connection.
- Rationale and evidence: confirmed by dedicated research before
  implementation (pid-discovery model, `runtime_dir` availability, seccomp
  filter stacking semantics) and then empirically, against a real Hakoniwa
  sandbox — not merely by analogy to `bwrap`.
- Consequences and two more real integration issues, neither anticipated by
  the research: (1) `rewrite_launch`'s shim path (`current_exe()`) is not
  guaranteed visible inside the sandbox just because the sandbox itself
  works — `HakoniwaBackend`'s `rootfs("/")` does not recursively cover
  other host mounts (a dev checkout under a separate `/home` mount was
  invisible, `ENOENT` on the shim's own re-exec) — fixed generally by
  copying the shim into `sandbox_runtime_dir` (already guaranteed reachable
  for the handshake socket) instead of referencing its host path directly,
  benefiting `bwrap` too, not just a Hakoniwa-only patch; (2)
  `HakoniwaBackend` already has its own, always-on Landlock-based
  descendant-exec enforcement, completely independent of
  `execution_governance` — it activates whenever `allowed_executables` is
  non-empty, regardless of strategy, and since the shim's own path is never
  itself in the operator's `allowed_executables`, Landlock denied the
  shim's own exec outright. `LaunchSpec` gained an `execution_governance`
  field so `HakoniwaBackend` can add its own (already-rewritten) launch
  target to the Landlock allow-list whenever a non-`Inherited` strategy is
  selected — generic over which strategy did the rewriting, not
  `PtraceSeccompExec`-specific — after which Landlock and the selected
  strategy both independently (and harmlessly redundantly) govern any
  further descendant exec.
- Open question, not resolved by this decision: `HakoniwaBackend`'s own
  Landlock mechanism already solves the same problem `execution_governance`
  exists to make selectable, unconditionally, regardless of which strategy
  is nominally chosen — meaning `Inherited` on Hakoniwa is not actually
  "root-command-only" the way it is on `bwrap`; Hakoniwa's descendant-exec
  restriction is always on. Whether `HakoniwaBackend`'s own Landlock
  construction should itself become conditional on `execution_governance`
  (e.g. skipped when a different strategy is explicitly selected, so
  `PtraceSeccompExec` becomes a genuine alternative mechanism rather than a
  redundant, always-coexisting one — of practical value chiefly as a
  fallback on kernels too old for Landlock) is a real design decision this
  plan does not make; this session verified the combination _works_
  (harmless redundancy) without deciding whether it _should remain_
  redundant going forward.

## Architecture and invariant ownership

- Architecture shape: one new module,
  `crates/firma-run/src/execution_governance/ptrace_seccomp.rs`, implementing
  the parent plan's `ExecutionGovernor` trait (`Types and signatures`,
  parent plan lines 423-450) with `rewrite_launch` (installs the shim as the
  actual `LaunchSpec.executable`, passing the real target through as
  argv — mirrors `linux_bwrap/mod.rs`'s existing `entrypoint script`
  indirection shape at `mod.rs:274-279`) and `supervise` (owns
  `PTRACE_SEIZE`, the handshake response, and the full unified wait loop, per
  `DEC-007`). One new binary-mode subcommand,
  `firma __exec-guarded-run` (`crates/firma/src/services/`, mirroring
  `egress_guarded_run.rs`'s shape exactly: thin, fails closed, Linux-only
  with an explicit non-Linux stub).

### `INV-EXEC-001`: Every `execve`/`execveat` reachable from the sandboxed root process is mediated by `AllowedExecutables` before it can complete

- Semantic predicate: for every process in the sandbox's process tree
  (root and all transitive descendants, however spawned — `fork`, `vfork`,
  `clone`, `posix_spawn`), any `execve`/`execveat` syscall it issues either
  completes only when its target path canonicalizes into
  `AllowedExecutables`, or fails (`-ENOSYS` via `DEC-015`) otherwise. This is
  the parent plan's `INV-001`, restated at this plan's proof granularity.
- Primary owner: `PtraceSeccompExec::supervise`'s unified wait loop.
- Detailed proof: see `PROOF-002` (argument-read correctness),
  `PROOF-005` (descendant-option-inheritance), `PROOF-006`
  (deny-mechanism correctness) below.

### `INV-EXEC-002`: The ptrace attacher never holds more privilege than `firma-run`'s host process already has

- Semantic predicate: `PTRACE_SEIZE` succeeds only via the real
  ancestor-through-`bwrap` relationship (no `CAP_SYS_PTRACE`, no elevated
  Yama scope grant). This is the parent plan's `INV-002`, unchanged.
- Primary owner: the `ptrace::seize` call site in `supervise`.
- Detailed proof: `PROOF-003` (parent plan, extended by `DEC-018`'s explicit
  gate here).

- Compatibility, migration, and failure semantics: unchanged from the parent
  plan (`execution_governance` defaults to `Inherited`; this strategy is
  additive and gated per `DEC-018`). References `DEC-007`, `DEC-018`.
- Durable documentation owner: unchanged from the parent plan —
  `docs/architecture/linux-local-command-enforcement.md`'s "Non-Cooperative
  Anti-Bypass Guarantees" section, updated once this slice ships.

## Implementation slices

### Slice 3a: Shim binary and filter install (no supervisor yet)

- Production, types, tests, and docs/config: new
  `crates/firma/src/services/exec_guarded_run.rs` (mirrors
  `egress_guarded_run.rs`); new
  `crates/firma-run/src/execution_governance/ptrace_seccomp.rs` module
  holding the raw BPF filter (`DEC-009`) and an `install_and_wait_for_ready`
  function: installs the filter, connects to the handshake socket, blocks
  for the readiness byte, then `execve`s the real target
  (`std::process::Command::exec`, matching `install_and_exec`'s shape).
  `firma __exec-guarded-run <socket-path> -- <command...>` CLI wiring
  mirrors `EgressGuardedRunArgs`.
- Affected decisions and traces: `DEC-009`, `DEC-011`.
- Proof obligations: none new at this slice — no supervisor exists yet to
  prove `INV-EXEC-001` against.
- Focused verification: a unit test that the filter-install function
  installs successfully and that connecting to an unreachable socket path
  fails closed (returns `Err`, never proceeds to `exec`) — same shape as
  `egress_guarded_run.rs`'s existing
  `run_fails_closed_when_supervisor_socket_is_unreachable` test.
- Dependencies: the parent plan's Slice 1 `ExecutionGovernor` trait must
  exist for this module to compile against it, but this slice's filter/shim
  logic is independently testable without it.
- Intentionally unsupported: no supervisor-side behavior; the shim will hang
  waiting for the handshake byte in any real run until Slice 3b lands (this
  slice is not independently shippable — it exists to isolate the filter/shim
  code for focused review before the more complex host-side loop is added).

#### Slice 3a implementation findings

Implemented and committed (`14dedb5f`). This slice's own focused
verification passed, and additionally surfaced a correction to an
assumption both prior plan-review rounds made without contest: the plan's
design assumed `SECCOMP_RET_TRACE` with no tracer attached behaves like
`SECCOMP_RET_ALLOW`. Measured directly (a real subprocess completing only
the byte handshake, deliberately with no `ptrace::seize`), that is false
on this kernel — the traced syscall returns `-ENOSYS` and does not execute
at all, matching `seccomp(2)`'s own documented description of
`SECCOMP_RET_TRACE` more closely than the plan's assumption did. This is
the _safer_ of the two possible behaviors (fail closed, not fail open),
locked in as a permanent regression test
(`tests/e2e/scenarios/exec_guarded_run.rs`,
`exec_guarded_run_fails_closed_without_a_real_ptrace_tracer`) rather than
left as a one-off observation.

### Slice 3b: Host-side attach, handshake, and unified wait loop

- Production, types, tests, and docs/config: `supervise()`'s core —
  `sandbox_child_pid` polling (`DEC-012`), a single atomic `ptrace::seize`
  carrying the full `DEC-013` option set, handshake-byte send, then the
  `waitpid`-based loop distinguishing `WaitStatus::PtraceEvent` by its event
  code (`PTRACE_EVENT_SECCOMP`, `PTRACE_EVENT_EXEC`,
  `PTRACE_EVENT_FORK`/`CLONE`/`VFORK`) from terminal exit, with
  `forward_signal` and the shared exit-code mapping reused (`DEC-014`,
  requires the `pub(crate)` visibility change on both
  `supervisor::forward_signal` and `supervisor::exit_code_from_outcome`). On
  every `PtraceEvent` except `PTRACE_EVENT_SECCOMP` (the fork/clone/vfork
  auto-attach stops and the `PTRACE_O_TRACEEXEC` stop that now fires on
  every successful exec, per `DEC-013`), immediately `PTRACE_CONT` — no
  decision needed, just continue past the event.
- Affected decisions and traces: `DEC-010`, `DEC-012`, `DEC-013`, `DEC-014`,
  `TRACE-003` (parent plan).
- Proof obligations: `PROOF-004` (parent plan — wait-loop unification;
  becomes concrete here), `PROOF-005` (new, below — descendant-option
  inheritance).
- Focused verification: an integration test spawning a small tree (a shell
  that backgrounds a child) under this strategy with an
  allow-everything `AllowedExecutables`, asserting every descendant's
  `execve` still completes (no false-positive denial) and that
  SIGINT/SIGTERM/SIGWINCH forwarding and exit/signal-death reporting match
  `wait_with_signal_forwarding`'s existing test assertions
  (`supervisor.rs`'s own test module is the direct behavioral template).
- Dependencies: Slice 3a (the shim must complete its handshake for this
  loop's happy path to be testable).
- Intentionally unsupported: allow/deny decisions (everything is implicitly
  allowed at this slice — the seccomp-stop branch just `PTRACE_CONT`s
  unconditionally); real denial lands in Slice 3c.

#### Slice 3b implementation findings

Implemented and committed (`66752c8f`). `PROOF-004`'s own focused test
(`tests/e2e/scenarios/child_process_governance/ptrace_seccomp_exec.rs`,
`ptrace_seccomp_governor_attaches_and_supervises_real_bwrap_sandbox`)
passes against a real `bwrap` sandbox, not a mock, and the full workspace
suite (2636 tests) shows no regressions.

Two real bugs surfaced only by that end-to-end test, not by unit tests or
`cargo clippy` — both fixed before this slice was committed:

1. **The handshake socket's own location was wrong.** The original design
   created it under an independent host `tempfile::tempdir()`. That
   directory is never bind-mounted into `bwrap`'s mount namespace, so the
   shim's `connect(2)` (running _inside_ the sandbox, per `DEC-011`'s own
   two-process design) failed `ENOENT` — the socket simply didn't exist
   from the sandboxed process's point of view. Fixed by placing the socket
   under `SandboxHandle::runtime_dir` instead — the same
   already-bind-mounted directory `egress_guard`'s own socket lives in.
   `ExecutionGovernor::rewrite_launch`'s signature gained a
   `sandbox_runtime_dir: &Path` parameter to make this available; this
   also let production code drop its dependency on `tempfile` entirely
   (that crate is now dev-only again in `firma-run`, as it was before this
   plan).
2. **`DEC-012`'s `sandbox_child_pid` polling attached to the wrong
   process.** With the socket-location bug fixed, the shim's own `execve`
   still failed `ENOSYS` — the same failure Slice 3a's test proves happens
   with _no_ tracer attached at all, meaning the `ptrace::seize` in this
   slice was not actually landing on the shim. `sandbox_child_pid`'s
   `/proc/<bwrap_pid>/task/<bwrap_pid>/children` read (fine for its
   existing best-effort signal-forwarding use, where a miss just delays
   one signal) named a different process than the one that was about to
   `execve` under a real `bwrap` launch. Fixed by reading the connecting
   process's real pid directly off the accepted handshake connection via
   `SO_PEERCRED` (`nix::sys::socket::sockopt::PeerCredentials`) instead —
   the kernel translates this into the _accepting_ process's own pid
   namespace automatically, so it names the shim exactly, with no polling
   or guessing. `poll_sandbox_child_pid` was removed entirely; `DEC-012`'s
   text above describes the design as originally accepted, superseded by
   this finding for the implementation itself.

Neither finding was anticipated by either plan-review round; both are the
kind of gap only a real `bwrap` launch (not a mock, not a unit test)
could surface — consistent with this plan's own stated Slice 3b/3c
verification requirement.

### Slice 3c: Decision logic and deny mechanism

- Production, types, tests, and docs/config: on a `PTRACE_EVENT_SECCOMP`
  stop, read the syscall-entry registers (`DEC-016`, `x86_64` first), read
  the pathname argument via `process_vm_readv` (`DEC-017`), canonicalize,
  check against `AllowedExecutables` (parent plan's Slice 1 type), and either
  `PTRACE_CONT` (allow) or rewrite the syscall number and `PTRACE_CONT`
  (deny, `DEC-015`). `DEC-018`'s "confirmed platforms" gate is checked once,
  at `supervise()` entry, before the first `seize`.
- Affected decisions and traces: `DEC-015`, `DEC-016`, `DEC-017`, `DEC-018`,
  `PROOF-001`/`PROOF-002` (parent plan, refined here).
- Proof obligations: `PROOF-002` (refined, below), `PROOF-006` (new,
  below — deny-mechanism correctness), `PROOF-003` (parent plan — the
  platform gate's negative path).
- Focused verification: the FIR-366 regression test itself
  (`tests/e2e/scenarios/child_process_governance/execution.rs`), run under
  `ptrace_seccomp_exec`; a dedicated test asserting the platform gate fails
  closed with a specific, named-cause error when the host is outside the
  confirmed allow-list (mocked, not a real unconfirmed host — matching
  `PROOF-003`'s `Controls/substitutions` row in the parent plan).
- Dependencies: Slice 3b.
- Intentionally unsupported: argv/context-based restriction (e.g. "allow
  `git status`, deny `git push`") — this strategy, like `LandlockExecute`, is
  path-based only, per `AllowedExecutables`'s scope in the parent plan.
  `aarch64` register access ships only once its own proof obligation
  (`DEC-016`) is independently satisfied — tracked as a follow-up, not part
  of this slice's acceptance outcome.

#### Slice 3c implementation findings

Implemented and committed (`9dd22a2f`, `1e33e0e9`, `26f4c8bd`). The FIR-366
acceptance test itself is the focused-verification requirement this slice
names, parametrized under `ptrace_seccomp_exec` as a new sibling test
(`ptrace_seccomp_exec_denies_forbidden_tool_as_child_of_allowed_bash_root`)
rather than a parametrization of `execution.rs`'s own `#[ignore]`d
`Inherited`-control test, since the two need different config (`execution_governance`
selected) and are clearer kept as separate, independently named scenarios —
both pass, and the control staying denied under `Inherited` while the very
same shape of scenario is now blocked under `PtraceSeccompExec` is the
actual acceptance proof, exercised end to end against a real `bwrap`
sandbox.

**`DEC-016`'s architecture ordering is inverted from what the plan
anticipated.** The plan's Choice paragraph frames `x86_64` as "first,"
with `aarch64` "gated behind its own proof obligation." The actual
implementation and verification environment for this whole plan is
`aarch64` (confirmed via `uname`/the e2e test's own logged
`target_arch=aarch64`) — so `aarch64` is the architecture with a real,
passing, real-hardware test, and `x86_64`'s register code (written,
compiles, `orig_rax`/`rax` per `user_regs_struct`'s real field layout) has
never run against real x86_64 hardware in this session. `DEC-018`'s gate
(`confirmed_platform()` in `crates/firma-run/src/execution_governance/ptrace_seccomp.rs`)
is implemented reflecting the actual evidence — `aarch64` confirmed,
every other architecture (including `x86_64`) rejected with an actionable
error — rather than the plan's assumed default ordering. `x86_64`
confirmation remains an explicit follow-up requiring real x86_64
hardware, not part of this implementation's own acceptance outcome.

**`DEC-018`'s gate is checked at `rewrite_launch`, not `supervise()`
entry** as the plan's Choice paragraph literally names. This fails closed
strictly earlier — before the sandboxed process is even spawned by the
backend, rather than after it starts but before this governor's first
`ptrace(2)` call — which the same "before any ptrace call is made"
rationale supports at least as well; recorded here as a deliberate,
stricter deviation from the literal text, not an oversight.

`DEC-017`'s multi-thread freeze (`freeze_thread_group_siblings`,
`thread_group_id`, `list_task_tids`) is implemented as specified —
enumerate-then-`PTRACE_INTERRUPT`-then-`waitpid`, looped until a full pass
adds nothing new — but has only been exercised against single-threaded
test processes (`bash`, `sh`); no test in this implementation actually
drives a genuinely multi-threaded tracee through a trap, so the
stabilization loop's own race-closing property is Implemented-and-Inferred-
correct against the documented kernel behavior `DEC-017` cites, not yet
Observed under real thread-creation contention. Flagged as a residual gap
for the post-implementation adversarial review, not silently closed.

`DEC-017`'s pathname-read design (bounded-chunk `process_vm_readv`,
NUL-search, short-read-without-NUL fails closed) is implemented as
`read_remote_cstring`; `execveat`'s `AT_EMPTY_PATH`/`dirfd`-relative cases
are implemented via `/proc/<tid>/fd/<dirfd>` and `/proc/<tid>/cwd`
(neither of which the plan's own Choice paragraph enumerated in this much
resolution-strategy detail — an implementation-level elaboration of
`DEC-017`, not a deviation from it), but only `execve`'s absolute/cwd-
relative path and a plain `execveat` were exercised by this
implementation's own tests; the `AT_EMPTY_PATH`/`fexecve`-style path has
no direct test, since no test scenario in this repository's e2e suite
currently drives an `fexecve`-based command launcher. Also flagged for
adversarial review rather than assumed correct from code inspection alone.

**`DEC-015`'s deny mechanism initially shipped broken on `aarch64`, and
the acceptance test above did not catch it** — a real post-implementation
adversarial review found that writing GPR `x8`
(`user_regs_struct.regs[8]`) through the ordinary `NT_PRSTATUS` regset
(what `nix::sys::ptrace::{getregs,setregs}` expose, and what
`set_syscall_number`'s `aarch64` arm originally did) has no effect on
which syscall `aarch64` actually dispatches — the kernel's dispatch
decision reads a separate `pt_regs.syscallno` field, populated from `x8`
once at syscall entry but never re-read from it afterward, reachable only
via the `NT_ARM_SYSTEM_CALL` (`0x404`) regset, which `nix` 0.31.3 does not
wrap. Denial had appeared to work (the FIR-366 test passed) purely by
accident: the _other_ register write in the same function (`x0` set to a
fake `-ENOSYS` return value) also lands on `execve`'s own first argument
register (or `execveat`'s `dirfd`), so the real syscall still ran and
failed on its own corrupted argument (`EFAULT`) rather than being skipped
(`ENOSYS`) as designed — a defect the original test's assertions (marker
file absent, forbidden output absent) could not distinguish from correct
behavior, since both failure modes produce the same _observable_ denial.

Fixed (`27d3d7bb`) by issuing a raw `PTRACE_SETREGSET` call against
`NT_ARM_SYSTEM_CALL` directly, and by adding a permanent regression
assertion on the FIR-366 test's own captured stderr distinguishing the
two failure modes (`"Function not implemented"` — the correct, genuine
skip — vs. `"Bad address"` — the accidental corruption bypass). A second,
independent review then verified the fix directly against this kernel's
own source (`arch/arm64/kernel/{ptrace,syscall}.c`, confirming
`REGSET_SYSTEM_CALL`'s `.set` writes `pt_regs.syscallno` and that
dispatch gates on it, not on `x8`) and reproduced both the original bug
(reverting to the pre-fix code reproduces the `EFAULT` failure) and the
fix (restoring it reproduces `ENOSYS`) against the real, running test —
not just a standalone C repro, though that was also done and matched.

This is the clearest evidence in this implementation that a plan's own
prose-level correctness claims (`DEC-015`'s "the kernel returns `-ENOSYS`
... instead of performing the real `execve`") and a passing acceptance
test are not sufficient proof for register-level, per-architecture
kernel-interaction code — only a reproduction against the actual kernel
source and observed behavior closed this gap.

**`DEC-019` (added post-review) mitigates, but does not eliminate, the
cross-process TOCTOU** the same review flagged: `handle_post_exec_verification`
now re-checks the actually-executing file (`/proc/<tid>/exe`, by
device+inode) at the `PTRACE_EVENT_EXEC` stop every successful exec
already produces, and kills the process on a mismatch. This closes the
_consequence_ (a swapped, unapproved binary running unchecked
indefinitely) but not the _window_ itself — the swapped file still runs
for the interval between the exec completing and this stop being
observed and acted on, which this design cannot make zero. The
comparison logic (`verify_post_exec_identity`) has its own direct unit
tests (matching, non-matching, and empty-allow-list cases, using this
test binary's own `/proc/<pid>/exe` as a real, non-racy stand-in for a
traced process); the full "a genuine concurrent race is actually
detected and killed" behavior has **not** been exercised end to end,
since — matching the original review's own admission that it "could not
construct a concrete cross-process race in the time available" —
deterministically forcing that exact race in a repository test would
require either a flaky timing-dependent test (rejected, matching this
repository's own testing culture) or a dedicated synchronization harness
that is its own, separate undertaking, not attempted here.

Outstanding before this plan can be considered fully delivered: RHEL/
CentOS Yama LSM presence remains Unknown, unchanged from the parent plan;
`x86_64` confirmation requires real hardware this session did not have
access to; closing the TOCTOU window itself (rather than only its
consequence, per `DEC-019`'s own "Consequences and rejected alternatives")
remains a follow-up, not part of this plan's delivered scope.

## Risks and gaps

- Existing risks (inherited from the parent plan, restated at this plan's
  granularity): RHEL/CentOS Yama presence remains Unknown — `DEC-018`'s gate
  must exclude RHEL-family hosts until directly verified. This is the single
  highest-impact unresolved item.
- Planned mitigations: `DEC-018`'s explicit allow-list plus Yama-scope
  surfacing in the failure message; slicing 3a/3b/3c so the highest-`unsafe`-
  density code (register read/write, `process_vm_readv`) is isolated to
  Slice 3c and reviewable independently of the attach/handshake plumbing.
- Explicit evidence gaps: (1) RHEL/CentOS Yama LSM presence — Unknown,
  unchanged from the parent plan; (2) `PTRACE_O_TRACEFORK`/`TRACECLONE`/
  `TRACEVFORK` option inheritance to auto-attached children — this plan's
  `DEC-013` states it as Inferred from documented kernel behavior, not yet
  Observed against this repository, and names `PROOF-005` as the obligation
  that must close it; (3) `aarch64` register-access correctness — no
  evidence either way yet, gated behind its own proof obligation per
  `DEC-016` rather than assumed to mirror `x86_64`; (4) whether
  `supervise()`'s `waitpid` call needs `nix::sys::wait::WaitPidFlag::__WALL`
  to reliably observe stop events, given `firma-run` is not the tracee's
  biological parent (bwrap is) — the flag exists in `nix` but its necessity
  in this specific non-parent-tracer configuration was not resolved by
  either review round; flagged by the second round as worth a targeted check
  before Slice 3b implementation, since an under-specified wait target is
  the same class of issue `PLAN-003`/`DEC-007` already had to fix once.
- Least-confident decisions: `DEC-013`'s inheritance-of-options claim (a
  documented kernel behavior this plan has not yet exercised in code) and
  `DEC-018`'s allow-list boundary (which hosts count as "confirmed" is a
  compatibility-matrix judgment call, not a technical derivation, and may
  need revision as real hosts are tested).

## Plan-review findings and dispositions

Independent review completed against repository revision `5e0dd567` on
`feat/backend-selection` (`adversarial-review` → `reviewing-plans`, a fresh
reviewer with no access to this plan's authoring rationale). All file:line
citations were independently re-verified against the repository and the
vendored `nix-0.31.3` source, not taken on faith.

```yaml
id: PLAN-101
severity: critical
category: security / proof-obligation correctness
classification: confirmed-conflict
claim: >
  DEC-017 and PROOF-002 assert that a PTRACE_EVENT_SECCOMP stop under
  PTRACE_SEIZE "halts the entire tracee including all its threads," and
  PROOF-002's Limits row attributes this to "PTRACE_SEIZE's documented
  group-stop semantics." This conflates two distinct, well-documented ptrace
  stop categories: a group-stop (triggered by stopping signals, which does
  stop every traced thread in the process) and a ptrace-event-stop
  (PTRACE_EVENT_FORK/CLONE/VFORK/EXEC/SECCOMP/etc., which per ptrace(2) is
  per-thread — only the thread that issued the trapping syscall stops;
  sibling threads in the same process continue running unless independently
  stopped). PTRACE_EVENT_SECCOMP is a ptrace-event-stop, not a group-stop, so
  the "all threads stop" premise is very likely false.
evidence:
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md (original DEC-017
    rationale and PROOF-002 stimulus/observable-effects/limits rows)
  - nix-0.31.3/src/sys/wait.rs:76-129 (WaitStatus::PtraceEvent(Pid, Signal,
    c_int) is per-tracee/per-pid, structurally identical in scope to
    Stopped(Pid, Signal))
  - Documented Linux ptrace(2) distinction between group-stop (whole
    thread-group) and PTRACE_EVENT stops/signal-delivery-stop (per-thread)
reachability: >
  A multi-threaded tracee spawns a helper thread before calling execve. The
  execve-issuing thread traps into PTRACE_EVENT_SECCOMP and stops; the
  helper thread is not stopped by that event and keeps running, and can
  repeatedly overwrite the pathname buffer between an allowed and a denied
  path, racing the supervisor's process_vm_readv read.
invariant_or_boundary: INV-EXEC-001
impact: >
  An adversarial agent process with a helper thread could defeat the
  allow/deny gate — the exact property FIR-366 and this strategy exist to
  close.
correction: >
  Do not state "halts the entire tracee" as settled reasoning; reclassify as
  corrected. Plan a concrete mitigation for the multithreaded case: enumerate
  and freeze every thread in the tracee's thread group before reading memory.
confidence: high
assumptions:
  - Based on documented Linux ptrace(2) semantics rather than a live-kernel
    reproduction in this environment.
```

```yaml
disposition:
  status: corrected
  rationale: >
    Confirmed high-confidence per documented ptrace(2) semantics (group-stop
    vs. per-thread ptrace-event-stop); the plan's own DEC-013 already hedges
    a comparable kernel-behavior claim as Inferred pending its own proof
    obligation, so DEC-017's inline "halts the entire tracee" assertion was
    inconsistent with the plan's own stated evidentiary standard elsewhere.
    Adopted the reviewer's suggested mitigation shape (enumerate every
    thread via /proc/<tgid>/task/, freeze any not already stopped via
    PTRACE_INTERRUPT before trusting the read) over the narrower alternative
    of scoping the strategy's guarantee to single-threaded processes, because
    many real agent runtimes are multithreaded well before their first
    execve and a scope restriction would silence the gap rather than close
    it for the workloads this strategy exists to cover.
  incorporated_at: "DEC-017 (rewritten), PROOF-002 (rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-102
severity: major
category: constructibility / missed visibility change
classification: confirmed-conflict
claim: >
  DEC-014 and the file-tree diff scope the only new visibility change to
  supervisor::forward_signal (private → pub(crate)). The parent plan's
  DEC-007 requires the new ptrace wait loop to share the same 128+signum
  mapping via the Slice-0-generalized exit-code function
  (exit_code_from_outcome), which is currently fully private — a module at
  execution_governance/ptrace_seccomp.rs cannot call it as-is.
evidence:
  - crates/firma-run/src/supervisor.rs:85-88 (exit_code_from_outcome, no pub
    qualifier)
  - docs/architecture/selectable-execution-governance-plan.md:96 (DEC-007's
    "share the same 128+signum mapping instead of duplicating it")
reachability: Reached at compile time by Slice 3b's implementer.
invariant_or_boundary: DEC-007 (parent plan)
impact: >
  The plan's own stated compliance with DEC-007 is not achievable with the
  visibility changes it lists — risk of undocumented scope creep or silent
  duplication.
correction: >
  Add exit_code_from_outcome to DEC-014's pub(crate) change and the
  file-tree diff's supervisor.rs entry, alongside forward_signal.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Direct, unambiguous constructibility gap; single clearly correct fix.
  incorporated_at: "DEC-014 (rewritten), file-tree diff, Current behavior and problem"
  decided_by: planner
```

```yaml
id: PLAN-103
severity: major
category: internal consistency / constructibility
classification: confirmed-conflict
claim: >
  DEC-013 sets PTRACE_O_TRACESECCOMP | PTRACE_O_TRACEFORK |
  PTRACE_O_TRACECLONE | PTRACE_O_TRACEVFORK. The already-accepted parent
  plan's DEC-008 specifies PTRACE_O_TRACESECCOMP | PTRACE_O_TRACEEXEC |
  PTRACE_O_TRACEFORK | PTRACE_O_TRACECLONE | PTRACE_O_TRACEVFORK. This child
  plan silently dropped PTRACE_O_TRACEEXEC with no stated rationale.
evidence:
  - docs/architecture/selectable-execution-governance-plan.md:100 (parent
    DEC-008, five-flag list including PTRACE_O_TRACEEXEC)
  - nix-0.31.3/src/sys/ptrace/linux.rs:308 (Options::PTRACE_O_TRACEEXEC
    present in the pinned nix version)
reachability: >
  Without PTRACE_O_TRACEEXEC, every allowed exec delivers a plain SIGTRAP
  signal-delivery-stop rather than a distinguishable PTRACE_EVENT_EXEC stop,
  which the unified wait loop must recognize and swallow by inference.
invariant_or_boundary: DEC-007's wait-loop unification correctness (PROOF-004)
impact: >
  Either an undocumented deviation gets resolved informally during
  implementation, or every allowed exec produces an unhandled plain-SIGTRAP
  stop the loop's "ordinary stops" bucket must correctly classify — easy to
  get wrong.
correction: >
  Restore PTRACE_O_TRACEEXEC to DEC-013's option set to match the accepted
  parent DEC-008, and specify how the resulting explicit
  PTRACE_EVENT_EXEC stop is classified and continued in the unified loop.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Restored to match the already-accepted parent DEC-008 exactly, per the
    reviewer's preferred resolution — gives the loop an explicit,
    classifiable event for every successful exec instead of relying on
    plain-SIGTRAP inference.
  incorporated_at: "DEC-013 (rewritten), Slice 3b description"
  decided_by: planner
```

```yaml
id: PLAN-104
severity: medium
category: internal consistency
classification: confirmed-conflict
claim: >
  DEC-011 states the shim connects "before installing the filter." DEC-012
  and Slice 3a both state the opposite order: filter installed, then
  handshake connect.
evidence:
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md (original DEC-011 vs.
    DEC-012/Slice 3a text)
  - crates/firma-run/src/egress_guard.rs:473-480 (the precedent DEC-011
    borrowed from connects before installing its filter specifically because
    that filter traps connect — a rationale that doesn't transfer to an
    execve-only filter)
reachability: An implementer following DEC-011's literal text would contradict
  Slice 3a's actual sequencing.
invariant_or_boundary: The readiness-handshake design itself.
impact: >
  Not a security race either way here, but a genuine unresolved contradiction
  between two decision records presented as jointly authoritative.
correction: >
  Pick filter-first (consistent with DEC-012/Slice 3a) and correct DEC-011's
  text, removing the borrowed-but-inapplicable framing.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Filter-first is the version consistent with DEC-012's attach-ordering
    proof narrative and avoids any window where the shim could reach execve
    before the filter exists; DEC-011 rewritten to match, with the
    egress_guard.rs rationale mismatch made explicit so a future reader
    doesn't reintroduce it.
  incorporated_at: "DEC-011 (rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-105
severity: medium
category: constructibility / reliability
classification: design-risk
claim: >
  DEC-017 proposes reading the pathname by "structurally reusing
  read_remote_mem... extended to read a NUL-terminated string." read_remote_mem
  is built around a known, exact length and fails any short read
  unconditionally; a NUL-terminated pathname has no a-priori known length, so
  naively inheriting that semantics could spuriously deny legitimate execs
  whose pathname straddles a page boundary.
evidence:
  - crates/firma-run/src/egress_guard.rs:210-236 (read_remote_mem: fixed
    length, any short read is a hard failure)
reachability: >
  Any exec whose pathname's final bytes plus NUL straddle a page boundary
  with the following page unmapped — plausible in ordinary use, not only
  adversarial.
invariant_or_boundary: PROOF-002, TRACE-004's "indistinguishable from
  Inherited" success outcome.
impact: >
  Legitimate, allow-listed execs could be spuriously denied — a reliability
  regression for no security benefit.
correction: >
  Specify the actual read algorithm: capped maximum length, search for NUL
  within whatever was successfully returned, only fail when no NUL is found
  in a short read.
confidence: medium
assumptions:
  - Assumes the implementation would otherwise copy read_remote_mem's
    short-read-is-failure branch verbatim.
```

```yaml
disposition:
  status: corrected
  rationale: >
    Adopted the reviewer's suggested bounded-chunk-with-in-prefix-NUL-search
    algorithm, folded into the same DEC-017 rewrite that addresses PLAN-101
    (both concern the same read path).
  incorporated_at: "DEC-017 (rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-106
severity: low
category: constructibility / simplification opportunity
classification: design-risk
claim: >
  DEC-012/DEC-013 describe attaching in two steps (seize, then a separate
  setoptions call), but nix 0.31.3's seize signature is
  `seize(pid: Pid, options: Options) -> Result<()>`, applying options
  atomically as part of the single PTRACE_SEIZE syscall — a strictly
  simpler, atomic one-call alternative the plan didn't use.
evidence:
  - nix-0.31.3/src/sys/ptrace/linux.rs:646-659 (seize's actual signature)
reachability: n/a — design-clarity gap, not a runtime bug.
invariant_or_boundary: INV-EXEC-001's attach-ordering reasoning.
impact: >
  Not unsafe as originally described, but misses a simplification nix's own
  API already offers, leaving an unnecessary intermediate
  attached-but-unconfigured state in the design.
correction: >
  Collapse to a single seize(pid, all_options) call.
confidence: medium
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Direct API-level simplification with no identified downside; adopted as described.
  incorporated_at: "DEC-012, DEC-013 (both rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-107
severity: low
category: technical accuracy
classification: unverified-hypothesis
claim: >
  DEC-011's rationale states a missed/mistimed attach means "zero
  enforcement," framing the risk as a fail-open bypass. Per documented
  SECCOMP_RET_TRACE semantics, when no tracer is attached the kernel does not
  execute the syscall and returns -ENOSYS — i.e. it fails closed, not open.
evidence:
  - Documented SECCOMP_RET_TRACE semantics (seccomp_filter.rst): "If there is
    no tracer present, the system call is not executed and -ENOSYS is
    returned"
reachability: n/a — documentation-accuracy concern.
invariant_or_boundary: DEC-011's stated rationale.
impact: >
  Inverts the actual risk direction, which could misdirect review/test
  priority toward the wrong obligation as primary.
correction: >
  Correct DEC-011's rationale: a missed/mistimed attach causes spurious
  denial of legitimate execs during the startup window (reliability), not
  bypass.
confidence: medium
assumptions:
  - Based on documented kernel behavior, not independently re-verified
    against a running kernel in this environment.
```

```yaml
disposition:
  status: corrected
  rationale: >
    Confirmed against documented seccomp semantics; DEC-011 rewritten to
    state the risk as spurious-denial/reliability rather than bypass. This
    also means the real fail-open-shaped risk in this plan is PLAN-101's
    (now-corrected) TOCTOU gap, not the handshake — noted for future
    reviewers' prioritization.
  incorporated_at: "DEC-011 (rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-108
severity: low
category: citation accuracy
classification: confirmed-conflict
claim: >
  DEC-016 cites "nix::sys::ptrace::getregset::<NtPrStatus>/
  AArch64RegisterSet-style API." The actual type in nix-0.31.3 is
  `nix::sys::ptrace::regset::NT_PRSTATUS`; there is no NtPrStatus or
  AArch64RegisterSet type in the crate.
evidence:
  - nix-0.31.3/src/sys/ptrace/linux.rs:264-269 (NT_PRSTATUS, RegisterSet impl)
reachability: n/a — citation naming inaccuracy.
invariant_or_boundary: none.
impact: >
  Low; the underlying conclusion (aarch64 needs its own gated proof) is
  correct, but the cited type name doesn't exist.
correction: >
  Correct the citation; clarify that nix already abstracts the
  GETREGS-vs-GETREGSET choice behind uniform getregs/setregs on both
  architectures — the arch-visible surface to gate is user_regs_struct's
  field layout, not the nix function surface.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Direct factual correction, adopted verbatim including the reviewer's clarified framing of what actually needs gating.
  incorporated_at: "DEC-016 (rewritten)"
  decided_by: planner
```

The reviewer also confirmed (no finding needed): all other file:line citations
against `runtime/mod.rs`, `linux_bwrap/mod.rs`, `supervisor.rs`,
`egress_guard.rs`, and `crates/firma/src/services/egress_guarded_run.rs` are
accurate; `nix` 0.31.3 exposes `seize`, `setoptions`, `getevent`, `cont`,
`getregs`/`setregs`/`getregset`, the five `Options::PTRACE_O_TRACE*` flags,
`Event::PTRACE_EVENT_SECCOMP`, and `WaitStatus::PtraceEvent` as claimed;
`firma-run/Cargo.toml:45` confirmed to enable `nix` features `["process",
"signal", "socket", "uio"]` only, not `"ptrace"`; `DEC-013`'s
descendant-option-inheritance premise matches documented, correct ptrace(2)
behavior and was already appropriately hedged as Inferred pending
`PROOF-005`; zero repository matches for `ExecutionGovernance`/
`ExecutionGovernor`/`PtraceSeccompExec`/`LandlockExecute` confirm Slice 1/2/3
genuinely haven't landed yet.

### Second review round

A second, independent review pass (fresh reviewer, no access to this
document's authoring history) re-verified all `PLAN-101`-`108` corrections
above as genuinely incorporated into the current `DEC-*` prose, re-checked
the underlying `nix` API evidence independently, and surfaced three new
findings that survived the first round.

```yaml
id: PLAN-109
severity: major
category: technical accuracy / trust-boundary reasoning
classification: confirmed-conflict
claim: >
  DEC-017's Choice paragraph stated sibling threads "were already seized
  individually, since PTRACE_SEIZE on a multi-threaded process attaches to
  the whole thread group." This is false: ptrace attach (via PTRACE_ATTACH
  or PTRACE_SEIZE) is per-thread, not per-thread-group. Only threads created
  after attach are auto-attached, and only when PTRACE_O_TRACECLONE is set.
evidence:
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md, DEC-017 Choice
    paragraph (original wording)
  - man 2 ptrace, "Attaching and detaching": attach is per-thread for both
    PTRACE_ATTACH and PTRACE_SEIZE
reachability: >
  The claim happens to be practically survivable in this specific design
  only because seize always targets the shim at a point where it is
  provably single-threaded (pre-execve), with every later thread
  auto-attached via PTRACE_O_TRACECLONE — but the plan stated a false
  premise instead of this actual justification, which would mislead any
  future change to the attach point.
invariant_or_boundary: INV-EXEC-001; PROOF-002's TOCTOU closure argument.
impact: >
  Not a bypass in the design as scoped today, but a false premise about the
  underlying kernel primitive that could misdirect a future reviewer or
  implementer reasoning about a related change.
correction: >
  Replace the false rationale with the real one: siblings are covered
  because the tracee is single-threaded at seize time plus
  PTRACE_O_TRACECLONE auto-attachment, not because SEIZE attaches to the
  whole group.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Confirmed against documented ptrace(2) semantics. DEC-017's Choice and
    Rationale paragraphs rewritten to state the actual justification
    (single-threaded-at-seize plus PTRACE_O_TRACECLONE) instead of the false
    whole-group-attach claim.
  incorporated_at: "DEC-017 (rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-110
severity: major
category: proof-obligation soundness / TOCTOU closure
classification: confirmed-conflict
claim: >
  DEC-017's Choice text specified a single enumerate-then-freeze pass over
  /proc/<tgid>/task/, with no requirement to re-scan until the thread set
  stabilizes. The re-scan-until-stable step — the only thing that actually
  closes the race where a thread is created between the enumeration pass and
  the freeze completing — appeared only in PROOF-002's Failure cases/Limits
  rows as a "must be checked" caveat, not as a mandated design step.
evidence:
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md, DEC-017 Choice
    (single pass, no stabilization loop, original wording)
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md, PROOF-002 Failure
    cases row (the actual closing mechanism, stated only as a caveat)
reachability: >
  A helper thread spawned concurrently with the supervisor's first
  enumeration pass (created after the directory listing is read but before
  the freeze pass finishes) would not be enumerated or frozen, and could
  overwrite the pathname buffer before the read — reopening the TOCTOU race
  for a realistic timing, not an exotic one.
invariant_or_boundary: INV-EXEC-001; PROOF-002.
impact: >
  As originally specified, DEC-017's mitigation was incomplete unless an
  implementer independently noticed and applied the appendix caveat rather
  than following the Choice text as written — the actual race-closing
  mechanism was effectively hidden in a proof appendix instead of stated as
  part of the design.
correction: >
  Fold the re-scan-until-stable requirement into DEC-017's Choice paragraph
  as a mandatory step.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Direct, unambiguous design-completeness gap. DEC-017's Choice paragraph
    rewritten to mandate the enumerate-freeze stabilization loop explicitly,
    rather than leaving it as a proof-appendix caveat.
  incorporated_at: "DEC-017 (rewritten)"
  decided_by: planner
```

```yaml
id: PLAN-111
severity: major
category: proof-obligation reachability / slice scoping
classification: confirmed-conflict
claim: >
  PROOF-005 (descendant-option-inheritance) was assigned to Slice 3b and
  required observing an actual denial ("the grandchild's execve is trapped
  and denied exactly as the root's would be"). But Slice 3b's own
  "Intentionally unsupported" line states denial logic doesn't exist until
  Slice 3c — Slice 3b, as scoped, has no deny mechanism to produce the
  observable effect PROOF-005 required.
evidence:
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md, Slice 3b
    "Intentionally unsupported" (real denial lands in Slice 3c)
  - docs/architecture/ptrace-seccomp-exec-gate-plan.md, PROOF-005 (original
    Stimulus/Observable effects requiring a denial, tagged Slice 3b)
reachability: >
  Directly reachable by any implementer attempting to close PROOF-005 during
  Slice 3b per its own slice tag.
invariant_or_boundary: >
  INV-EXEC-001 (extended to descendants); the Slice 3a/3b/3c
  independent-verifiability claim.
impact: >
  Either Slice 3b's stated proof obligations were not actually closeable
  within that slice's own scope, or an implementer would quietly pull
  deny-logic scope forward into 3b, blurring the 3b/3c split the plan
  otherwise argues for.
correction: >
  Redefine PROOF-005 for Slice 3b to require only that the grandchild's
  execve produces an observed PTRACE_EVENT_SECCOMP stop the
  unconditional-allow loop correctly continues past (proving the trap
  reaches the tracer two generations deep), deferring the "and is actually
  denied" half to a two-levels-deep variant of PROOF-006 once Slice 3c
  lands.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Adopted the reviewer's option (a): PROOF-005 now proves trap-delivery
    depth only, consistent with Slice 3b's actual scope; PROOF-006 gains an
    explicit two-levels-deep variant so the "deep descendant + real deny"
    composition is still proven, just at Slice 3c where deny logic actually
    exists.
  incorporated_at: "PROOF-005 (rewritten), PROOF-006 (extended)"
  decided_by: planner
```

```yaml
id: PLAN-112
severity: low
category: citation accuracy
classification: confirmed-conflict
claim: >
  The child plan cited "egress_guard.rs:314-362" for CONNECT_NOTIFY_PROG.
  The array itself runs 314-357; lines 358-362 are a section-header comment
  and the start of a different function's doc comment.
evidence:
  - crates/firma-run/src/egress_guard.rs:314 (const declaration), :357
    (closing `];`)
reachability: n/a — citation-range looseness only.
invariant_or_boundary: none.
impact: Trivial.
correction: Tighten the citation to 314-357.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: Direct factual correction.
  incorporated_at: "Current behavior and problem section"
  decided_by: planner
```

The second reviewer also confirmed (no finding needed): every `PLAN-101`-`108`
correction from the first round is genuinely present in the current `DEC-*`
prose, not merely acknowledged in a disposition block; all sampled
`file:line` citations against `runtime/mod.rs`, `supervisor.rs`,
`linux_bwrap/mod.rs`, `egress_guard.rs`, `egress_guarded_run.rs`, and
`seccomp.rs` are accurate; the `nix` 0.31.3 API surface (`seize`,
`getregs`/`setregs`/`getregset`, `regset::NT_PRSTATUS`, the five
`Options::PTRACE_O_TRACE*` flags, `WaitStatus::PtraceEvent`) matches every
claim made about it; `seccompiler` is confirmed absent from every workspace
`Cargo.toml`, supporting `DEC-009`'s (corrected) rationale; the Trust
analysis and Vocabulary "Not applicable" markers were independently
challenged and found defensible.

**Explicit gaps the second reviewer flagged but did not resolve** (not
findings — open questions for implementation time, not design defects):
whether `supervise()`'s `waitpid` call needs `nix::sys::wait::WaitPidFlag::
__WALL` to reliably observe stop events given `firma-run` is not the
tracee's biological parent (bwrap is) — confirmed present in `nix` but not
confirmed necessary or unnecessary from documentation alone; this is exactly
the class of issue `PLAN-003`/`DEC-007` already had to fix once for the
reaper-unification problem, so it is worth a targeted check before Slice 3b
implementation rather than assuming either default is correct.

## Final verification

- Focused checks: per-slice, as listed above (3a/3b/3c).
- Workspace checks: `just check` after each slice lands, per repository
  convention; `cargo clippy -p firma-run -p firma --all-features --all-targets`
  focused runs during iteration given the `unsafe_code = deny` /
  `unwrap_used = deny` workspace lints apply in full to this plan's new
  `unsafe` surface (register access, `process_vm_readv`, raw `ptrace`
  syscalls) — every `unsafe` block needs a `// SAFETY:` comment scoped and
  reasoned per the `egress_guard.rs` precedent (`DEC-004` in the parent
  plan).
- Post-implementation independent review: required per
  `adversarial-review`, in addition to this plan's own pre-implementation
  review — this is explicitly the parent plan's "highest-complexity,
  highest-risk slice."

## Technical evidence

### Applicability assessment

| Section                     | Applicability  | Reason or evidence                                                               |
| --------------------------- | -------------- | -------------------------------------------------------------------------------- |
| Vocabulary                  | Not applicable | No new terms beyond the parent plan's "Ptrace-seccomp exec-gate" vocabulary row  |
| Alternatives                | Applicable     | Plain seccomp-notify-only was a live alternative, considered and decided against |
| File-tree diff              | Applicable     | New shim binary and module                                                       |
| Type and signature sketches | Applicable     | Register-access and BPF-filter shapes affect review decisions                    |
| Semantic call traces        | Applicable     | Refines the parent plan's `TRACE-003` into per-slice detail                      |
| Trust analysis              | Not applicable | Unchanged from the parent plan's trust analysis                                  |
| Detailed proof obligations  | Applicable     | New proof obligations (`PROOF-005`, `PROOF-006`) plus refinement of `PROOF-002`  |

### Conditional: Alternatives

- **Plain `SECCOMP_RET_USER_NOTIF` on `execve`/`execveat`, no ptrace at all**
  (shape/owner: a supervisor thread structurally identical to
  `egress_guard.rs`'s existing `connect` guard, gating `execve` instead).
  Benefits: eliminates the Yama/RHEL-unknown risk entirely (no ancestor-tracer
  relationship required — the listener fd is handed over explicitly via
  `SCM_RIGHTS`, exactly as the egress guard already does); eliminates
  `DEC-007`'s wait-loop-unification problem (the notify supervisor runs on
  its own thread alongside `wait_with_signal_forwarding`, unmodified, exactly
  as the egress guard does today); eliminates the `PTRACE_O_TRACEFORK`
  inheritance Unknown (`DEC-013`/`PROOF-005`) since seccomp filters inherit
  through `fork`/`exec` automatically without any explicit descendant-
  tracking mechanism. Costs: none identified that are unique to this
  alternative for the connect()-shaped case, but it inherits the same class
  of memory-content TOCTOU this plan closes in `DEC-017` — it is not actually
  free of that risk, only free of the ptrace-specific attach/Yama risk.
  Compatibility/migration consequences: same `AllowedExecutables`-based,
  path-only decision surface; same `firma __exec-guarded-run`-shaped shim
  (differing only in filter flags and the fd-handoff-vs-PID-attach
  mechanics). Evidence: `egress_guard.rs` already implements the entire
  mechanism for a different trapped syscall; adapting it to `execve` is a
  smaller, more precedented change than the ptrace design. Disposition:
  presented to the user in this session as a direct choice against the full
  ptrace design; the user chose to retain the ptrace design. No technical
  defect was found in the plain-seccomp-notify alternative — the decision
  not to adopt it is a user choice, not a correction of a flawed proposal,
  and is recorded here so a future planning pass does not need to rediscover
  it.

### Conditional: File-tree diff

```diff
 crates/firma/src/services/
+├── exec_guarded_run.rs          # NEW — `firma __exec-guarded-run` shim wrapper, mirrors egress_guarded_run.rs
 crates/firma-run/src/
~├── supervisor.rs                 # MODIFIED — `forward_signal` and `exit_code_from_outcome` become `pub(crate)` (DEC-014)
 crates/firma-run/src/execution_governance/
+└── ptrace_seccomp.rs             # NEW — filter, shim-side install/handshake, host-side supervise() (Slice 1's module, per parent plan)
 crates/firma-run/Cargo.toml
~└── (deps)                        # MODIFIED — enable `nix`'s "ptrace" feature (DEC-010)
 tests/e2e/scenarios/child_process_governance/
~└── execution.rs                  # MODIFIED — un-ignore/parametrize the FIR-366 test under this strategy (parent plan's Slice 4 territory; this plan's Slice 3c exercises the same test directly)
```

### Conditional: Types and signatures

```rust
// crates/firma-run/src/execution_governance/ptrace_seccomp.rs

/// Raw BPF program trapping only execve/execveat via SECCOMP_RET_TRACE;
/// SECCOMP_RET_ALLOW otherwise. Structurally mirrors
/// `egress_guard.rs::CONNECT_NOTIFY_PROG`.
const EXEC_TRACE_PROG: [libc::sock_filter; N] = [ /* ... */ ];

/// Shim-side: install the filter, hand the readiness handshake, exec.
/// Never returns on success. Mirrors `egress_guard::install_and_exec`'s
/// fail-closed shape.
pub fn install_and_wait_for_ready(
    handshake_socket: &Path,
    argv: &[String],
) -> Result<std::convert::Infallible, RunError>;

/// Host-side: seize, set options, signal readiness, own the wait loop.
/// This is `PtraceSeccompExec::supervise`'s body.
fn supervise_ptrace_loop(
    bwrap_pid: u32,
    handshake_socket: &Path,
    allowed: &AllowedExecutables,
    backend: BackendKind,
) -> Result<ExitStatus, RunError>;
```

**Constructibility note**: `supervise_ptrace_loop` takes `bwrap_pid`, not the
already-resolved inner PID — `sandbox_child_pid`'s polling happens _inside_
this function (`DEC-012`), not before it, so there is no way to construct a
call with a stale or wrong inner PID from outside; the only externally
supplied PID is bwrap's own, which is unambiguous (it is exactly the `Child`
`start_agent` returned).

### Conditional: Semantic call traces

| Field                      | `TRACE-004` (Slice 3a/3b, happy path, refines parent `TRACE-003`)                                                                                                                                                                                                                                                                                                                                                 |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Proposed                                                                                                                                                                                                                                                                                                                                                                                                          |
| Entry and stimulus         | `execute_run` under `ptrace_seccomp_exec`; root command spawns an allowed descendant                                                                                                                                                                                                                                                                                                                              |
| Path                       | `rewrite_launch` points `LaunchSpec` at `firma __exec-guarded-run` → `backend.start_agent` spawns bwrap → shim installs `EXEC_TRACE_PROG`, connects handshake socket → `supervise` polls `sandbox_child_pid`, `seize`+options, sends readiness byte → shim `execve`s real root command → descendant `execve` traps → `PTRACE_EVENT_SECCOMP` stop → `supervise` reads registers+pathname → allowed → `PTRACE_CONT` |
| Input/output types         | `u32` (bwrap pid) → `u32` (inner pid) → ptrace stop → raw registers + remote-memory bytes → `AllowedExecutables` lookup → `PTRACE_CONT`                                                                                                                                                                                                                                                                           |
| Validation/trust crossings | One `AllowedExecutables` check per trapped `execve`, host-trusted, tracee-untrusted (`DEC-017`)                                                                                                                                                                                                                                                                                                                   |
| Invariant established      | `INV-EXEC-001` for this specific `execve`                                                                                                                                                                                                                                                                                                                                                                         |
| Invariant assumed          | `DEC-013`'s option-inheritance claim (not yet proven — `PROOF-005`)                                                                                                                                                                                                                                                                                                                                               |
| Success outcome            | Descendant `execve` proceeds normally, indistinguishable from `Inherited` except for the one-time attach overhead                                                                                                                                                                                                                                                                                                 |
| Failure path               | N/A — this is the allow path                                                                                                                                                                                                                                                                                                                                                                                      |
| Evidence                   | Slice 3b/3c integration tests                                                                                                                                                                                                                                                                                                                                                                                     |
| Proof boundary             | Integration + e2e                                                                                                                                                                                                                                                                                                                                                                                                 |
| Unknowns                   | None beyond `PROOF-005`                                                                                                                                                                                                                                                                                                                                                                                           |

| Field                      | `TRACE-005` (Slice 3c, deny path)                                                                                                                                                                                                                                     |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Proposed                                                                                                                                                                                                                                                              |
| Entry and stimulus         | Same as `TRACE-004`, descendant `execve`s a non-allow-listed path                                                                                                                                                                                                     |
| Path                       | Same up through the `PTRACE_EVENT_SECCOMP` stop, then: pathname resolves outside `AllowedExecutables` → register rewrite (`DEC-015`) → `PTRACE_CONT` → kernel returns `-ENOSYS` for the neutralized syscall to the tracee                                             |
| Input/output types         | Same as `TRACE-004` through the lookup, then a register write instead of a bare continue                                                                                                                                                                              |
| Validation/trust crossings | Same as `TRACE-004`                                                                                                                                                                                                                                                   |
| Invariant established      | `INV-EXEC-001`, negative case                                                                                                                                                                                                                                         |
| Invariant assumed          | None new                                                                                                                                                                                                                                                              |
| Success outcome            | The tracee's `exec` call fails; the tracee process continues running (does not proceed into the denied program) and observes a failed `exec` exactly as it would from an ordinary `-ENOSYS`                                                                           |
| Failure path               | This row's "success" and "failure" collapse into one outcome by design — a denied `execve` failing is the intended behavior, not an error condition for the supervisor itself                                                                                         |
| Evidence                   | Slice 3c's FIR-366 regression test                                                                                                                                                                                                                                    |
| Proof boundary             | e2e                                                                                                                                                                                                                                                                   |
| Unknowns                   | `PROOF-006` (below) — whether `-ENOSYS` from a neutralized `execve` is observably distinguishable, by the calling shell/program, from a genuine "command not found," matching `TRACE-003`'s "observable failure matches today's seccomp-deny shape" success criterion |

| Field                      | `TRACE-006` (Slice 3b, platform-gate failure path)                                                                               |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| State                      | Proposed                                                                                                                         |
| Entry and stimulus         | `execution_governance = "ptrace_seccomp_exec"` selected on a host outside `DEC-018`'s confirmed allow-list                       |
| Path                       | `supervise` checked before `seize` → allow-list lookup fails → `RunError` returned, launch aborted, wrapped command never starts |
| Input/output types         | Host platform signal (distro/kernel identification) → `bool` → `RunError`                                                        |
| Validation/trust crossings | None — pure host-fact check                                                                                                      |
| Invariant established      | `INV-EXEC-002`'s negative case (fails closed rather than attempting attach on an unconfirmed host)                               |
| Invariant assumed          | None                                                                                                                             |
| Success outcome            | N/A — this is the failure path being proven                                                                                      |
| Failure path               | Actionable error naming the specific unconfirmed characteristic, per `DEC-018`                                                   |
| Evidence                   | Slice 3c's dedicated platform-gate test (mocked host facts)                                                                      |
| Proof boundary             | Unit/config                                                                                                                      |
| Unknowns                   | Exact allow-list contents — an open decision (Scope section), not a design gap                                                   |

### Conditional: Detailed proof obligations

| Field                  | `PROOF-002` (refined from the parent plan — TOCTOU on the `execve` pathname read, corrected after `PLAN-101`)                                                                                                                                                                                                                                                                                                                                                        |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-EXEC-001`                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Kind                   | Runtime / Trust                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Owner/proof boundary   | `ptrace_seccomp.rs`'s per-thread freeze plus `process_vm_readv`-based pathname read (`DEC-017`)                                                                                                                                                                                                                                                                                                                                                                      |
| Suite/boundary         | Integration (Slice 3c) + a dedicated adversarial test                                                                                                                                                                                                                                                                                                                                                                                                                |
| Stimulus               | A malicious tracee with a second thread that overwrites the pathname buffer's memory between the `PTRACE_EVENT_SECCOMP` stop and the supervisor's `process_vm_readv` call, attempting to have an allowed path checked but a different (denied) path actually executed                                                                                                                                                                                                |
| Observable effects     | Because `PTRACE_EVENT_SECCOMP` is a per-thread ptrace-event-stop, not a whole-process group-stop, sibling threads are not stopped by the event itself — `DEC-017`'s per-thread enumeration and freeze (via `/proc/<tgid>/task/` plus `PTRACE_INTERRUPT` on any not-yet-stopped sibling) is required precisely because this race _is_ reachable without it. This test must confirm the freeze step actually blocks the race, not merely that the read itself succeeds |
| Controls/substitutions | A multi-threaded fixture attempting the memory-overwrite race, run both with and without the freeze step (temporarily) disabled, to confirm the fixture would have detected the race absent the mitigation                                                                                                                                                                                                                                                           |
| Failure cases          | If thread enumeration via `/proc/<tgid>/task/` misses a thread created concurrently with the freeze pass (a TOCTOU on the enumeration itself), the freeze is incomplete and the underlying race reopens — this must be checked, e.g. by re-scanning `/proc/<tgid>/task/` after the first freeze pass until it stabilizes                                                                                                                                             |
| Evidence               | New adversarial test, Slice 3c                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Status                 | Gap                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Slice                  | 3c                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Limits                 | Proves the specific overwrite race is closed by the freeze-then-read sequence; does not prove absence of every conceivable adversarial timing (e.g. a thread created after enumeration stabilizes but before the read — the re-scan-until-stable control above is this proof's primary defense against that, but is not itself formally exhaustive)                                                                                                                  |

| Field                  | `PROOF-005` (new — descendant option inheritance, `DEC-013`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-EXEC-001`, extended to descendants beyond the immediate child                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Kind                   | Runtime                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Owner/proof boundary   | `ptrace::setoptions` call in `supervise` (`DEC-012`/`DEC-013`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Suite/boundary         | Integration (Slice 3b)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Stimulus               | The traced root process forks a child, which itself forks a grandchild, which attempts an `execve` (allow-everything policy, per Slice 3b's own scope — see `Limits`)                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Observable effects     | The grandchild's `execve` produces an observed `PTRACE_EVENT_SECCOMP` stop that the unconditional-allow loop correctly recognizes and continues past, with no explicit per-descendant `setoptions` call in the supervisor's code — proving the trap itself reaches the tracer two generations deep                                                                                                                                                                                                                                                                                             |
| Controls/substitutions | A test fixture forking two levels deep before attempting the exec                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Failure cases          | If options do not propagate automatically, the grandchild's `execve` never traps at all (no `PTRACE_EVENT_SECCOMP` observed) — a silent reopening of the FIR-366 gap this whole strategy exists to close, specifically for grandchildren+                                                                                                                                                                                                                                                                                                                                                      |
| Evidence               | New test, Slice 3b                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Status                 | Gap — `DEC-013`'s claim is Inferred, not yet Observed                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Slice                  | 3b (trap-reaches-tracer half only; the "and is actually denied" half is `PROOF-006`'s territory once Slice 3c's deny mechanism exists — Slice 3b has no deny logic per its own "Intentionally unsupported" line, so this proof cannot require an observed denial, corrected per `PLAN-111`)                                                                                                                                                                                                                                                                                                    |
| Limits                 | Proves two levels of descent for trap-delivery only; does not prove arbitrarily deep trees, though the mechanism (option inheritance is unconditional per the kernel, not depth-limited) gives no reason to expect a depth-dependent failure. Does not by itself prove the grandchild's `execve` would actually be _denied_ if disallowed — that composition (deep descendant + real deny decision) is `PROOF-006`'s job once Slice 3c lands, and should get its own two-levels-deep variant there rather than being assumed transitively from this proof plus `PROOF-006`'s single-level one. |

| Field                  | `PROOF-006` (new — deny-mechanism observable correctness, `DEC-015`)                                                                                                                                                                                                                                                                       |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Invariant              | `INV-EXEC-001`'s negative case                                                                                                                                                                                                                                                                                                             |
| Kind                   | Runtime                                                                                                                                                                                                                                                                                                                                    |
| Owner/proof boundary   | The register-rewrite-before-`PTRACE_CONT` logic (`DEC-015`)                                                                                                                                                                                                                                                                                |
| Suite/boundary         | E2E (Slice 3c) — the FIR-366 regression test itself                                                                                                                                                                                                                                                                                        |
| Stimulus               | Same as `PROOF-001`/`PROOF-002` (parent plan): root spawns a denied descendant                                                                                                                                                                                                                                                             |
| Observable effects     | No marker file the denied tool would have written appears (same fixture as `PROOF-001`); the calling shell observes a nonzero exit / "command not found"-shaped failure, not a hang, crash, or partial execution                                                                                                                           |
| Controls/substitutions | Same `FORBIDDEN_MARKER`/`write_forbidden_tool` fixture as `PROOF-001`                                                                                                                                                                                                                                                                      |
| Failure cases          | The neutralized syscall number happens to be a _valid_ syscall with unrelated side effects on some kernel/arch combination, rather than a clean `-ENOSYS` — must be checked, not assumed, since the specific "invalid number" chosen matters                                                                                               |
| Evidence               | New e2e test, Slice 3c; a second variant two levels deep (grandchild, composing with `PROOF-005`'s trap-delivery evidence) rather than only the root-level case, per `PLAN-111`'s correction — depth of descent and the deny decision itself were previously proven by two different, non-composed proofs and this variant closes that gap |
| Status                 | Gap                                                                                                                                                                                                                                                                                                                                        |
| Slice                  | 3c                                                                                                                                                                                                                                                                                                                                         |
| Limits                 | Proves this specific denial shape is observably safe on the tested platform(s), for both a direct child and (via the added variant) a grandchild; cross-architecture parity depends on `DEC-016`'s own gate                                                                                                                                |
