//! Mapping table for intent normalization.
//!
//! Loaded from TOML configuration at startup. Each rule maps an HTTP
//! method + host + path pattern to a canonical action class from the
//! Canonical Action Class Registry v0.1. Rules are sorted by descending
//! specificity so that the first match wins.
//!
//! The `intent.action_class` field produced by the normalizer MUST be one of
//! the configured registry identifiers. Unknown protected actions that cannot
//! be deterministically mapped to a registry entry fail closed with
//! `DENY: UNCLASSIFIED_INTENT` (FEP \[I-N1\]).

use firma_http::{Authority, Method};

use crate::config::{MappingRuleConfig, MappingRulesFile};
use crate::enforcement::registry::ActionClassRegistry;

/// Errors that can occur while building a [`MappingTable`] from configuration.
#[derive(Debug, thiserror::Error)]
pub enum MappingTableError {
    /// The mapping rules file itself failed validation (empty file,
    /// malformed rule, invalid HTTP method, etc).
    #[error("invalid mapping rules file: {0}")]
    InvalidRulesFile(String),

    /// Two rules share the same `(method, host, path)` tuple, which
    /// makes classification ambiguous.
    #[error(
        "rule {index}: duplicate mapping tuple method={:?} host={:?} path={:?}",
        rule.method.as_ref().map(|method| method.as_str()).unwrap_or_default(),
        rule.host,
        rule.path.as_deref().unwrap_or_default()
    )]
    DuplicateRule {
        index: usize,
        rule: MappingRuleConfig,
    },

    /// A rule references an action class outside the built-in FEP
    /// registry.
    #[error(
        "rule {index}: action class '{}' is not in the built-in FEP action class registry.\n\
         Note: schema_path in [authority] only affects Cedar policy validation in the \
         Authority — it does not extend this registry, which is fixed when the Sidecar \
         loads.\nTo use this action class in a mapping rule, add it to the registry first.\n\
         See: https://firma-ai.github.io/openfirma/guides/extend-mapping/",
        rule.action_class
    )]
    NonExistingActionClass {
        index: usize,
        rule: MappingRuleConfig,
    },

    /// A rule shares an exact `(host, path)` mapping tuple with other rules
    /// whose combined method coverage already claims every method this
    /// rule's own pattern could ever match — it can never be the first
    /// match for any real request. See
    /// `docs/architecture/mapping-rules-prover-plan.md`, `DEC-002`/`INV-001`.
    #[error(
        "rule {index}: unreachable — every method it could match is already claimed by a \
         higher-priority rule sharing host={:?} path={:?}",
        rule.host,
        rule.path.as_deref().unwrap_or_default()
    )]
    ShadowedRule {
        index: usize,
        rule: MappingRuleConfig,
    },
}

/// A validated mapping rule ready for matching.
#[derive(Debug, Clone)]
pub struct MappingRule {
    pub method: Option<Method>,
    pub host_pattern: String,
    pub path_pattern: Option<String>,
    pub action_class: String,
    specificity: u32,
}

/// Collection of validated, specificity-ordered mapping rules.
///
/// Loaded from TOML configuration at startup. Immutable after initialization.
#[derive(Debug, Clone)]
pub struct MappingTable {
    rules: Vec<MappingRule>,
    default_protected: bool,
}

/// The result of matching a request against the mapping table.
#[derive(Debug)]
pub enum MatchResult<'a> {
    /// Matched a rule — use this action class.
    Matched(&'a MappingRule),
    /// No rule matched and the host is protected — deny as unclassified.
    UnclassifiedProtected,
    /// No rule matched and the host is not protected — passthrough.
    NotProtected,
}

impl MappingRule {
    fn compute_specificity(method: Option<&Method>, host: &str, path: Option<&String>) -> u32 {
        let mut score = 0u32;

        // Exact host > wildcard host
        if !host.contains('*') {
            score += 100;
        }

        // Specific method > any method
        if method.is_some() {
            score += 10;
        }

        // Path presence and specificity
        if let Some(p) = path {
            score += 5;
            // Longer path = more specific
            #[expect(
                clippy::cast_possible_truncation,
                reason = "path segments will never exceed u32"
            )]
            let segments = p.split('/').filter(|s| !s.is_empty()).count() as u32;
            score += segments;
            // No wildcards in path = more specific
            if !p.contains('*') {
                score += 10;
            }
        }

        score
    }
}

