# `HakoniwaBackend`: reconstructing `/etc` as a writable layer

## Artifact metadata

- Status: Accepted (independent plan review complete; all six findings
  addressed — see "Plan-review findings and dispositions")
- Durable locator: `docs/architecture/hakoniwa-etc-reconstruction-plan.md` (this
  file, in-repo, `openfirma-all-backends` worktree)
- Repository revision researched: `6b974040e894f773956829e842416bb3f88c71a1`
  (`openfirma-all-backends`, branch `exp/all-backends`)
- Task or requirement source: user planning request (2026-09-15), which itself
  cites and continues the open gap recorded in
  `docs/architecture/hakoniwa-backend-plan.md`'s Slice 2 "Implemented" notes
  ("Resolv.conf: still blocked, no equivalent alternate mechanism found" and
  "closing this needs `/etc` reconstructed as a fresh, writable layer ... a
  design decision for a future slice, not scoped here") and the matching
  comment in `crates/firma-run/src/backend/hakoniwa/mod.rs`'s `prepare` (lines
  76-104).
- Supersedes: Not applicable. Extends, and cross-references,
  `docs/architecture/hakoniwa-backend-plan.md` (Slice 2, `INV-002`) without
  modifying its accepted content. That document has been updated with a
  pointer to this one (see "Cross-references" below).

## Goal and acceptance outcomes

- Goal: give `HakoniwaBackend` a writable `/etc` inside the sandbox so its own
  DNS-refusal stub content can be placed at `/etc/resolv.conf` — closing a
  real information leak (the real host's nameserver/search-domain content is
  currently readable inside the sandbox) and making ordinary
  `getaddrinfo`-based DNS resolution actually reach the sandbox's own stub
  (today only a query hand-crafted to `127.0.0.1:53` does) — without breaking
  any real host `/etc` content other in-sandbox tooling (locale-adjacent
  lookups, dynamic linking, `SandboxIdentityMode::SandboxUser`'s `nobody`
  resolution) already depends on.
- Observable acceptance outcomes:
  - A real `firma run --backend hakoniwa` invocation with
    `enforce_network_namespace = true`: `cat /etc/resolv.conf` inside the
    sandbox shows only the synthetic stub-pointing content (never the real
    host's nameservers/search domain); a plain `python3 -c
    "socket.getaddrinfo('example.invalid', 80)"` (no raw-socket tricks)
    reaches the sandbox's own DNS-refusal stub and fails predictably, instead
    of silently succeeding via `/etc/hosts` or hanging/erroring some other
    way.
  - The existing Hakoniwa e2e suites (`tests/e2e/scenarios/hakoniwa_backend.rs`,
    the Hakoniwa leg of `tests/e2e/scenarios/config_masking.rs`) continue to
    pass unmodified, including `SandboxIdentityMode::SandboxUser`'s `whoami`/
    `getpwuid`-based `nobody` resolution.
  - `/etc/hosts` inside the sandbox no longer carries the real host's static
    entries beyond loopback aliases; `/etc/machine-id`, `/etc/hostname` are
    absent rather than exposing real host identifiers.

## Scope

- In scope: `HakoniwaBackend::prepare`/`mount::build_mount_ops` (mount-plan
  construction, in `crates/firma-run/src/backend/hakoniwa/`); the small,
  additive extension to `SandboxInfrastructureKind` (`backend/mod.rs`) needed
  to carry a synthesized `/etc/hosts`; a new Hakoniwa-only e2e test proving
  `getaddrinfo`-based resolution reaches the DNS stub; docs updates
  (`docs-site`, this plan's cross-reference in `hakoniwa-backend-plan.md`).
- Out of scope:
  - `BwrapBackend` — not touched, including its test files (`DEC-007` chooses
    a Hakoniwa-only test specifically to keep this true even at the test
    level).
  - `firma-hakoniwa-runner` (the sibling binary crate) — this plan's design
    (`DEC-001`) produces only `HakoniwaMountOp::Bind`/`Tmpfs` operations, both
    already replayed generically by the runner's existing `apply_mount_ops`;
    no runner-side code, launch-contract schema, or `LAUNCH_CONTRACT_VERSION`
    bump is needed. Verified, not assumed — see Technical evidence,
    "Semantic call traces".
  - Any new `hakoniwa` crate feature or upstream change (no overlayfs support
    exists there; see the separate draft GitHub issue this task also
    produces, noted in the final report).
  - Fixing the pre-existing, cross-backend gap that a real, unmodified
    `/etc/hosts` can bypass DNS-based confinement for `BwrapBackend` (which
    also exposes the real host `/etc` wholesale) — named in `DEC-006` as a
    discovered-but-deferred issue, not silently left unrecorded.
  - Unsharing `Namespace::Uts` / calling `sethostname` for `HakoniwaBackend`
    (the sandbox's `gethostname()` syscall currently still returns the real
    host's hostname regardless of `/etc/hostname` content) — a separate,
    pre-existing invariant gap, named in Risks and gaps, not fixed here.
  - Implementation. This plan produces a durably published, reviewed design
    only.
- Assumptions:
  - `hakoniwa = "1.7.2"` (already pinned; unchanged by this plan) has no
    overlayfs mount type and no plan to add one before this work would need
    to ship (re-confirmed against the same cached crate source used by the
    original backend plan; see Technical evidence).
  - The host's real `/etc/nsswitch.conf`, `/etc/ld.so.cache`/`ld.so.conf`/
    `ld.so.conf.d`, `/etc/localtime`, `/etc/services`, `/etc/protocols`,
    `/etc/passwd`, `/etc/group` are the load-bearing paths for ordinary
    Python/bash/curl-class tooling under glibc-based Linux distributions —
    reasoned from documented glibc/dynamic-linker behavior and this
    repository's own existing tooling (see `DEC-002` and Risks and gaps for
    confidence level per path).
- Open decisions: none blocking; `DEC-002`'s preserved-path list is the
  plan's single most consequential judgment call and is flagged as
  lowest-confidence in Risks and gaps.
- Cohesion and split assessment: kept as one plan. Both slices share one
  owner (the new `/etc`-reconstruction logic in `hakoniwa/mod.rs` and
  `hakoniwa/mount.rs`) and one invariant (`INV-003`); splitting further would
  separate "prove the mechanism" from "flip the two security-relevant paths"
  in a way that would leave Slice 1 alone with no independent product value
  (see Slice 1's own framing).
- Deferred child plans: Not applicable.

## Routing

- Mode: Full (pre-established by the task; not re-litigated here).
- Trigger evidence: (1) a trust boundary — what host filesystem content is
  exposed inside a structural sandbox; (3) invariant ownership — extends
  `INV-001` (owned by `HakoniwaBackend`/the runner's bootstrap sequence) and
  must not weaken `INV-002` (`.firma`/config masking, owned by
  `hakoniwa/mount.rs`), both already-established invariants this plan must
  not silently contradict.
- Higher-mode triggers checked: no additional triggers beyond Full apply.
- Downgrade evidence and reason: Not applicable.

## Current behavior and problem

- Owners and entry points: `HakoniwaBackend::prepare`
  (`crates/firma-run/src/backend/hakoniwa/mod.rs:64-120`) builds the mount
  list handed to `mount::build_mount_ops`
  (`crates/firma-run/src/backend/hakoniwa/mount.rs:90-152`), which the
  `firma-hakoniwa-runner` binary (`crates/firma-hakoniwa-runner/src/main.rs`)
  replays verbatim via `apply_mount_ops` (`main.rs:322-343`) against a
  `hakoniwa::Container`. `container.rootfs("/")`
  (`main.rs:211-213`) is called first and binds `/bin`, `/etc`, `/lib`,
  `/lib64`, `/lib32`, `/sbin`, `/usr` from the real host, read-only, as one
  mount per top-level directory
  (`hakoniwa-1.7.2/src/container.rs:176-213`, `rootfs_imp`).
- Current success and failure outcomes: every other real host `/etc` path
  (locale-adjacent files, `nsswitch.conf`, `ld.so.cache`, `ca`-adjacent
  material, `passwd`/`group`) is available inside the sandbox exactly as on
  the host, which is why ordinary tooling (`python3`, `bash`, dynamically
  linked binaries) already works. But `/etc/resolv.conf` is also the real
  host file: its content (real nameserver IPs, search domain) is readable
  from inside the sandbox (an information leak, since network-namespace
  isolation already blocks any real query those nameservers would need), and
  ordinary `getaddrinfo`-based resolution consults it and queries the real
  (unreachable, namespace-isolated) nameservers instead of the sandbox's own
  DNS-refusal stub — the stub is only reachable today via a query
  hand-crafted directly to `127.0.0.1:53`
  (`tests/e2e/scenarios/hakoniwa_backend.rs:194-280`,
  `hakoniwa_backend_dns_stub_answers_real_queries`, confirmed by re-reading:
  it opens a raw UDP socket to `127.0.0.1:53` from Python, not
  `socket.getaddrinfo`). Attempting to overlay new content at
  `/etc/resolv.conf` (or `/etc/passwd`/`/etc/group`, tried and reverted
  previously) fails with `touch("etc/group") => Permission denied`
  (`hakoniwa/mod.rs:76-104`'s own comment), because
  `hakoniwa-1.7.2/src/runc/unshare.rs:145-161`'s bind-mount branch of
  `initialize_rootfs` calls `sys::touch(target_relpath)` against whatever is
  already mounted at that relative path — which, for an existing `/etc`
  bind-mounted read-only from the real host, is the real, root-owned host
  file, not a placeholder Hakoniwa controls.
- Evidence: as cited inline above; re-verified directly against the cached
  `hakoniwa-1.7.2` source
  (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/hakoniwa-1.7.2`)
  and the current `openfirma-all-backends` tree at the researched revision,
  not re-derived from the prior plan's prose alone.

## Key decisions and tradeoffs

### `DEC-001`: Reconstruct `/etc` via a `Tmpfs` anchor plus a curated `Bind` allowlist — no new `HakoniwaMountOp` variant, no runner changes

