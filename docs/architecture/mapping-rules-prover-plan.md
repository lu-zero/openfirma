# Mapping-rules layer structural verification (narrow OpenShell-prover parity)

## Artifact metadata

- Status: Accepted (all plan-review findings dispositioned as `corrected`)
- Durable locator: `docs/architecture/mapping-rules-prover-plan.md` (this file), branch `exp/mapping-rules-prover`, worktree `/home/lu_zero/Sources/openfirma-mapping-prover`
- Repository revision researched: `8496b5d27b8457531013a82c8dafc472e79770d1` (`origin/main`)
- Task or requirement source: user request this session — "start with reaching parity on that [OpenShell's formal-verification gap] in a standalone branch since this is something orthogonal to the rest," scoped by explicit user decision to the mapping-rules layer only (not Cedar policy verification), following research recorded at `~/Sources/openfirma-notes/notes/openshell-z3-prover.md`, `openshell-comparison.md`, `openshell-extension-points.md`
- Supersedes: Not applicable

## Goal and acceptance outcomes

- Goal: give openfirma's mapping-rules layer (`MappingTable`, `ActionClassRegistry`,
  `crates/firma-sidecar/config/mappings/*.toml`) the same kind of structural,
  exhaustive-over-a-closed-vocabulary verification OpenShell's `openshell-prover`
  gives its binary-capability registry — checking properties existing
  example-based tests don't cover, not re-implementing OpenShell's own Z3
  encoding (which this repo's research found adds no real value over a plain
  decision procedure for its own flat, pre-resolved facts).
- Observable acceptance outcomes:
  1. A merged mapping-rule set (`rules_path` + `rules_paths`) containing a rule
     that can never actually match any request (fully shadowed by
     higher-priority rules already checked first) fails Sidecar startup with a
     specific, actionable error — the same fail-closed contract
     `MappingTableError::DuplicateRule` already gives operators today.
  2. A new `firma mapping-rules validate` CLI subcommand lets an operator check a
     mapping-rule configuration offline/pre-deploy, reporting both the
     shadowing check above and a second, advisory check: which
     `ActionClassRegistry` classes neither a static mapping rule, a Composio
     catalog entry, nor a documented dynamic-reclassification path can ever
     produce — explicitly scoped to the Sidecar's HTTP-intent-classification
     paths, not `firma-run`'s separate local execution-governance subsystem
     (`DEC-006`).
  3. Neither check reasons about Cedar policy evaluation, `Allow`/`Deny`
     outcomes, or anything downstream of classification — confirmed
     out-of-reach for this layer by research (see `Current behavior and
     problem`).

## Scope

- In scope:
  - A new structural check (`INV-001`): every rule in the fully merged,
    already-validated rule set matches at least one request that no
    higher-priority rule already claims. Fail-closed, wired into
    `MappingTable::from_config`'s existing validation (Slice 1).
  - A new advisory check (`INV-002`): every `ActionClassRegistry` class is
    reachable by a static mapping rule, a Composio catalog entry, or is in a
    documented dynamic-reclassification exemption list (`DEC-006`).
    Non-blocking, CLI-only (Slice 2).
  - A new `firma mapping-rules validate` subcommand exposing both checks offline.
- Out of scope: Cedar policy verification, `cedar-policy-symcc`, the
  `cedar-policy` 3→4 version bump, any `PolicyEvaluator`/`Decision`-level
  reasoning, an "agent proposes a policy change" concept or OpenShell's live
  delta-gated auto-approval pattern (no such concept exists in openfirma
  today — not invented here), promoting `ActionClassRegistry::risk_level`
  from today's unconsumed metadata into a pipeline-wide "sensitivity" concept
  (this plan's checks use the registry's existing class list only, not
  `risk_level`), and `firma-run`'s local execution-governance `deny_actions`
  configuration — a separate subsystem (process/syscall governance, not HTTP
  request classification) explicitly excluded from `INV-002` by user decision
  after plan review (`DEC-006`, `PLAN-001`).
- Assumptions: the mapping-rule glob dialect stays exactly as documented today
  (`*` as the only wildcard, no character classes, no regex) — `DEC-002`'s
  algorithm depends on this and must be revisited if the dialect changes.
- Open decisions: whether `INV-001`'s fail-closed startup gate should be
  defeatable by an explicit operator override (for a rule an operator knows is
  shadowed but wants to keep as documentation/defense-in-depth). This plan
  defaults to no override — matching `DuplicateRule`'s own precedent of
  failing outright with no escape hatch — but flags it for reviewer/user
  confirmation rather than deciding it silently.
- Cohesion and split assessment: `INV-001` (fail-closed, startup-blocking) and
  `INV-002` (advisory, CLI-only) have different failure semantics and
  different consumers (Sidecar startup vs. an operator CLI run), but share one
  proof boundary (the merged `MappingRulesFile` against `ActionClassRegistry`)
  and one new CLI subcommand — kept in one plan; splitting further would
  obscure that shared boundary without an independent invariant owner on
  either side.
- Deferred child plans: Not applicable — the explicitly out-of-scope Cedar
  verification work is a distinct future engagement, not a child of this plan
  (it has its own dependency, the `cedar-policy` version bump, and its own
  invariant owner).

## Routing

- Mode: Full
- Trigger evidence: (1) fail-closed behavior — `INV-001` extends the exact
  fail-closed startup contract `MappingTableError::DuplicateRule` already
  establishes (`normalizer/mapping.rs:130-157`); (3) invariant proof boundary —
  this plan establishes two invariants (`INV-001`, `INV-002`) that do not
  exist today (research confirmed no test proves rule-reachability or
  registry-coverage properties, only example-based cases); (6) multiple
  viable designs with material tradeoffs — exact-tuple-grouping vs. general
  glob containment vs. Z3 string/regex theory (`DEC-002`), and CLI surface
  as a new `firma mapping-rules` command vs. extending `firma policy validate`
  (`DEC-004`).
- Higher-mode triggers checked: none apply beyond Full (no distributed/
  concurrency/migration/multi-crate-with-substantial-uncertainty shape).
- Downgrade evidence and reason: Not applicable.

## Current behavior and problem

- Owners and entry points: `MappingTable::from_config`
  (`crates/firma-sidecar/src/normalizer/mapping.rs:130-213`) is the sole
  owner of mapping-rule validation and matching. It is invoked exactly once,
  at Sidecar startup, by `build_pipeline_runtime`
  (`crates/firma-sidecar/src/startup/pipeline.rs:220-329`), after
  `load_mapping_rules` (`startup/pipeline.rs:43-84`) merges `rules_path` +
  `rules_paths` into one `MappingRulesFile`. No CLI command loads or checks
  mapping-rule TOML content today — `firma policy {list,validate,test}`
  (`crates/firma/src/args/policy.rs:14-30`) are Cedar-only.
