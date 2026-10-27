# Console Timeline Scalability Trace

## Requirements

| ID | Requirement | Phase | Runtime Caller | Evidence | Status |
| --- | --- | --- | --- | --- | --- |
| CONTRACT-001 | Timeline queries support explicit `since` and `recent` modes. | 0 | `mobkit/console/query_timeline`, `GET /console/timeline` | `cargo test -p meerkat-mobkit console_aggregator::store::tests`; `npm --prefix console run phase0:types --silent` | VALIDATED |
| CONTRACT-002 | Timeline responses expose continuation state beyond `frames` and `next_cursor`. | 0 | Console RPC/REST clients | `npm --prefix console run phase0:types --silent`; `npm --prefix console run e2e:browser --silent` | VALIDATED |
| REQ-001 | Global initial activity loads recent visible frames, not cursor 0. | 1 | `ConsoleApp` global activity seed | `browser global timeline recent seed ok` in `npm --prefix console run e2e:browser --silent` | VALIDATED |
| REQ-002 | Identity chat initial load shows recent usable history after one bounded request. | 1 | `ConsoleApp` agent chat panel | `browser chat pane recent first page ok` in `npm --prefix console run e2e:browser --silent` | VALIDATED |
| REQ-003 | Identity history rendering is incremental and does not wait for full transcript paging. | 2 | `ConsoleApp` identity timeline loader | `browser chat pane recent first page ok`; `browser chat pane older history demand paging ok` | VALIDATED |
| REQ-004 | Timeline paging skips invisible cursor gaps within a bounded raw scan budget. | 1 | `MobKitConsoleAggregator::query_timeline` | `cargo test -p meerkat-mobkit query_timeline_since_skips_hidden_raw_gaps_with_bounded_paging -- --nocapture` | VALIDATED |
| REQ-005 | Recent/tail queries use store-level reverse/indexed access, not whole-log replay. | 1 | `ConsoleLogStore::query_frames` | `cargo test -p meerkat-mobkit console_aggregator::store::tests -- --nocapture`; `fresh_timeline_snapshot_reads_tail_without_full_log_replay` | VALIDATED |
| REQ-006 | Identity recent snapshots keep a useful turn-start anchor before noisy recent tails. | 1 | `MobKitConsoleAggregator::query_timeline` | `cargo test -p meerkat-mobkit fresh_identity_snapshot_keeps_user_input_anchor_before_noisy_tail -- --nocapture` | VALIDATED |
| REQ-007 | Open-panel refresh uses continuation/recent semantics rather than repeating full oldest-first scans. | 2 | `ConsoleApp` refresh loop | `npm --prefix console run e2e:browser --silent`; source path uses initial `recent`, follow-up `since` by `latestTimelineCursor` | VALIDATED |
| E2E-001 | Browser E2E proves large global logs show recent worker activity. | 3 | Bundled console in Chromium | `browser global timeline recent seed ok` | VALIDATED |
| E2E-002 | Browser E2E proves delayed multi-page identity history renders before all pages finish. | 3 | Bundled console in Chromium | `browser chat pane recent first page ok`; `browser chat pane older history demand paging ok` | VALIDATED |
| E2E-003 | Browser/backend E2E proves invisible/empty cursor windows do not strand timeline loading. | 3 | Bundled console in Chromium plus aggregator entrypoint | `browser chat pane older history demand paging ok`; `query_timeline_since_skips_hidden_raw_gaps_with_bounded_paging` | VALIDATED |
| INV-001 | Timeline reads remain store-local and do not synchronously refresh broad session history. | 1 | Aggregator query path | `cargo test -p meerkat-mobkit query_timeline_is_store_local_for_registered_runtimes -- --nocapture`; `cargo test -p meerkat-mobkit` | VALIDATED |
| INV-002 | No Fugue-specific console path is introduced. | final | Repository scan | `rg -n "Fugue\|fugue\|LUC-631\|kapellmeister\|Kapellmeister" ...` only finds design-doc context/examples | VALIDATED |
| INV-003 | No Meerkat core/RPC changes are required for this bug. | final | Repository diff | `git diff --name-only` only shows MobKit console/backend files and design docs | VALIDATED |
| INV-004 | `since` pagination must not advance past visible frames that were not returned. | review | Aggregator query path, SSE replay clients | `query_timeline_since_cursor_stops_at_last_visible_returned_frame`; `cargo test -p meerkat-mobkit` | VALIDATED |
| INV-005 | Idle chat refreshes must not force broad session-history backfill. | review | Open chat panel refresh loop | `query_timeline_since_empty_continuation_does_not_force_backfill`; `cargo test -p meerkat-mobkit` | VALIDATED |
| INV-006 | Timeline cursor parsing must reject out-of-range values instead of wrapping. | review | SQLite console log store | `sqlite_log_rejects_out_of_range_console_cursors`; `cargo test -p meerkat-mobkit` | VALIDATED |
| INV-007 | Older-history paging must preserve the user's scroll position. | review | `ChatPane` browser behavior | `browser chat pane older history demand paging ok`; `npm --prefix console run e2e:browser --silent` | VALIDATED |
| INV-008 | Stale timeline SSE cursors must be normalized as replay-unavailable and resubscribe at the latest cursor. | review | Console-core timeline subscription | `subscribeTimelineEvents recovers from stale timeline cursors`; `npm --prefix console run phase0:types --silent` | VALIDATED |
| INV-009 | SSE reconnect snapshots drain persisted `since` backlog across store pages before switching to live broadcast. | review | `/console/timeline/stream` snapshot path | `timeline_snapshot_drains_since_backlog_across_store_pages`; `cargo test -p meerkat-mobkit timeline_snapshot -- --nocapture` | VALIDATED |
| INV-010 | Future cursors are replay-unavailable, while live stream lag reconnects from the last delivered cursor so persisted backlog can be replayed. | review | SSE replay client/server | `timeline_snapshot_rejects_after_cursor_beyond_store_frontier`; `subscribeTimelineEvents reconnects from the last delivered cursor after stream end`; `npm --prefix console run phase0:types --silent` | VALIDATED |
| INV-011 | Public Rust timeline structs remain source-compatible while new windowed semantics are additive. | review | Rust public API | `legacy_timeline_struct_literals_remain_source_compatible`; `cargo test -p meerkat-mobkit legacy_timeline_struct_literals_remain_source_compatible -- --nocapture` | VALIDATED |
| INV-012 | New timeline wire semantics advertise a new contract version. | review | REST/RPC/SSE contract artifact and runtime handshake | `phase0_contract_004_console_rest_sse_contract_version_is_pinned_and_enforced`; `contract_version` filtered tests | VALIDATED |
| INV-013 | Custom console log stores must not silently serve v0.4 window queries with legacy oldest-first semantics. | review | `ConsoleLogStore::query_windowed_frames` | `legacy_store_default_rejects_v04_window_queries_loudly`; `scripts/repo-cargo test -p meerkat-mobkit console_aggregator::store::tests -- --nocapture` | VALIDATED |
| INV-014 | `since` snapshots must drain durable backlog beyond 100 bounded pages without falsely reporting exhaustion inside a partially delivered raw page. | review | `/console/timeline/stream` snapshot path and aggregator windowing | `timeline_snapshot_drains_since_backlog_beyond_old_page_budget`; `scripts/repo-cargo test -p meerkat-mobkit timeline_snapshot -- --nocapture` | VALIDATED |
| INV-015 | JSON-RPC timeline replay errors carry recoverable cursor data and are advertised only on `mobkit/console/query_timeline`. | review | `/console/rpc`, SDK error mapping, v0.4 contract artifact | `console_timeline_replay_unavailable_rpc_uses_dedicated_error_code`; `phase0_contract_004_console_rest_sse_contract_version_is_pinned_and_enforced`; Python and TypeScript SDK tests | VALIDATED |
| INV-016 | Browser timeline stream continuation uses `Last-Event-ID` instead of URL `after` while preserving reconnect recovery. | review | Console network adapter and browser e2e mock server | `subscribeTimelineEvents reconnects with the latest aggregate cursor`; `browser global timeline recent seed ok` | VALIDATED |
| INV-017 | Identity recent anchors are useful but bounded: retaining the turn anchor must not exceed the caller's requested result limit. | review | Aggregator identity recent window merge | `identity_recent_anchor_respects_query_limit`; `scripts/repo-cargo test -p meerkat-mobkit identity_recent_anchor_respects_query_limit -- --nocapture` | VALIDATED |
| INV-018 | Provisional recent-page exhaustion must not permanently hide older-history paging after async backfill, including non-empty live-first pages. | review | `ConsoleApp` identity timeline loader | `browser chat pane async backfill older history recovery ok`; `browser chat pane non-empty async backfill older history recovery ok` | VALIDATED |
| INV-019 | Replay-unavailable recovery must replace stale local identity frames before loading the fresh recent tail. | review | `ConsoleApp` stale cursor recovery path | `browser chat pane replay recovery replaces stale local log ok` | VALIDATED |

