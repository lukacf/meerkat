# ADR-001 implementation acceptance plan

> **Acceptance scope revised, 2026-10-01.** The owner approved the
> [local governed default](../adr-001-local-governed-default.md). Its detailed
> r8 contract has four-reviewer design acceptance. The previous high-assurance
> budgets, witness requirements and restricted first profile below are historical,
> not current default gates.
> New target: no added hot-path RTT/fsync, under 1 ms p99 added authorization
> per tool/model dispatch and at most 10 percent turn overhead, with broad
> provider, streaming, tool, compaction and live coverage.

Status: implementation in progress. This plan does not claim runtime support.
Luka's objective is full ADR implementation, adversarial acceptance by the GCP
Meerkat/MobKit lead, Homecore, OB3 and Toolkit reviewers, and PRs with green CI.
The first vertical slice is an implementation gate, not completion.

Read [ADR-001](../adr-001-runtime-security.md) and
[governed profiles](../governed-deployment-profiles.md) through the accepted
[local-default amendment](../adr-001-local-governed-default.md) and
[confinement and consent addendum](../adr-001-confinement-and-consent.md).
The earlier [r6 review](../adr-001-runtime-security-review.md) does not restore
parked witness/time requirements to the default path. Full ABAC and sandbox
coverage remain required; deployment opt-in
does not make sandbox implementation optional. No design verdict counts as
implementation acceptance, and missing coverage cannot be relabeled out of scope.

Use the [implementation checkpoint](implementation-progress.md) as the single
record of current source identities, publication and CI, executed test inventories,
performance measurements and queued work. It distinguishes current evidence
from historical failures, local candidates and unexecuted proposals; this plan
keeps the acceptance scope and measurement rules.

Qualified restart coverage includes 16 default native-loop cases, including
completed-turn SQLite reconstruction in a new process followed by fresh governed
work. The cold-reopen fixture accepts a historical claimed association through its
current ingress before the current grant owner refuses it, with no effect or
durable input entry. These
direct acceptance results do not imply a CI exact-selector count beyond the
recorded job inventory. Interrupted recovery and persistent administration remain
open. Fixed commissioned JSONL acceptance and Console feedback/access projection
are separate qualifications, not integrated native-plus-Console activation.

Luka rejected the former 6-7-hour representative allocation. Every benchmark
attempt must finish in less than 20 minutes total, including setup, warmup,
measured work, all oracles, cleanup and output. The approved representative
profile is fixed W20/N32 in all six existing depth/workload cells, with no interim
analysis. Six simultaneous paired Fieller mean-ratio intervals use family alpha
.05, conditional on stationary, independent, approximately bivariate-normal
opposite-order blocks; exhausted or incomplete attempts are UNCERTAIN.
Fresh-admission cells measure whole turns; continuing intervals and summed direct
calls remain separate units. This mean-only profile supplies no small-N p99
acceptance. The implementation lead owns measurement and the GCP resource owner
explicitly clears the host before START. The under-1-ms p99 added authorization
per-operation and at-most-10-percent representative-turn targets remain open with
full required coverage. Added whole-turn p99 is not an individual-operation
measurement. Profiling diagnostics and build/correctness checks do not replace
uninstrumented cost results or establish the full acceptance gates.

## Historical baselines and isolation

- Meerkat implementation base: main `df1fae188`, integrated into isolated branch
  `codex/security-authorization-adr` at `b4ecf8a10` after the reviewed-document
  checkpoint `f64890fe9`.
- MobKit inspected base: `e2795b5dbbabe224e2e912256d7b99e480b5c9c9`.
- Elephant inspected base: `1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a`.
- Existing release branches and production deployments are outside this change.
  The lead requested PRs to main, outside the 0.8.50 release batch.
- Runtime implementation is opt-in through a governed Cargo feature. The
  trusted-embedded profile and existing wasm32 builds retain their contract.

The core evidence is actual sink calls and bytes, persisted authority, owner
transitions, and failure/recovery behavior. A test asserting a returned refusal
without asserting sink non-entry is insufficient. A ledger or receipt cannot
be a second work-execution authority.

## Sequence and ownership

1. Complete the existing-owner tracer through public ingress, durable admission,
   a real process restart, actual delegation, model context and output delivery.
   Record absent capabilities without using a prompt, label or side map to
   manufacture authority. Toolkit's C1 prototype is evidence-only and must not
   become implementation by copying its observations into a new authority.
2. Prove principal/domain mappings and participating-authority composition.
   Model revoke/entry and receipt/recovery races in the existing catalog and
   TLA+ path. Coordinate MeerkatMachine edits with the release lead's active
   live-session work before changing its shared catalog.
3. Implement feature-owned contract, policy/grant and receipt semantics plus
   actual existing-owner enforcement. Preserve domain policy ownership and
   independently versioned wire/conformance contracts with Elephant.
4. Implement and execute the governed end-to-end slice, then cover every other
   required operation, transport, store, sink and reachable deployment behavior.
   Unsupported initial profiles refuse before effects; this does not remove
   the wider ADR obligations or its declared future-profile boundaries.
5. Review source and evidence adversarially; resolve every material finding.
   Ask all four bus reviewers to accept exact implementation commits and tests.
   Create independently green PRs to main, declare source breaks and migration,
   run local WASM and semantic/model gates, and verify actual PR CI state.