- Current success and failure outcomes: on a successful match,
  `MatchResult::Matched` yields only a classified `NormalizedEnvelope` — this
  is necessarily followed by capability-token validation and full Cedar
  policy evaluation before any `Allow`/`Deny` is observable
  (`pipeline.rs:365-607`); a mapping-rule match can never itself produce an
  `Allow`. On no match, a per-table `default_protected` flag (default
  `true`) decides `DenyReason::UnclassifiedIntent` (fail-closed,
  `mapping.rs:76-83`, `normalizer/mod.rs:330-337`) vs.
  `EnforcementDecision::Passthrough` (fail-open by explicit operator config).
  Today's only structural checks at load time are: duplicate
  `(method, host, path)` tuples across the merged file set
  (`MappingTableError::DuplicateRule`, `mapping.rs:138-157`) and rejecting an
  action-class name absent from `ActionClassRegistry`
  (`mapping_rules_rejects_nonexisting_action_class`). Neither checks rule
  reachability or registry coverage.
- Evidence: rule matching uses a numeric specificity score
  (`MappingRule::compute_specificity`, `mapping.rs:86-116`: exact host +100,
  specific method +10, path presence +5 plus per-segment scoring, wildcard-free
  path +10) to sort the merged rule list, then returns the first rule whose
  glob pattern (`glob_match`, re-exported from `firma-secret-provider`,
  `*`-only wildcard semantics) matches the request
  (`find_match`, `mapping.rs:196-213`). Because sorting is global and matching
  is first-match-wins, a lower-specificity rule is _reachable_ only if some
  concrete request exists that its own pattern matches and every
  higher-priority rule's pattern does not — this is a real, currently
  unverified correctness property, not merely a duplicate-key check.
  Separately, `enrich_github_git_metadata`
  (`normalizer/mod.rs:397-415`) can reclassify a matched rule's action class
  at runtime for `github.com`/`api.github.com` traffic specifically (a
  `git-receive-pack` delete becomes `code.destructive` regardless of what the
  static rule said) — the final `action_class` is not always fully
  determined by static TOML content alone.

## Key decisions and tradeoffs

### `DEC-001`: Verify classification determinism and coverage, not enforcement bypass

- Choice: frame both new invariants around the mapping layer's actual job —
  producing the correct, deterministic `action_class` for a request — not
  around "preventing bypass of enforcement," which research confirmed is not
  a property this layer can violate (every matched request still passes
  capability validation and full Cedar evaluation; `pipeline.rs:365-607`).
- Rationale and evidence: `Current behavior and problem` traces a mapping
  match to a `NormalizedEnvelope`, never directly to `Allow`. OpenShell's own
  categories (credential reach, capability expansion) are meaningful there
  because its Landlock/seccomp layer's classification _is_ the enforcement
  decision; openfirma's is not.
- Consequences and rejected alternatives: rejected reframing OpenShell's
  `credential_reach_expansion`/`l7_bypass_credentialed` categories onto the
  mapping layer directly (as the user's own earlier research draft
  tentatively did) — would silently overclaim a bypass property this layer
  cannot exhibit. The genuinely analogous claims involving actual `Allow`
  outcomes require Cedar-policy reasoning and are explicitly out of scope
  (see `~/Sources/openfirma-notes/notes/openshell-z3-prover.md`, claims 1-3).

### `DEC-002`: Exact-tuple equality grouping plus method-set union, not general glob containment or an SMT dependency — refined twice: after plan review (`PLAN-002`), then during implementation

- Choice: implement `INV-001`'s shadowing check as: (1) group rules by exact
  string equality of `(host_pattern, path_pattern)` — no glob interpretation
  at this step, a literal tuple comparison; (2) within each group, order by
  the existing specificity score and check whether the union of
  higher-priority rules' method coverage (`None` contributing all of
  `firma_core::HttpMethod`'s 8 closed variants, `Some(m)` contributing just
  `m`) is a superset of each lower-priority rule's own method requirement in
  that same group.
- Rationale and evidence: plan review (`PLAN-002`) found the original
  single-pattern-pairwise framing insufficient — it constructed a concrete,
  realistic witness: N higher-priority rules, one per method value, sharing
  an identical wildcard host/path pattern, jointly (not individually) shadow
  a lower-priority any-method rule with the same pattern. Implementing the
  reviewer's own suggested fix (general pairwise host/path _containment_,
  e.g. does `*.github.com` contain `api.github.com`) turned out to need real
  automaton-based reasoning once path patterns were inspected directly: the
  shipped `config/mappings/*.toml` files have host patterns that are always
  exact (zero wildcards, confirmed by direct inspection) but path patterns
  with up to 4 wildcards each (e.g. `/repos/*/*/pulls/*/reviews/*/events`) —
  general containment between two _different_ multi-wildcard patterns is a
  genuine language-containment problem, not "direct segment comparison" as
  first claimed. Exact-tuple-equality grouping sidesteps this entirely: it
  needs no pattern interpretation at all, and directly targets the reviewer's
  own witness shape, which is also confirmed _real and common_ in the
  shipped config — direct inspection found 30+ exact `(host, path)` tuples
  already shared by multiple rules differing only by method (e.g.
  `api.stripe.com`+`/v1/charges`: `GET`→`payment.read`, `POST`→
  `payment.transfer`), none of which currently include a `None`-method rule
  (so nothing is shadowed today), but any future rule addition to such a
  group using `method = None` would be exactly the case this check exists to
  catch.
  Grounding on `HttpMethod` rather than `firma_http::Method`'s 9 named
  constants is also a refinement found during implementation: `Method` wraps
  the open `http::Method` (any HTTP token is technically acceptable in TOML),
  but `normalizer/mod.rs:303`'s `HttpMethod::try_from` — which runs _after_
  mapping-table matching, only in the `Matched` arm — denies any request
  whose method isn't one of `HttpMethod`'s 8 closed variants, regardless of
  which rule matched. A rule reachable only via a method outside that set
  can never produce an observable envelope either way, so grounding
  reachability in the 8-value set is the precise, evidence-backed choice.
