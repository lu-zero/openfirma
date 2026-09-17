---
title: Validate mapping rules offline with firma mapping-rules
description: Check a mapping-rules configuration for unreachable rules and registry classes no rule ever produces, before deploying it.
---

`firma mapping-rules validate` checks the mapping-rules layer — the
`(method, host, path)` → `action_class` table the [normalizer](/concepts/sandbox/)
uses to classify every outbound request — offline, before you deploy a
configuration. It reports two things:

1. **Unreachable rules (a hard error).** A rule that shares an exact
   `host`/`path` with other rules whose combined method coverage already
   claims every method it could itself match can never be the first match for
   any real request. This is the same fail-closed contract a duplicate
   `(method, host, path)` tuple already gets today — the Sidecar itself
   refuses to start on either.
2. **Orphaned registry classes (a warning).** An
   [action class](../../concepts/action-classes/) that no mapping rule and no
   built-in Composio catalog entry ever produces. Warnings never fail the
   command — they're a hygiene signal, not a live gap: nothing is
   under-protected by an unused class, and a class governed solely through
   `firma-run`'s local execution config (a separate, non-HTTP subsystem this
   check has no visibility into) will legitimately show up here too.

Run it against a scaffolded configuration:

```console
$ firma mapping-rules validate --config .firma/firma.toml
[OK]   no unreachable mapping rules found
[WARN] registry class 'payment.transfer' is not producible by any mapping rule or Composio catalog entry — may be governed solely by firma-run's local execution config, which this check cannot see
```

Exit code is `0` unless an unreachable rule is found — a real misconfiguration
an operator should fix before it reaches Sidecar startup, where the identical
check would otherwise refuse to start:

```console
$ firma mapping-rules validate --config .firma/firma.toml
[ERR]  rule 4: unreachable — every method it could match is already claimed by a higher-priority rule sharing host="api.example.com" path="/widgets"
```

`--config` resolves the same way every other config-consuming subcommand
does: an explicit flag, then `$FIRMA_CONFIG`, then the nearest
`.firma/firma.toml`. Mapping rules are loaded and rebased against the config
directory exactly as Sidecar startup does it — a clean report here means
Sidecar startup will load the same configuration without error too.

This check is scoped to the Sidecar's HTTP request-classification path only.
It does not evaluate Cedar policy, and it cannot tell you whether a request
will be `ALLOW`ed or `DENY`ed — a matched rule only produces a classified
intent, which then still passes through capability validation and full Cedar
evaluation before any decision is made. For that, see
[testing policies offline](../test-policies-offline/). To add coverage for a
new provider or endpoint, see
[extending the action-class mapping](../extend-mapping/).