/// The eight HTTP methods a matched request can actually reach an
/// observable decision through. `normalizer::IntentNormalizer::normalize`
/// denies any matched request whose method isn't one of these
/// (`HttpMethod::try_from`, checked *after* mapping-table matching,
/// regardless of which rule matched) — so a mapping rule reachable only via
/// a method outside this set can never produce an observable envelope
/// either way, and grounding [`find_shadowed_rule`]'s method universe here
/// (rather than `firma_http::Method`'s broader, unchecked domain, which
/// also includes `TRACE`) is the precise choice. See
/// `docs/architecture/mapping-rules-prover-plan.md`, `DEC-002`.
const REACHABLE_METHODS: [Method; 8] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::DELETE,
    Method::PATCH,
    Method::HEAD,
    Method::OPTIONS,
    Method::CONNECT,
];

/// Maps a rule's method requirement to the [`REACHABLE_METHODS`] indices it
/// covers: `None` (any method) covers all eight; `Some(m)` covers just `m`'s
/// index, or none if `m` isn't one of the eight.
fn method_indices(method: Option<&Method>) -> Vec<usize> {
    method.map_or_else(
        || (0..REACHABLE_METHODS.len()).collect(),
        |m| {
            REACHABLE_METHODS
                .iter()
                .position(|reachable| reachable == m)
                .into_iter()
                .collect()
        },
    )
}

/// Finds a rule that can never be the first match for any real request: one
/// sharing an exact `(host_pattern, path_pattern)` with other rules whose
/// combined method coverage, ranked above it by the existing specificity
/// order, already claims every method it could itself match.
///
/// `rules` must already be sorted by descending specificity (as
/// `MappingTable::from_config` sorts them) — this function does not
/// re-sort; it trusts and uses the given order directly, both to determine
/// priority within each exact-tuple group and to return a position stable
/// against the caller's own indexing of that same order.
///
/// Sound in both directions — never misses a real shadowed rule, never
/// flags a reachable one — but **not** guaranteed to return the
/// globally-earliest violation when more than one exact-tuple group is
/// independently shadowed: groups are scanned in order of each group's own
/// lowest-specificity member, not by each group's own violating position, so
/// a later group whose lowest member sorts first can be reported ahead of an
/// earlier one. Callers should treat the returned position as "a shadowed
/// rule exists, here is one," not "the first one in specificity order."
///
/// Deliberately narrower than "no rule is ever shadowed": this proves
/// nothing about two rules with *different* `(host_pattern, path_pattern)`
/// strings, regardless of whether one pattern's matching language is a
/// glob-subset of the other's. See
/// `docs/architecture/mapping-rules-prover-plan.md`, `DEC-002`/`INV-001`,
/// for why that broader property is explicitly out of scope rather than
/// incompletely covered.
fn find_shadowed_rule<'a>(rules: impl Iterator<Item = &'a MappingRule>) -> Option<usize> {
    let rules: Vec<&MappingRule> = rules.collect();

    let mut groups: std::collections::HashMap<(&str, Option<&str>), Vec<usize>> =
        std::collections::HashMap::new();
    for (position, rule) in rules.iter().enumerate() {
        groups
            .entry((rule.host_pattern.as_str(), rule.path_pattern.as_deref()))
            .or_default()
            .push(position);
    }

    // `HashMap` iteration order is unspecified; sort candidate groups by
    // their lowest member position so the *first* shadowed rule found
    // (in specificity order) is reported deterministically, not
    // arbitrarily by hash-bucket order.
    let mut group_positions: Vec<&Vec<usize>> = groups.values().collect();
    group_positions.sort_by_key(|positions| positions[0]);

    for positions in group_positions {
        if positions.len() < 2 {
            continue; // A group of one can't be shadowed by anything.
        }

        let mut covered = [false; REACHABLE_METHODS.len()];
        for &position in positions {
            let this_rule_methods = method_indices(rules[position].method.as_ref());
            if this_rule_methods.is_empty() {
                // No recognized method: this rule can never produce an
                // observable envelope regardless of shadowing (see
                // REACHABLE_METHODS) — it neither needs nor grants
                // coverage.
                continue;
            }
            if this_rule_methods.iter().all(|&m| covered[m]) {
                return Some(position);
            }
            for &m in &this_rule_methods {
                covered[m] = true;
            }
        }
    }

    None
}