- Consequences and rejected alternatives: rejected Z3 string/regex theory —
  same dependency-weight reasoning as before, now more clearly justified:
  the _actually implemented_ check needs no string-theory reasoning at all.
  Rejected general pairwise glob containment (the `PLAN-002`-era design) —
  correct in principle but requires automaton construction for multi-wildcard
  path patterns, disproportionate to what the exact-tuple-equality
  alternative already proves for a real, present pattern. The completeness
  claim is consequently narrower than either prior version: this procedure
  proves `INV-001` **only** for rules sharing an _exact_ `(host_pattern,
  path_pattern)` string with at least one other rule — it proves nothing
  about shadowing across two _different_ patterns (whether or not one's
  language is a subset of the other's), which remains fully out of scope,
  not merely incompletely covered. `INV-001`'s semantic predicate and
  `TRACE-001`/proof-obligation "Limits" reflect this precise, narrower scope.
  **Still flagged as this plan's least-confident decision** (see `Risks and
  gaps`) only insofar as the exact-tuple scope may prove too narrow to be
  useful in practice if real shadowing bugs turn out to occur across
  genuinely different patterns rather than within exact-tuple groups — the
  shipped-config inspection above is evidence, not proof, that the narrower
  scope is where real bugs would occur.

### `DEC-003`: `INV-001` is fail-closed and startup-blocking; `INV-002` is advisory and CLI-only

- Choice: a shadowed rule (`INV-001`) aborts Sidecar startup exactly like
  `MappingTableError::DuplicateRule` does today. An orphaned registry class
  (`INV-002`) is reported only by the new `firma mapping-rules validate` CLI
  command and never blocks startup.
- Rationale and evidence: mapping-rule config is load-once and not
  hot-reloadable (`build_pipeline_runtime` calls `from_config` exactly once;
  no `notify`/`watch`/reload path exists for it, unlike the Cedar bundle's
  `SwappablePolicyEvaluation` or the capability map's reload task,
  `startup/pipeline.rs:258-276`) — "load once, fail closed, no partial
  fallback" is this layer's established lifecycle, and `INV-001` is a
  correctness gap in that same class as `DuplicateRule`. An orphaned registry
  class is a hygiene signal (an intended-but-unused class), not a live gap —
  nothing is under-protected by it; failing startup over it would be an
  availability regression disproportionate to its non-impact.
- Consequences and rejected alternatives: rejected making `INV-002` also
  fail-closed at startup — would turn a hygiene lint into an outage risk for
  unused registry entries that cause no actual exposure.

### `DEC-004`: New `firma mapping-rules validate` subcommand, not an extension of `firma policy validate` — renamed after plan review (`PLAN-004`)

- Choice: expose both checks through a new, sibling top-level `firma
  mapping-rules validate` subcommand, not a flag on the existing `firma
  policy validate`.
- Rationale and evidence: `firma policy validate` takes one `.cedar` file
  path (`args/policy.rs:20-23`) and checks it against the Firma Cedar schema
  — a fundamentally different artifact shape from mapping rules, which
  resolve from a profile/`firma.toml`'s `[enforcement.mapping]`
  `rules_path`/`rules_paths` (potentially several merged files), the same way
  Sidecar startup resolves them (`load_mapping_rules`).
- Consequences and rejected alternatives: rejected a `--mapping <path>` flag
  on `firma policy validate` — would conflate two independently-validated
  artifact kinds with incompatible resolution shapes (single explicit file vs.
  profile-relative multi-file merge) under one subcommand. Originally named
  `firma mapping validate`; plan review (`PLAN-004`) found this collides with
  the existing `--mapping` flag on `firma config` (`args/config.rs:64`,
  selecting a built-in mapping _template_ to scaffold — a different concept)
  and `firma policy list`'s "Mappings (--mapping, repeatable)" output
  section. Renamed to `firma mapping-rules validate` to avoid the collision.

### `DEC-005`: Dynamic-reclassification exemption list is explicit and named, not inferred

- Choice: `INV-002`'s check consults a small, explicitly maintained constant
  listing action classes producible only via runtime reclassification (today:
  `code.destructive`, from `enrich_github_git_metadata`'s `git-receive-pack`
  delete path, `normalizer/mod.rs:412-415`), not a heuristic or an attempt to
  statically analyze the enrichment functions.
- Rationale and evidence: research found exactly one such dynamic path today,
  fully traced. A hard-coded, documented exemption list keeps the checker
  honest about what it can and cannot see, rather than silently
  false-positiving on a legitimate class or silently trying to be clever
  about analyzing arbitrary Rust reclassification logic.
- Consequences and rejected alternatives: rejected leaving `code.destructive`
  unexempted (would produce a permanent false-positive `INV-002` finding) and
  rejected building a general static analyzer over `enrich_*` functions
  (unbounded scope for one known, small case). Recorded as a maintenance
  obligation in `Risks and gaps` — a future `enrich_*` addition must update
  this list by hand.

### `DEC-006`: `INV-002` covers the mapping-rules TOML and the Composio JSON catalog; the local execution-governance registry stays explicitly excluded — added after plan review (`PLAN-001`)

- Choice: `find_orphaned_action_classes` checks a class as "producible" if it
  appears in the merged `MappingRulesFile`, in any
  `crates/firma-sidecar/config/composio/*.mapping.json` catalog, or in the
  `DEC-005` dynamic-reclassification exemption list. Classes used only by
  `firma-run`'s local seccomp/exec-gate `deny_actions` configuration
  (`crates/firma-run/src/seccomp.rs`) are explicitly named as out of scope and
  excluded from the "orphaned" check entirely — reported by neither an error
  nor a warning, and not silently treated as covered either.
- Rationale and evidence: plan review (`PLAN-001`, confirmed independently —
  see `Plan-review findings and dispositions`) found `INV-002` as originally
  scoped would report roughly 20 of 52 registry classes as false-positive
  "orphaned," because the Composio catalog (`composio/mod.rs:902-956`,
  `action_for_tool` → `logical_envelope`) is a second, real producer of
  `ActionClassRegistry` classes that bypasses `MappingTable`/
  `IntentNormalizer::normalize` entirely — e.g. `calendar.read`,
  `communication.external.manage`, `credential.read` appear only in catalog
  JSON, never in any `mapping-rules.toml`. This is still squarely "the
  Sidecar's HTTP-intent-classification job," just via a second producer, so
  including it keeps `INV-002` honest without expanding its actual purpose.
  `firma-run`'s local execution-governance use of the same string constants
  (`system.execute`, `filesystem.delete`, etc., `seccomp.rs:747-832`) is a
  different subsystem entirely — local process/syscall governance, not HTTP
  request classification — confirmed by the user as out of scope for this
  plan rather than folded in.
- Consequences and rejected alternatives: rejected covering all three
  producers (mapping-rules TOML, Composio catalog, and `firma-run`'s local
  exec gate) — would pull a second crate and a structurally different
  enforcement mechanism into this plan's scope, undermining "standalone,
  orthogonal branch." Rejected leaving `INV-002` scoped to mapping-rules TOML
  only with a static exemption list standing in for the Composio catalog —
  would keep the check technically non-false-positive but silently give up
  proving anything about ~20 real classes' actual reachability, which is a
  materially weaker claim than "checked" while still calling itself
  `check_registry_reachability`.

### `DEC-007`: Expose `load_mapping_rules` as `pub` from `firma-sidecar`; the `firma` CLI calls it directly — added after plan review (`PLAN-003`)

- Choice: make `load_mapping_rules` (`crates/firma-sidecar/src/startup/pipeline.rs:43-84`)
  `pub` (re-exported from `firma-sidecar`'s crate root or a `pub` submodule),
  and have the new `firma mapping-rules validate` command call it directly.
  No changes to `firma-config-loader`.
- Rationale and evidence: plan review (`PLAN-003`) found the plan originally
  assumed the CLI could reuse `load_mapping_rules` "the same way" without
  addressing that it is a private function in `firma-sidecar`, not visible to
  the `firma` binary crate. Verified independently: `crates/firma/Cargo.toml`
  already depends on `firma-sidecar` (`firma-sidecar = { workspace = true }`)
  — there is no new crate dependency to add, only a visibility change.
  `firma-config-loader` has no `MappingRulesFile`/mapping-rules awareness at
  all (confirmed absent by grep) and is not a better fit than exposing the
  function where the type it returns is already owned.
- Consequences and rejected alternatives: rejected reimplementing the
  resolution logic separately in the `firma` crate — would create two
  independent implementations of mapping-rules-file resolution that could
  silently drift, undermining the CLI check's claim to prove anything about
  what the Sidecar will actually load (exactly the risk `PLAN-003` named).
  Rejected relocating the logic into `firma-config-loader` — that crate owns
  `firma.toml` discovery/schema loading generally, but has no existing
  mapping-rules-specific types to receive this logic without expanding its
  own charter for a single caller.

## Architecture and invariant ownership

- Architecture shape: both checks are pure functions added alongside
  `MappingTable` in `crates/firma-sidecar/src/normalizer/`, consuming the
  already-merged `MappingRulesFile` and the existing `ActionClassRegistry` —
  no new crate, no new runtime dependency, mirroring the precedent
  `firma_core::cedar::validate_policies` already sets (one typed-error/typed-
  finding library function, reused by both a startup loader and a CLI
  subcommand).

### `INV-001`: Every rule sharing an exact host/path tuple with another rule has a reachable method

- Semantic predicate (narrowed twice — after `PLAN-002`, then during
  implementation, `DEC-002`): group merged rules by exact string equality of
  `(host_pattern, path_pattern)`. Within each group, order by specificity;
  for every rule R in a group, some higher-priority rule in the _same group_
  must exist whose method requirement is `None` (any) or equals R's own —
  otherwise R's method requirement is uncovered by anything ranked before it
  within its group and R is unreachable within `HttpMethod`'s closed 8-value
  method universe. This proves nothing about shadowing across two rules with
  _different_ `(host_pattern, path_pattern)` strings, whether or not one
  pattern's language is a glob-subset of the other's — that case is fully
  out of scope, not an incompletely-covered edge of this check.
- Primary owner: `MappingTable::from_config`, extending its existing
  validation (alongside `DuplicateRule` and unknown-action-class rejection).
- Detailed proof: `TRACE-001` (Technical evidence).

### `INV-002`: Every registry action class is producible by the Sidecar's HTTP-intent-classification paths

- Semantic predicate (expanded after `PLAN-001`): for every class C in
  `ActionClassRegistry`, at least one of the following holds: some merged
  mapping rule's `action_class` field equals C; some entry in any
  `crates/firma-sidecar/config/composio/*.mapping.json` catalog maps to C; or
  C appears in the `DEC-005` dynamic-reclassification exemption list. Classes
  used exclusively by `firma-run`'s local execution-governance
  `deny_actions` configuration (a different subsystem, `DEC-006`) are
  excluded from this predicate entirely — neither counted as covered nor
  flagged as orphaned.
- Primary owner: the new `check_registry_reachability` function, consumed
  only by the `firma mapping-rules validate` CLI subcommand (`DEC-003`).
- Detailed proof: `TRACE-002` (Technical evidence).

- Compatibility, migration, and failure semantics: `INV-001` changes
  `MappingTableError`'s exhaustive match (a new variant) — any code
  exhaustively matching this enum must add an arm; grep confirms this error
  type is only matched inside `firma-sidecar` itself. `INV-002` is additive
  (new function, new CLI subcommand, and per `DEC-007`, a new `pub` surface
  on `load_mapping_rules` — `firma-sidecar` is `publish = false`, so per this
  repo's own API-stability rule this is an internal implementation detail,
  not a SemVer-relevant change, despite crossing a crate boundary within the
  workspace) with no external compatibility impact.
- Durable documentation owner: this plan folds into
  `docs/architecture/sidecar-overview.md`'s mapping-rules description on
  acceptance (cross-reference, not duplication).

## Implementation slices

### Slice 1: Fail-closed rule-reachability check (`INV-001`)

- Production, types, tests, and docs/config: add
  `MappingTableError::ShadowedRule { rule: MappingRuleId, shadowed_by:
  Vec<MappingRuleId> }` (or equivalent descriptive fields) and the
  exact-tuple-equality-group-plus-method-union check (`DEC-002`), invoked
  from inside `MappingTable::from_config` immediately after the existing
  duplicate-tuple check. Unit tests: a constructed rule group sharing one
  exact `(host, path)` where a `None`-method rule is fully shadowed by the
  union of method-specific rules ranked above it (must fail), the same
  shape with one method left uncovered (must pass), and property-based
  tests (via the workspace's existing `proptest` dependency) generating
  random small rule sets — varying rule _count_ and method combinations
  per group, not just single pairs, so the generator actually exercises
  joint coverage rather than reproducing `PLAN-002`'s own blind spot — and
  cross-checking the decision procedure's answer against a brute-force
  enumerative oracle over `HttpMethod`'s 8 closed values. Integration test:
  `MappingTable::from_config` given a fixture config with a shadowed rule
  returns an `Err` naming the shadowed rule — **corrected after
  post-implementation review**: originally planned one level higher, at
  `build_pipeline_runtime`, but no other `MappingTableError` variant
  (including the pre-existing `DuplicateRule`) is tested at that level
  either, and `build_pipeline_runtime` calls `from_config` directly with no
  intervening logic, so the risk left uncovered by testing one level lower
  is negligible — matching existing precedent rather than adding a novel,
  redundant integration layer this repo doesn't otherwise use for this
  error type.
- Affected decisions and traces: `DEC-001`, `DEC-002`, `DEC-003`; `TRACE-001`.
- Proof obligations: `INV-001`.
- Focused verification: `cargo nextest run -p firma-sidecar -E
  'test(mapping)'`; the new startup integration test.
- Dependencies: none (first slice).
- Intentionally unsupported: no operator override/escape hatch for a
  known-shadowed rule (see `Scope`'s open decision) — a shadowed rule must be
  fixed or removed, not suppressed, matching `DuplicateRule`'s own precedent
  unless the reviewer/user decides otherwise.

### Slice 2: Advisory registry-reachability check and `firma mapping-rules validate` (`INV-002`)

- Production, types, tests, and docs/config: make `load_mapping_rules`
  `pub` in `firma-sidecar` (`DEC-007`); add
  `check_registry_reachability(&MappingRulesFile, &[ComposioMappingCatalog],
  &ActionClassRegistry) -> Vec<OrphanedClassFinding>` (loading and parsing
  the shipped `config/composio/*.mapping.json` catalogs alongside the merged
  `MappingRulesFile`), the `DEC-005` exemption constant, the `DEC-006`
  excluded-class list for `firma-run`'s local execution-governance classes,
  and the new `firma mapping-rules validate --config <path>` subcommand
  (`crates/firma/src/args/mapping_rules.rs`, new) that calls the now-`pub`
  `load_mapping_rules` directly, runs Slice 1's check (reported as errors)
  and this slice's check (reported as warnings), and exits non-zero only if
  any error is present — mirroring `firma policy validate`'s existing
  exit-code convention. Unit test for the exemption list against the real
  shipped `github.toml` (confirms `code.destructive` is correctly exempted,
  not silently unreachable forever); unit test against the real shipped
  Composio catalogs confirming their classes are recognized as covered, not
  orphaned. CLI black-box integration test per `writing-black-box-tests`
  (spawn `env!("CARGO_BIN_EXE_firma")`, assert exit code and rendered
  findings for a fixture with one orphaned class and no errors).
- Affected decisions and traces: `DEC-003`, `DEC-004`, `DEC-005`, `DEC-006`,
  `DEC-007`; `TRACE-002`.
- Proof obligations: `INV-002`.
- Focused verification: `cargo nextest run -p firma --test cli -E
  'test(mapping_rules_validate)'`.
- Dependencies: Slice 1 (reuses its check function for the error half of the
  CLI's combined report) and `DEC-007`'s visibility change.
- Intentionally unsupported: no static analysis of `enrich_*` functions
  themselves — the exemption list is hand-maintained (`DEC-005`); no coverage
  of `firma-run`'s local execution-governance classes (`DEC-006`).

## Risks and gaps

- Existing risks: `DEC-002`'s two-phase decision procedure carries its own
  correctness risk — an incorrect containment or method-union check could
  either miss a real shadowing case (silent false negative, no worse than
  today) or wrongly reject a valid rule set (false positive, an availability
  regression at startup). Mitigated, not eliminated, by Slice 1's
  property-based cross-check against a brute-force oracle — the oracle's
  generator must vary rule _counts_ and method combinations, not just
  single-pattern pairs, or it would reproduce `PLAN-002`'s own blind spot
  instead of catching it.
- Planned mitigations: property-based testing (above), explicitly generating
  multi-rule scenarios exercising joint method-union coverage; keeping the
  checked grammar deliberately narrow (today's `*`-only dialect) rather than
  generalizing ahead of need.
- Explicit evidence gaps: whether a future `enrich_*` addition reliably
  updates `DEC-005`'s exemption list, or a future Composio catalog update
  adds a class this check doesn't re-derive automatically, are both
  process/maintenance gaps, not technical ones — no automated cross-check
  ties any of them together. Whether operators want an override escape
  hatch for `INV-001` (`Scope`'s open decision) is unresolved pending
  reviewer/user input. `INV-001`'s corrected algorithm (`DEC-002`) is now
  explicitly known-incomplete for shadowing via a union of higher-priority
  rules with _different_ host/path patterns — not a silent gap, but a real
  one: such a rule set would pass `INV-001` today without actually being
  proven reachable.
- Least-confident decisions: `DEC-002` (bespoke two-phase algorithm vs. Z3
  string theory, or vs. a fuller automaton-based union-coverage algorithm
  that would also close the different-host/path-patterns gap above) — this
  is the one place in this plan where reaching for an SMT solver would be
  solving the actual class of problem solvers exist for, unlike OpenShell's
  own usage; rejected primarily on dependency-weight and scope grounds,
  which is a judgment call rather than a hard technical disqualification.

## Plan-review findings and dispositions

Independent review performed by a fresh reviewer with no prior context on
this plan, against the researched revision (`8496b5d2`), reconstructing every
cited claim directly in the repository rather than trusting the plan's
citations. Confirmed accurate, unchanged: `DEC-001`'s core claim (a mapping
match can never itself produce `Allow`), the glob dialect's `*`-only
semantics, `risk_level`'s unconsumed status, the absence of any pre-existing
test proving `INV-001`/`INV-002`-shaped properties, `MappingTable`'s
load-once/no-hot-reload lifecycle, and `MappingTableError`'s exhaustive-match
compatibility scope. Four findings follow, preserved as reported.

### PLAN-001 — Critical — Design gap / unsound proof boundary — Confirmed conflict

- Claim challenged: `DEC-005`'s exemption list (`code.destructive` only) is
  complete, and `INV-002` can correctly determine "no static mapping rule and
  no documented dynamic-reclassification path can ever produce" a given
  `ActionClassRegistry` class by inspecting only `MappingRulesFile`.
- Evidence: `ActionClassRegistry` is a global registry consumed by at least
  two producers the plan never mentioned: (1) the Composio catalog
  (`crates/firma-sidecar/src/composio/catalog.rs`,
  `action_for_tool`/`logical_envelope` at `composio/mod.rs:902-956`), sourced
  from `crates/firma-sidecar/config/composio/*.mapping.json`, entirely
  bypassing `MappingTable`/`IntentNormalizer::normalize` — a computed set-diff
  shows classes producible only via this catalog (`calendar.read/create/
  update/delete`, `document.read/write/delete/schema.write`, `credential.read`,
  plus others); `crates/firma-sidecar/config/mappings/composio.toml` itself
  maps only one class for all Composio hosts. (2) `firma-run`'s local
  seccomp/exec gate (`crates/firma-run/src/seccomp.rs`), matching literal
  `ActionClassRegistry` strings entirely outside the Sidecar/HTTP mapping-rules
  layer.
- Reachability: any run of the plan's proposed `firma mapping-rules validate`
  against the real shipped config would report roughly 20 of ~50 registry
  classes as "orphaned" — a majority false-positive rate.
- Invariant/boundary: `INV-002`'s primary owner as originally scoped cannot
  honor its own semantic predicate.
- Impact: directly falsifies acceptance outcome #2 for the real, shipped
  configuration; the advisory check as scoped is not merely incomplete but
  actively misleading.
- Correction: either (a) expand the exemption mechanism to cover both other
  producers with the same rigor as `code.destructive`, or (b) narrow
  `INV-002`'s semantic predicate and acceptance-outcome wording to an
  explicit, disclosed subset, with user sign-off.
- Confidence: High.
- Assumptions: none material.

```yaml
disposition:
  status: corrected
  rationale: >
    User decision (asked directly, since this changes what the feature
    claims to operators): cover mapping-rules.toml + the Composio catalog as
    two legitimate producers of the same job (Sidecar HTTP-intent
    classification); explicitly exclude firma-run's local execution-governance
    classes as a genuinely separate subsystem, named rather than silently
    treated as covered or silently ignored.
  incorporated_at: DEC-006 (new), INV-002 semantic predicate, Slice 2, Goal/
    acceptance outcome 2, Scope (in/out), Vocabulary, both Technical evidence
    proof-obligation tables' Limits rows.
  decided_by: user
```

### PLAN-002 — High — Algorithmic-soundness gap in DEC-002 — Confirmed conflict (design risk)

- Claim challenged: `DEC-002`'s tractability argument ("containment between
  two such patterns... decidable in low-degree polynomial time by direct
  segment comparison") is sufficient to implement `INV-001`.
- Evidence: `DEC-002`'s rationale frames the algorithm as pairwise pattern
  containment, but `INV-001`'s semantic predicate requires that a rule's
  matching set be covered by the _union_ of all higher-priority rules, not
  any single one. Concrete witness: `firma_http::Method` is a small closed
  set; N higher-priority rules, one per method, identical wildcard host/path,
  jointly (not individually) shadow a lower-priority any-method rule with the
  same pattern. `github.toml`'s real shape (52 of 54 rules specify a method,
  2 don't, on overlapping patterns) makes this realistic, not contrived.
- Reachability: triggered whenever several method-specific rules' union (not
  any single member) covers a lower-priority any-method rule's domain.
- Invariant/boundary: `INV-001`.
- Impact: if implemented per the original pairwise-only rationale, the
  fail-closed startup gate could silently fail to catch a genuinely
  unreachable rule — a correctness regression undermining the guarantee
  `INV-001` exists to provide. The plan's property-based mitigation doesn't
  guarantee catching this unless the oracle varies rule count/method
  combinations, which the original design text didn't specify.
- Correction: specify (and re-argue tractability for) a genuine multi-rule/
  union-coverage decision procedure, not single-pattern-vs-single-pattern
  containment.
- Confidence: High for the gap between what was written and what's needed.
- Assumptions: none material.

```yaml
disposition:
  status: corrected
  rationale: >
    DEC-002 revised to a two-phase procedure: pairwise host/path containment
    (unchanged, still tractable) followed by a set-union check over the
    closed 9-value Method enum. This catches the reported witness. Explicitly
    does NOT claim to solve the fully general case (union-coverage across
    genuinely different, non-identical host/path patterns) — that remains a
    disclosed limitation in INV-001's semantic predicate and its
    proof-obligation Limits row, not silently treated as solved.
  incorporated_at: DEC-002, INV-001 semantic predicate, Slice 1 (implicitly,
    algorithm shape), Risks and gaps (existing risk + least-confident
    decision), INV-001 proof-obligation Limits row.
  decided_by: planner
```

### PLAN-003 — Medium/High — Undeclared cross-crate ownership gap — Confirmed conflict

- Claim challenged: the `firma mapping-rules validate` CLI subcommand can
  "resolve the profile's mapping-rule files the same way `load_mapping_rules`
  does."
- Evidence: `load_mapping_rules` is a private function inside
  `firma-sidecar::startup::pipeline`, not exported; the `firma` binary crate
  cannot call it as-is. `firma-config-loader` has no `MappingRulesFile`/
  mapping-rules awareness at all.
- Reachability: build-time/design-time gap — Slice 2 as specified cannot
  compile against current module visibility.
- Invariant/boundary: touches "validation happens once at the boundary that
  can own it" — reimplementing the resolution in `firma` would create two
  independent implementations that could silently drift.
- Impact: the plan presented this as a solved wiring detail; it was an
  undecided design choice with real drift risk.
- Correction: add an explicit decision choosing between exposing/relocating
  `load_mapping_rules` vs. reimplementing, naming the drift risk.
- Confidence: High (direct visibility check).
- Assumptions: none material.

```yaml
disposition:
  status: corrected
  rationale: >
    Verified firma/Cargo.toml already depends on firma-sidecar (workspace
    dependency) — no new crate dependency needed. Added DEC-007: make
    load_mapping_rules pub in firma-sidecar, call it directly from the new
    CLI code. Rejected reimplementation (drift risk, exactly as the finding
    named) and rejected relocating into firma-config-loader (no existing
    mapping-rules awareness there, would expand that crate's charter for one
    caller).
  incorporated_at: DEC-007 (new), Slice 2, file-tree diff, TRACE-002.
  decided_by: planner
```

### PLAN-004 — Low/Medium — Vocabulary collision, not audited — Design risk

- Claim challenged: the plan's Vocabulary section is complete, and `DEC-004`'s
  new `firma mapping` top-level subcommand doesn't conflict with existing CLI
  terminology.
- Evidence: `firma config --mapping` already exists (selects a built-in
  mapping _template_ to scaffold, `args/config.rs:64`); `firma policy list`
  already prints a "Mappings (--mapping, repeatable)" section. The proposed
  `firma mapping validate` introduces a new top-level noun "mapping" meaning
  something different from both existing uses.
- Reachability: user-facing only — CLI-discoverability/naming confusion.
- Invariant/boundary: none (UX), but `reviewing-plans` calls for challenging
  terminology overlap.
- Impact: minor but real naming confusion; not a correctness issue.
- Correction: record the existing usage in Vocabulary and either accept the
  overlap explicitly or pick a less overloaded subcommand name.
- Confidence: High that the collision exists; the severity judgment is a
  product-naming call for the user.
- Assumptions: none material.

```yaml
disposition:
  status: corrected
  rationale: >
    Renamed the subcommand to `firma mapping-rules validate` throughout the
    plan, avoiding the collision rather than accepting it. Recorded the
    existing --mapping usages in the Vocabulary table with an explicit
    "avoid confusing with" note.
  incorporated_at: DEC-004, Goal/acceptance outcome 2, Scope, Slice 2,
    file-tree diff, Vocabulary, TRACE-002.
  decided_by: planner
```

No findings were raised regarding the "agent proposes a policy change"
non-invention constraint or the Cedar/`cedar-policy-symcc`/`PolicyEvaluator`
out-of-scope boundary — the review confirmed the plan stays clear of both
throughout. Full-mode routing was independently confirmed as justified;
Slice 1 was confirmed independently shippable; Slice 2's dependency on
resolving `PLAN-003` is now recorded explicitly (`DEC-007`).

## Final verification

- Focused checks: `cargo nextest run -p firma-sidecar -p firma` (mapping/CLI
  test selectors above); `cargo test --doc -p firma-sidecar -p firma`.
- Workspace checks: `just check` on the branch before merge.
- Post-implementation independent review: required per
  `adversarial-review`/`reviewing-changes` after implementation, independent
  of this plan review.

## Technical evidence

### Applicability assessment

| Section                     | Applicability | Reason or evidence                                                                |
| --------------------------- | ------------- | --------------------------------------------------------------------------------- |
| Vocabulary                  | Applicable    | "shadowed rule," "reachable," "orphaned class" are new terms this plan introduces |
| Alternatives                | Applicable    | `DEC-002`, `DEC-003`, `DEC-004` each have a material rejected alternative         |
| File-tree diff              | Applicable    | new files in `firma-sidecar` and `firma`                                          |
| Type and signature sketches | Applicable    | `MappingFinding`-shaped types are new                                             |
| Semantic call traces        | Applicable    | startup and CLI paths both change                                                 |
| Trust analysis              | Applicable    | fail-closed startup behavior is a trust-relevant path                             |
| Detailed proof obligations  | Applicable    | `INV-001`/`INV-002` need explicit proof-boundary records                          |

### Conditional: Vocabulary

| Canonical term                 | Meaning                                                                                                                                                                                                                                                         | Owner/context                            | Synonyms or terms to avoid                                                                                                                                                                                                                         | Conflict or decision |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------- |
| Shadowed rule                  | A configured mapping rule whose pattern is fully subsumed by one or more higher-priority rules, so it can never match a real request                                                                                                                            | `MappingTable::from_config`              | "dead rule," "unreachable rule" (avoid — "shadowed" is used consistently in this plan)                                                                                                                                                             | `DEC-001`, `DEC-002` |
| Orphaned action class          | A registry class with no static mapping rule, no Composio catalog entry, and no dynamic-reclassification exemption producing it — excludes classes used only by `firma-run`'s local execution-governance config, which this term deliberately does not describe | `check_registry_reachability`            | "unused class," "dead class" (avoid — "orphaned" distinguishes it from `risk_level`'s unrelated dead-code status)                                                                                                                                  | `DEC-005`, `DEC-006` |
| Sensitivity                    | Deliberately NOT introduced by this plan — `risk_level` remains unconsumed metadata outside this checker's scope                                                                                                                                                | N/A                                      | Do not conflate with `INV-001`/`INV-002`, which don't consult `risk_level` at all                                                                                                                                                                  | Scope                |
| `firma mapping-rules validate` | The new CLI subcommand this plan adds                                                                                                                                                                                                                           | `crates/firma/src/args/mapping_rules.rs` | Avoid confusing with `firma config --mapping` (selects a built-in mapping _template_ to scaffold) or `firma policy list`'s "Mappings" section — same word, different concept, collision confirmed and resolved by renaming (`DEC-004`, `PLAN-004`) | `DEC-004`            |

### Conditional: Alternatives

See `DEC-002` (bespoke two-phase decision procedure vs. Z3 string theory),
`DEC-003` (fail-closed vs. advisory for `INV-002`), `DEC-004` (new subcommand
vs. extending `firma policy validate`), `DEC-006` (which producers `INV-002`
covers, added after `PLAN-001`), and `DEC-007` (expose vs. relocate vs.
reimplement mapping-rules resolution, added after `PLAN-003`) in `Key
decisions and tradeoffs` — each already records shape, benefits/costs, and
rejection rationale in the human review path; not repeated here.

### Conditional: File-tree diff

```diff
 crates/firma-sidecar/src/normalizer/
+├── mapping_prover.rs        # NEW — INV-001/INV-002 check functions, MappingFinding types
~├── mapping.rs               # MODIFIED — from_config calls the new shadowing check; new MappingTableError variant
 crates/firma-sidecar/src/startup/
~├── pipeline.rs              # MODIFIED — `load_mapping_rules` becomes `pub` (DEC-007)
 crates/firma/src/args/
+├── mapping_rules.rs         # NEW — `firma mapping-rules validate` subcommand args
~├── mod.rs                   # MODIFIED — registers the new `mapping-rules` top-level command
 crates/firma/src/services/
+├── mapping_rules.rs         # NEW — `firma mapping-rules validate` implementation, reuses mapping_prover + Composio catalog loading
 crates/firma/tests/integration/
+├── mapping_rules_validate.rs # NEW — CLI black-box tests
 crates/firma-sidecar/tests/integration/
~├── mapping_rules.rs         # MODIFIED — new startup-abort-on-shadowed-rule test
```

### Conditional: Types and signatures

No unrepresentable-state claim is made for the new types below — they are
diagnostic report types, not domain types enforcing an invariant by
construction; the invariant is established by the check functions' logic
(and proved by tests), not by the type system. Recorded here to avoid
overclaiming, per the template's constructibility-attack requirement.

```rust
// crates/firma-sidecar/src/normalizer/mapping_prover.rs (new)

/// Identifies one rule within a merged `MappingRulesFile`, stable for the
/// duration of one `from_config` call (not persisted).
pub struct MappingRuleId(usize);

pub enum MappingFinding {
    /// INV-001 violation — fail-closed, blocks `from_config`.
    ShadowedRule {
        rule: MappingRuleId,
        shadowed_by: Vec<MappingRuleId>,
    },
    /// INV-002 violation — advisory, CLI-only.
    OrphanedActionClass { class: &'static str },
}

/// Returns every INV-001 violation in `rules`, using the same specificity
/// ordering `MappingTable` itself computes. Called from
/// `MappingTable::from_config`; a non-empty result becomes
/// `MappingTableError::ShadowedRule`.
pub(crate) fn find_shadowed_rules(rules: &MappingRulesFile) -> Vec<MappingFinding>;

/// One shipped Composio catalog (`config/composio/*.mapping.json`),
/// mapping a provider tool slug to the `action_class` it produces —
/// a second producer INV-002 must consult, alongside `rules`.
pub struct ComposioMappingCatalog {/* .. */}

