# Pre-ledger 0.7.x realm corpus

Realms written by published pre-0.8.10 `rkat` binaries, kept so the explicit
storage bridge is tested against the schema and the rows those releases
actually wrote rather than against this repository's reading of them.

## Why this exists

Meerkat 0.8.23 shipped a bridge that could not bridge. A normal run on an old
realm warned that historical sessions were not loaded and named a remedy:

```text
rkat --state-root <ROOT> --realm <REALM> storage migrate --apply --bridge-pre-0-8-10
```

Running exactly that failed:

```text
unledgered schema domain `runtime-store` does not match any authorized source
catalog through version 3
```

The bridge accepted two runtime-store catalogs, of 9 and 11 tables. Every realm
created before 0.8.0 has 7. The oldest shape the "pre-0.8.10 bridge" recognized
was younger than the realms it was named for, and no test in the tree ran the
bridge over bytes a released binary had written, so nothing caught it.

A hand-built fixture would not have caught it either: it would only have proved
that the fixture author and the verifier shared one reading of history. The
corpus here is minted by executing the published binaries.

## Shape

```text
corpus/
  fixture-manifest.json
  realms/
    0.7.5/{bootstrap-only,attempted-turn}/...
    0.7.19/bootstrap-only/...
    0.7.21/bootstrap-only/...
    0.7.28/{bootstrap-only,attempted-turn}/...
```

A *capture* is one execution of one published binary under one documented
environment, and it owns the realm directory that execution left behind.

| capture | environment | what the writer did |
|---|---|---|
| `bootstrap-only` | no provider credentials at all | wrote its realm files during storage bootstrap, then died at LLM client construction before admitting anything |
| `attempted-turn` | `ANTHROPIC_API_KEY` set to a dummy value, model named explicitly with `-m` | admitted the operator's prompt, persisted runtime and session state, then died at the provider call |

`fixture-manifest.json` binds, per release: the published release asset and its
SHA-256, the extracted binary's SHA-256 and `--version` output, and then per
capture: the capture command, the writer's exit code and first error line,
every corpus payload by size and SHA-256, the complete `sessions.sqlite3`
catalog as the released writer left it, per-table row counts for both
databases, and one descriptor per `runtime_input_states` row.

What it does *not* bind is a transient artifact of the capturing process. A
sequence lock is held by the writer while it runs and means nothing once it has
stopped, and the repo `.gitignore` drops `corpus/**/*.lock`, so binding one
would describe a file no clean checkout can have. `realm_relative_files` in the
minting script excludes those suffixes, which is the one place the copy into the
corpus and the manifest payload list are both derived from;
`scripts/test_mint_pre_ledger_fixture.py` (run by `make
verify-fixture-mint-generator`) holds that exclusion and the `.gitignore` rule
to the same answer. WAL/SHM sidecars are refused rather than excluded, because
they mean the realm was opened after the writer stopped.

The four releases span the 0.7.x line. Their runtime-store catalogs are
byte-identical, which `pre_ledger_runtime_catalog_is_identical_across_the_published_span`
proves by comparison across every release *and* every capture, so a row-bearing
realm cannot be dismissed as "a different schema". Their session-store catalogs
are not identical: 0.7.28 added the strand tables. Their workgraph schemas are
not: 0.7.5 wrote version 1 and 0.7.28 wrote version 2. Keeping all four is what
makes those differences visible instead of assumed.

## What is actually in these realms

Row counts are read out of the committed databases by
`pre_ledger_corpus_row_counts_are_read_from_the_committed_bytes`, not believed
from the manifest.

| capture | `sessions.sqlite3` rows |
|---|---|
| `0.7.5/bootstrap-only` | `runtime_states`=1; all 10 other tables 0 |
| `0.7.19/bootstrap-only` | `runtime_states`=1; all 10 other tables 0 |
| `0.7.21/bootstrap-only` | `runtime_states`=1; all 10 other tables 0 |
| `0.7.28/bootstrap-only` | `runtime_states`=1; all 13 other tables 0 |
| `0.7.5/attempted-turn` | `runtime_input_states`=1, `runtime_session_snapshots`=1, `runtime_states`=1, `sessions`=1; 7 others 0 |
| `0.7.28/attempted-turn` | `runtime_input_states`=1, `runtime_session_snapshots`=1, `runtime_states`=1, `session_heads`=1, `session_strand_messages`=1; 9 others 0 |

`workgraph.sqlite3` is empty in every capture: 4 tables, 0 rows.

The single `runtime_input_states` row in each `attempted-turn` capture is the
operator's `hello` prompt at `stored_input_state_version` 3, ending
`current_state = "abandoned"` with `terminal_outcome = {abandoned, stopped}`
after the history `QueueAccepted -> StageForRun -> ResolveStagedRollback ->
Abandon`.

## What is deliberately absent

The previous version of this section was false, and the falsehood hid a
blocking defect. It claimed a row-bearing corpus needed "a completed turn from
the published binary against a deterministic endpoint" and belonged in a
separate corpus. It does not. It needs one ordinary run with a dummy API key
and an explicit `-m`, which is what `attempted-turn` is. Because every realm in
the first corpus was written by a run that died before admitting any input,
nothing here could reach `prepare_pre_0_8_10_runtime_input_states`, and a
defect inside that callback shipped unseen behind a green test file.

What is genuinely absent:

- **No completed assistant turn.** The provider rejects the dummy key, so there
  is no assistant message, no `runtime_ops_lifecycle` row, no
  `runtime_boundary_receipts` row, and no `session_rewrites` row.
- **No multi-turn, resumed, or concurrent input.** Each realm carries exactly
  one input row from one run.
