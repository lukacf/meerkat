# Owner composition review record

The four requested bus reviewers accepted revision 7 as a protocol/source
design. The exact candidate is `owner-composition-proposal.md`, SHA-256
`751503730f93c57ac5a08446034d92ea153db52c3f5b1c2684e00832faac8515`.
The candidate's historical pending-review status is preserved with its bytes.
This record supplies the subsequent disposition.

| Reviewer | UTC verdict time | Disposition |
| --- | --- | --- |
| Meerkat/MobKit lead | 2026-09-30 23:31:19 | Protocol design accepted; generated implementation, physical enforcement and deployment measurements remain required. |
| The operator deployment | 2026-09-30 23:31:33 | Design accepted; synchronous ingress acknowledgment and model witness cost remain operational consequences. |
| The downstream app | 2026-09-30 23:32:24 | Protocol accepted; witness outage prevents model operation, and unresolved external effects need honest recovery. |
| Meerkat Toolkit | 2026-09-30 23:35:57 | No remaining protocol blocker; source-contract findings closed, with all runtime and adoption requirements retained. |

The [operational addendum](owner-composition-operational-addendum.md) records
the resulting availability, batching and deferred-extension decisions. Raw bus
messages are retained in the local evidence archive, outside the repository,
because they contain deployment-specific information. This public record is a
sanitized disposition, not a substitute for exact implementation review.

No reviewer has accepted the complete runtime implementation. The requirements
inventory stays open until actual owners, sinks, recovery and budgets pass on
the exact candidate commits. Pure contracts, proposed transitions and a green
model process with an unreachable witness do not satisfy those requirements.

## Keyed request amendment

All four reviewers accepted the exact [keyed request amendment](owner-composition-keyed-request-amendment.md), SHA-256 `a936b4932d2cf736fbe455decb7becdb3799c8a0aa8823074bb5ad200daab31f`, on 2026-10-01. Its historical proposed/pending wording is preserved in the frozen bytes; this record gives the later disposition.

| Reviewer | UTC verdict time | Disposition |
| --- | --- | --- |
| The operator deployment | 02:11:01 | Bounded protocol accepted; dependent entry now triggers scoped recovery without waiting for a control request. |
| Meerkat/MobKit lead | 02:11:03 | Protocol accepted; no periodic retry, no entry fence held during recovery, separately authenticated phases over one mutation binding. |
| The downstream app | 02:11:12 | Protocol accepted; automatic member-scoped recovery and independent member progress retained. |
| Meerkat Toolkit | 02:19:28 | Bounded protocol source accepted; recovery single-flight lifetime through cancellation and exact phase authentication still require implementation evidence. |

The reviews closed specific design gaps: a realm-wide control mutex, ambiguous background retry behavior, recovery depending on rare control mutations, and phase envelopes conflicting despite binding the same mutation. Generated keyed joins, canonical owner transitions, physical transactions, retention, antirollback custody and actual protected-entry tests remain required. These verdicts do not accept a deployment or the complete ADR implementation.

## Post-fence time clarification

The lead conditionally accepted the explicit distinction between a fresh
post-fence witness decision and its later historical durable reply. The
[time clarification](owner-composition-time-clarification.md) records the
required final release checks, fresh retry observations, uncertainty refusal
and actual cleanup tests. GL4 remains open until those conditions are executed;
no fsync-time expiry or production trusted-clock claim is made.