/// Returns every INV-002 violation — classes in `registry` that neither
/// `rules` nor any `catalogs` entry produces and that aren't in the DEC-005
/// exemption list. Classes exclusively used by firma-run's local
/// execution-governance configuration (DEC-006) are excluded from
/// `registry` iteration entirely before this check runs, not flagged by it.
/// Called only from the `firma mapping-rules validate` CLI path.
pub fn find_orphaned_action_classes(
    rules: &MappingRulesFile,
    catalogs: &[ComposioMappingCatalog],
    registry: &ActionClassRegistry,
) -> Vec<MappingFinding>;
```

### Conditional: Semantic call traces

| Field                      | Content                                                                                                                                                                                                                    |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Trace ID                   | `TRACE-001`                                                                                                                                                                                                                |
| State                      | Landed (Slice 1, `1a6b14e9`)                                                                                                                                                                                               |
| Entry and stimulus         | `firma-sidecar` process startup with a `firma.toml` whose merged mapping-rule files contain a shadowed rule                                                                                                                |
| Path                       | `build_pipeline_runtime → load_mapping_rules → MappingTable::from_config → find_shadowed_rules → MappingTableError::ShadowedRule → anyhow::Error → process exit before serving traffic`                                    |
| Input/output types         | `MappingRulesFile → Result<MappingTable, MappingTableError>`                                                                                                                                                               |
| Validation/trust crossings | None new — same load-time trust boundary as the existing `DuplicateRule`/unknown-action-class checks (operator-authored config, already-trusted input)                                                                     |
| Invariant established      | `INV-001`                                                                                                                                                                                                                  |
| Invariant assumed          | Downstream: none — this is a load-time gate, nothing downstream assumes reachability beyond "the process either started or didn't"                                                                                         |
| Success outcome            | No shadowed rules → `Ok(MappingTable)`, startup proceeds unchanged from today                                                                                                                                              |
| Failure path               | Fail-closed abort, identical shape to `DuplicateRule`                                                                                                                                                                      |
| Evidence                   | New unit tests (constructed shadowed-rule fixture, property-based cross-check) + new integration test on `MappingTable::from_config` (not `build_pipeline_runtime` — corrected post-review, see the note in Slice 1 above) |
| Proof boundary             | Unit (algorithm) + integration (`MappingTable::from_config`)                                                                                                                                                               |
| Unknowns                   | Whether an operator override is wanted (`Scope`'s open decision)                                                                                                                                                           |

| Field                      | Content                                                                                                                                                                                                                                                                                                                  |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Trace ID                   | `TRACE-002`                                                                                                                                                                                                                                                                                                              |
| State                      | Proposed                                                                                                                                                                                                                                                                                                                 |
| Entry and stimulus         | Operator runs `firma mapping-rules validate --config <firma.toml>`                                                                                                                                                                                                                                                       |
| Path                       | `firma mapping-rules validate → firma_sidecar::load_mapping_rules (now pub, DEC-007) → MappingTable::from_config (INV-001, as errors) → find_orphaned_action_classes(rules, composio_catalogs, registry) (INV-002, as warnings) → render report → exit 0 if no errors, 1 if any error (warnings never affect exit code)` |
| Input/output types         | `PathBuf (config) → Vec<MappingFinding> → process exit code`                                                                                                                                                                                                                                                             |
| Validation/trust crossings | None — offline tool, operator-invoked, same trust level as `firma policy validate`                                                                                                                                                                                                                                       |
| Invariant established      | `INV-002` (advisory)                                                                                                                                                                                                                                                                                                     |
| Invariant assumed          | None downstream                                                                                                                                                                                                                                                                                                          |
| Success outcome            | Report printed; exit 0 if no `INV-001` violations (warnings may still be present)                                                                                                                                                                                                                                        |
| Failure path               | Exit 1 if any `INV-001` violation; warnings are always non-fatal                                                                                                                                                                                                                                                         |
| Evidence                   | New CLI black-box integration test                                                                                                                                                                                                                                                                                       |
| Proof boundary             | CLI integration test                                                                                                                                                                                                                                                                                                     |
| Unknowns                   | None material                                                                                                                                                                                                                                                                                                            |

### Conditional: Trust analysis

- Actors: the operator authoring `firma.toml`/mapping-rule TOML files (trusted
  config author); the Sidecar process itself (trust boundary already
  established, unchanged by this plan).
- Supported workloads/deployment modes: unchanged — this plan adds
  compile-time-checked-at-load-time structure to an existing, already-trusted
  config surface.
- Attacker capability: not attacker-facing — mapping-rule files are operator
  config, not external input; this is a correctness/hygiene guardrail against
  operator misconfiguration, not a security boundary against a hostile actor.
  Consistent with `reviewing-changes`' distinction between accident-prevention
  guardrails and security boundaries: `INV-001`/`INV-002` are the former.
- Protected assets: correct request classification (indirectly protects
  whatever a misclassified request's Cedar policy would have gotten wrong),
  not a boundary against bypassing enforcement (impossible at this layer per
  `DEC-001`).
- Trust transitions: none new.
- Reachable abuse paths: none identified — this is a misconfiguration
  guardrail, not a boundary an adversary crosses.

### Conditional: Proof obligations

| Field                  | Content                                                                                                                                                                                                                                                                                                                                                                                |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-001`                                                                                                                                                                                                                                                                                                                                                                              |
| Kind                   | Runtime (load-time)                                                                                                                                                                                                                                                                                                                                                                    |
| Owner/proof boundary   | `MappingTable::from_config`                                                                                                                                                                                                                                                                                                                                                            |
| Suite/boundary         | Unit + integration                                                                                                                                                                                                                                                                                                                                                                     |
| Stimulus               | A merged rule set with a rule fully subsumed by higher-priority rules                                                                                                                                                                                                                                                                                                                  |
| Observable effects     | `Err(MappingTableError::ShadowedRule)`, startup aborts                                                                                                                                                                                                                                                                                                                                 |
| Controls/substitutions | Constructed in-memory `MappingRulesFile` fixtures; property-based random generation                                                                                                                                                                                                                                                                                                    |
| Failure cases          | The shadowing algorithm itself failing to detect a real case (false negative) or over-flagging a valid rule (false positive)                                                                                                                                                                                                                                                           |
| Evidence               | New tests, Slice 1                                                                                                                                                                                                                                                                                                                                                                     |
| Status                 | Planned                                                                                                                                                                                                                                                                                                                                                                                |
| Slice                  | 1                                                                                                                                                                                                                                                                                                                                                                                      |
| Limits                 | Proves method-reachability only within exact `(host_pattern, path_pattern)`-equality groups (`DEC-002`) — proves nothing about shadowing across two rules with different patterns, regardless of glob-subset relationship; grounds the method universe in `HttpMethod`'s 8 closed variants, not the full open `http::Method` space; does not prove anything about Cedar-level outcomes |