- **No nonterminal input row.** The one row is terminal-`abandoned`, so the
  callback's queued-row replay path, its per-runtime idempotency-owner check,
  and its duplicate-history-timestamp check are still unexercised. A capture
  that leaves a row `queued` or `staged` would need the writer to be stopped
  mid-turn, which no capture here does.
- **No `idempotency_key` on any row.** The published writer omits the field
  when it is unset, so the corpus cannot say what the bridge does with a row
  that carries one.
- **No non-Anthropic provider, and no realm-level `config.toml`.** Both
  captures run with no config file present.
- **No `jobs.sqlite3`, `memory/`, `tasks.db`, or mob databases.** The captures
  never create them, so the realm-level bridge's other single-domain files are
  covered only by hand-built tests elsewhere.
- **One target only.** Every binary is `aarch64-apple-darwin`.

## Resolved state: the row-bearing realms bridge and open

The defect this corpus was minted to expose is fixed. Both `attempted-turn`
captures now bridge to the current schema, and
`bridged_pre_ledger_realm_opens_through_the_ordinary_path` (in `meerkat`)
proves the realm afterwards opens through the ordinary path with its session
readable - bridging exit 0 was never the deliverable.

What the corpus caught, recorded because it is the reason to keep minting from
released binaries rather than constructing bytes from source:

```text
migration 1 (`maintenance-prepare`) for domain `runtime-store` failed:
pre-0.8.10 runtime input <runtime_id>/<input_id> failed frozen maintenance
import: released v3 input-state row is missing required fields
["last_run_id", "last_boundary_sequence"]
```

The row gate's constants had been read off this repository's source, and
disagreed with released bytes on nearly all of them. Only a row-bearing realm
could tell the catalog leg (fixed) from the row leg (not) apart; every
`bootstrap-only` capture passed throughout, because a realm with no input rows
never enters the callback at all.

Three tests are pinned to this history and must not be quietly weakened:

- `bridge_eligibility_agrees_with_the_bridge_on_row_bearing_realms` asserts
  that whatever the eligibility predicate promises about a row-bearing realm,
  the bridge delivers on the identical bytes. Its predecessor was pinned to
  `bootstrap-only` precisely because `attempted-turn` exposed the divergence,
  and its successor asks every co-tenant domain of `sessions.sqlite3` rather
  than the runtime store alone: asking only the runtime store is how the same
  file's session domain came to be described to operators as unrecoverable
  while this test stayed green.
- `the_row_preparation_callback_preserves_every_durable_fact` asserts that
  admission was not bought by dropping state. A fix that admitted rows by
  blanking a field passes every other test in the file.
- `bridging_a_row_bearing_realm_keeps_the_row_and_every_ingress_payload`
  asserts the same thing across the WHOLE bridge, not just the callback, and
  refuses to run at all against a capture that carries no payload text. Its
  predecessor asserted the opposite - that the runtime-store v1 -> v2 released
  importer had retired each terminal row's ingress payload - and that is
  exactly what a successful bridge was doing to the operator's own `"hello"`.
  The released-row importers no longer run under the rescue, so a rescued realm
  keeps every payload it arrived with.

## Re-minting

The script is additive. It refuses to overwrite a capture that already exists,
and it carries an existing capture's recorded provenance forward while
re-hashing its committed bytes, so adding a capture never re-mints (and never
silently replaces) bytes that are already committed and verified.

```bash
gh release download v0.7.5 -R lukacf/meerkat \
    -p 'rkat-0.7.5-aarch64-apple-darwin.tar.gz' -p 'checksums.sha256' -D /tmp/v0.7.5

python3 mint_pre_ledger_fixture.py \
    --capture attempted-turn \
    --release 0.7.5=/tmp/v0.7.5/rkat-0.7.5-aarch64-apple-darwin.tar.gz \
    --checksums 0.7.5=/tmp/v0.7.5/checksums.sha256 \
    --corpus "$PWD/corpus"
```

With no `--release`, the script only re-verifies every capture on disk against
the manifest and rewrites the manifest from those bytes.

The script refuses an asset whose SHA-256 differs from the release's published
`checksums.sha256`, a binary whose `--version` does not match the release, a
realm carrying a WAL or SHM sidecar (which would mean something opened it after
the writer stopped), a run that *succeeded* (which would mean a real provider
answered and the corpus is no longer synthetic), an `attempted-turn` capture
that produced no `runtime_input_states` rows, a `bootstrap-only` capture that
produced any, and a capture directory on disk that no manifest explains. It
never inherits the host environment: the published binary runs with its own
`HOME`, `TMPDIR`, and nothing else, so an ambient key or `RKAT_*` override
cannot change what gets minted. The model for `attempted-turn` is read from the
published binary's own catalog rather than hardcoded here.

Corpus bytes are excluded from the `trailing-whitespace` and `end-of-file-fixer`
pre-commit hooks in `.pre-commit-config.yaml`. Without that, the hooks rewrite
`realm_manifest.json` (the published writer emits no trailing newline) and every
manifest digest breaks on the next push.

## Consumers

- `meerkat-runtime/tests/pre_ledger_realm_bridge.rs` - corpus integrity,
  row counts read from the committed bytes, the unledgered 7-table shape,
  catalog stability across the span and across captures, the end-to-end bridge
  to the current schema, fail-closed refusal of a catalog no release wrote, and
  the honest-remedy predicate.
- `meerkat/src/persistence.rs` tests - the realm-level bridge through the same
  facade entry point the CLI calls, including per-domain outcomes when one
  domain is unbridgeable.

Both run in the ordinary workspace test lanes (`cargo unit` and `cargo int`).
A corpus that is never executed is how the original gap survived; a corpus that
cannot reach the code under test is how the second one did.
