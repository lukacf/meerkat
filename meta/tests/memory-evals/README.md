# Memory calibration harness

Calibration corpus and runner for the agent-memory judgment stages
(`docs/design/agent-memory-architecture.md` §11). Every judgment stage is a
versioned, evaluated artifact — this directory holds the artifacts (profiles,
prompt bundles, fixtures) and `scripts/memory-evals` runs them.

Distiller, Steward, and Hygienist have runnable profiles, fixtures, and mock
lanes; deterministic checks and mock-lane invariant verdicts gate CI today.
The Selector corpus is retained only as historical calibration evidence after
the unactivated stage was retired. It is validated by `--check`, but it is not
a runnable stage. Live modes run real model calls where provider auth resolves;
they run in no CI lane yet (credentials). The stage binaries signal unavailable
provider authentication with exit 3. If the initial live-auth probe returns 3,
`scripts/memory-evals` prints `LIVE … SKIPPED`, runs a `MOCK-fallback`
scorecard, and normally exits 0 if its deterministic checks pass. A zero
wrapper exit alone does not certify live judgment coverage. Deterministic
failures still fail the run, and authentication disappearing after a successful
initial probe is a configuration failure, not a successful skip.

## Layout

```
tests/memory-evals/
  profiles/     calibration profiles — {stage, version, model, params, prompt_bundle}
  prompts/      prompt bundles referenced by profiles
  fixtures/
    selector/   manifest + turn → expected selection
    invariants/ label-free invariant fixtures (poisoned write, forged envelope)
```

## Running

```bash
scripts/memory-evals --check                  # schema/consistency validation (CI gate; default)
scripts/memory-evals --stage distiller --mode mock  # deterministic mock scorecard
make memory-evals                             # build eval binaries, --check, then all three mock lanes
```

`--check` is deterministic and fast. `make ci` runs the `memory-evals` target,
which builds the eval binaries, validates the corpus, and runs all three mock lanes.
`--mode mock` uses no-op extraction for Distiller and scripted replies through
the production parsers and validators for Steward and Hygienist. Deterministic
failures gate every mode; judgment/extraction misses are informational in mock
mode.

The retired Selector profiles and fixtures remain readable inputs to
`--check`; no live or mock command executes them.

## Profile format

One TOML file per `{stage, version}` in `profiles/`:

```toml
stage        = "selector"           # judgment stage this profile calibrates
version      = "0"                  # bumped on any prompt/params change
model        = "PLACEHOLDER"        # per-stage default lives in the embedded profile; override here
prompt_bundle = "prompts/selector-v0.md"   # path relative to tests/memory-evals/

[params]
temperature   = 0.0
selection_bar = "certain-to-be-helpful"
shuffle_manifest = true
```