| Field                  | Content                                                                                                                                                                                                                                  |
| ---------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Invariant              | `INV-002`                                                                                                                                                                                                                                |
| Kind                   | Operational                                                                                                                                                                                                                              |
| Owner/proof boundary   | `find_orphaned_action_classes`, consumed by `firma mapping-rules validate`                                                                                                                                                               |
| Suite/boundary         | Unit + CLI integration                                                                                                                                                                                                                   |
| Stimulus               | A registry class absent from the merged rule set, every Composio catalog, and the exemption list                                                                                                                                         |
| Observable effects     | A warning line in the CLI report; exit code unaffected                                                                                                                                                                                   |
| Controls/substitutions | Real shipped `config/mappings/*.toml` and `config/composio/*.mapping.json` files used directly in unit tests                                                                                                                             |
| Failure cases          | An exemption-list omission causing a permanent false-positive warning for a legitimately dynamic-only class; a Composio catalog update this check isn't re-run against                                                                   |
| Evidence               | New tests, Slice 2                                                                                                                                                                                                                       |
| Status                 | Planned                                                                                                                                                                                                                                  |
| Slice                  | 2                                                                                                                                                                                                                                        |
| Limits                 | Advisory only — does not block startup or deployment; does not verify `enrich_*` functions themselves; does not cover `firma-run`'s local execution-governance classes (`DEC-006`, explicitly excluded, not silently treated as covered) |