## Typed But Unwired

- None.

## Phase Evidence

- `2026-05-26T20:59:26Z`, base `e4390bd`, branch `codex/console-timeline-scalability`.
- `cargo test -p meerkat-mobkit console_aggregator::store::tests -- --nocapture`: 7 passed.
- `cargo test -p meerkat-mobkit query_timeline -- --nocapture`: 5 passed before the hidden-gap test was added; full package run later covered the expanded set.
- `cargo test -p meerkat-mobkit timeline_snapshot -- --nocapture`: 3 passed.
- `cargo test -p meerkat-mobkit query_timeline_since_skips_hidden_raw_gaps_with_bounded_paging -- --nocapture`: 1 passed.
- `cargo test -p meerkat-mobkit`: passed, including 227 lib tests, integration tests, and doctests.
- `cargo clippy -p meerkat-mobkit --all-targets -- -D warnings`: passed.
- `npm --prefix console run phase0:types --silent`: 121 JS/type-normalization tests passed.
- `npm --prefix console run e2e:browser --silent`: passed, including `browser global timeline recent seed ok`, `browser chat pane recent first page ok`, and `browser chat pane older history demand paging ok`.
- `git diff --name-only`: changes are limited to MobKit console/backend files and design docs; no Meerkat core/RPC files changed.
- `rg -n "Fugue|fugue|LUC-631|kapellmeister|Kapellmeister" ...`: matches only the design docs' motivating context and examples, not runtime code.
- Adversarial review follow-up fixed visible-cursor skipping, idle-refresh backfill, exact final-page exhaustion, stale SSE replay normalization, SQLite cursor overflow, and older-history scroll preservation.
- `cargo test -p meerkat-mobkit query_timeline_since -- --nocapture`: 3 passed.
- `cargo test -p meerkat-mobkit sqlite_log -- --nocapture`: 7 passed.
- `cargo test -p meerkat-mobkit --lib`: 232 passed.
- `cargo test -p meerkat-mobkit`: passed, including 232 lib tests, integration tests, and doctests.
- `cargo clippy -p meerkat-mobkit --all-targets -- -D warnings`: passed.
- `npm --prefix console run phase0:types --silent`: 122 JS/type-normalization tests passed.
- `npm --prefix console run e2e:browser --silent`: passed, including recent seed, recent chat, older-history paging, and scroll-preservation coverage.
- `git diff --check`: passed.
- Adversarial review follow-up fixed same-stream replay loss after one store page, in-stream lag recovery, future-cursor replay handling, public Rust source compatibility, and contract-version gating.
- `cargo test -p meerkat-mobkit timeline_snapshot -- --nocapture`: 5 passed, including a 250,000-frame recent snapshot, multi-page `since` replay, and future-cursor rejection on an empty/reset store.
- `cargo test -p meerkat-mobkit sqlite_log_queries_250k_sparse_identity_recent_window_with_index -- --nocapture`: 1 passed, including a 250,000-frame SQLite sparse-identity recent query and an `EXPLAIN QUERY PLAN` assertion on `idx_console_frames_identity_cursor`.
- `cargo test -p meerkat-mobkit legacy_timeline_struct_literals_remain_source_compatible -- --nocapture`: 1 passed.
- `cargo test -p meerkat-mobkit phase0_contract_004_console_rest_sse_contract_version_is_pinned_and_enforced -- --nocapture`: 1 passed.
- `cargo test -p meerkat-mobkit contract_version -- --nocapture`: 2 passed.
- `npm --prefix console run phase0:types --silent`: 123 JS/type-normalization tests passed.
- `2026-05-27` fix batch gate: `cargo fmt --check`; `git diff --check`; `scripts/repo-cargo clippy -p meerkat-mobkit --all-targets -- -D warnings`; `scripts/repo-cargo test -p meerkat-mobkit` (240 lib tests plus integration/doctests); `npm --prefix console run phase0:types --silent` (124 tests); `npm --prefix console run e2e:browser --silent`; `PYTHONPATH=sdk/python pytest -q sdk/python/tests` (466 tests); `npm --prefix sdk/typescript run validate --silent` (458 tests).
- `2026-05-27` adversarial review fix batch gate: `cargo fmt --check`; `git diff --check`; `scripts/repo-cargo clippy -p meerkat-mobkit --all-targets -- -D warnings`; `scripts/repo-cargo test -p meerkat-mobkit` (243 lib tests plus integration/doctests); `npm --prefix console run phase0:types --silent` (125 tests); `npm --prefix console run e2e:browser --silent`; `PYTHONPATH=sdk/python pytest -q sdk/python/tests` (466 tests); `npm --prefix sdk/typescript run validate --silent` (458 tests).
- `2026-05-27` frontend review fix batch: `git diff --check`; `npm --prefix console run phase0:types --silent` (125 tests); `npm --prefix console run e2e:browser --silent`, including async empty, async non-empty, and replay-unavailable stale-log recovery browser proofs.