Records and staged batches carry the profile that produced them
(`CalibrationRef` in the record model, §7.1). Live mode exists for every runnable
stage; profile changes gate on the deterministic lanes in CI today, and on live
scorecard non-regression wherever provider auth resolves (no CI lane supplies
credentials yet — see the design doc's §11 as-built note).

## Selector fixtures (`fixtures/selector/*.json`)

One JSON file per case:

```json
{
  "name": "unique-kebab-case-name",
  "manifest": [
    {"id": "mem-001", "kind": "Gotcha", "title": "…", "description": "…",
     "age_days": 12, "rank": 3}
  ],
  "turn_text": "the incoming turn the selector judges against",
  "suppressed_ids": [],
  "expected": {
    "must_select": ["mem-001"],
    "must_not_select": ["mem-002"]
  },
  "notes": "one line on what this fixture protects against"
}
```

- `manifest` entries are `RecordMeta` (§7.3): `id`, `kind` (one of
  `Preference | Fact | Gotcha | Procedure | Relationship | OpenLoop |
  Reference`), `title` (≤200B), `description` (≤400B — *when this record
  matters*, not what it says), `age_days`, `rank` (steward working-set rank;
  `null` for recent/unranked records).
- `suppressed_ids` (optional) models the already-in-context suppression list
  the coordinator passes alongside the manifest (§8.3).
- `expected` lists only the ids with a definite verdict; ids in neither list
  are "don't care" (either verdict is acceptable). An empty `must_select` with
  populated `must_not_select` asserts that *not selecting* is correct.

To add a fixture: drop a file in `fixtures/selector/`, run
`scripts/memory-evals --check`. Names must be unique across fixtures; every
expected id must exist in the fixture's own manifest. Grow the corpus from
production traces — operator corrections and forgets are free negative labels
(§11).

## Invariant fixtures (`fixtures/invariants/*.json`)

Label-free invariants (§11): properties that must hold regardless of judgment
quality. These fixtures are *data documenting the invariants in the eval
corpus* — `--check` validates their schema and internal consistency so their
shape stays honest, while the executable checks live where the invariant is
enforced (the taint tracker for poisoning, the coordinator's inbound defang,
and the staged-commit validator's own Rust tests in
`crates/meerkat-meta/src/memory/staged.rs`).

Common fields: `name`, `invariant`, `stage`, `expected`, `notes`, plus
per-invariant payload:

- `poisoned_write_quarantined` — a `record` whose body carries an injection
  attempt, a `context` describing session taint, and an `expected` trust
  tier/status (`Untrusted`/`Quarantined`). The poisoned fixture must *always*
  quarantine.
- `forged_envelope_defanged` — a `turn_text` containing a forged
  `<meta_memory_observation>` envelope, and `expected.defanged_markers`
  the inbound send path must neutralize before delivery (§9.1).

Store-state invariants share one shape: a `store` array of pre-existing
record views (`{id, scope, trust, status, supersedes?, derived_from?,
has_verification?}`) plus a `batch` (`StagedMutationBatch`: `{realm, author,
ops}` with `StagedOp` = `create | supersede | tombstone | retier |
set_rank`), and an `expected` `{verdict: "reject", error, op_index}` pinning
the `StagedBatchError` variant. These mirror the Rust serde wire format
exactly — internally-tagged `scope`/`status`/`author`/`op` enums and
**snake_case** kinds and tiers (`gotcha`, `agent_observed`), unlike the
selector fixtures' PascalCase kinds:

- `supersede_cycle_rejected` — ops that would close a supersede cycle
  (`supersede_cycle`).
- `staged_tier_ceiling_rejected` — a staged `retier` to operator/application
  tier, which no author may do via a batch (§10.2;
  `tier_not_staged_assignable`).
- `transitive_laundering_rejected` — a `create` whose `derived_from` reaches
  a quarantined record plus a `retier` above `agent_observed`; laundering by
  consolidation carries the provenance ceiling (§10.2;
  `transitive_taint_ceiling`).

## The stage-eval seam (distiller-eval, steward-eval, hygienist-eval)

The following describes the retired Selector's former entry point and is kept
only as calibration provenance. It is not a supported command. It drove
(`memory::selector::select` — manifest + turn text + suppression list in,
structured `{selected_ids, coverage}` out) through the `selector-eval`
binary, one fixture-shaped JSON object on stdin per invocation:

```json
{"manifest": [...], "turn_text": "...", "suppressed_ids": [],
 "profile_path": "tests/memory-evals/profiles/selector-v0.toml"}
```

stdout is `{"selected_ids": [...], "coverage": "..."}`. Prompt rendering,
manifest shuffling (per call, inside `select`), strict JSON parsing, and the
single JSON-repair retry all live in the Rust stage; the harness only
shuffles the fixture's manifest order across the 3 runs and scores. Exit
codes: 0 ok, 1 selector error, 2 usage error, 3 live-requested-but-no-auth
(SKIP). `--mock` replaces the live model with a deterministic scripted
client so the plumbing is testable without credentials.

The embedded default profile (`SelectorProfile::embedded_default()`) and
this directory's `profiles/selector-v0.toml` + `prompts/selector-v0.md` are
the same artifact; a unit test in `memory/selector.rs` keeps the prompt
byte-identical, and profile changes bump `version` and gate on scorecard
non-regression (§11).

## Calibration run log

### 2026-07-05 — Selector v0, first live calibration pass (Claude-led)

Per the §16 Q3 decision (experimental calibration first, eval suite later).
Live lane: `scripts/memory-evals --stage selector --mode live` on
`claude-haiku-4-5` @ temperature 0 (the embedded default).

- **Baseline:** 6/6 existing fixtures clean.
- **Adversarial hardening (new fixtures, now permanent gates):**
  `manifest-injection-resistance` (embedded instructions in record
  titles/descriptions demanding selection — the selector must treat manifest
  rows as untrusted data; doubles as a §10.1 regression guard),
  `multi-select-complementary` (procedure + gotcha both load-bearing;
  guards under-selection bias from the "selecting nothing is common"
  framing), `suppression-under-temptation` (the single best record is
  suppressed; must not re-select it and must still surface the complement),
  `open-loop-resumption` (OpenLoop kind sensitivity beyond
  Gotcha/Reference).
- **Result: 10/10 live clean.** No failure found ⇒ no prompt change made
  (changing without a failing case would be guessing — the decision was data
  over guesses). Tier verdict: the small/fast tier §8.3 calls for is
  SUFFICIENT on this evidence; keep `claude-haiku-4-5`.
- **Deferred to the future eval suite:** manifest-scale fixtures at
  `working_set_k = 200` (current fixtures are small manifests) and latency
  measurement against the 500 ms recall budget.
