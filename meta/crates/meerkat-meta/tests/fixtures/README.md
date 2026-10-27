Frozen released-corpus fixtures. Never regenerate these with current code:
current writers cannot and must not mint released envelopes, and a
fixture re-synthesized by the pinned writer silently passes exactly the
writer-drift bugs these tests exist to catch (the 0.8.11 fleet-import
regression shipped past a synthetic 26-chain test for that reason).

All free-text content in these fixtures (system prompts, instructions,
message bodies, labels) is synthetic. Only the structural shape (keys, key
order, row layout, ids, timestamps, digests) is preserved, because that
shape is what the tests pin.

- v0_8_10_released_session.json - SYNTHETIC: a meerkat 0.8.10 (producer
  version) system-only session envelope, byte-identical to Meerkat core's
  synthetic recovery-migration fixture. It keeps the released envelope
  shape: version 2, exactly one system message, no transcript-history key,
  the seven released metadata keys, a legacy recovery_migration checkpoint
  stamp (schema 1, generation 0, revision 6) and the four usage token keys.
  Its system prompt, roughly 12 KB build state, tool catalogue, labels and
  identifiers are synthetic; the prompt carries the marker phrase
  `Example Review Agent`. Excluded from the published crate. Used by the
  adapter import-on-load regression and the released-v2 import leg of
  identity_first_lazy_recall_continuity.rs.

- v0_8_10_zero_rewrite_supervisor_session.json - a 0.8.10-shaped
  mob-supervisor snapshot envelope (zero-rewrite transcript graph: one
  revision, no commits key on the wire, singleton live-head body equal to
  the live transcript; session 019f2bdc-a781-7060-bff8-0b97b7a4fcee) with
  SYNTHETIC content. The released envelope's keys, key order, ids,
  timestamps and spelling are kept; every free-text value (system prompt,
  build-state prompts) is deterministic synthetic text, the mob namespace
  is generic, and the transcript revision digest (and head) is computed
  with meerkat_core::released_0810_transcript_serialized_rows_digest.
  The retired checkpoint stamp is kept verbatim: the importer strips it as
  untrusted metadata. The strict importer refuses it with a typed error.
  Excluded from the published crate. Used by
  `released_zero_rewrite_history_refuses_typed_on_adapter_load` in
  `src/identity_first/adapters.rs`, which pins typed, session-scoped import
  refusal, byte preservation, and unrelated-session usability.

- released_0_8_8_realms/ - four COMPLETE realms (continuity.sqlite3 v2 +
  runtime.sqlite v1 + mobkit satellites + WAL/SHM/.mfence sidecars, no
  realm_manifest.json) minted by the released mobkit 0.8.8 rpc_gateway
  (embedding meerkat SDK 0.8.10), driven over stdin JSONL with synthetic
  turns. Consumed by tests/released_realm_upgrade_drive.rs, which boots the
  CURRENT gateway over a staged copy. Never regenerate with current code,
  and never open these files with sqlite in place: a read/write open
  checkpoints and truncates the WAL, destroying the released byte shape
  (the test pins the continuity main/WAL sizes and fails loudly). The empty
  blobs/ directory is part of the released shape; git cannot track it, so
  the test's staging helper restores it.
    - baseline/       multi-boot clean-shutdown history, 5 turns
                      (head: 11 messages)
    - burst_drain/    pipelined 6-send burst fully drained by
                      mobkit/shutdown (head: 21 messages)
    - crash_sigkill/  SIGKILLed after the burst drained: un-checkpointed
                      WALs, no shutdown attestation; all 10 inputs consumed
                      and the legacy runtime snapshot in sync with the head
                      (the kill landed at idle)
    - deploy_cycles/  4 boot/turn/clean-shutdown cycles (head: 9
                      messages); rewrite_count is 0 - the released binary
                      minted no resume rewrites for an unchanged system
                      prompt

