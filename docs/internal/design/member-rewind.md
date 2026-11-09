# Same-identity member rewind

Status: draft for review, third revision. This note supplies the design note that the disposition of #2117 asked for before same-identity rewind is built. Same-identity rewind is deferred: it is not part of the current release, and no implementation is expected against this note until it is selected. The note settles the contract. It is docs only: no API, wire type or test changes, and `a_member_session_forks_at_a_message_boundary` in `crates/meerkat-rpc/tests/member_session_fork_at_boundary.rs` stays `#[ignore]`d.

Source baseline: Meerkat `main` at `ad31c1fd31517c0c1908084fae459758808cfd2d`. Paths and names below describe that baseline. This revision restates the contract in the engine's own terms, after the maintainer's review: the rewind is a third rotation target, not a new seat transition.

## Problem

A host that runs an interactive agent as a durable mob member wants "rewind": the user picks an earlier message, and the member's own conversation continues from that boundary. The continuation must have the exact context (the prefix byte for byte, including the copied system prompt), keep the same identity, and record where it came from.

Today none of the member surfaces gives that:

- `mob/fork_member` (`MobHandle::fork_member`) copies the exact prefix, but seats it under a **new** identity. A fork onto the source identity is refused before anything is forked: `admit_fork_member` returns `MobError::MemberAlreadyExists` when `member.identity == source_identity` (pinned over RPC by `mob_fork_member_onto_a_seated_identity_commits_nothing`).
- `session/fork_at` refuses a member's session with `INVALID_PARAMS`, "mob-owned session transcripts cannot be edited through the generic session surface" (`session_fork_at_refuses_a_member_session`). The same refusal covers `session/fork_replace`, `session/rewrite_transcript` and `session/restore_transcript_revision`.
- Re-seating the identity on a fork child's session is refused with `data.kind = resume_session_bound_to_another_member` (`main_cannot_be_reseated_on_another_members_session`): the child's session is durably bound to the child's identity at fork commit, and `crates/meerkat-mob/src/build.rs` refuses a resume whose identity does not match the stored binding.

The consumer is rkat-tui's `/rewind` in attach mode: one durable host-root member `main` per workspace, driven over the daemon. The user picks an earlier message, and `main` continues from that boundary.

## Current owners and APIs

| Owner | What it does today |
| --- | --- |
| `RotationTarget`, `MemberRotationRecord`, `RotationSuccessor` (`crates/meerkat-mob/src/runtime/rotation_intent.rs`) | The two ways MobMachine moves an identity onto its next incarnation: `FreshSession` (`MobHandle::rotate_member`) and `SameSession` (`MobHandle::respawn_member`). The record is stored on the retirement's start row by the internal event codec; a new target or changed field goes under a new stored key with a rollback fixture. |
| `BeginMemberRotation*`, `CommitMemberRotation*`, `RecoverMemberRotationIntent*` (`crates/meerkat-machine-schema/src/catalog/dsl/mob_machine.rs`) | The rotation start (intent recorded, current incarnation `Retiring`), the terminal (one step appending `MemberRetired` and `MemberSpawned` in one batch and seating the successor Dormant; the fresh-session terminal also emits `MemberSessionBindingChanged { old_session_id, new_session_id }`), and the replay that re-records the intent so the restarted mob finishes the rotation itself. The terminal requires the kickoff quiesced. |
| Dormant seat (`crates/meerkat-mob/src/runtime/actor/dormant_seat.rs`) | Seats a successor with no runtime; its first delivery wakes and builds it. Refuses a placed or fork-derived spec (`fork_source`, `fork_job`, forked participant attachment, `fork_build_inheritance`, operator authority ceiling). The same-session successor check refuses fork-derived members and members holding forked participant attachments. |
| `SessionService::fork_durable_session_with_planned_identity` (`crates/meerkat-session/src/persistent.rs`) | A durable fork whose child id the owner reserved in its own record first, so a retried create reaches the same child. Used the same way by forked participants (`crates/meerkat-mob/src/runtime/session_service.rs`). |
| `DurableSessionForkTarget::member_binding` (`meerkat_core`) | The binding written onto a fork child at commit. The durable fork checks the comms name it derives against the source and copies no key pair. |
| `source_session_for_transcript_edit_locked` / `reject_if_archived_session` (`persistent.rs`) | The fork read of a source: refuses a busy source, refuses an archived one. |
| `MemberCreationRecord`, `MemberCreationProvenance::Successor` (`crates/meerkat-mob`) | A member's creation id, kept by every rotation successor (`handle.rs`, `rotate_member` docs). |
| `ControlScope::Retire` (`crates/meerkat-mob/src/runtime/scope_gate.rs`), `admit_member_host_door` (`handle.rs`) | The scope rotations, respawn and retirement require; the door rule that reaches an identity entry's member only with `AdminHost`. |
| `SubmitWork` guards, `ResolveSubmitWorkRejectionRetiringAsMemberNotFound` (`mob_machine.rs`) | `SubmitWork` refuses a `Retiring` incarnation; its rejection answers member-not-found today. `respawn_member` answers a retirement in flight `MobError::LifecycleOperationPending`, retryable. |
| `MemberDeliveryScope`, `JournalFold` (`crates/meerkat-mob/src/runtime/delivery_scope.rs`) | A captured `{generation, fence, session}`; the replay fold that counts every `MemberSpawned` as a new membership. |
| Event router (`crates/meerkat-mob/src/runtime/event_router.rs`), `MemberStreamEpoch` and the per-member forwarder (`crates/meerkat-rpc/src/router.rs`) | The mob-wide router re-attaches a member at stream end or when it becomes Live, independently of the journal poll. The per-member forwarder stamps one epoch per stream, fixed at subscribe or first attach. A member subscription names an exact actor incarnation. |
| `end_member_identity_tool_grants` (`crates/meerkat-mob/src/runtime/actor.rs`) | Ends identity- and session-scoped tool grants when an identity ends. |
| `MemberReset` (`event.rs`) | A legacy row no live path writes; its replay arm survives for old journals. Not a model for new work. |