## Historical high-assurance numeric budgets

These thresholds are declared before runtime implementation. They are acceptance
criteria, not measurements or claims that the current code passes. Compare the
same deterministic provider, tool, payloads, backend and concurrency against
trusted-embedded operation on the same host. Report absolute latency as well as
the difference, sample count, receipt sizes, dependency count and store settings.
Network service latency must be reported separately, never hidden in the delta.

| Workload | Acceptance bound |
| --- | --- |
| Local protected tool entry, up to 1,000 dependencies, durable SSD receipt store | Added p50 <= 2 ms and p95 <= 10 ms at 16 concurrent dispatches in one turn. |
| Model-attempt admission on the same context/dependency set | Added p50 <= 5 ms and p95 <= 25 ms before provider entry. |
| Representative turn: one model attempt plus 10 tool calls | Added end-to-end wall time <= 10 percent of identical trusted-embedded workload. |
| Model attempt with 10,000 dependencies | Added p95 <= 50 ms; above the supported bound, typed refusal or authorized context reset. Measure steady-state incremental validation across 100, 1,000 and 10,000 dependencies with fixed changed-set size. |
| Security memory overhead | <= 256 KiB resident fixed overhead per active session and <= 1 KiB per retained dependency, including indexes; measure growth after completed attempts are reclaimed. |
| Durable pre-entry receipt append at 16 concurrent independent attempts | At least 200 distinct receipts/second; group commit retains each identity. |
| Saturation at twice the rated receipt rate | Bounded queue of at most 256 attempts per authority; typed backpressure/refusal within 100 ms p95, no unbounded queues, lost evidence or unauthorized fallback. |
| Cold boot and resume of an 18-member roster with existing stored work | Added p95 <= max(10 seconds, 10 percent of trusted-embedded baseline); measure total as well as per-member cost. |
| Same local entry and model workloads under active swap/memory pressure | Added p95 <= 100 ms with no lost evidence or silent profile downgrade; report actual pressure and baseline latency. |
| Receipt retention workload | At 100,000 attempts/day, average combined decision/entry/outcome records <= 8 KiB/attempt before declared archival/compression, <= 24 GiB for 30 days. Include index/WAL amplification separately and prove configured storage quota refuses new protected work before losing custody. |
| Reserved alarm evidence | Separate bounded durable capacity and explicit current mandate; test ordinary store exhaustion. No unaudited fallback, guaranteed life-safety availability, or stale snapshot mandate. |
| Added bounded security TLC witnesses | <= 120 seconds on the CI runner; added routine CI work <= 3 minutes within the lead's approximately 20-minute PR budget. |

Minimum advertised host class for the first local governed profile: 2 vCPU,
4 GiB RAM and SSD storage. Measure that class explicitly with its OS, filesystem,
CPU and memory limits; a faster workstation measurement is not a substitute.
The local development host is an Apple M5 Max, 18 cores, 128 GiB RAM. Homecore's
reported host is M2 Pro, 10 cores, 16 GiB, sharing memory with Home Assistant's
VM, Elephant and the gateway; contention and active swap are required benchmark
conditions. No tests run against Homecore or OB3 production.

Receipt retention and quota are authorized policy, not permission to erase
required recovery custody. Active attempts, unresolved appends and pending
settlement retain necessary durable evidence. Archival integrity anchors and
current antirollback authority must survive outside restored application state.

### Optional clock-profile budget failure, 2026-10-01

The accepted `LinuxChronyNtsV1` producer proposal r5
(`5ebb51becc516783aa178305d0dc1873056cc0b48e3f45a1ef751ddd4526218d`)
has a known product limitation: its conservative host-rate bound leaves samples
usable for seconds or tens of seconds, and a fresh stateless authenticated NTS
acquisition takes multiple seconds. Calls after a sufficiently long idle period
therefore miss the 5 ms p50 / 25 ms p95 model-entry budget and the representative
turn overhead budget. This is a known design failure against those budgets,
not merely a missing benchmark. The numeric requirements above remain unchanged.

Demand-triggered early refresh is selected to improve active conversations: an
actual current observation below a declared remaining-validity threshold may
request one acquisition outside all owner fences, through the same owned
single-flight worker, while the previous sample remains valid. Failed refresh
preserves a still-valid previous sample. There is no timer, polling daemon,
detached task or reusable observation permission. The final sink still checks
the selected sample at its own decision point. This change remains unexecuted
and does not resolve latency after idle periods. Full acceptance requires a
tighter, actually qualified clock/host profile and measured compliance; neither
source review nor early refresh closes this item.

## Required evidence packages

- Requirement inventory with exact requirement-to-source/test mapping and
  explicit missing/incomplete status, including each Core/Profile table row.
- Native causal tracer, real process restart and sink transcripts.
- Protocol vectors and Elephant differential results.
- Race/fault tests and TLC evidence from the same catalog used by production.
- Latency, throughput, restart and pressure measurements on both host classes.
- Feature-off, wasm32, schema/codegen, source-break and store-conformance checks.
- All four final bus verdicts bound to exact implementation revisions.
- PR URLs and verified green CI for the final reviewed heads.