impl MappingTable {
    /// Load and validate mapping rules from a parsed config.
    ///
    /// Host patterns are normalized like runtime request hosts (lowercased,
    /// trailing dot stripped): request hosts arrive in that form, so an
    /// unnormalized pattern such as `API.GitHub.COM.` could never match
    /// anything and would sit in the table as a dead rule.
    ///
    /// # Errors
    /// Returns an error if the rules are structurally invalid or
    /// ambiguous.
    pub fn from_config(
        file: &MappingRulesFile,
        registry: &ActionClassRegistry,
        default_protected: bool,
    ) -> Result<Self, MappingTableError> {
        file.validate()
            .map_err(MappingTableError::InvalidRulesFile)?;

        // Duplicate (method, host, path) tuple detection. Two rules
        // with the same triple across merged mapping files produces
        // ambiguous classification — fail-closed at startup. Hosts are
        // compared in normalized form so case or trailing-dot variants of
        // the same rule cannot slip past the check.
        let mut seen: std::collections::HashSet<(Option<Method>, String, String)> =
            std::collections::HashSet::new();
        for (i, rule_cfg) in file.rules.iter().enumerate() {
            let key = (
                rule_cfg.method.clone(),
                normalize_host_pattern(&rule_cfg.host),
                rule_cfg.path.clone().unwrap_or_default(),
            );
            if !seen.insert(key) {
                return Err(MappingTableError::DuplicateRule {
                    index: i,
                    rule: rule_cfg.clone(),
                });
            }
        }

        // Paired with each rule's original `file.rules` index so a shadowing
        // finding (below) can report against the operator-authored config,
        // the same way `DuplicateRule`/`NonExistingActionClass` do.
        let mut indexed_rules: Vec<(usize, MappingRule)> = Vec::with_capacity(file.rules.len());

        for (i, rule_cfg) in file.rules.iter().enumerate() {
            if !registry.contains(&rule_cfg.action_class) {
                return Err(MappingTableError::NonExistingActionClass {
                    index: i,
                    rule: rule_cfg.clone(),
                });
            }

            let host_pattern = normalize_host_pattern(&rule_cfg.host);
            let specificity = MappingRule::compute_specificity(
                rule_cfg.method.as_ref(),
                &host_pattern,
                rule_cfg.path.as_ref(),
            );

            indexed_rules.push((
                i,
                MappingRule {
                    method: rule_cfg.method.clone(),
                    host_pattern,
                    path_pattern: rule_cfg.path.clone(),
                    action_class: rule_cfg.action_class.clone(),
                    specificity,
                },
            ));
        }

        // Sort by descending specificity (most specific first)
        indexed_rules.sort_by_key(|(_, rule)| std::cmp::Reverse(rule.specificity));

        if let Some(shadowed_index) = find_shadowed_rule(indexed_rules.iter().map(|(_, rule)| rule))
        {
            let (original_index, _) = indexed_rules[shadowed_index];
            return Err(MappingTableError::ShadowedRule {
                index: original_index,
                rule: file.rules[original_index].clone(),
            });
        }

        let rules = indexed_rules.into_iter().map(|(_, rule)| rule).collect();

        Ok(Self {
            rules,
            default_protected,
        })
    }

    /// Find the first (most specific) matching rule for a request.
    #[must_use]
    pub fn find_match<'a>(
        &'a self,
        method: &Method,
        host: &Authority,
        path: &str,
    ) -> MatchResult<'a> {
        for rule in &self.rules {
            if Self::rule_matches(rule, method, host, path) {
                return MatchResult::Matched(rule);
            }
        }

        if self.default_protected {
            MatchResult::UnclassifiedProtected
        } else {
            MatchResult::NotProtected
        }
    }

    fn rule_matches(rule: &MappingRule, method: &Method, host: &Authority, path: &str) -> bool {
        // Check method (None = any method)
        if rule
            .method
            .as_ref()
            .is_some_and(|rule_method| rule_method != method)
        {
            return false;
        }

        // Check host pattern
        if !glob_match(&rule.host_pattern, host.as_str()) {
            return false;
        }

        // Check path pattern (None = any path)
        if let Some(ref pattern) = rule.path_pattern
            && !glob_match(pattern, path)
        {
            return false;
        }

        true
    }
}

