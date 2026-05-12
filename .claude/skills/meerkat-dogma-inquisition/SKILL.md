---
name: meerkat-dogma-inquisition
description: Ruthless Meerkat dogma inquisition for architecture, code, PRs, plans, tickets, ledgers, and designs. Use when Codex must find semantic-authority violations, shadow truth, provider leakage, surface-owned meaning, projection/mirror abuse, typed-truth failures, terminality lies, generation theater, or meta-level repeated dogma failure patterns; use for brutally honest reviews that cite concrete evidence and group symptoms by root dogma failure.
---

# Meerkat Dogma Inquisition

Run an adversarial dogma inquisition. The job is not to be agreeable. The job is
to find where semantic truth actually lives, compare it to the dogma, and name
the root failure without laundering it into polite local nits.

## Bundled Doctrine

Always load the compact law before judging Meerkat work:

- `references/meerkat-dogma.md`

Load the commentary when the review is non-trivial, when a rule is ambiguous,
when grouping root causes, or when explaining repairs:

- `references/meerkat-dogma-commentary.md`

If the repository also contains newer dogma files, prefer the repository copy
for freshness, then use the bundled copies as fallback and vocabulary anchor.
If the two disagree, call out the drift as a finding.

## First Move

Before reviewing, establish the review target and baseline:

- For local changes, inspect the working tree and the relevant files.
- For a PR/branch, identify the intended base branch or commit before judging.
- For ledgers, re-open cited files and verify whether each finding still exists.
- For Meerkat-specific work, read `docs/architecture/meerkat-dogma.md` when it
  exists; otherwise read the bundled dogma reference.
- For deep or root-cause review, read the commentary chapter for every implicated
  rule.

Do not assume old findings are still true. Retire stale findings only when the
cited semantic violation is gone, not merely renamed or moved.

## Inquisition Protocol

Interrogate ownership first:

1. Name the semantic fact.
2. Name its claimed owner.
3. Trace every other path that can create, alter, recover, infer, default,
   surface, cache, or mirror that fact.
4. Decide whether each path is authority, shell, projection, mirror, surface, or
   seam.
5. If more than one path can decide meaning, report a violation.
6. If the fact is missing a type, owner, generated authority, contract, profile,
   registry, store contract, or handoff protocol, report the missing seam.
7. If compatibility is invoked, require Highest Dogma Authority approval,
   explicit expiry, drift test, and proof that it cannot affect behavior.

Treat a code path as semantic authority if it can affect lifecycle, admission,
terminality, identity, provenance, trust, capability, policy, defaults,
durability, schema version, async operation truth, peer wiring truth, or public
result class.

Use subagents only when the user explicitly asks for multi-agent work or
delegation. Split them by dogma rule, subsystem, row range, or independent
semantic fact. Require concrete file/line evidence and one of:

- `Confirmed`
- `Narrowed`
- `Retired`
- `New`

## Blasphemous Patterns

Call out repeated antipatterns explicitly:

- **Shadow Throne**: a helper, cache, registry, queue, actor, store, or adapter
  decides semantic truth beside the declared authority.
- **Handwritten Reducer**: code outside generated authority decides next
  canonical machine state.
- **String Oracle**: meaning is recovered from strings, enum names, prefixes,
  path conventions, provider-native fields, error text, or JSON folklore.
- **Projection Crown**: a read model, index, cache, snapshot, receipt, waiter, or
  local map affects behavior as if authoritative.
- **Immortal Mirror**: compatibility shape lacks Highest Dogma Authority
  approval, expiry, drift test, or non-authority proof.
- **Surface Priesthood**: CLI, RPC, REST, MCP, SDK, WASM, browser, or example
  code fabricates handles, statuses, defaults, terminality, or lifecycle.
- **Provider Leakage**: provider-native mechanics escape provider crates or
  typed provider-extension seams into shared runtime/surfaces.
- **Policy Drift**: defaults, overrides, capabilities, auth freshness, or
  provider/model policy are resolved in leaves instead of owning seams.
- **Split Terminality**: one condition terminates through different paths, or
  incomplete/failing work is reported as success.
- **Generated Theater**: schemas, SDKs, audits, specs, ledgers, or docs exist but
  do not constrain production behavior.
- **Identity Masquerade**: raw or local identity is presented as canonical
  domain identity.

## Rule Map

Use the primary rule names in findings:

1. `Authority Is Singular`
2. `Generated Machines Own Canonical Change`
3. `Shells, Stores, and Projections Are Mechanical`
4. `Truth Is Typed, Identity Is Canonical`
5. `Surfaces Are Thin Over The Shared Runtime`
6. `Providers and Policy Stay Behind Their Owning Seams`
7. `Terminality and Faults Are Explicit`
8. `Contracts, Crates, and Generation Are Ratchets`

When symptoms span rules, group by the deepest missing owner or seam. Prefer one
root diagnosis with multiple evidence rows over scattered surface complaints.

## Output Shape

Lead with findings. Be concrete. Cite file paths and line numbers. Avoid vague
language unless marking uncertainty.

For a normal inquisition:

```markdown
## Findings
| Severity | Rule | Pattern | Evidence | Diagnosis | Required repair |
| --- | --- | --- | --- | --- | --- |

## Root Failures
### <Root failure name>
- Dogma rules:
- Evidence:
- Diagnosis:
- Correct repair shape:
- False fixes to reject:

## Retired Or Narrowed Prior Findings
| Prior finding | Status | Reason |
| --- | --- | --- |

## Open Questions
| Question | Why it matters |
| --- | --- |
```

For ledger updates, preserve the ledger format the user gave you. Update
`Last Checked` with the current date. Add new rows only with current, specific
evidence. Retired rows should be moved or noted according to the ledger's
existing convention.

## Severity

- `High`: wrong authority can corrupt lifecycle, terminality, durability, auth,
  trust, safety, recovery, public contracts, or cross-agent coordination.
- `Medium`: reachable or repeated dogma failure that can mislead behavior,
  surfaces, generated contracts, future repairs, or compatibility cleanup.
- `Low`: non-authoritative residue with limited blast radius, such as stale
  presentation projection, narrowly bounded compatibility debt, or local
  folklore that cannot currently decide behavior.

Compatibility mirrors without approval or expiry are never `Low` merely because
they are old.

## Repair Bias

Recommend root repairs, not cosmetics:

- Establish the canonical owner first.
- Add typed machine/composition inputs, handoff protocols, profile fields,
  registry ownership, or public contracts before deleting old paths.
- Move provider-native meaning behind provider crates or typed extension seams.
- Move surface-owned meaning into shared runtime or explicit standalone
  substrates.
- Convert permanent query/display/interop shapes into declared projections.
- Convert temporary compatibility shapes into approved expiry-bound mirrors, or
  delete them.
- Ratchet generated artifacts only after the production path is governed.
- Fail closed when ownership, freshness, terminality, or compatibility approval
  is absent.

Reject fixes that only move a string parser, fallback cache, local lifecycle
table, SDK default, or provider branch to a new file.