- Choice: `mount::build_mount_ops` gains a new step that, when the
  reconstruction is active (`DEC-003`), pushes `HakoniwaMountOp::Tmpfs {
  target: "/etc" }` before any other `/etc/*`-targeted operation, then relies
  on Hakoniwa's own existing target-path lexicographic mount ordering
  (`hakoniwa-1.7.2/src/container.rs:452-456`'s `get_mounts`, applied by
  `runc/unshare.rs:98-164`'s `initialize_rootfs`) to apply every deeper
  `/etc/*` `Bind` after it. `HakoniwaMountOp`'s two existing variants
  (`Bind`, `Tmpfs`) are unchanged and sufficient; `firma-hakoniwa-runner`'s
  `apply_mount_ops` (`main.rs:322-343`) needs no code change, and the
  on-disk `LaunchContract`/`LAUNCH_CONTRACT_VERSION` (currently `6`) does not
  change.
- Rationale and evidence: `Container`'s `mounts` field is a `HashMap<String,
  Mount>` keyed by the literal target string
  (`hakoniwa-1.7.2/src/container.rs:47`, `236-244`'s `mount()`); `rootfs_imp`
  registers `/etc` under exactly that key
  (`container.rs:176-213`), so a later `tmpfsmount("/etc")` call for the same
  key silently replaces it (established finding, re-confirmed by reading
  `mount()`'s `self.mounts.insert(target.clone(), ...)`). Re-tracing
  `initialize_rootfs` (`runc/unshare.rs:98-164`) confirms the tmpfs branch
  (`mount.fstype == "tmpfs"`) only calls `sys::mkdir_p`+`sys::mount_filesystem`
  — no permission dependency on the real host `/etc`'s ownership at all —
  and that the tmpfs mount, once created, is owned by the sandboxed
  process's own (namespace-mapped) uid, so a subsequent `Bind` mount whose
  _target_ (not source) lands under it can `sys::touch()` successfully: the
  touch creates a placeholder _inside the fresh tmpfs_, not inside the real
  host directory. This is the mechanism that fixes the originally-reported
  `Permission denied` — re-verified by reading the exact code path, not only
  citing the earlier empirical repro. `remount_rdonly`
  (`runc/unshare.rs:220-244`) only re-applies `MS_REMOUNT` to mounts whose
  `options` include `MountOptions::BIND`; the `/etc` tmpfs itself (no `BIND`
  flag) is never touched by that pass, so it stays writable through
  `pivot_root` — though this plan does not need to write into it after pivot
  (`DEC-004`/`DEC-006` use ordinary `Bind` mounts registered before pivot,
  not `Container::file()`/`apply_fs_operations`, which only runs _after_
  `remount_rdonly` — `runc/unshare.rs:56-95`'s `mount()` function, confirmed
  by reading the call order directly).
- Consequences and rejected alternatives: rejected building `/etc` via
  `Container::file()`/`dir()`/`symlink()` (`fs_operations`, applied via
  `apply_fs_operations` strictly after `pivot_root`+`remount_rdonly`,
  `runc/unshare.rs:90-91`, `246-263`) as the _primary_ mechanism — using it
  would require every "preserve real content" path to be read into
  `firma-run`'s own memory and re-written byte-for-byte as string content
  (`Container::file` takes `&str` content, not a source path), which is both
  more code and loses the read-only-bind-mount semantics (`Bind` mounts can
  be marked read-only via `remount_rdonly`; a `file()`-written copy is a
  plain writable regular file with no such guarantee) — strictly worse for
  paths whose real content should stay read-only in the sandbox, with no
  compensating benefit. Rejected any new `HakoniwaMountOp` variant (e.g. a
  `File{target, contents}` op mirroring `Container::file`) — unnecessary,
  since every path this plan needs to place is either a real host source
  (`Bind`) or content this plan can pre-write to a real file under
  `handle.runtime_dir` and then `Bind` in (mirroring
  `SandboxInfrastructureKind::ResolverConfig`'s existing pattern, `DEC-004`).
  Rejected asking upstream `hakoniwa` for overlayfs support and blocking on
  it — no such support exists today (re-confirmed against `souk4711/hakoniwa`
  `main`, current release `v1.7.2`), and this plan's own mechanism does not
  need it; a draft issue requesting it is produced separately as a low-risk,
  independent ask for a future version (see the accompanying draft, not part
  of this plan's own acceptance criteria).
- **Corrected after plan review (`PLAN-002`)**: the reconstruction's
  correctness must not depend on which of two ops targeting the same key
  happens to be pushed last into `ops: Vec<HakoniwaMountOp>` — `Container`'s
  `mounts: HashMap<String, Mount>` overwrites on exact-target collision
  (`DEC-001`'s own rationale above), and nothing before this correction
  stopped an ordinary operator `mounts` config entry (`SandboxMountAuthority
  ::OperatorProvided`, unrestricted target) from targeting `/etc` itself —
  which is not a `SandboxMount` at all (the `/etc` `Tmpfs` anchor is pushed
  directly into `ops`, bypassing the authority/validation pipeline
  entirely, per the Architecture shape below) and so was invisible to any
  existing duplicate-target check. Relying on push order to make the
  reconstruction's own anchor win that collision would be fragile and
  silent — exactly the kind of "depends on sort/insertion order" risk this
  backend already rejected once for the analogous masking problem
  (`reject_overlay_targets_inside_masked_zones`, `hakoniwa-backend-plan.md`
  Slice 2). Fixed by `DEC-008`, a new, explicit, fail-closed validation
  rather than a push-order guarantee.

### `DEC-002`: A hardcoded, skip-if-absent allowlist of preserved real `/etc` paths — not a dynamic mirror-minus-blocklist

- Choice: a new `const PRESERVED_ETC_HOST_PATHS: &[&str]` in
  `crates/firma-run/src/backend/hakoniwa/mount.rs` lists individual files and
  directories to re-bind read-only from the real host onto the reconstructed
  `/etc`, each guarded by an existence check before it is added (mirroring
  `firma-hakoniwa-runner`'s own `LANDLOCK_READ_ONLY_DIRS`/
  `LANDLOCK_LIBRARY_DIRS`'s `if Path::new(dir).is_dir()` pattern,
  `main.rs:66-91`, `371-378`, and `firma-run`'s own
  `SYSTEM_CA_BUNDLE_CANDIDATES` file-probe list, `runtime/mod.rs:738-745`).
  Everything not on the list is simply absent from the reconstructed `/etc`
  (an allowlist, not "copy everything except a blocklist").

  | Path                                           | Kind | Included? | Reason                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
  | ---------------------------------------------- | ---- | --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
  | `/etc/nsswitch.conf`                           | file | yes       | NSS lookup order for `getaddrinfo`/`getpwnam`-class calls; without it glibc falls back to compiled-in defaults, which is a behavior change this plan does not need to force.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
  | `/etc/ld.so.cache`                             | file | yes       | Fast dynamic-linker library resolution; some distro layouts (multiarch paths under `/usr/lib/<triplet>`) are only found via the cache, not the linker's compiled-in default search path.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
  | `/etc/ld.so.conf`                              | file | yes       | Source `ld.so.cache` derives from; harmless to expose, cheap defense-in-depth if some tool re-parses it directly.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
  | `/etc/ld.so.conf.d/`                           | dir  | yes       | Same reasoning, recursive (`Bind` uses `MS_BIND\|MS_REC`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
  | `/etc/localtime`                               | file | yes       | Timezone data many programs read directly; low sensitivity (reveals only the host's configured timezone), high correctness value. Re-bound as a plain file even though the real path is usually a symlink (`stat`, not `lstat`, is used by Hakoniwa's own bind-mount metadata check — see Technical evidence — so the _resolved_ zoneinfo content lands at `/etc/localtime`, which is behaviorally indistinguishable for any consumer that reads file content rather than `readlink()`s the path).                                                                                                                                                                                                                                                     |
  | `/etc/passwd`                                  | file | yes       | `SandboxIdentityMode::SandboxUser`'s kernel-level `uidmap`/`gidmap` remap to uid/gid `65534` (`main.rs:30-42`, `199-202`) depends on the _real_ host's `nobody` entry to resolve via `getpwuid`; see `DEC-005`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
  | `/etc/group`                                   | file | yes       | Same reasoning, for `nogroup`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
  | `/etc/services`                                | file | yes       | `getservbyname`-class lookups (some tooling, e.g. certain Python stdlib paths); static, low-sensitivity table.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
  | `/etc/protocols`                               | file | yes       | Same reasoning, for `getprotobyname`-class lookups.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
  | `/etc/resolv.conf`                             | file | replaced  | Synthesized stub content, not the real file — this plan's core goal (`DEC-004`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
  | `/etc/hosts`                                   | file | replaced  | Synthesized loopback-only stub, not the real file (`DEC-006`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
  | `/etc/machine-id`                              | —    | excluded  | Host-identifying; this sandbox never runs D-Bus/systemd session services that would need it, so omitting it has no observed functional cost. **Corrected after plan review (`PLAN-004`)**: whether a tool can actually (re-)write one at runtime depends on Landlock — when `allowed_executables` is non-empty, `firma-hakoniwa-runner`'s own `LANDLOCK_READ_ONLY_DIRS` (`main.rs:66`) already grants only `FsAccess::R` (never `W`) on `/etc`, so such a write is denied (`EACCES`/`EPERM`), fail-closed, not silently allowed; only when Landlock is inactive (`allowed_executables` empty) is `/etc` actually writable and a fresh, ephemeral, non-identifying id could appear. Either way, no real host `/etc/machine-id` content is ever exposed. |
  | `/etc/hostname`                                | —    | excluded  | Host-identifying via direct file read; note this does **not** close the _syscall_-level leak (`gethostname()` still returns the real host's name, since `Namespace::Uts` is never unshared — a separate, pre-existing gap named in Risks and gaps, not fixed here).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
  | `/etc/ssl`, `/etc/pki`, `/etc/ca-certificates` | dir  | excluded  | Deliberately not mounted at all by default. `firma-run`'s own `SYSTEM_CA_BUNDLE_CANDIDATES`/`build_appended_ca_bundle` mechanism (`runtime/mod.rs:738-800`) already injects `REQUESTS_CA_BUNDLE`/`SSL_CERT_FILE`/`CURL_CA_BUNDLE`/`NODE_EXTRA_CA_CERTS` env vars pointing at a synthesized bundle under the already-bind-mounted sandbox runtime dir, covering curl/`requests`/OpenSSL-via-env-var/Node's _extra_-trust-anchor path without needing the system trust store mounted at all; some distros additionally keep private key material under `/etc/ssl/private`, which a broad directory bind would otherwise re-expose for no compensating benefit.                                                                                           |
  | everything else                                | —    | excluded  | Not on the list; absent from the reconstructed `/etc` unless a tool creates it itself at runtime — and, per the `PLAN-004` correction above, only when Landlock is inactive; a Landlock-active launch denies writes into `/etc` entirely.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |

- Rationale and evidence: this repository already establishes the
  "hardcoded, skip-if-absent const list" shape twice for adjacent problems
  (`LANDLOCK_READ_ONLY_DIRS`/`LANDLOCK_LIBRARY_DIRS`,
  `SYSTEM_CA_BUNDLE_CANDIDATES`) — following the same shape is consistent,
  low-novelty, and reviewable by the same pattern a reviewer already knows.
  More fundamentally, CLAUDE.md's own named invariant ("Deterministic
  enforcement: same context plus same policy bundle yields the same
  decision") favors a fixed, explicit, audited list over a dynamic
  "mirror-the-host-except" rule: the latter would make sandbox exposure
  depend on whatever a future host happens to have under `/etc`, silently
  widening exposure whenever the host gains a new, unreviewed file —
  exactly the kind of unaudited drift a fail-closed posture should avoid.
  An allowlist fails closed (an unlisted path is simply absent) rather than
  failing open.
- Consequences and rejected alternatives: rejected "bind everything under
  `/etc` except a hardcoded blocklist" — inverts the fail-closed default (a
  newly-added, unreviewed host `/etc` entry would be exposed automatically);
  rejected making the list configurable via `firma.toml` — no evidence any
  operator-visible variation is needed yet, and configurability would let an
  operator (or a compromised config) widen exposure without a code review,
  which is exactly the audit trail this plan wants to keep. Both are
  revisitable if implementation-time e2e testing (Slice 1's own focused
  verification) finds a real tool that needs a path not on this list.

### `DEC-003`: Gate the entire reconstruction behind `NetworkPolicy.enforce_network_namespace`

- Choice: `HakoniwaBackend::prepare` only synthesizes `/etc/resolv.conf`/
  `/etc/hosts` content and `mount::build_mount_ops` only emits the `/etc`
  `Tmpfs` anchor and its preserved-path `Bind`s when
  `request.profile.network.enforce_network_namespace` is `true`. When it is
  `false` (cooperative routing mode), `/etc` is left exactly as
  `container.rootfs("/")` already provides it today — unchanged.
- Rationale and evidence: `BwrapBackend::prepare` already gates its own
  resolv.conf synthesis identically
  (`linux_bwrap/mod.rs:105-145`: `if request.profile.network
  .enforce_network_namespace { ... }`), for the same reason — there is no DNS
  stub running, and no namespace isolation forcing a fallback path, in
  cooperative mode, so pointing `/etc/resolv.conf` at `127.0.0.1:53`
  unconditionally would break real DNS resolution in that mode instead of
  fixing an information leak. The same reasoning applies transitively to
  `/etc/hosts` (no DNS-bypass concern exists when network confinement itself
  is off) and to the `/etc` reconstruction as a whole (no reason to take on
  the mechanism's risk when nothing it enables is active).
- Consequences and rejected alternatives: rejected reconstructing `/etc`
  unconditionally (simpler code, but breaks real DNS resolution in
  cooperative mode and reconstructs `/etc` for zero benefit whenever
  `enforce_network_namespace` is `false`).

### `DEC-004`: Regenerate `/etc/resolv.conf` via the existing `SandboxInfrastructureKind::ResolverConfig` mechanism — reused, not reinvented

- Choice: `HakoniwaBackend::prepare` writes the same stub content
  `BwrapBackend::prepare` already writes ("nameserver 127.0.0.1\noptions
  ndots:0 timeout:1 attempts:1\n" — deliberately duplicated, not shared, per
  this backend's established "separate, duplicated implementation" posture,
  `hakoniwa/mount.rs`'s own module doc) into
  `handle.runtime_dir.join("resolv.conf")`, then appends a `SandboxMount`
  built via the _existing_, already-implemented
  `SandboxMount::sandbox_infrastructure(SandboxInfrastructureKind
  ::ResolverConfig, resolv_conf_path, PathBuf::from("/etc/resolv.conf"))`
  constructor (`backend/mod.rs:279-297` — module-private `fn`, not
  explicitly `pub(in crate::backend)`; corrected citation after plan review,
  `PLAN-006` — already reachable from `hakoniwa/mod.rs` under ordinary Rust
  privacy rules since it is a child module of `backend`, already called by
  `linux_bwrap/mod.rs:130-144`). This mount already flows through
  `mount::build_mount_ops`'s existing `for mount in &mounts { ops
  .push(Bind { ... }) }` loop (`mount.rs:123-130`) and already passes
  `validate_infrastructure_mount`'s existing `ResolverConfig` arm
  (`mount.rs:374-378`) unchanged — that validation code has existed,
  unused by `HakoniwaBackend::prepare`, since Slice 2 of the original plan.
- Rationale and evidence: this is the _only_ piece of the mechanism that was
  already fully built and validated but simply never invoked for Hakoniwa
  (confirmed by grepping for `SandboxInfrastructureKind::ResolverConfig`
  construction sites: only `linux_bwrap/mod.rs` calls it today). Reusing it
  needs no new validation code and no new authority type.
- **Corrected after plan review (`PLAN-003`)**: reusing this mechanism
  inherits one of its existing, pre-established properties, not a new one —
  `validate_overlay_destinations`'s duplicate-target check (`mount.rs:317
  -328`, identically in `linux_bwrap/mount.rs:559-570`) filters out
  `SandboxMountAuthority::SandboxInfrastructure` mounts before comparing
  targets, so an operator `mounts` config entry that happens to target
  `/etc/resolv.conf` while `enforce_network_namespace` is also `true` is
  never flagged as colliding with this synthesized mount — whichever is
  last in `ops` (by construction, this plan appends the infrastructure
  mount after operator mounts in `HakoniwaBackend::prepare`, so the stub
  wins) silently determines the outcome, with no diagnostic either way.
  This is not a new hole this plan introduces: `BwrapBackend` has had the
  exact same property for its own `/etc/resolv.conf`/`/etc/passwd`/
  `/etc/group` infrastructure mounts since the base plan's Slice 2, and this
  plan simply reactivates the same, already-accepted mechanism for
  Hakoniwa. Named here explicitly rather than asserted away; not fixed in
  this plan (fixing it would be a cross-backend validation change beyond
  this plan's scope — see Risks and gaps).
- Consequences and rejected alternatives: rejected inventing a
  Hakoniwa-specific resolv.conf mount authority — the existing one already
  fits with no new authority type. Symlink-resolution parity with
  `platform::resolve_resolv_conf_target()` (bwrap's dual-mount handling for
  hosts where `/etc/resolv.conf` is itself a managed symlink, e.g.
  systemd-resolved/WSL) is reused identically, since
  `validate_infrastructure_mount`'s `ResolverConfig` arm already accepts
  either target.

### `DEC-005`: Preserve the real host `/etc/passwd`/`/etc/group` — do not synthesize bwrap-style fake content

- Choice: `/etc/passwd`/`/etc/group` are on `DEC-002`'s preserved-path list
  as ordinary read-only `Bind`s of the _real_ host files, not
  `BwrapBackend`-style synthesized single-user content.
- Rationale and evidence: `SandboxIdentityMode::SandboxUser` for
  `HakoniwaBackend` already works via a genuine kernel-level
  `Container::uidmap`/`gidmap` remap to `65534` (`main.rs:30-42`,
  `199-202`), and depends on the _real_ host `/etc/passwd`'s universal
  `nobody`/`nogroup` entries to make `getpwuid`/`getgrgid` resolve
  gracefully (established finding, re-confirmed by reading `main.rs`'s own
  doc comments). Synthesizing bwrap-style content instead (a fabricated
  `firma-user` entry at a specific uid) would require _also_ faking the
  entry at uid `65534` specifically to keep this working, which is more
  code for no behavioral gain over just preserving the real file — and the
  real file's content here is not new exposure (`rootfs("/")` already
  exposes it wholesale today; this plan does not change what `/etc/passwd`
  contains inside the sandbox, only how it gets there).
- Consequences and rejected alternatives: rejected synthesizing `passwd`/
  `group` content (extra code, no security benefit, and would need
  independent verification that the uid-`65534` entry it fabricates matches
  every host's actual `nobody` uid — unnecessary risk when the real file
  already guarantees this by construction).

### `DEC-006`: Replace `/etc/hosts` with a minimal, synthesized loopback-only stub

- Choice: a new `SandboxInfrastructureKind::Hosts` variant (mirroring
  `Passwd`/`Group`/`ResolverConfig`'s existing shape exactly) carries a
  synthesized `/etc/hosts` ("127.0.0.1 localhost\n::1 localhost
  ip6-localhost ip6-loopback\n") written to
  `handle.runtime_dir.join("hosts")` and bound read-only at `/etc/hosts`,
  the same way `DEC-004` handles `resolv.conf`. `validate_infrastructure_mount`
  gains one new match arm: `Hosts => spec.target == Path::new("/etc/hosts")`.
- Rationale and evidence: glibc's NSS `hosts:` database consults
  `/etc/hosts` _before_ `dns` in the conventional `files dns` order — a real
  host `/etc/hosts` entry for an internal or corporate hostname would
  resolve without ever consulting `/etc/resolv.conf` or reaching this
  sandbox's own DNS-refusal stub at all, undermining this plan's own goal
  (an ordinary `getaddrinfo` call for such a name would silently succeed via
  the static entry, never observing the stub's `REFUSED`). The real file
  also directly names any custom hosts the operator's machine resolves,
  which is host-identifying information with no compensating need inside a
  sandbox whose only legitimate route out is loopback/the Sidecar.
- Consequences and rejected alternatives: rejected preserving the real
  `/etc/hosts` unchanged (parity with `BwrapBackend`'s current, unmodified
  behavior — `linux_bwrap/mod.rs` has no `/etc/hosts` override at all today,
  so bwrap sandboxes retain this exact bypass) — this is a **deliberate,
  recorded asymmetry** between the two backends, not an oversight: fixing it
  for bwrap too is out of scope (`BwrapBackend` is untouched by this plan
  per the user's explicit instruction) and is named in Risks and gaps as a
  pre-existing, cross-backend gap worth a future, separate item (mirroring
  `DEC-012`'s own precedent of naming, not silently leaving, a
  bwrap-side gap this kind of plan cannot fix in the same pass). Rejected
  omitting `/etc/hosts` entirely (absent file) — some tooling assumes
  `localhost` resolves without a DNS round trip at all costs; a minimal
  stub is cheap and matches common container base-image convention.
- **Note (`PLAN-003`)**: the new `Hosts` `SandboxInfrastructure` mount
  inherits the same operator-collision-is-undiagnosed property `DEC-004`
  names for `ResolverConfig` — see that decision's correction for the full
  reasoning, which applies identically here.

### `DEC-008`: Explicitly reject any operator/framework mount whose target is exactly `/etc` — do not rely on push order

- Choice: when the `/etc` reconstruction is active (`DEC-003`),
  `mount::build_mount_ops` runs a new, explicit validation,
  `reject_operator_mount_targeting_etc_anchor`, over the already-computed
  `mounts: Vec<ValidatedMount>` (the same list `validate_overlay_destinations`
  and `reject_overlay_targets_inside_masked_zones` already inspect) before
  emitting any ops: any mount whose `authority` is _not_
  `SandboxInfrastructure` and whose normalized `target` equals exactly
  `/etc` fails the launch closed, with a clear message, rather than being
  silently resolved by whichever op happens to land last in `ops`.
- Rationale and evidence (`PLAN-002`): the `/etc` `Tmpfs` anchor is not a
  `SandboxMount` at all (`Tmpfs` has no `MountSpec` representation, so it is
  pushed directly into `ops`, bypassing `validate_mounts`/
  `validate_overlay_destinations` entirely — see Architecture shape). Today,
  nothing prevents an ordinary operator `mounts` config entry from also
  targeting `/etc` itself; because `Container`'s `mounts` HashMap overwrites
  on exact-target collision (`DEC-001`), whichever of the two same-keyed
  ops is replayed last by `apply_mount_ops` wins, and that order was not
  previously pinned by anything in this plan. This repository already
  rejected exactly this shape of risk once before, for the analogous
  masking problem (`reject_overlay_targets_inside_masked_zones`,
  `hakoniwa-backend-plan.md` Slice 2) — the same fail-closed-explicit-check
  approach is used here rather than repeating the "argue about push order"
  approach that check itself was added to avoid.
- Consequences and rejected alternatives: rejected relying on push order
  alone (pin `rebuild_etc`'s anchor push after the general operator-mounts
  loop, so it always wins) — works today, but is a silent, order-dependent
  guarantee with no diagnostic if a future refactor reorders `build_mount_ops`'s
  body; an explicit, fail-closed rejection is strictly more robust for
  the same amount of code, and gives an operator a clear error instead of a
  silently-ignored mount. This check is deliberately narrower than a full
  "operator can never target any `/etc/*` path" rule: an operator mount
  under a _deeper_ `/etc/*` path (e.g. `/etc/my-app.conf`) is legitimate and
  lands correctly on the reconstructed tmpfs with no collision at all —
  only the exact `/etc` anchor target itself is reserved.

### `DEC-007`: New `getaddrinfo`-based e2e test lives in the existing Hakoniwa-only `hakoniwa_backend.rs`, not a `{Bwrap, Hakoniwa}`-parametrized file

- Choice: the new test proving ordinary resolver-based DNS resolution
  reaches the stub is added to `tests/e2e/scenarios/hakoniwa_backend.rs`
  (which already hosts the raw-UDP-based
  `hakoniwa_backend_dns_stub_answers_real_queries`), not to a shared,
  `DEC-011`-style parametrized file that would also exercise `BwrapBackend`.
- Rationale and evidence: the task's explicit scope instruction is "do not
  touch `BwrapBackend` at all"; a new parametrized test that runs against
  `Bwrap` too would not change `BwrapBackend`'s production code, but it does
  add new test surface exercising it, which a strict reading of that
  instruction excludes. `hakoniwa_backend.rs` already establishes the
  single-backend-file convention for exactly this class of DNS-stub proof.
- Consequences and rejected alternatives: rejected the `{Bwrap, Hakoniwa}`
  parametrized shape `DEC-011` established elsewhere in this codebase (a
  reasonable, arguably preferable choice on pure engineering merit, since
  `BwrapBackend`'s own resolv.conf mechanism likely already passes such a
  test) — deferred to a future, separate item if bwrap-side parity testing
  is ever wanted; recorded here rather than assumed silently either way.

## Architecture and invariant ownership

- Architecture shape: `HakoniwaBackend::prepare`
  (`crates/firma-run/src/backend/hakoniwa/mod.rs`) gains a block mirroring
  `BwrapBackend::prepare`'s existing `if
  request.profile.network.enforce_network_namespace { ... }` shape
  (`DEC-003`), synthesizing `resolv.conf`/`hosts` content into
  `handle.runtime_dir` and appending two `SandboxMount`s built via the
  existing `sandbox_infrastructure` constructor (`DEC-004`, `DEC-006`), plus
  `DEC-002`'s preserved-path list appended via the existing `framework`
  constructor — **all of these flow through the ordinary
  `handle.mounts`/`build_mount_ops` pipeline** (`validate_mounts`,
  `validate_overlay_destinations`), the same as any operator mount, so
  exact-target collisions among them are already caught by existing code
  (`DEC-004`'s `PLAN-003` note names the one already-accepted exception:
  `SandboxInfrastructure`-authority targets). `mount::build_mount_ops`
  (`crates/firma-run/src/backend/hakoniwa/mount.rs`) gains exactly one new
  function that pushes _only_ the `/etc` `Tmpfs` anchor directly into `ops`
  (mirroring `mask_firma_dir`'s existing `emit_tmpfs` pattern, which already
  bypasses the `SandboxMount`/authority pipeline for backend-owned,
  non-operator paths) when the reconstruction is active — plus `DEC-008`'s
  new explicit rejection guarding that one bypassed path. No other file
  changes. This is entirely additive to the existing mount-plan-then-replay
  architecture: `firma-hakoniwa-runner` is unchanged (`DEC-001`).

### `INV-001` (existing, extended): Structural network confinement — no route out except loopback

- Semantic predicate: unchanged from `docs/architecture/hakoniwa-backend-plan.md`'s
  definition. This plan extends its _practical_ truth: the predicate's own
  text already claims ordinary DNS resolution reaches the stub, which was
  not actually true before this plan (only a hand-crafted raw query did).
- Primary owner: unchanged — `HakoniwaBackend`/the runner binary's bootstrap
  sequence; this plan does not move ownership, it closes a gap inside the
  existing owner's scope (`HakoniwaBackend::prepare`/`mount.rs`, not the
  runner).
- Detailed proof: see Technical evidence, `PROOF-ETC-001`.

### `INV-002` (existing, unchanged): `.firma`/config masking robustness

- Semantic predicate and primary owner: unchanged, per
  `docs/architecture/hakoniwa-backend-plan.md`.
- This plan's interaction: none observed today, but the disjointness is
  practical, not structurally enforced — **softened after plan review
  (`PLAN-005`)**. `.firma` masks are rooted at `cwd`, `$HOME`, or
  `launch.config_file` (`mask_firma_dir`, `mount.rs:480-514`); the first two
  can never resolve under `/etc` in this codebase's own conventions, but
  `launch.config_file` is an operator/config-loader-supplied path with no
  code-level constraint against pointing under `/etc` (e.g. `--config
  /etc/firma/.firma/firma.toml`) — no evidence this occurs in practice, and
  no code path in this repository constructs such a value today, but it is
  not impossible by construction. If it ever did, the resulting mask would
  target a path under `/etc`, which `reject_overlay_targets_inside_masked_zones`
  (`mount.rs:645-671`) would not compare against `rebuild_etc`'s bypassed
  ops either way (that check only inspects `mounts: &[ValidatedMount]`, and
  `rebuild_etc`'s anchor is not one) — so this remains an unlikely, narrow,
  _unenforced_ edge case rather than a proven-impossible one. Not fixed in
  this plan (no known trigger); named rather than overclaimed.
- Detailed proof: Not applicable (no change to `INV-002`'s proof
  obligations; the practical-disjointness argument above, now correctly
  scoped, is the complete case).

### `INV-003` (new): `/etc` reconstruction exposes only an explicitly reviewed allowlist, and does not regress `SandboxIdentityMode::SandboxUser`

- Semantic predicate: for a `HakoniwaBackend` sandbox with
  `enforce_network_namespace = true`, every path under `/etc` inside the
  sandbox is either (a) on `DEC-002`'s preserved list (read-only, real host
  content, unchanged from today), (b) one of the two synthesized files
  (`resolv.conf`, `hosts`, `DEC-004`/`DEC-006`), or (c) absent — never any
  other real host `/etc` content. `getpwuid(65534)`/`getgrgid(65534)`
  continue to resolve to `nobody`/`nogroup` under
  `SandboxIdentityMode::SandboxUser`.
- Primary owner: `PRESERVED_ETC_HOST_PATHS` and the `/etc`-reconstruction
  code in `crates/firma-run/src/backend/hakoniwa/mod.rs` and `mount.rs`.
- Detailed proof: see Technical evidence, `PROOF-ETC-002`.

- Compatibility, migration, and failure semantics: purely additive to an
  experimental, opt-in backend (`DEC-010` in the base plan); no config or
  wire-format change; gated (`DEC-003`) so cooperative-routing-mode
  behavior is byte-for-byte unchanged.
- Durable documentation owner: `docs-site/src/content/docs/concepts/sandbox.md`
  (Hakoniwa's existing entry gets a short note that `/etc/resolv.conf`/
  `/etc/hosts` are now sandbox-controlled, matching bwrap's already-documented
  behavior); `docs/architecture/hakoniwa-backend-plan.md`'s Slice 2 section
  gets a one-line pointer to this document instead of restating its content
  (added below, see "Cross-references").

## Cross-references

- `docs/architecture/hakoniwa-backend-plan.md`'s Slice 2 "Implemented" notes
  (the paragraph beginning "**Resolv.conf: still blocked...**") gets a
  trailing pointer: "Superseded by
  `docs/architecture/hakoniwa-etc-reconstruction-plan.md`, which resolves
  this gap." (to be added when this plan is accepted, alongside publication
  — see Implementation slices).
- This document cross-references that one for `INV-002`'s existing
  definition and the mount-ordering mechanics it already established, rather
  than restating them (done inline above via citation, not duplication).

## Implementation slices

### Slice 1: `/etc` reconstruction mechanism, with `/etc/resolv.conf`/`/etc/hosts` temporarily preserved as real content

- Production, types, tests, and docs/config: `mount::build_mount_ops` gains
  `push_etc_reconstruction_anchor` (`DEC-001`) and
  `reject_operator_mount_targeting_etc_anchor` (`DEC-008`), both gated by
  `DEC-003`; `HakoniwaBackend::prepare` gains the
  `enforce_network_namespace`-gated block appending
  `mount::preserved_etc_host_mounts()`'s results (`DEC-002`) to
  `handle.mounts` via the existing `SandboxMount::framework` constructor,
  but for this slice `/etc/resolv.conf` and `/etc/hosts` are _also_
  temporarily on that preserved list (real host content, unchanged
  behavior) rather than synthesized — this slice's entire purpose is
  proving the reconstruction mechanism (tmpfs anchor + real-content
  re-binding, through the ordinary validated mount pipeline) works with
  zero observable behavior change, before Slice 2 flips the two
  security-relevant paths.
- Affected decisions and traces: `DEC-001`, `DEC-002`, `DEC-003`, `DEC-005`,
  `DEC-008`.
- Proof obligations: `INV-003` (allowlist correctness, `nobody` resolution);
  no claim on `INV-001` yet (resolv.conf/hosts unchanged this slice).
- Focused verification: the full existing Hakoniwa e2e suite
  (`hakoniwa_backend.rs`'s 6 tests, the Hakoniwa leg of
  `config_masking.rs`) re-run unmodified and must continue passing — the
  regression proof for this slice. A new unit test in `mount.rs` asserting
  the `/etc` `Tmpfs` anchor op is emitted, and a new unit test asserting
  `reject_operator_mount_targeting_etc_anchor` fails closed for an operator
  mount targeting exactly `/etc` while allowing one targeting a deeper
  `/etc/*` path (`DEC-008`'s own positive/negative pair). A new e2e test (or
  extension of an existing one) asserting a preserved path's content inside
  the sandbox matches the real host's (e.g. `cat /etc/nsswitch.conf` inside
  the sandbox equals the host's own file) — proving the preserve-list
  mechanism actually rebinds real content, not just that nothing crashes.
  **Corrected after plan review (`PLAN-001`)**: no existing e2e test, for
  either backend, actually exercises `SandboxIdentityMode::SandboxUser`'s
  in-sandbox resolution behavior (`whoami`/`getpwuid`/`nobody`) today — a
  full-repository search for `SandboxUser`/`whoami`/`nobody`/`65534` across
  `tests/e2e/` returns no matches. The earlier draft's "existing coverage"
  claim was false. This slice therefore adds a genuinely new,
  Hakoniwa-only e2e test asserting `id -u`/`id -g` inside a `SandboxUser`
  -mode sandbox report `65534`, and `whoami` reports `nobody` — the actual
  regression proof `INV-003`'s own predicate requires, not a citation of
  nonexistent coverage.
- Dependencies: none (builds on the already-Accepted, already-Implemented
  base `HakoniwaBackend` plan).
- Intentionally unsupported: `/etc/resolv.conf`/`/etc/hosts` still show real
  host content after this slice — the plan's actual goal is not yet
  delivered; that is Slice 2's job, by design (see Cohesion assessment).

### Slice 2: synthesize `/etc/resolv.conf` and `/etc/hosts`, closing the DNS leak/bypass

- Production, types, tests, and docs/config: remove `/etc/resolv.conf`/
  `/etc/hosts` from the "preserved real content" path and instead
  synthesize them (`DEC-004`, `DEC-006`), including the new
  `SandboxInfrastructureKind::Hosts` variant and its
  `validate_infrastructure_mount` arm; docs-site update noted under
  "Durable documentation owner" above; the cross-reference edit to
  `hakoniwa-backend-plan.md` (see "Cross-references").
- Affected decisions and traces: `DEC-004`, `DEC-006`, `DEC-007`;
  `TRACE-ETC-001` (Technical evidence).
- Proof obligations: `INV-001` (extended — `PROOF-ETC-001`); `INV-003`
  (`PROOF-ETC-002`, the "replaced, not merely preserved" half).
- Focused verification: the new e2e test (`DEC-007`) proving a plain
  `python3 -c "import socket; socket.getaddrinfo(...)"` call (no raw
  sockets) against a query that only DNS could answer reaches the stub and
  fails predictably; a test asserting `cat /etc/resolv.conf`/`cat
  /etc/hosts` inside the sandbox show only synthesized content, never the
  real host's; the existing raw-UDP-based
  `hakoniwa_backend_dns_stub_answers_real_queries` continues passing
  unmodified (proves this slice does not regress the already-shipped stub
  path); the existing `hakoniwa_backend_wrapped_command_cannot_bind_an
  _unrelated_privileged_port` test continues passing (proves this slice
  does not touch the netns-wide port-bind floor `DEC-012` was careful
  about).
- Dependencies: Slice 1 (the reconstruction mechanism this flips two paths
  within).
- Intentionally unsupported: the pre-existing `BwrapBackend`/`/etc/hosts`
  cross-backend asymmetry named in `DEC-006` remains, deliberately, out of
  scope; `Namespace::Uts`/`gethostname()` leak remains, named in Risks and
  gaps, not fixed here.

## Risks and gaps

- Existing risks: `DEC-002`'s preserved-path list is a judgment call
  informed by documented glibc/dynamic-linker behavior and this
  repository's own existing tooling assumptions, not by exhaustively testing
  every real-world agent tool against a reconstructed `/etc` — a tool that
  reads a path not on the list and behaves differently (not crashes, but
  degrades) would not necessarily be caught by the existing e2e suite. Slice
  1's focused verification (existing suite + one content-equality check) is
  the primary mitigation but is not exhaustive.
- Planned mitigations: the allowlist model (`DEC-002`) fails closed by
  default — a missing path is absent, not a crash, and is cheap to add
  later with a recorded reason if implementation-time testing surfaces a
  real gap; the two-slice split (Slice 1 proves the mechanism inertly before
  Slice 2 changes security-relevant behavior) bounds the blast radius of any
  single review pass.
- Explicit evidence gaps:
  - Whether `/etc/ssl`/`/etc/pki` exclusion (`DEC-002`) is safe for every
    realistic agent tool (not just curl/`requests`/Node, which the existing
    `SYSTEM_CA_BUNDLE_CANDIDATES` env-var mechanism already covers) is
    **Unknown** — flagged, not assumed; revisit if Slice 2's e2e testing (or
    real-world usage) finds a TLS-verifying tool that ignores
    `SSL_CERT_FILE`/`CURL_CA_BUNDLE` and needs the system trust store
    directly.
  - The pre-existing `Namespace::Uts` gap (confirmed: `firma-hakoniwa-runner`'s
    `main.rs` never calls `container.unshare(Namespace::Uts)` — only `Mount`,
    `User` (both via `Container::new()`'s own defaults, which also add
    `Pid`, `container.rs:73-99`), and `Network` (`main.rs:191`) are
    unshared) means `gethostname()` inside the sandbox returns the real
    host's hostname regardless of `/etc/hostname` content, independent of
    this plan. Named here as a related, pre-existing, out-of-scope
    invariant gap — not silently left unrecorded, mirroring `DEC-012`'s own
    precedent for a bwrap-side gap this class of plan cannot fix in the
    same pass. Candidate future item, not tracked to a specific pending.md
    entry by this plan (none currently exists for it).
  - The `BwrapBackend`/`/etc/hosts` cross-backend asymmetry named in
    `DEC-006` is a real, pre-existing gap on the bwrap side, confirmed by
    reading `linux_bwrap/mod.rs` in full (no `/etc/hosts` handling exists
    there at all) — named, not silently left unrecorded, and explicitly out
    of scope per the user's own instruction not to touch `BwrapBackend`.
  - **Added after plan review (`PLAN-003`)**: an operator `mounts` config
    entry targeting exactly `/etc/resolv.conf` or `/etc/hosts` while
    `enforce_network_namespace` is `true` collides with this plan's own
    synthesized infrastructure mounts with no diagnostic either way
    (`validate_overlay_destinations` excludes `SandboxInfrastructure`
    -authority mounts from its duplicate-target check). This is an
    inherited, pre-existing property of the mechanism this plan reuses
    (already true for `BwrapBackend` today), not a new hole — named here,
    not fixed, since fixing it is a cross-backend validation change outside
    this plan's scope. `DEC-008`'s narrower fix only covers the `/etc`
    anchor itself, which has no pre-existing precedent to inherit from
    (nothing occupied that exact key before this plan).
  - **Added after plan review (`PLAN-005`)**: `INV-002`'s disjointness from
    this plan's changes is argued practically (no code path in this
    repository constructs a discoverable `.firma`/config-file location
    under `/etc` today), not structurally guaranteed — `launch.config_file`
    is operator/config-loader-supplied with no code-level constraint
    against such a value. No known trigger exists; named as an unenforced
    edge case rather than overclaimed as impossible.
- Least-confident decisions: `DEC-002`'s exact preserved-path list (see
  "Explicit evidence gaps" above) is this plan's single lowest-confidence
  judgment call; everything else follows more directly from re-verified
  `hakoniwa` crate mechanics, independently re-confirmed by plan review
  (`PLAN-001` through `PLAN-006`, all addressed — see "Plan-review findings
  and dispositions").

## Plan-review findings and dispositions

Independent review performed by a fresh reviewer agent with no prior context
on this plan, given only the task, its explicit scope constraints, and the
candidate artifact — instructed to independently inspect the repository and
the cached `hakoniwa-1.7.2` crate source rather than trust cited evidence.
Reviewer-authored fields below are preserved verbatim; each disposition block
is appended separately, per this repository's `reviewing-plans`/
`design-plan-template` contract.

```yaml
id: PLAN-001
severity: high
category: Proof obligations / Evidence
classification: confirmed-conflict
claim: >
  Slice 1's focused verification and the Risks/gaps section rely on
  "existing coverage in config_masking.rs/hakoniwa_backend.rs already
  exercises identity mode" to justify treating SandboxIdentityMode::
  SandboxUser's whoami/getpwuid(65534) -> nobody resolution as already
  regression-tested, so no new test is planned for it (only "manual
  verification").
evidence:
  - "grep -rn \"SandboxUser|whoami|nobody|65534|identity_mode|getpwuid\" tests/e2e/scenarios/hakoniwa_backend.rs tests/e2e/scenarios/config_masking.rs returns zero matches"
  - "grep -rln \"SandboxUser|getpwuid|nobody|identity_mode\" tests/e2e/ also returns nothing"
reachability: directly checkable, no special conditions
invariant_or_boundary: "INV-003's own semantic predicate explicitly requires getpwuid(65534)/getgrgid(65534) resolution; PROOF-ETC-002 cited this non-existent coverage as its Slice-1 evidence"
impact: >
  One half of INV-003's proof obligation currently has zero automated
  coverage and the plan did not schedule any; if unaddressed, Slice 1 could
  ship (and the reconstruction could regress identity-mode resolution) with
  nothing catching it beyond ad hoc manual testing.
correction: >
  Add a real e2e test (Hakoniwa-only, per the project's own scope
  constraint) that runs id/whoami inside a SandboxUser-mode sandbox and
  asserts uid/gid 65534/nobody/nogroup, and cite that new test explicitly
  rather than nonexistent "existing coverage."
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Confirmed by an independent grep re-run: no existing e2e test for either
    backend exercises SandboxUser identity-mode resolution. Slice 1's
    "Focused verification" now states this explicitly and adds a new,
    genuinely new Hakoniwa-only e2e test (id/whoami -> 65534/nobody) rather
    than citing nonexistent coverage.
  incorporated_at: "Implementation slices, Slice 1"
  decided_by: planner
```

```yaml
id: PLAN-002
severity: high
category: Architecture/ownership (mount-plan construction order)
classification: design-risk
claim: >
  rebuild_etc() pushes Tmpfs{target:"/etc"} then PRESERVED_ETC_HOST_PATHS
  "directly into ops," bypassing the SandboxMount/authority pipeline,
  mirroring mask_firma_dir's existing bypass pattern; the plan asserts this
  closes the gap with "no other file changes" beyond the sketched additions.
  Because Container::mounts is a HashMap<String,Mount> keyed by target
  (last insert wins), and the existing general mounts loop already pushes a
  Bind op for every entry in handle.mounts (including ordinary operator-
  provided MountSpecs with no restriction ever preventing an /etc/* target),
  the final content of any /etc path this plan cares about is determined by
  whichever of (a) the pre-existing general mounts loop or (b) the new
  rebuild_etc() bypass function is called later in build_mount_ops's body -
  an ordering the plan's own sketch did not pin. If the /etc Tmpfs anchor
  ends up ordered before an operator mount that also targets exactly "/etc",
  the operator mount silently wins, re-exposing the real host /etc bind
  Hakoniwa's rootfs("/") still registers earlier, reopening the exact leak
  this plan exists to close, with no error at all.
evidence:
  - "container.rs:47, 251-273 (mount() inserts by target key, overwrite semantics)"
  - "hakoniwa/mount.rs:123-130 (existing general mounts loop, unrestricted operator target)"
  - "config.rs:265 MountSpec{source,target,read_only}, no target restriction"
reachability: "an ordinary, already-supported operator mounts config entry targeting an /etc path (or /etc itself) - no attacker action, no exotic host state"
invariant_or_boundary: "INV-003's 'every /etc path is preserved/synthesized/absent' predicate, and transitively INV-001"
impact: >
  Either (i) an operator's legitimate mount is silently discarded with no
  warning, or (ii) - if ordering goes the other way for the top-level "/etc"
  key specifically - the entire reconstruction mechanism can be silently
  defeated by configuration that today is unremarkable and unrestricted.
correction: >
  Pin the exact call order of rebuild_etc() relative to the general mounts
  loop as a plan decision (not left to sketch ambiguity), and add validation
  that rejects (not silently resolves) any operator/framework MountSpec
  whose target collides with the /etc anchor or any PRESERVED_ETC_HOST_PATHS
  /synthesized-infrastructure target - mirroring the existing duplicate-
  target rejection already used for ordinary overlay collisions.
confidence: "high on the mechanism (HashMap-overwrite semantics and the existing general loop are directly confirmed in code); medium on real-world likelihood (requires a specific, currently-unusual operator config), hence high rather than critical severity"
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Confirmed: the candidate's own Types-and-signatures sketch was
    internally inconsistent with its Architecture-shape prose (the sketch
    pushed the whole preserved-path list through the bypass function; the
    prose said it flowed through SandboxMount::framework). Resolved by
    fixing the design, not just the wording: PRESERVED_ETC_HOST_PATHS
    entries now construct SandboxMount::framework(...) values appended to
    handle.mounts from HakoniwaBackend::prepare, so they flow through the
    existing validate_mounts/validate_overlay_destinations pipeline and an
    operator-target collision with any of them is already caught by
    existing, unmodified code (duplicate-target rejection already applies
    to Framework-authority mounts, confirmed by re-reading
    validate_overlay_destinations's filter, which excludes only
    SandboxInfrastructure). Only the raw /etc Tmpfs anchor itself remains
    genuinely un-covered by that existing check (Tmpfs has no MountSpec
    representation at all), so a new, explicit, fail-closed validation
    (DEC-008, reject_operator_mount_targeting_etc_anchor) was added
    specifically for that one bypassed path, rather than relying on push
    order. This is a stricter fix than the reviewer's own suggested
    correction (which would have been satisfied by pinning order alone).
  incorporated_at: "DEC-001 (added note), Architecture shape, new DEC-008, Types and signatures, Slice 1, new PROOF-ETC-003"
  decided_by: planner
```

```yaml
id: PLAN-003
severity: medium
category: Trust analysis / validation gap
classification: confirmed-conflict
claim: >
  DEC-004 claimed reusing SandboxInfrastructureKind::ResolverConfig for
  Hakoniwa "needs zero validation changes." validate_overlay_destinations
  (hakoniwa/mount.rs:317-328, identically linux_bwrap/mount.rs:559-570)
  excludes SandboxMountAuthority::SandboxInfrastructure mounts from its
  duplicate-target check, so an operator-provided MountSpec targeting
  /etc/resolv.conf (or, after this plan, /etc/hosts) is never compared
  against this plan's own synthesized infrastructure mount at that same
  target - no duplicate-target error is raised for that cross-authority
  collision, in either backend. This pattern is inherited verbatim from
  BwrapBackend (already live there today), but HakoniwaBackend::prepare
  currently never constructs any SandboxInfrastructure mount at all, so
  this collision path is presently dead code for Hakoniwa - this plan is
  what reactivates it for the first time on this backend.
evidence:
  - "hakoniwa/mount.rs:317-328"
  - "linux_bwrap/mount.rs:559-570"
  - "HakoniwaBackend::prepare's current code only does .map(SandboxMount::operator_provided)"
reachability: "an operator mounts entry targeting /etc/resolv.conf or /etc/hosts while enforce_network_namespace is also true"
invariant_or_boundary: "INV-003; also affects BwrapBackend today, out of this plan's stated scope"
impact: "silent, order-dependent precedence between an operator's own DNS/hosts override and the plan's synthesized stub, with no diagnostic either way"
correction: >
  At minimum, note this as a named, deferred cross-backend gap (the way
  DEC-006 already names the /etc/hosts bwrap asymmetry) rather than
  asserting "zero validation changes" as if the interaction were absent;
  ideally extend the duplicate check to also flag operator/framework mounts
  colliding with SandboxInfrastructure targets.
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted as a named, deferred, pre-existing (bwrap-inherited) gap rather
    than fixed in this plan, per the reviewer's own "at minimum" option:
    extending the duplicate check to cover SandboxInfrastructure-authority
    targets would be a cross-backend validation change (it would also
    change BwrapBackend's existing, already-accepted behavior for its own
    resolv.conf/passwd/group mounts), which is out of this plan's explicit
    scope. DEC-004's "zero validation changes" overclaim is removed and
    replaced with an explicit description of this inherited property;
    DEC-006 and Risks and gaps cross-reference the same explanation rather
    than each re-deriving it.
  incorporated_at: "DEC-004, DEC-006, Risks and gaps (Explicit evidence gaps)"
  decided_by: planner
```

```yaml
id: PLAN-004
severity: medium
category: Technical evidence / Risks and gaps
classification: confirmed-conflict
claim: >
  DEC-002 and Risks and gaps state "everything else ... absent from the
  reconstructed /etc unless a tool creates it itself at runtime (the tmpfs
  is writable)," and for /etc/machine-id: "If a tool tries to (re-)write one
  into the now-writable /etc, that is a fresh, ephemeral, non-identifying id
  ... not a problem." This is only true when Landlock is inactive.
  firma-hakoniwa-runner/src/main.rs:66-67 already defines
  LANDLOCK_READ_ONLY_DIRS = ["/bin","/sbin","/etc","/dev","/usr"], and
  build_landlock_ruleset (main.rs:367-387) grants only FsAccess::R (never W)
  on /etc, recursively. Tracing the hakoniwa crate's own call order confirms
  Landlock is fully installed and enforced by the time the wrapped command
  (or any tool it invokes) actually runs. So whenever Landlock is active
  (allowed_executables non-empty - an existing, already-documented
  configuration path for this backend), a tool attempting to write into
  /etc would be denied (EACCES/EPERM), not silently succeed as the plan
  describes. None of the plan's own cited proof obligations configure
  allowed_executables, so this discrepancy would not be caught by the
  plan's own verification plan either.
evidence:
  - "firma-hakoniwa-runner/src/main.rs:66-67 (LANDLOCK_READ_ONLY_DIRS)"
  - "main.rs:367-387 (build_landlock_ruleset, FsAccess::R only on /etc)"
  - "runc.rs call-order tracing: Landlock installed before the wrapped command runs"
reachability: "any Hakoniwa launch with a non-empty exec allow-list (sidecar_local_exec.enforce_known_executables)"
invariant_or_boundary: "not a security regression (the actual behavior is more conservative than the plan assumed); a factual inaccuracy in the plan's own stated rationale, and an unconsidered interaction between two already-implemented mechanisms in the same runner binary"
impact: >
  Low practical severity (no security defect), but the plan's risk analysis
  is not actually correct about what happens, and a reader relying on "the
  tmpfs is writable" to reason about tool compatibility would be misled
  under Landlock-active configurations.
correction: "note the Landlock interaction explicitly and correct the stated behavior to 'writable only when Landlock is inactive; denied (fail-closed) when Landlock is active.'"
confidence: "high on the mechanism; medium on whether this is considered material enough to require a plan edit versus a footnote, hence medium severity"
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted as stated; DEC-002's /etc/machine-id row and the "everything
    else" row now explicitly condition the writable-tmpfs claim on Landlock
    being inactive, citing LANDLOCK_READ_ONLY_DIRS/build_landlock_ruleset
    directly.
  incorporated_at: "DEC-002 (preserved-path table)"
  decided_by: planner
```

```yaml
id: PLAN-005
severity: low
category: Trust analysis / INV-002 orthogonality
classification: unverified-hypothesis
claim: >
  The plan's claim that ".firma masks are always rooted at cwd, $HOME, or an
  explicit config-file location - never under /etc," used to argue INV-002
  and this plan's new paths are "structurally disjoint," overstates what the
  code guarantees. mask_firma_dir also masks around launch.config_file - an
  operator/config-loader-supplied path with no code-level constraint against
  resolving under /etc (e.g. an operator pointing --config at a system-wide
  /etc/firma/.firma/firma.toml). If that ever happened, the resulting mask
  would be invisible to reject_overlay_targets_inside_masked_zones either
  way (it only inspects mounts: &[ValidatedMount], and rebuild_etc()'s
  bypass-pipeline ops are invisible to this check regardless).
evidence:
  - "hakoniwa/mount.rs:480-514 (mask_firma_dir also masks around launch.config_file)"
  - "hakoniwa/mount.rs:645-671 (reject_overlay_targets_inside_masked_zones only inspects ValidatedMount)"
reachability: "very narrow - requires an operator to place their discoverable firma.toml/.firma directory literally under /etc; no evidence this occurs in practice"
invariant_or_boundary: "INV-002 (unchanged, but the disjointness argument is not code-enforced, only empirically likely)"
impact: "low - no known real trigger, but the 'structurally disjoint' framing overstates what the code actually guarantees (it's a practical, not structural, disjointness)"
correction: >
  Soften the claim to "no code path today configures a discoverable .firma
  under /etc" rather than "structurally disjoint," or add an explicit
  guard.
confidence: "medium (the underlying flexibility of config_file is confirmed; the practical risk is speculative)"
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: >
    Accepted; softened the claim exactly as suggested rather than adding a
    new guard (no known trigger exists, and adding an enforcement mechanism
    for a purely hypothetical, currently-unconstructible input would be
    speculative code with no proof obligation behind it) - now explicit in
    both INV-002's own section and Risks and gaps.
  incorporated_at: "INV-002 (This plan's interaction), Risks and gaps"
  decided_by: planner
```

```yaml
id: PLAN-006
severity: low
category: Technical evidence / citation accuracy
classification: confirmed-conflict
claim: >
  DEC-004 states SandboxMount::sandbox_infrastructure is "already
  pub(in crate::backend)." backend/mod.rs:288 declares
  `fn sandbox_infrastructure(...)` with no visibility modifier at all (bare
  private), not an explicit pub(in crate::backend) - unlike
  SandboxMountAuthority/SandboxInfrastructureKind, which genuinely are
  annotated that way. The plan's practical conclusion (no visibility change
  needed, since hakoniwa and linux_bwrap are both child modules of backend
  and can already see backend's private items under ordinary Rust privacy
  rules) is still correct, but the specific citation is wrong.
evidence:
  - "backend/mod.rs:288 (bare private fn, not pub(in crate::backend))"
reachability: "N/A (documentation accuracy only)"
invariant_or_boundary: "N/A"
impact: "negligible - does not change the plan's validity, but is exactly the kind of citation the review methodology asks to verify rather than trust"
correction: "fix the citation to describe the function as module-private (not explicitly pub(in ...)), while keeping the substantive conclusion"
confidence: high
assumptions: []
```

```yaml
disposition:
  status: corrected
  rationale: "Citation fixed in DEC-004; substantive conclusion (reachable from hakoniwa/mod.rs as a child-module private item) unchanged."
  incorporated_at: "DEC-004"
  decided_by: planner
```

Reviewer's explicit no-finding statements (preserved for completeness): the
central `DEC-001` tmpfs-anchor mechanism (mount HashMap overwrite semantics,
`initialize_rootfs`'s tmpfs/bind branches, `remount_rdonly`'s BIND-only
scope, `apply_fs_operations`'s post-pivot timing, the `/etc/localtime`
symlink-resolution behavior) was independently re-derived against the cached
`hakoniwa-1.7.2` source and found to hold exactly as claimed, with the
residual caveat that the reviewer traced this statically and did not build
and run the actual sandbox; the "no `firma-hakoniwa-runner` changes needed"
claim, the CA-bundle mechanism's backend-agnosticism, the "do not touch
`BwrapBackend`" constraint's honoring throughout, and general plan hygiene
(stable IDs, no main-path/appendix duplication, no material decision hidden
only in an appendix) were all independently checked with no findings.

The accepted artifact at the durable locator above contains the complete
disposition log.

## Final verification

- Focused checks: `cargo nextest run -p firma-run` (unit tests for
  `mount.rs`'s new function and `PRESERVED_ETC_HOST_PATHS`); the two new/
  extended e2e tests from Slices 1 and 2; the full existing Hakoniwa e2e
  suite re-run unmodified.
- Workspace checks: `just check` (fmt, lint, hawk, test, build) per this
  repository's standard CI-parity gate.
- Post-implementation independent review: required per
  `adversarial-review`/`reviewing-changes` (with `review-rust-code` for the
  Rust diff), separate from this plan's own review — not yet performed,
  since this plan authorizes no implementation.

## Technical evidence

### Applicability assessment

| Section                     | Applicability | Reason or evidence                                                                                                                                                                                                                                                                   |
| --------------------------- | ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Vocabulary                  | Applicable    | "reconstruction," "preserved path," "allowlist" are used precisely and repeatedly; worth pinning.                                                                                                                                                                                    |
| Alternatives                | Applicable    | Folded into each `DEC-*`'s own "Consequences and rejected alternatives" above rather than restated as a separate section, per the template's own "reference IDs instead of repeating prose" guidance — no separate table needed since each decision already carries its alternative. |
| File-tree diff              | Applicable    | Two existing files modified, no new files.                                                                                                                                                                                                                                           |
| Type and signature sketches | Applicable    | One new enum variant, one new match arm, one new const, one new function signature.                                                                                                                                                                                                  |
| Semantic call traces        | Applicable    | The current-failure and proposed-success paths cross a trust boundary (real host `/etc` vs. sandbox-visible `/etc`).                                                                                                                                                                 |
| Trust analysis              | Applicable    | This is precisely a trust-boundary change.                                                                                                                                                                                                                                           |
| Detailed proof obligations  | Applicable    | `INV-001`'s extension and the new `INV-003` need evidence across e2e suites.                                                                                                                                                                                                         |

### Conditional: Vocabulary

| Canonical term      | Meaning                                                                                                                | Owner/context                          | Synonyms or terms to avoid                                                                                                              | Conflict or decision |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | -------------------- |
| Reconstruction      | Replacing `container.rootfs("/")`'s default `/etc` bind with a fresh `Tmpfs` anchor, then selectively re-populating it | `hakoniwa/mount.rs`                    | "rebuild," "overlay" (avoid "overlay" here — Hakoniwa has no overlayfs; this is tmpfs + selective binds, not a real overlay filesystem) | N/A                  |
| Preserved path      | A real host `/etc` path re-bound read-only onto the reconstructed `/etc`, unchanged in content from today              | `DEC-002`'s `PRESERVED_ETC_HOST_PATHS` | "whitelisted path" (avoid; use "preserved" or "allowlisted")                                                                            | N/A                  |
| Synthesized content | New content `firma-run` writes to `handle.runtime_dir` and binds in, replacing what the real host file would show      | `DEC-004`/`DEC-006`                    | "stub," "fake" (avoid "fake" — it is real content the sandbox actually uses, not a placeholder)                                         | N/A                  |
| Allowlist model     | Default-absent; a path is exposed only if explicitly listed                                                            | `DEC-002`                              | "denylist," "blocklist" (this plan explicitly rejects that model)                                                                       | N/A                  |

### Conditional: File-tree diff

```diff
 crates/firma-run/src/backend/hakoniwa/
~├── mod.rs    # MODIFIED — HakoniwaBackend::prepare gains an enforce_network_namespace-gated
~│               block synthesizing resolv.conf/hosts content and appending SandboxMounts
~└── mount.rs  # MODIFIED — build_mount_ops gains the /etc Tmpfs anchor + PRESERVED_ETC_HOST_PATHS
 crates/firma-run/src/backend/
~└── mod.rs    # MODIFIED — SandboxInfrastructureKind gains a Hosts variant
 crates/firma-hakoniwa-runner/src/main.rs   # UNCHANGED — replays Bind/Tmpfs ops generically already
 tests/e2e/scenarios/
~└── hakoniwa_backend.rs   # MODIFIED — new getaddrinfo-based test, new preserved-content test
 docs-site/src/content/docs/concepts/sandbox.md   # MODIFIED — short note on Hakoniwa's /etc parity
 docs/architecture/hakoniwa-backend-plan.md        # MODIFIED — one-line cross-reference pointer
```

### Conditional: Types and signatures

```rust
// crates/firma-run/src/backend/mod.rs — additive enum variant, no removal
pub(in crate::backend) enum SandboxInfrastructureKind {
    Passwd,
    Group,
    ResolverConfig,
    Hosts, // NEW
}

// crates/firma-run/src/backend/hakoniwa/mount.rs — new const, mirrors
// firma-hakoniwa-runner's LANDLOCK_READ_ONLY_DIRS/LANDLOCK_LIBRARY_DIRS shape.
// Corrected after plan review (PLAN-002): this list is NOT pushed directly
// into `ops` — each entry becomes an ordinary SandboxMount::framework(...),
// appended to `handle.mounts` from HakoniwaBackend::prepare, so it flows
// through the existing validate_mounts/validate_overlay_destinations
// pipeline (duplicate-target collisions with an operator mount are already
// caught by existing code, not a new gap).
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

// crates/firma-run/src/backend/hakoniwa/mount.rs — new pub(super) helper,
// called from HakoniwaBackend::prepare (mod.rs) when DEC-003's gate is
// active; skip-if-absent, mirroring LANDLOCK_READ_ONLY_DIRS's own pattern.
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

// crates/firma-run/src/backend/hakoniwa/mount.rs — the ONLY op this plan
// pushes directly into `ops`, bypassing the SandboxMount pipeline (Tmpfs
// has no MountSpec representation at all); mirrors mask_firma_dir's
// existing emit_tmpfs bypass pattern for the same reason. Called from
// build_mount_ops when DEC-003's gate is active.
fn push_etc_reconstruction_anchor(ops: &mut Vec<HakoniwaMountOp>) {
    ops.push(HakoniwaMountOp::Tmpfs {
        target: PathBuf::from("/etc"),
    });
}

// crates/firma-run/src/backend/hakoniwa/mount.rs — new explicit,
// fail-closed validation (DEC-008), called from build_mount_ops before any
// op is emitted, over the same `mounts: &[ValidatedMount]` other
// validations already inspect. Guards specifically the one path
// (`push_etc_reconstruction_anchor`'s target) that bypasses the ordinary
// duplicate-target check.
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
```

No unrepresentable-state claim is made for this enum addition beyond
ordinary exhaustive-match enforcement: `validate_infrastructure_mount`'s
existing `match kind { Passwd => ..., Group => ..., ResolverConfig => ... }`
(`mount.rs:367-389`) has no wildcard arm, so the compiler forces a new arm
for `Hosts` at every match site over `SandboxInfrastructureKind` — a
correctness aid, not a claim requiring a constructibility witness (no new
newtype, no new illegal-construction surface is introduced).

### Conditional: Semantic call traces

| Field                      | Content                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Trace ID                   | `TRACE-ETC-001`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| State                      | Current                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Entry and stimulus         | Wrapped command inside a `HakoniwaBackend` sandbox calls `socket.getaddrinfo("example.invalid", 80)` (ordinary Python stdlib, no raw sockets)                                                                                                                                                                                                                                                                                                                                                         |
| Path                       | glibc `getaddrinfo` → NSS `hosts:` order from `/etc/nsswitch.conf` (real host file, via `rootfs("/")`) → `files` lookup against the real host `/etc/hosts` (may resolve unexpectedly if a real entry matches) → `dns` lookup consults the real host `/etc/resolv.conf` (real nameserver IPs) → attempts a real UDP query to those nameservers → namespace-isolated, no route out → failure, but _not_ via the sandbox's own DNS-refusal stub, and the real nameserver IPs were readable along the way |
| Input/output types         | n/a (external resolver behavior, not an internal API)                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Validation/trust crossings | None — this is precisely the absence of a trust boundary this plan adds                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Invariant established      | None                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Invariant assumed          | None                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Success outcome            | n/a — this is the failure-shaped current state                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Failure path               | Either a real (leaked) nameserver IP is read, or resolution fails via network-namespace isolation rather than the sandbox's own deterministic stub                                                                                                                                                                                                                                                                                                                                                    |
| Evidence                   | `hakoniwa/mod.rs:76-104`; `hakoniwa_backend.rs:194-280` (existing raw-UDP-only test, confirming no `getaddrinfo` path is exercised today)                                                                                                                                                                                                                                                                                                                                                             |
| Proof boundary             | e2e (`hakoniwa_backend.rs`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Unknowns                   | None for this trace — fully re-confirmed this session                                                                                                                                                                                                                                                                                                                                                                                                                                                 |

| Field                      | Content                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Trace ID                   | `TRACE-ETC-002`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| State                      | Proposed                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Entry and stimulus         | Same stimulus as `TRACE-ETC-001`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Path                       | `HakoniwaBackend::prepare` (synthesizes `resolv.conf`/`hosts`, `DEC-004`/`DEC-006`) → `mount::build_mount_ops` (`rebuild_etc` anchor + preserved list + existing mount loop picks up the two synthesized `SandboxMount`s, `DEC-001`/`DEC-002`) → `firma-hakoniwa-runner::apply_mount_ops` (unchanged, replays `Bind`/`Tmpfs` verbatim) → sandboxed `getaddrinfo` → NSS `files` against synthesized `/etc/hosts` (no match for `example.invalid`) → NSS `dns` against synthesized `/etc/resolv.conf` (`nameserver 127.0.0.1`) → UDP query to `127.0.0.1:53` → the sandbox's own DNS-stub (already fixed to bind that port, `DEC-012`/Slice 7 of the base plan) → `REFUSED` |
| Input/output types         | n/a                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Validation/trust crossings | `HakoniwaBackend::prepare` (trusted `firma-run` process) is the only place that decides what real host content crosses into the sandbox; the wrapped command never gets to choose                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Invariant established      | `INV-001` (extended, now practically true for ordinary resolution, not just a hand-crafted query); `INV-003`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Invariant assumed          | `INV-002` (masking) remains structurally disjoint, argued above, not re-proved per-call                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Success outcome            | Deterministic `REFUSED` via the sandbox's own stub, no real nameserver IP or static `/etc/hosts` entry ever read                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Failure path               | If the DNS stub itself is down (a `DEC-012`-scoped concern, unchanged by this plan), resolution still fails closed (no external route exists regardless)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Evidence                   | This plan's `DEC-001` through `DEC-006`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Proof boundary             | e2e (`hakoniwa_backend.rs`, Slice 2's new test)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Unknowns                   | None material; `DEC-002`'s preserved-list completeness is the residual, named risk                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |

### Conditional: Trust analysis

- Actors: the wrapped/untrusted agent process running inside the sandbox
  (treated as potentially hostile, per this repository's existing posture
  for every `SandboxBackend`); `firma-run`'s own trusted host process
  (constructs the mount plan); `firma-hakoniwa-runner` (trusted, same
  build, replays the plan verbatim — unchanged by this plan).
- Supported workloads and deployment modes: unchanged — this plan only
  affects `HakoniwaBackend` (experimental, opt-in, never a platform
  default).
- Attacker capabilities: full control over what runs inside the sandbox
  (arbitrary code execution as the wrapped command), no control over the
  mount plan itself (computed entirely in the trusted host process before
  the sandbox exists).
- Protected assets: the real host's DNS nameserver configuration and
  `/etc/hosts` static entries (this plan's own target); real host
  `/etc/machine-id`/`/etc/hostname` file content (excluded by `DEC-002`);
  `/etc/ssl`/`/etc/pki` private key material some distros keep alongside
  public certs (excluded by `DEC-002`). Explicitly _not newly_ protected by
  this plan (already exposed today, unchanged): the real host
  `/etc/passwd`/`/etc/group` user/group list, and `gethostname()`'s
  syscall-level real hostname (named as a pre-existing gap, not addressed
  here).
- Trust transitions: none new — the mount plan is still computed entirely
  by the same trusted `firma-run` process, before the sandbox exists,
  exactly as today.
- Reachable abuse paths: an adversarial agent attempting to read the real
  host's DNS configuration, plant a hosts-file bypass, or discover the real
  hostname — the first two are what this plan closes; the third
  (`gethostname()`) is named as unaddressed, not silently left unconsidered.

### Conditional: Proof obligations

| Field                  | Content                                                                                                                                                                                                                                                                                                                                      |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-001` (extended)                                                                                                                                                                                                                                                                                                                         |
| Kind                   | Runtime                                                                                                                                                                                                                                                                                                                                      |
| Owner/proof boundary   | `HakoniwaBackend::prepare`/`mount.rs` (mount-plan construction); e2e suite (observable proof)                                                                                                                                                                                                                                                |
| Suite/boundary         | E2E (`hakoniwa_backend.rs`)                                                                                                                                                                                                                                                                                                                  |
| Stimulus               | A real `firma run --backend hakoniwa` invocation with `enforce_network_namespace = true`, wrapped command runs `python3 -c "socket.getaddrinfo(...)"` against a name that only DNS resolution would attempt                                                                                                                                  |
| Observable effects     | The call reaches the sandbox's own DNS stub (observable via the stub's own deterministic `REFUSED`/failure behavior, matching the existing raw-UDP test's assertion style) rather than a real nameserver or a static `/etc/hosts` entry                                                                                                      |
| Controls/substitutions | None — real `python3`, real sandbox, no fixture DNS server (matching the existing raw-UDP test's own realism)                                                                                                                                                                                                                                |
| Failure cases          | A query for a name with a real `/etc/hosts`-style override would (before this plan) resolve without ever reaching the stub — the negative control this plan's new test should include, proving the old bypass is closed, not merely that the new path works                                                                                  |
| Evidence               | `PROOF-ETC-001` (this row)                                                                                                                                                                                                                                                                                                                   |
| Status                 | Planned (Slice 2)                                                                                                                                                                                                                                                                                                                            |
| Slice                  | Slice 2                                                                                                                                                                                                                                                                                                                                      |
| Limits                 | Proves the ordinary resolver path reaches the stub for this host's default NSS configuration; does not prove every possible `nsswitch.conf` module ordering a different host might configure (`DEC-002`'s preserved real `/etc/nsswitch.conf` means behavior tracks whatever the host actually has, which is the intended design, not a gap) |

| Field                  | Content                                                                                                                                                                                                                                 |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-003`                                                                                                                                                                                                                               |
| Kind                   | Runtime                                                                                                                                                                                                                                 |
| Owner/proof boundary   | `mount.rs`'s `PRESERVED_ETC_HOST_PATHS`/`rebuild_etc`; e2e suite                                                                                                                                                                        |
| Suite/boundary         | Unit (`mount.rs`, existence/emission of the anchor and preserved binds) + E2E (`hakoniwa_backend.rs`, content equality and identity-mode resolution)                                                                                    |
| Stimulus               | A real sandbox launch with `enforce_network_namespace = true` and `identity_mode = SandboxUser`                                                                                                                                         |
| Observable effects     | `cat`ting each preserved path inside the sandbox matches the real host's content; `whoami`/`id` inside the sandbox resolves to `nobody`/`nogroup` at uid/gid `65534`; `cat /etc/resolv.conf`/`/etc/hosts` show only synthesized content |
| Controls/substitutions | None                                                                                                                                                                                                                                    |
| Failure cases          | A preserved path missing on a given host (e.g. no `/etc/services` on a minimal distro) must not fail the sandbox launch — covered by the existence check in `rebuild_etc`                                                               |
| Evidence               | `PROOF-ETC-002` (this row)                                                                                                                                                                                                              |
| Status                 | Planned (Slice 1 for preserved-content equality/identity-mode; Slice 2 for the synthesized-content half)                                                                                                                                |
| Slice                  | Slice 1 (preserve), Slice 2 (synthesize)                                                                                                                                                                                                |
| Limits                 | Does not prove every real-world agent tool tolerates the preserved-path list (`DEC-002`'s named evidence gap) — only that the specific tools this repo's own e2e suite already exercises (`python3`, `bash`) do                         |

| Field                  | Content                                                                                                                                                                                                                                                                                                  |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-003` (the `DEC-008` sub-claim: the `/etc` anchor cannot be silently overridden by an operator mount)                                                                                                                                                                                                |
| Kind                   | Type-adjacent / Runtime                                                                                                                                                                                                                                                                                  |
| Owner/proof boundary   | `mount.rs`'s `reject_operator_mount_targeting_etc_anchor`                                                                                                                                                                                                                                                |
| Suite/boundary         | Unit (`mount.rs`)                                                                                                                                                                                                                                                                                        |
| Stimulus               | A `ValidatedMount` list containing an `OperatorProvided`-authority mount whose target is exactly `/etc` (positive control: the rejection fires); a second list containing one whose target is a deeper `/etc/*` path (negative control: no rejection)                                                    |
| Observable effects     | `Err(RunError::Backend{..})` for the exact-`/etc` case; `Ok(())` for the deeper-path case                                                                                                                                                                                                                |
| Controls/substitutions | None — pure function over an in-memory `Vec`                                                                                                                                                                                                                                                             |
| Failure cases          | An operator mount at `/etc` itself must be rejected; one at `/etc/anything-else` must not be                                                                                                                                                                                                             |
| Evidence               | `PROOF-ETC-003` (this row)                                                                                                                                                                                                                                                                               |
| Status                 | Planned (Slice 1)                                                                                                                                                                                                                                                                                        |
| Slice                  | Slice 1                                                                                                                                                                                                                                                                                                  |
| Limits                 | Proves the check's own logic in isolation; does not by itself prove `build_mount_ops` calls it before any op is emitted — that ordering is the function's caller-side responsibility, verified by the e2e suite continuing to pass with no observed silent `/etc` collision, not by this unit test alone |
