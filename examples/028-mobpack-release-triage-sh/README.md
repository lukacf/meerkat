# 028 — Mobpack Release Triage (Shell)

Build, sign, inspect, validate, and deploy a **portable release-triage mobpack**
that feels like something an ops team could actually use during a bad rollout.

## What This Example Teaches

This example is intentionally opinionated: instead of packing a single toy
agent, it creates a small incident room with distinct roles and skill files,
then deploys the exact signed artifact against a concrete production scenario.

It demonstrates why mobpacks matter in real workflows:
- you can hand off one signed artifact between teams and environments
- the behavior is deterministic because deploy runs the exact packed contents
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
with:
- `manifest.toml` — artifact identity, runtime requirements, surface support
- `definition.json` — a 4-role mob with orchestrator + specialists
- `skills/*.md` — role playbooks packed into the artifact
- `config/defaults.toml` — default agent/budget settings
- `release.key` — demo signing key for local verification

## Concepts

- `rkat mob pack` for artifact creation from generated mob source
- `--sign` to attach provenance to the packed artifact
- `rkat mob inspect` to see what was embedded
- `rkat mob validate` to check the artifact contract before use
- `rkat mob deploy` to run the same signed artifact with a real prompt
- skill files referenced by `path` and inlined at pack time

## Prerequisites

```bash
export ANTHROPIC_API_KEY=sk-...
cargo install rkat

# Optional when working from this repo instead of a global install:
export RKAT=../../target/debug/rkat
```

## Run

```bash
./examples.sh
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

- The signing key in `.work/release.key` is for local demonstration only.
- The script uses `--trust-policy permissive` so you can run the example
  without pre-configuring a trust store.
- For a stricter production-like flow, switch the deploy step to
  `--trust-policy strict` and manage keys outside the example directory.