/// Normalize a host or rule host pattern into its canonical matching form.
///
/// Splits the authority into a name and an optional port, per address
/// family (see [`split_authority`] and [`split_ipv6_authority`]), then
/// drops a default `:443`/`:80` port and re-appends a nonstandard all-digit
/// one. This is the single normalization shared by runtime request hosts,
/// rule host patterns, and config validation, so the three can never
/// disagree on what a spelling means.
pub fn normalize_host_pattern(host: &str) -> String {
    let trimmed = host.trim();
    let (name, port) = if trimmed.starts_with('[') {
        split_ipv6_authority(trimmed)
    } else {
        split_authority(trimmed)
    };
    match port.as_deref() {
        Some("443" | "80") | None => name,
        Some(port) => format!("{name}:{port}"),
    }
}

/// Normalize a request host into the authority used for outbound dispatch.
///
/// Splits the authority exactly like [`normalize_host_pattern`], but drops
/// the port only when it is the default for the request's *actual* scheme
/// (`443` for HTTPS, `80` for HTTP) rather than either default. An explicit
/// port that names the opposite scheme's default — `:443` on a plain HTTP
/// request, or `:80` on HTTPS — is not this request's default port and must
/// be preserved, since the connector dispatches to exactly the authority
/// stored in the normalized envelope's resource host.
pub fn normalize_dispatch_host(host: &str, is_https: bool) -> String {
    let trimmed = host.trim();
    let (name, port) = if trimmed.starts_with('[') {
        split_ipv6_authority(trimmed)
    } else {
        split_authority(trimmed)
    };
    let default_port = if is_https { "443" } else { "80" };
    match port.as_deref() {
        Some(port) if port != default_port => format!("{name}:{port}"),
        _ => name,
    }
}

/// Returns `true` for a nonempty all-digit port, the only form accepted as a
/// port when splitting an authority.
fn is_valid_port(port: &str) -> bool {
    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
}

/// Splits a hostname/IPv4 authority into a lowercased name and an optional
/// port.
///
/// Trailing dots are stripped both before the port split (`host:443.`) and
/// from the name part after it (`host.:443`); either position spells the
/// same authority (RFC 4343 case-insensitivity plus the FQDN trailing dot).
/// Wildcards pass through untouched except for this dot handling (`*.:443`
/// normalizes to `*`, which validation rejects as a silent catch-all
/// promotion).
fn split_authority(host: &str) -> (String, Option<String>) {
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    let (name, port) = match lower.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && is_valid_port(port) => (name, Some(port)),
        _ => (lower.as_str(), None),
    };
    (
        name.trim_end_matches('.').to_string(),
        port.map(str::to_string),
    )
}

/// Splits a bracketed IPv6 authority into a lowercased, bracketed name and
/// an optional port.
///
/// The port is split at the closing bracket rather than the last colon,
/// since an IPv6 address itself contains colons.
fn split_ipv6_authority(host: &str) -> (String, Option<String>) {
    let lower = host.to_ascii_lowercase();
    match lower.split_once("]:") {
        Some((name, port)) if is_valid_port(port) => (format!("{name}]"), Some(port.to_string())),
        _ => (lower, None),
    }
}

