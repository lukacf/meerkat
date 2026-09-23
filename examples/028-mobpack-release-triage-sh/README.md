# 028 — Mobpack Release Triage (Shell)

Build, sign, inspect, and validate a **portable release-triage mobpack**.
This is an offline packaging example, not an incident-execution demo: it does
not spawn members, send a prompt to a model, or produce a triage result.

## What This Example Teaches

This example is intentionally opinionated: instead of packing a single toy
agent, it packages a small incident-room definition with distinct roles and skill
files for a host to instantiate later.

It demonstrates why mobpacks matter in real workflows:
- you can hand off one signed artifact between teams and environments
- the artifact contains the definition, skills, and typed deploy defaults
- inspection and validation need no provider credentials or model calls
- skill files, defaults, and runtime contract travel together

## Team Design

| Role | Purpose |
|------|---------|
| `lead` | Owns severity, decisions, and stakeholder updates |
| `signal-analyst` | Correlates deploy timing, metrics, and blast radius |
| `customer-ops` | Summarizes customer impact and communication risk |
| `rollback-chief` | Prepares mitigation, rollback, and verification plan |

The result is small enough to understand quickly, but rich enough to teach a
real release-triage coordination pattern.

## What The Script Builds

`examples.sh` generates a temporary mob source tree under `.work/release-triage/`
(relative to this example directory) with:
- `manifest.toml` - artifact identity, runtime requirements, and model aliases
- `definition.json` - a 4-role mob with orchestrator and specialists
- `skills/*.md` - role playbooks packed into the artifact
- `config/defaults.toml` - typed deploy defaults for per-turn output, provider model, and total budget

It also writes sibling outputs outside that source tree:
- `.work/release.key` - demo signing key for local verification
- `.work/release-triage.mobpack` - the signed artifact

## Concepts

- `rkat mob pack` for artifact creation from generated mob source
- `--sign` / `--signer-id` to attach provenance to the packed artifact
- `rkat mob inspect` to see what was embedded
- `rkat mob validate` to check the artifact contract before use
- skill files referenced by `path` and stored separately inside the archive
- fail-closed `MobpackDeployPolicy` parsing for `max_tokens`, `models`, and `budget`

Unknown sections or fields in `config/defaults.toml` are rejected rather than
merged into arbitrary runtime configuration. The typed policy also supports a
`[compaction]` group, but this example omits it because CLI deploy cannot
currently satisfy a mobpack-declared `session_compaction` capability.

## Prerequisites

```bash
./scripts/repo-cargo build -p rkat --bin rkat

# Optional override if you want to use a specific binary:
export RKAT=/path/to/rkat
```

No provider key is needed. All CLI roots are redirected into the example's
`.work/`; no global config is modified.

## Run

```bash
./examples/028-mobpack-release-triage-sh/examples.sh
```

## What The Script Does

1. Generate a realistic release-triage mob source tree in `.work/`
2. Create a throwaway demo signing key
3. Pack and sign `release-triage.mobpack`
4. Inspect the artifact contents
5. Validate the artifact contract
6. Report the artifact path and explicitly stop before agent execution

## From Packaging To Execution

Current CLI `rkat mob deploy` bootstraps an empty mob for this definition.
Its `deployed` message does **not** mean members were spawned or the supplied
incident prompt was processed. This example therefore does not invoke it.

A separate host must instantiate the roles, deliver a scenario and await
committed results, or author a callable flow and run it with `rkat mob run`.
This pack does not declare a callable flow. Packaging validation alone is not
proof of severity classification, rollback advice, or stakeholder updates.

## Offline Regression Test

```bash
python3 examples/028-mobpack-release-triage-sh/test_packaging.py
```

This uses the current CLI to pack, inspect and validate twice in a scratch copy,
without credentials, and checks that no sessions were created.

## Notes

- The fixed signing key in `.work/release.key` is publicly known, for local
  demonstration only, and unsuitable for production.
- The script uses `--trust-policy permissive` so you can run the example
  without pre-configuring a trust store.
- Signing a pack does not enroll its signer in a trust store.
- For a stricter production-like flow, use a privately managed signing key
  outside the example directory and choose its signer ID. Enroll that ID's
  matching Ed25519 public key in the `[signers]` mapping of the effective
  user/project `.rkat/trusted-signers.toml` trust store before validating or
  deploying with `--trust-policy strict`.
