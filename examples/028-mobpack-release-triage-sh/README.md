# 028 — Mobpack Release Triage (Shell)

Build, sign, inspect, validate, and deploy a **portable release-triage mobpack**
that feels like something an ops team could actually use during a bad rollout.

## What This Example Teaches

This example is intentionally opinionated: instead of packing a single toy
agent, it creates a small incident room with distinct roles and skill files,
then deploys the exact signed artifact against a concrete production scenario.

It demonstrates why mobpacks matter in real workflows:
- you can hand off one signed artifact between teams and environments
- deploy uses the exact packaged definition, skills, and defaults
- inspection and validation happen before the incident prompt hits the model
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
- `rkat mob deploy` to run the same signed artifact with a real prompt
- skill files referenced by `path` and inlined at pack time
- fail-closed `MobpackDeployPolicy` parsing for `max_tokens`, `models`, and `budget`

Unknown sections or fields in `config/defaults.toml` are rejected rather than
merged into arbitrary runtime configuration. The typed policy also supports a
`[compaction]` group, but this example omits it because CLI deploy cannot
currently satisfy a mobpack-declared `session_compaction` capability.

## Prerequisites

```bash
export ANTHROPIC_API_KEY=sk-...
./scripts/repo-cargo build -p rkat --bin rkat

# Optional override if you want to use a specific binary:
export RKAT=/path/to/rkat
```

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
6. Deploy the exact signed artifact against a realistic release-regression prompt

## Why The Scenario Feels Real

The deploy prompt simulates a rollout where:
- checkout error rate spikes after release
- EU latency regresses sharply
- enterprise support tickets arrive immediately

That forces the mob to do work an operator actually cares about:
- severity classification
- blast-radius estimation
- halt vs rollback recommendation
- stakeholder-update drafting

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