## Requirements

1. **Exact prefix.** The continued transcript is the source's messages before `message_index`, including the copied system prompt.
2. **Same identity continues.** After the rewind, `main` answers. `mob/member_status` and every member-targeted method resolve `main` to the rewound conversation.
3. **Lineage recorded.** The source session and boundary are durable and readable.
4. **Inherited restrictions.** The member keeps its tool access policy, dispatch admission and instruction policy.
5. **Idle source only.** A member that owes work is refused typed. Nothing is started, forked or rebound.
6. **Durable.** After a restart the member resumes on the rewound conversation; a crash at any point finishes forward.
7. **Redo.** The previous tail stays readable through `session/history`, and the host can continue the member from it again.

## Options

### (A) Re-seat the identity on a prefix fork

Fork into a staging identity, retire `main`, rewrite the child's persisted binding to `main`. Rejected: it needs the binding rewrite that `ResumedIdentityRejection::bound_to_another_member` exists to refuse, spans two identities and three journal facts, and is unnecessary because `DurableSessionForkTarget::member_binding` binds the child to any identity at commit.

### (B) A third rotation target: a committed fork of the identity's own session

The identity rotates onto a durable fork of its own session, born bound to the same identity, through the machinery `rotate_member` and `respawn_member` already use. The contract is below.

### (C) New identity, host follows

`mob/fork_member` and let the host switch to `main-2`. Right for forks; it does not meet "same identity", and identity churn reaches the spawn customizer, grants, wiring, status consumers and the host's conversation model.

### (D) Same-session rewind through the transcript revision graph

A mob-owned `session/rewrite_transcript` that cuts `[message_index, end)`; redo is `session/restore_transcript_revision`. Its strength is real: everything keyed to the session stays in place, with no seat change and no new session id. Its costs, as the maintainer assessed them, are open-ended:

- a mob-owned transcript edit authority that does not exist (there is no `RewriteTranscript` scope: `crates/meerkat-mob/src/control_policy.rs`);
- mob-scoped revision reads, since the generic ones refuse member sessions (`router.rs`);
- a reconciliation of state keyed to transcript positions: job outcomes owed to the session, memory projections, instruction activation rows in the cut tail, and members forked at later boundaries.

### Recommendation