/// Simple glob matching where `*` matches any sequence of characters.
///
/// Shared with `firma-secret-provider`'s `HttpIntegrationSpec` host/path
/// matching (a distinct crate this one already depends on) rather than
/// reimplemented here, so the two never silently diverge.
pub use firma_secret_provider::glob_match;

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use pretty_assertions::assert_matches;

    use super::*;
    use crate::config::MappingRuleConfig;

    #[test]
    fn mapping_rules_rejects_nonexisting_action_class() {
        let bad_file = MappingRulesFile {
            rules: vec![MappingRuleConfig {
                method: None,
                host: "*".to_string(),
                path: None,
                action_class: "nonexistent.action".to_string(),
            }],
        };
        let err =
            MappingTable::from_config(&bad_file, &ActionClassRegistry::v0_1(), true).unwrap_err();
        assert_matches!(
            err,
            MappingTableError::NonExistingActionClass { index: 0, ref rule }
                if rule.action_class == "nonexistent.action"
        );
        assert_snapshot!(err.to_string(), @"
        rule 0: action class 'nonexistent.action' is not in the built-in FEP action class registry.
        Note: schema_path in [authority] only affects Cedar policy validation in the Authority — it does not extend this registry, which is fixed when the Sidecar loads.
        To use this action class in a mapping rule, add it to the registry first.
        See: https://firma-ai.github.io/openfirma/guides/extend-mapping/
        ");
    }

    #[test]
    fn duplicated_mapping_rule_is_rejected() {
        let file = MappingRulesFile {
            rules: vec![
                MappingRuleConfig {
                    method: Some(Method::GET),
                    host: "api.github.com".to_string(),
                    path: Some("/repos/*/*".to_string()),
                    action_class: "code.read".to_string(),
                },
                MappingRuleConfig {
                    method: Some(Method::GET),
                    host: "api.github.com".to_string(),
                    path: Some("/repos/*/*".to_string()),
                    action_class: "code.review.read".to_string(),
                },
            ],
        };
        let err = MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true).unwrap_err();
        assert_matches!(
            err,
            MappingTableError::DuplicateRule { index: 1, ref rule }
                if rule.action_class == "code.review.read"
        );
        assert_snapshot!(err.to_string(), @r#"rule 1: duplicate mapping tuple method="GET" host="api.github.com" path="/repos/*/*""#);
    }

    #[test]
    fn specific_rule_matches_first() {
        let file = MappingRulesFile {
            rules: vec![
                MappingRuleConfig {
                    method: Some(Method::POST),
                    host: "*".to_string(),
                    path: None,
                    action_class: "communication.internal.send".to_string(),
                },
                MappingRuleConfig {
                    method: Some(Method::POST),
                    host: "api.openai.com".to_string(),
                    path: Some("/v1/chat/completions".to_string()),
                    action_class: "communication.external.send".to_string(),
                },
            ],
        };
        let table = MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true).unwrap();

        match table.find_match(
            &Method::POST,
            &Authority::from_static("api.openai.com"),
            "/v1/chat/completions",
        ) {
            MatchResult::Matched(rule) => {
                assert_eq!(rule.action_class, "communication.external.send");
            }
            other => panic!("expected Matched, got {other:?}"),
        }
    }

    #[test]
    fn wildcard_rule_matches_unknown_host() {
        let file = MappingRulesFile {
            rules: vec![
                MappingRuleConfig {
                    method: Some(Method::GET),
                    host: "api.other.com".to_string(),
                    path: None,
                    action_class: "communication.internal.send".to_string(),
                },
                MappingRuleConfig {
                    method: Some(Method::GET),
                    host: "*".to_string(),
                    path: None,
                    action_class: "filesystem.read".to_string(),
                },
            ],
        };
        let table = MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true).unwrap();

        match table.find_match(
            &Method::GET,
            &Authority::from_static("api.weather.com"),
            "/forecast",
        ) {
            MatchResult::Matched(rule) => assert_eq!(rule.action_class, "filesystem.read"),
            other => panic!("expected Matched, got {other:?}"),
        }
    }

    #[test]
    fn no_match_protected_returns_unclassified() {
        // Table with only specific rules, no wildcard
        let file = MappingRulesFile {
            rules: vec![MappingRuleConfig {
                method: Some(Method::POST),
                host: "api.openai.com".to_string(),
                path: Some("/v1/chat/completions".to_string()),
                action_class: "communication.external.send".to_string(),
            }],
        };
        let table = MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true).unwrap();

        assert!(matches!(
            table.find_match(&Method::GET, &Authority::from_static("unknown.host"), "/"),
            MatchResult::UnclassifiedProtected
        ));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("api.openai.com", "api.openai.com"));
        assert!(!glob_match("api.openai.com", "api.anthropic.com"));
    }

    #[test]
    fn glob_match_wildcard_prefix() {
        assert!(glob_match("*.openai.com", "api.openai.com"));
        assert!(!glob_match("*.openai.com", "api.anthropic.com"));
    }

    #[test]
    fn glob_match_star_matches_all() {
        assert!(glob_match("*", "anything.at.all"));
    }

    #[test]
    fn glob_match_path_wildcard() {
        assert!(glob_match("/v1/*/completions", "/v1/chat/completions"));
        assert!(!glob_match("/v1/*/completions", "/v2/chat/completions"));
    }

    fn rule(method: Option<Method>, action_class: &str) -> MappingRuleConfig {
        MappingRuleConfig {
            method,
            host: "api.example.com".to_string(),
            path: Some("/widgets".to_string()),
            action_class: action_class.to_string(),
        }
    }

    #[test]
    fn any_method_rule_shadowed_when_every_method_is_covered() {
        // Eight higher-priority rules, one per REACHABLE_METHODS entry,
        // sharing the exact (host, path) tuple with a lower-priority
        // any-method rule -- none of these individually duplicates any
        // other (different method each), so `DuplicateRule` does not
        // fire, but their union already claims every method the
        // any-method rule could ever match.
        let mut rules: Vec<MappingRuleConfig> = REACHABLE_METHODS
            .iter()
            .map(|m| rule(Some(m.clone()), "filesystem.read"))
            .collect();
        rules.push(rule(None, "communication.external.send"));
        let file = MappingRulesFile { rules };

        let err = MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true).unwrap_err();
        assert_matches!(
            err,
            MappingTableError::ShadowedRule { index: 8, ref rule }
                if rule.action_class == "communication.external.send"
        );
    }

    #[test]
    fn any_method_rule_not_shadowed_when_one_method_is_uncovered() {
        // Same shape, but CONNECT is left uncovered -- the any-method rule
        // remains reachable via a CONNECT request, so it must not be
        // rejected.
        let mut rules: Vec<MappingRuleConfig> = REACHABLE_METHODS
            .iter()
            .filter(|m| **m != Method::CONNECT)
            .map(|m| rule(Some(m.clone()), "filesystem.read"))
            .collect();
        rules.push(rule(None, "communication.external.send"));
        let file = MappingRulesFile { rules };

        MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true)
            .expect("any-method rule is reachable via the uncovered CONNECT method");
    }

    #[test]
    fn different_host_or_path_never_counts_as_shadowing() {
        // Same any-method rule as the shadowed case, but the eight
        // covering rules target a DIFFERENT path -- INV-001 is deliberately
        // scoped to exact (host, path) tuples only (DEC-002), so this must
        // succeed even though the any-method rule's host wildcard would,
        // under a general containment reading, arguably overlap.
        let mut rules: Vec<MappingRuleConfig> = REACHABLE_METHODS
            .iter()
            .map(|m| MappingRuleConfig {
                method: Some(m.clone()),
                host: "api.example.com".to_string(),
                path: Some("/gadgets".to_string()),
                action_class: "filesystem.read".to_string(),
            })
            .collect();
        rules.push(rule(None, "communication.external.send"));
        let file = MappingRulesFile { rules };

        MappingTable::from_config(&file, &ActionClassRegistry::v0_1(), true)
            .expect("different path means no shadowing is claimed, by design (DEC-002)");
    }

    /// Independent re-derivation of shadowing, deliberately not sharing
    /// [`find_shadowed_rule`]'s running-bitset implementation: for each
    /// rule in priority order, it is reachable iff at least one concrete
    /// method it requires was not already required by some earlier rule.
    fn oracle_first_shadowed(rules: &[MappingRule]) -> Option<usize> {
        for (i, candidate) in rules.iter().enumerate() {
            let candidate_methods = method_indices(candidate.method.as_ref());
            if candidate_methods.is_empty() {
                continue;
            }
            let fully_covered = candidate_methods.iter().all(|&m| {
                rules[..i]
                    .iter()
                    .any(|earlier| method_indices(earlier.method.as_ref()).contains(&m))
            });
            if fully_covered {
                return Some(i);
            }
        }
        None
    }

    fn arbitrary_method_requirement() -> impl proptest::strategy::Strategy<Value = Option<Method>> {
        use proptest::prelude::*;
        prop_oneof![
            Just(None),
            (0..REACHABLE_METHODS.len()).prop_map(|i| Some(REACHABLE_METHODS[i].clone())),
        ]
    }

    proptest::proptest! {
        #[test]
        fn shadowing_matches_independent_oracle(
            requirements in proptest::collection::vec(arbitrary_method_requirement(), 1..12)
        ) {
            let rules: Vec<MappingRule> = requirements
                .into_iter()
                .enumerate()
                .map(|(i, method)| MappingRule {
                    method,
                    host_pattern: "api.example.com".to_string(),
                    path_pattern: Some("/widgets".to_string()),
                    action_class: format!("rule-{i}"),
                    specificity: 0,
                })
                .collect();

            let actual = find_shadowed_rule(rules.iter());
            let expected = oracle_first_shadowed(&rules);
            proptest::prop_assert_eq!(actual, expected);
        }
    }
}