- released_0_8_26_store/ - one COMPLETE persistent store (`state/`) minted
  by the released MobKit 0.8.26 rpc_gateway (release archive sha256
  fdac071b0a1b3983c67e6a75fc279135aa9bff9dcae56d8caaaaa8bb9dfbafb1) over
  stdin JSONL with no provider key (`runtime_options.demo_llm`):
  mob_storage persistent, so `event_log.sqlite3` holds the mob's events
  and `event_log.sqlite3.composition.json` carries the predecessor
  creator field `created_by_mobkit`; one durable identity
  (`learner:fixture-0001`, session 01a13aaa-4918-71e2-b44c-366e9b2d4de3)
  with one driven turn; two agent memories in
  `agent-memory/default.sqlite3`; one memory ledger assertion; the
  predecessor database names and ledger domains and the `mobkit/` realm.
  `mint_released_0_8_26_store.py` beside it is the capture driver (the
  definition it used includes `tools.mob_tasks`, which 0.8.26 accepted,
  and no profile `system_prompt`, which 0.8.26 ignored and 0.9 refuses);
  `checksums.sha256` pins every file. Two things were left out of the
  capture: the empty `blobs/` directory (git cannot track it; the test's
  staging restores it) and the gateway peer key `peer_key.ed25519` (key
  material; an installation without host pairings mints a new one on
  boot). The mob supervisor key inside `event_log.sqlite3` was minted by
  this capture and authorizes nothing outside it. All content is
  synthetic. Never regenerate with current code and never open these files
  in place. Excluded from the published crate. Consumed by
  tests/released_0_8_26_store_adoption.rs.