**(B), in rotation shape.** The maintainers lean the same way. (B)'s remaining costs (items 2 to 5 of the review: the dormant seat, comms identity, the fence on the old session, owed work) each map onto something that exists, and (B) leaves the old conversation intact as a whole session for history and redo.

## Proposed contract for (B)

**Method:** `mob/rewind_member`, replacing the ignored test's stand-in call.

```
MobRewindMemberParams  { mob_id, agent_identity, message_index, source_session_id? }
MobRewindMemberResult  { agent_identity, member_ref, session_id, source_session_id, message_count, generation }
```

`message_index` has its `session/fork_at` meaning. The source is the member's current session unless `source_session_id` names a session of the identity's rewind chain (see **Redo**). There is no `inherited_instructions` field: `mob/fork_member` deliberately has none, and a rewind is the same member continuing, so the fork copies the source's system prompt and instruction state as they stand, and the successor's builds apply the member's own instruction policy.

**Owner and authority:** `MobHandle::rewind_member` in `crates/meerkat-mob/src/runtime/handle.rs`, admitted under `ControlScope::Retire` (a rewind retires the current incarnation, as rotations and respawn do), through `admit_member_host_door(Retire, identity)`. Under that door rule an identity entry's member can be rewound only by a principal that also holds `AdminHost`, which its host does; any other principal is refused `MobError::IdentityMember`. No agent-facing tool reaches it: it is not in the mob tool catalog, and the agent lane never acts on an identity entry's member.

**Rotation target.** `RotationTarget::CommittedFork` joins `FreshSession` and `SameSession`. Its record, `MemberForkRotationRecord { planned_session_id, source_session_id, message_count, successor }`, is stored on the start row under a **new key** (`fork_rotation`), never in the existing record's key, because a release that supports rollback to 0.9.8 must not write a record 0.9.8 cannot decode (`rotation_intent.rs`). A rollback fixture pins what the older reader does with it: a start row it reads as a plain retirement, a terminal pair it reads as an ordinary retirement and spawn (the successor's `rewound_from` is ignored).

### Sequence

1. **Admission** (no state change): the scope, the source resolution and its lineage check (**Redo**), the boundary check (`message_index` is a message boundary at or before the source's committed end, read from the source, which is idle), and the refusals at Begin in the owed-work table. A refusal here writes nothing.
2. **Begin** = the rotation start, one actor step: stage the mob-owned run-start hold on the current session `S0` (`RunStartHoldReason`, a new mob-owned reason, as `HostPreparation` is), re-check `S0` idle **under the hold** (no run in flight, no non-terminal input in its ledger), then `BeginMemberRotation` with target `CommittedFork`: record the intent (`member_rotation_targets`, the planned child session id) and mark the incarnation `Retiring`, writing the start row `MemberRetirementStarted` with the record. If the re-check fails, release the hold and refuse; nothing is written. The pending state is the rotation intent plus `Retiring`; there is no separate `member_rewind_pending` field.
3. **Fork** (outside the actor): `fork_durable_session_with_planned_identity` of the source at `message_count`, into the planned id, with the member's own `MobMemberBinding` as target and its tool access policy. The child `S1` is born bound to `main`. A retried fork reaches the same `S1`, so no orphan child is left. Then carry the comms key pair (see **Comms identity**).
4. **Retire `S0`'s incarnation**: release its runtime (it is idle), settle `S0`'s owed compaction projections cold as the fresh-session rotation does (`settle_member_owed_compaction_projections`), and archive `S0` (disposal `Archived`, as a retirement does). The archive ends any input still in `S0`'s ledger typed; none can have run, because the hold stood.
5. **Commit** = the rotation terminal, one machine step: append `MemberRetired` and `MemberSpawned` in one batch; the successor's `MemberSpawned` carries `rewound_from { source_session_id, message_count }` and keeps the member's creation id (`MemberCreationProvenance::Successor`); seat the successor **Dormant** on `S1` at the next generation and fence; move the binding; append the chain entry (**Redo**); emit `MemberSessionBindingChanged { old_session_id: S0, new_session_id: S1 }`.
6. **Recovery**: replay re-records the intent from the start row (`RecoverMemberRotationIntent` gains a `CommittedFork` arm) and the mob finishes forward: step 3 again (the planned id makes it idempotent; the key copy writes the same bytes), step 4 again (an archive already complete is observed, as `retirement_archive_already_complete` does today), then step 5. A rotation started is never rolled back; every refusal that could stop it is decided at admission or Begin, before the start row. A transient store failure after the start leaves the rotation stuck and re-drivable (`redrive_retirement`), as other rotations are.

**No Build step.** Both existing rotations build nothing: the successor is seated Dormant and its first delivery builds it. The rewind does the same, so a dormant member is not made live as a side effect, no second runtime ever claims the comms name while the old one holds it (the in-process registry would refuse it, `crates/meerkat-comms/src/inproc.rs`), and there is no `PendingProvision` to cancel: the wake path carries its own guard and rollback. The first build after the rewind resolves the dispatch admission and the per-spawn overlay through the spawn customizer, as for any rebuild (`RotationSuccessor` records neither).

**Fork-derived members cannot be rewound** in this contract. A dormant seat refuses a spec with `fork_source`, `fork_job` or a forked participant attachment, and a fork-derived member has no same-session successor. Begin refuses such a member `MobError::UnsupportedForMode` ("a fork-derived member cannot be rewound"), before anything starts, until a dormant seat can carry fork lineage. rkat-tui's `main` is host-root and unaffected.

**Autonomous members cannot be rewound.** Begin refuses any member whose runtime mode is not `TurnDriven` (`UnsupportedForMode`). An `AutonomousHost` member's loop and kickoff obligations are keyed to its session's conversation, and a rewound conversation would need a rule for re-owing them that this contract does not define.

### Comms identity

Choice: **carry the old session's key pair into the new session.** The comms key pair is stored per session (`<root>/<session>/identity.key`, `crates/meerkat-mob/src/runtime/tests/seat_comms_key.rs`), and the durable fork copies none, so without a carry `S1` gets a new key and endpoint while peers trust the old one. The two existing answers each lose something: a fresh-session successor records a new endpoint and drops the member's edges, re-wiring only declared ones (explicit `wire()` edges and external peer edges are lost); a same-session successor under its own role keeps key, endpoint and edges. The rewind takes the second answer's outcome: step 3 copies `S0`'s key pair to `S1`'s key path, the record names the member's existing endpoint, and the terminal keeps the member's endpoint, peer id and edges, as `CommitMemberRotationSameSession` does under the member's own role. This is safe only because `S0` is fenced: archived in step 4, so no runtime can come up on it with the same key, and its runtime was released before the successor can wake. A member whose session has no key pair (network comms before the first wake) carries none and owes its endpoint as a same-session successor does.

### Fence on the old session

`turn/start` does no owner check (`router.rs`), and a session whose mob owner no longer binds it "proceeds as before" (`crates/meerkat-rpc/src/session_runtime.rs`); session-targeted schedules (`resumable_session`, `crates/meerkat-schedule/src/tools.rs`) resume such a session too. Only archived sessions are refused. Step 4 therefore archives `S0` as part of the rotation, so after the terminal no door can run it, its ledger runs nothing, and its session-scoped tool grants end with it. `session/history` still reads archived member sessions (`router.rs`, "archived mob-owned sessions should still route to session/history"), so requirement 7's read holds. Redo forks from an archived session, so the fork read must accept an archived source **for lineage-authorized sources only**: the `reject_if_archived_session` refusal in `source_session_for_transcript_edit_locked` is lifted on a mob-internal fork entry that carries the chain proof; the generic fork surfaces keep refusing archived sources.

### Owed work outside the runtime ledger

The idle re-check covers the runtime and its input ledger. Everything else is decided at Begin:

| Owed work | Decision |
| --- | --- |
| A run in flight, including one waiting on an approval decision | Refuse: `SESSION_BUSY`, `data.kind = fork_source_unavailable`, `cause = running`. A run blocked on an approval is a run in flight. |
| Detached and `fork_off` jobs whose outcomes are owed to `S0` (`ForkJobTerminalEvent::owner_session_id`, `detached_jobs` on `mob/member_status`) | Refuse, the same busy refusal (retryable once the jobs end). Carrying would need a re-owning of outcomes to `S1` whose prefix never saw the dispatch. |
| Owed compaction projections of `S0` | Carry by settling: step 4 settles them cold before the archive, as the fresh-session rotation does, so the finalization reaches the host. |
| An owed kickoff | Refuse: a turn-driven member that still owes an initial message is refused `SESSION_BUSY` / `fork_source_unavailable` / `running`, since the rotation terminal requires the kickoff quiesced. (Autonomous members are refused before this, by the `TurnDriven` rule.) |
| Forked participant attachments and live delegation sources | Refuse `UnsupportedForMode`, as respawn does. |
| Members forked from this one (a seated member whose `fork_source` names this identity and `S0`) | Refuse `UnsupportedForMode`, naming them: their overlay link follows the source only while it is bound to `S0` (`follow_fork_link`, `crates/meerkat-mob/src/runtime/fork_build.rs`), so a rewind would silently break it. The host retires them first. |
| Scheduled resumes of `S0` | End typed: after the archive they are refused `SessionArchived`. |

### Admission of turns and member work

`mob/turn_start` keeps its carrier (`turn/start`, which answers the turn's result and carries per-turn options); the generation check goes into the turn start's own admission. The handler resolves its target and checks the member in one actor-gated step: the identity's current generation, fence and session, and not `Retiring`. The run it then starts on that session meets the run-start hold Begin staged. So a turn either started before the hold (Begin's re-check sees it and refuses the rewind busy), or meets the hold or a `Retiring` member (refused `MobError::LifecycleOperationPending`, retryable, the kind respawn already uses for a retirement in flight), or, resolved before the commit and started after it, meets an archived `S0` (refused `SessionArchived`). Nothing is retargeted. Today a `Retiring` member's submitted work is answered member-not-found (`ResolveSubmitWorkRejectionRetiringAsMemberNotFound`); for a `CommittedFork` rotation the rejection answers `LifecycleOperationPending` instead, so `mob/member_send` and other member-targeted admissions get the retryable kind. No new `data.kind` is added.

### Streams

The notice is the existing `MemberSessionBindingChanged`, which the fresh-session terminal already emits with the old and new session; the `CommittedFork` terminal emits the same. It is projected onto `mob/stream_open` as one typed mob-level item, not an `AgentEvent` variant, so one notice serves every session change of a member.

- **Mob-wide.** The router forwards the item when the terminal commits. It attaches a member's stream only after an incarnation check: the stream it attaches must belong to the incarnation MobMachine binds now, so new-session events never arrive before the notice under the old runtime id.
- **Per-member.** The per-member follow rule applies to **every rotation** of the member, not only rewinds: on the binding change the stream ends `S0`'s source after its last event, yields the notice, and attaches the successor at its next wake (it is Dormant), on the same `stream_id`. The epoch change happens in the RPC forwarder as well: `MemberStreamEpoch` is reset at the notice and set again at the successor's attach, so events from `S1` carry `S1`'s epoch.
- **Reattach.** No producer writes the notice into `S1`'s retained sequence, and nothing is retained across a restart, so a client reattaching with an old-epoch cursor does not replay it. Such a client, and one with no open stream, learns of the switch from `mob/member_status` (`current_session_id`, generation, `rewound_from`), or from `mob/events`.

### Redo

After a rewind the current session holds only the prefix. Redo is `mob/rewind_member` with `source_session_id` set to a session of the identity's rewind chain, at its committed end (full redo) or any earlier boundary (partial redo). The same sequence runs: the hold and idle check are on the current session, which is the one replaced; the source is an archived session, read through the lineage-authorized fork read.

- **The chain is machine state.** `member_rewind_chains[identity] = { creation_id, entries: [{ session_id, source_session_id, message_count, generation }] }`, appended by the `CommittedFork` terminal and restored on replay from its rows. The start step checks a requested source against it: it must be an entry's `session_id` or `source_session_id`, not the current session.
- **The boundary is the member's creation id**, not `MemberSpawned`. Rotation successors keep the creation id, so a respawn, role move, runtime-mode restamp, reset or the rewind's own terminal keeps the chain. A retirement with no successor ends it, and a member created anew (a new root, or an `Unproven` successor with a new id) starts empty, so a predecessor's sessions, though bound to the same identity, are refused `INVALID_PARAMS`, `data.kind = rewind_source_not_in_lineage`.
- **Status reads never fold the journal.** `mob/member_status` carries `rewind_lineage`, newest first, from the roster's projection of the machine state.
- Re-seating a retired session as-is is not offered: it would bind one session to the identity in two generations. Every redo produces a fresh session.

### What carries over

The successor keeps the member's identity, creation id, profile, runtime mode, labels, spawner, wiring exemption, endpoint, edges and identity-scoped tool grants (subject `member(mob, incarnation, identity)`). `S0`'s session-scoped grants end with its archive. The roster's `fork_source` is unchanged and is never set to the member's own session; lineage lives only in `rewound_from` and the chain.

### Typed refusals

| Case | Refusal (nothing started unless stated) |
| --- | --- |
| Owes work (table above) | `SESSION_BUSY` / `fork_source_unavailable` / `running`, or `UnsupportedForMode` |
| Rotation already in flight, or a turn meeting a pending rewind | `LifecycleOperationPending`, retryable |
| Fork-derived, placed, not `TurnDriven`, or not runtime-backed with durable sessions | `UnsupportedForMode` |
| Seated without a session | `fork_source_unavailable`, `cause = no_session` |
| Unknown or retired identity | `MobError::MemberNotFound` |
| Principal lacks `Retire`, or `AdminHost` for an identity entry | `ScopeDenied` / `IdentityMember` |
| `source_session_id` not in the chain | `INVALID_PARAMS`, `rewind_source_not_in_lineage` |
| `message_index` not a boundary of the source | the durable fork's refusal, mapped as `session/fork_at` maps it |

**Mapping onto the ignored test:** the stand-in becomes `mob/rewind_member`. Its assertions hold, except that lineage reads `rewound_from` instead of `fork_source`.

## Acceptance tests

In `crates/meerkat-rpc/tests/member_session_fork_at_boundary.rs` unless noted:

- `a_member_session_forks_at_a_message_boundary`, un-ignored on `mob/rewind_member`; `main` is Dormant on `S1` after the call and its next turn builds it there.
- Rotation rows (`crates/meerkat-mob/src/runtime/tests/rotation_intent_rows.rs` siblings): the start row carries the `fork_rotation` record under its own key; the terminal appends `MemberRetired` and `MemberSpawned` in one batch with `rewound_from` and the kept creation id; a rollback fixture decodes both with the 0.9.8 reader.
- Restart finishes forward: crash after the start row, after the fork, and after the archive; each restart finishes the rotation on the same planned `S1`, with no second child.
- Archived old session: after the rewind `S0` is archived, `turn/start` and a session-targeted schedule on it are refused, and `session/history` still reads it.
- Wired peer: a peer wired explicitly with `wire()` sends to `main` before and after the rewind, and the second message lands on `S1` with the same peer id.
- Owed-work refusals: a run waiting on an approval decision, a pending detached job, a queued input, an owed kickoff, a forked participant attachment and a member forked from `main` each refuse the rewind with nothing started.
- Turn admission races (`crates/meerkat-mob` runtime tests, paused after Begin): a `mob/turn_start` and a `mob/member_send` are refused `LifecycleOperationPending`; one resolved before the commit and started after it is refused on the archived `S0`; nothing ran on `S0`.
- Creation-id boundary: rewind, respawn, rewind again; redo from the first chain entry succeeds. Retire, recreate, and redo from the old chain is refused `rewind_source_not_in_lineage`; `rewind_lineage` is empty. The same after a restart.
- Streams, both forms: a subscriber sees `MemberSessionBindingChanged` before any `S1` event; the per-member stream stays on its `stream_id` and carries `S1`'s epoch; the same holds for a `respawn_member` and a `rotate_member` of the member; a reattach with an old-epoch cursor learns the switch from `mob/member_status`.
- Restrictions: the deny list and the host dispatch admission (#2112 lineage case) hold on `S1`, and a rebuild emits no fork-source warning.

## Consumers

- rkat-tui `/rewind` in attach mode: requirements 1 to 7.
- An interactive host runtime that offers rewind on a durable member.
- `rkat mob rewind <member> --at <index>` in the CLI.