- released_0_9_3_h_local/ - six COMPLETE persistent state directories
  minted by the released Meta 0.9.3 rpc_gateway (release archive sha256
  b416df1ba5648ca3ed849dec2692f664c37b0e7277232a6d2421ac53961a18b0, its
  rpc_gateway sha256
  757519ed1210d16d865e2e37de3fba098a1cca6ccba6a7e1dc646963fb04a335) over
  stdin JSONL the way an H-local host runs it. H-local is the #306 plan's
  name for a local persistent host deployment: a host that runs the Meta
  gateway on its own machine, speaks JSON-RPC to it over stdin/stdout and
  keeps a persistent state directory. Each run has persistent state, a roster
  served through `callback/roster_provider/roster` (18 durable identities
  `agent:hl-01` .. `agent:hl-18`, profile `specialist`), eager identity
  bootstrap and automatic compaction, with a loopback chat-completions model
  and no provider key. Compaction as used in every run: the host policy pins
  auto_compact_threshold 50000 and recent_turn_budget 1, a user message
  carrying `[ct:<key>]` makes the model report 60000 input tokens once, and
  min_turns_between_compactions (3) and history_retained_rewrites (4) stay
  the 0.9.3 defaults, which 0.9.3's compaction keys cannot override. Across
  the boots: one turn per identity, compactions, retire then wake, reset,
  delete_identity, a roster reconcile, meta/compact_member, and a roster that
  drops `agent:hl-18`. The variants:
    - bootable/ three clean boots; the roster drops hl-18 at boot 3, hl-15
                stays retired; the 0.9.3 gateway starts on the final state
                again
    - stuck/    two clean boots; the roster drops hl-18 at boot 2, which
                then runs the lifecycle verbs; the 0.9.3 gateway can no
                longer start the final state (meta/init fails with "mob actor
                command channel closed")
    - killed/   as bootable, then meta/compact_member on hl-14 never
                completes, meta/shutdown gets no answer and the gateway is
                killed (SIGKILL)
    - reanchored/ as bootable, plus `agent:hl-16` and `agent:hl-17`
                compacted six times each, past the retention of 4: both
                committed transcript graphs keep rewrites 3-6 and retired
                1-2. hl-16 stays bound to its session (three compactions
                before a restart, three after); hl-17 is deleted at boot 2,
                which leaves its re-anchored session as a runtime-only body
                (no continuity.sqlite3 copy) that nothing is bound to
    - rematerialized/ a plan of its own on the same host setup, over
                hl-01 .. hl-03: boot 1 runs a turn for hl-01 and hl-02, boot 2
                deletes hl-02, and boot 3 materializes hl-02 again and hl-03
                for the first time, with no turn before the clean shutdown.
                Each of the two is bound to a one-message session at
                revision 1 that continuity.sqlite3 holds only as the
                snapshot written at materialization, with no head (0.9.3
                writes the head at a session's first turn); hl-02's
                pre-delete session is a runtime-only body
    - direct-member/ a plan of its own on the same host setup: the roster
                lists only hl-01, and over two clean boots the host makes the
                member hl-direct itself with `meta/ensure_member` (the stdio
                gateway's raw mob path, which writes no continuity record),
                with the application label team=direct, and runs one turn on
                it per boot through `meta/send_message`. 0.9.3 leaves
                hl-direct a live mob member on one session that only
                runtime.sqlite holds
  Each variant holds `state/`, `events.jsonl` (every operation with its
  result or error, including the 0.9.3 refusals), `MANIFEST.json` and
  `MANIFEST.txt` (rosters, shutdown outcomes, per-identity session history
  and last status, the restart probe, committed sessions read from
  runtime.sqlite with its WAL, raw file digests; reanchored/ also records
  each session's transcript graph, the compaction config and the two
  re-anchored sessions; rematerialized/ records each session's continuity
  head, strand rows, snapshot row and record, and the two sessions bound
  without a head; direct-member/ records hl-direct's session with its
  continuity facts) and `checksums.sha256` (every stored file).
  `mint_released_0_9_3_h_local.py` beside them is the capture driver of
  bootable/, stuck/ and killed/; `mint_released_0_9_3_h_local_reanchored.py`
  (the same driver with the hl-16/hl-17 schedule) is that of reanchored/,
  `mint_released_0_9_3_h_local_rematerialized.py` (the same driver with
  the three-identity plan) that of rematerialized/, and
  `mint_released_0_9_3_h_local_direct_member.py` (the rematerialized driver
  with the direct-member plan) that of direct-member/.
  Left out of each capture: the gateway peer key
  `peer_key.ed25519` (key material) and the empty `blobs/` directory (git
  cannot track it). The mob's comms secret key inside event_log.sqlite3 was
  minted by this capture and authorizes nothing outside it. Files above
  900 KiB (some WAL files; in reanchored/ also runtime.sqlite and
  meta_console.sqlite3) are stored gzip-compressed as `<name>.gz`, under
  the repository's per-file size limit; MANIFEST.json `packed` lists each
  with its raw size and sha256, and `empty_dirs` the directories to
  recreate. Stage a variant only through
  tests/support/released_0_9_3_h_local.rs (`stage_variant`): it copies
  `state/` to a scratch directory, inflates the packed files and checks
  their raw sha256, recreates the empty directories, and checks the staged
  tree against MANIFEST.json. Never open the files in place. All content is
  synthetic. Never regenerate with current code. Excluded from the
  published crate. Consumed by released_0_9_3_h_local_capture.rs (the
  captures, their provenance, and the 0.9.3 outcomes the upgrade tests
  start from); inputs for the upgrade acceptance tests and the
  storage-migrate-native census.

- released_0_9_3_h_app/ - four COMPLETE persistent state directories written
  by the published Meta 0.9.3 crates from crates.io (meerkat-meta =0.9.3 and
  every meerkat crate at 0.9.3) the way an H-app runs them. H-app is the #306
  plan's name for an application that embeds the Meta runtime through
  `UnifiedRuntimeBuilder` with `persistent_state`. Each run builds the
  runtime with comms, compaction pinned to auto_compact_threshold 50000 and
  recent_turn_budget 1, and a scripted in-process model ("app reply N"; a
  user message carrying `[compact-now]` reports 60000 input tokens once), so
  nothing leaves the machine and no provider key is read. The variants:
    - roster/   a `RosterProvider` (eager bootstrap) over `app:app-01` ..
                `app:app-06` with the builder's default in-memory mob
                storage. Boot 1: a turn each; app-01 compacts; app-03 is
                retired and woken by a turn; app-04 is reset; app-05 is
                deleted (its next turn is refused). Boot 2: the same roster
                re-adds app-05 on a fresh session that takes no turn. Boot 3:
                the roster drops app-06, whose continuity record stays
    - roster-durable/ the roster plan with persistent mob storage at
                `state/event_log.sqlite3` (the gateway's file name), supplied
                through `mob_storage(persistent_mob_storage(..))`
    - roster-durable-mob-sqlite/ the same at `state/mob.sqlite3`, the name
                0.9.3's own non-gateway call sites use
    - plain/    no roster provider: over two boots the app spawns
                `worker-1` and `worker-2` on its in-memory mob and runs a turn
                on each, so boot 2 spawns them again on fresh sessions
  With a roster, 0.9.3 keeps every session body in `sessions.sqlite3`, as
  plain/ does, and only the identity bindings in `continuity.sqlite3`: a
  schema-1 store (`meerkat_schema` ('meta-continuity', 1)) holding
  `continuity_records` and an empty `session_snapshots`, without the
  gateway's head, strand or rewrite tables. Each variant holds `state/`,
  `events.jsonl` (every driver operation with its result or error),
  `probe.json` (0.9.3 builds once more on a copy of the final state),
  `MANIFEST.json` and `MANIFEST.txt` (producer, statuses per boot, refusals,
  committed sessions read from runtime.sqlite with its WAL and their
  `sessions.sqlite3` projection and continuity record, the continuity
  store's schema and rows, file digests) and `checksums.sha256` (every
  stored file). 0.9.3 keeps each session's comms keypair outside the state
  directory, under `$XDG_STATE_HOME/meerkat/session_comms_identity/`;
  MANIFEST.json `outside_state` lists those files by session, and none is
  stored. `mint_released_0_9_3_h_app.py` beside the variants is the capture
  script, and `driver/` the standalone crate whose binary it runs. The
  driver keeps its manifest and lockfile as `Cargo.toml.fixture` and
  `Cargo.lock.fixture`, so that nothing in this repository reads it as a
  crate or a lockfile; the script's docstring gives the build steps, which
  restore the real names in a copy outside the repository. Every MANIFEST
  pins the script, the driver files and the binary by sha256. Left out of
  each capture: the empty `blobs/` directory (MANIFEST.json `empty_dirs`).
  Stage a variant only through tests/support/released_0_9_3_h_app.rs
  (`stage_variant`), which stages the H-local way. Never open the files in
  place. All content is synthetic. Never regenerate with current code.
  Excluded from the published crate. Consumed by
  released_0_9_3_h_app_capture.rs (the captures, their provenance, the
  builder layout and the 0.9.3 outcomes the upgrade tests start from);
  inputs for the upgrade acceptance tests and the storage-migrate-native
  census.

- released_0_9_3_h_remote/ - two captures of a host that kept its sessions in
  a continuity provider, written by the published rpc_gateway 0.9.3 binary in
  raw RPC mode (persistent_state, a file-backed continuity store, a lease
  provider, a roster provider), each with the export the continuity export
  scripts wrote from that provider: memory (in-memory mob storage; the state
  is boot 2's new volume, which lacks two sessions) and durable (durable mob
  storage; one directory across both boots). Six listed identities: four
  ready, one 0.9.3 deleted, one that never existed; one document is 3.9 MB.
  All content is synthetic. See the directory's README.md for provenance.
  Consumed by storage_migrate_native_census.rs (`--continuity-export`, new
  and in place).

- security_idempotency/ - a three-state head+snapshot evolution of one
  member session (019fae11-4e87-7482-8796-54b2dac1f410): the untouched
  gen-20 corpus, the row after ONE boot of the fixed binary on a fresh
  seed, and the row after a SECOND boot (the exactly-once violation:
  identical head_revision, same length, different bytes) - with SYNTHETIC
  content. Keys, ids, timestamps, the tool-visibility state and the
  boot-to-boot drift keep their structural shape; every free-text value
  (prompts, instructions, message bodies) is deterministic synthetic text,
  the mob namespace is generic, and each cas_token is computed over its
  head_json. sha256 pinned in checksums.sha256. Excluded from the
  published crate. Consumed by
  identity_first::adapters::tests::downstream_security_boot_drift_is_zero_durable_change,
  which pins that strict head equality SEES the two-boot drift (updated_at +
  the HashSet-ordered tool-visibility Allow arrays, filed upstream as S5)
  while the scoped exact-resave equality reads it as zero durable change.

- released_0_9_3_started_0_9_2_adopted_store/ - one COMPLETE persistent
  store (`state/`) in the lineage the supported 0.9.4 route expects of a
  0.9.2 installation: minted by the released MobKit 0.8.26 rpc_gateway,
  adopted by the released Meta 0.9.2 meta_gateway
  (`storage-adopt-predecessor --apply`), run by the released Meta 0.9.2
  rpc_gateway for two boots (2 + 1 turns), then started ONCE by the
  released Meta 0.9.3 rpc_gateway (meta/init, meta/status_identity,
  meta/shutdown; no turn), so released 0.9.3 wrote it last. That first
  0.9.3 open moved the mob ledger domain from 10 to 11 (council,
  event_log and mobs/realm_profiles.db) and nothing else. The composition
  manifest carries the predecessor creator field `created_by_mobkit` (the
  bytes 0.8.26 wrote, as an adoption without the manifest step leaves
  them); the 0.9.3 start did not rewrite it. One durable identity
  (`learner:fixture-0001`, member `mk--learner_cfixture-0001`), its
  session since the 0.8.26 mint, two agent memories and one memory ledger
  assertion; `fixture-summary.json` names them, with the mob definition
  (including `tools.mob_tasks`, which 0.9 ignores), the roster, the init
  params, the ledgers before and after the 0.9.3 start, and every binary
  and release-archive digest. `mint_released_0_9_3_started_store.py` beside
  it reproduces the whole lineage from the four published binaries (it
  drives ../released_0_8_26_store/mint_released_0_8_26_store.py for the
  first step); `checksums.sha256` pins every file. Left out, as in
  released_0_8_26_store/: the empty `blobs/` directory, the gateway peer
  key `peer_key.ed25519`, and the member's session comms identity keys
  (outside the state directory). Without those keys, a 0.9.3 start still
  reports the identity active (recorded in the summary); a test that drives
  a turn needs keys it mints itself. All content is synthetic. Never
  regenerate with current code and never open these files in place.
  Excluded from the published crate. Consumed by
  tests/released_0_9_3_started_store_migration.rs.

The current wire-contract fixtures below are shared across languages and are
not part of the frozen corpus. They are expected to change when the wire
changes, following each fixture's maintenance rules. The freeze above applies
only to the released corpora.

- role_migrations_init_params.json - the hand-authored wire contract for
  boot-scoped member role migrations: one gateway init-params object
  carrying a top-level role_migrations array of {identity, from_role}
  declarations (synthetic example values: identity domain:automation
  migrating from role domain). Read by BOTH languages from this ONE file.
  Rust: identity_first::bridge::tests::
  the_committed_wire_fixture_deserializes_into_declarations include_str!s it
  and deserializes params["role_migrations"] into Vec<RoleMigrationDeclaration>.
  Python: sdk/python/tests/test_role_migrations.py::
  test_builder_output_matches_the_committed_wire_fixture asserts that
  MobKit.builder().role_migrations([...]) puts exactly this array in the
  gateway init params. Renaming a key on either side goes red on that side
  instead of both sides staying green while a host arms nothing.

- application_tool_policies_init_params.json - generated gateway init params
  shared by `src/member_tool_policy.rs` and
  `sdk/python/tests/test_application_tool_policies.py`. Rust's
  `the_committed_wire_fixture_installs_its_carried_provider` test verifies
  installation of the carried policy; Python checks the builder's wire output.
  The canonical policy bytes contain a computed digest, so do not hand-edit
  this fixture into validity. Regenerate it by setting `MOBKIT_WRITE_FIXTURE=1`
  when running that Rust test from the repository root:

  ```bash
  MOBKIT_WRITE_FIXTURE=1 ./scripts/repo-cargo test -p meerkat-mobkit --lib \
    the_committed_wire_fixture_installs_its_carried_provider
  ```

- live_contracts_v1.json - shared live wire-contract cases (synthetic
  realm, principal, auth binding and session instructions) read by Rust
  `crates/meerkat-meta/tests/live_contracts.rs` and
  `src/public_live_config.rs`, Python
  `sdk/python/tests/test_live_contracts.py`, and TypeScript
  `sdk/typescript/tests/live.test.ts`. Update the current contract and its
  cross-language expectations together; this is not a frozen released capture.

- console_voice_v1.json - shared console voice wire-contract cases (typed
  voice error payloads, context-status and captions methods with synthetic
  request and channel ids) read by the Rust `src/console_voice/` modules and
  `console/src/lib/voice-session.test.ts`. Update it together with both
  readers when the console voice wire changes.

- continuity_export_v1/ - continuity export files (format version 1), one
  written by each standalone export script from the same synthetic 0.9.3
  provider store: `written_by_python.jsonl` by
  `tools/continuity-export/continuity_export.py` and `written_by_node.jsonl`
  by `tools/continuity-export/continuity-export.mjs`. The store holds two
  ready identities (one document is short synthetic text, the other is every
  byte value from 0 to 255) and two uninitialized identities, named so that
  code point order and UTF-16 order disagree. The documents are opaque
  synthetic bytes, not session documents. Each script's tests compare their
  own file byte for byte and verify both; the two files have the same entry
  lines, and their headers differ only in `writer`. To rewrite a file after
  an intended format change, set `META_WRITE_CONTINUITY_EXPORT_FIXTURE=1`
  when running that script's tests, then rerun both suites.
  `identities_bom_crlf.txt` is an identities file both scripts must read as
  the same five identities: it starts with a byte order mark, mixes CRLF and
  LF line ends, pads identities with ASCII and Unicode spaces, has blank
  lines, and ends without a newline.

- continuity_mirror_blobs_v1/ - the blob half of a continuity mirror, both
  ways. `blobs.jsonl` holds the rows the mirror copies for two synthetic
  blobs, a PNG-signed image under Meerkat's image address and a blob under
  Meta's raw address; `restored/blobs/` is the tree
  `tools/continuity-export/restore_blobs.py` writes for those rows into a
  state directory. `src/unified_runtime/continuity_mirror_blob_tests.rs`
  checks that the mirror writes exactly `blobs.jsonl` from a real blob store
  and that the gateway's blob store reads the restored tree back with the
  original bytes; `tools/continuity-export/test_restore_blobs.py` checks that
  the script writes exactly that tree (apart from each blob's write time).
  To rewrite them after an intended format change, run the script on
  `blobs.jsonl` into a fresh directory and copy its `blobs/` here.
