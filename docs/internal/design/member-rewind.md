# Same-identity member rewind

Status: draft for review, fourth revision. This note supplies the design note that the disposition of #2117 asked for before same-identity rewind is built. Same-identity rewind is deferred: it is not part of the current release, and no implementation is expected against this note until it is selected. The note settles the contract. It is docs only: no API, wire type or test changes, and `a_member_session_forks_at_a_message_boundary` in `crates/meerkat-rpc/tests/member_session_fork_at_boundary.rs` stays `#[ignore]`d.

Source baseline: Meerkat `main` at `ad31c1fd31517c0c1908084fae459758808cfd2d`. Paths and names below describe that baseline. The third revision restated the contract in the engine's own terms: the rewind is a third rotation target, not a new seat transition. This revision closes the maintainer's second review: a fork preflight at Begin and a defined end for a failure after the start row, the fork's instruction policy by name, recovery that observes the planned child instead of forking again, the turn window, an incarnation-change stream item with its wire shape, a redo boundary drawn by terminal kind instead of creation id, and the tool grants, session read and key file of the archived session.

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
| `BeginMemberRotation*`, `CommitMemberRotation*`, `RecoverMemberRotationIntent*` (`crates/meerkat-machine-schema/src/catalog/dsl/mob_machine.rs`) | The rotation start (intent recorded, current incarnation `Retiring`), the terminals (one step appending `MemberRetired` and `MemberSpawned` in one batch and seating the successor Dormant), and the replay that re-records the intent so the restarted mob finishes the rotation itself. The fresh-session terminals (`CommitMemberRotationLive`, `…Retired`, and their `…Kickoff` forms) seat the successor on an **unborn** session (`member_seat_session_unborn`, `unborn_seat_sessions`) that its first wake creates, and emit `MemberSessionBindingChanged { old_session_id, new_session_id }`; the same-session terminals (`CommitMemberRotationSameSession`, `…SameSessionKickoff`) emit no binding change. The terminal requires the kickoff quiesced. |
| `MemberSessionBindingChanged` (`mob_machine.rs`) | Emitted by nineteen transitions: spawns, seats, retirements, destroys and the fresh-session rotation terminals. Never by a same-session terminal, so a respawn produces none. |
| Dormant seat (`crates/meerkat-mob/src/runtime/actor/dormant_seat.rs`) | Seats a successor with no runtime; its first delivery wakes and builds it. Refuses a placed or fork-derived spec (`fork_source`, `fork_job`, forked participant attachment, `fork_build_inheritance`, operator authority ceiling). The same-session successor check refuses fork-derived members and members holding forked participant attachments. |
| `SessionService::fork_durable_session_with_planned_identity` (`crates/meerkat-session/src/persistent.rs`) | A durable fork whose child id the owner reserved in its own record first, so a retried create reaches the same child. Its refusals: the busy guard (`quiescent_fork_mutation_guard`), a pending input boundary (`refuse_quiescent_fork_of_pending_boundary`), a split tool group (`fork_at_complete_boundary`), blob verification (`preflight_transcript_fork_blobs`), the instruction policy (`apply_fork_instruction_policies`), and the binding and comms-name checks on the child before commit. |
| `ForkInstructionPolicy` (`crates/meerkat-core/src/session/instruction_activation.rs`) | `RequireExplicitActivation`, `ReactivateInherited`, `Refuse`. `ReactivateInherited` re-activates in the child, before it commits, every key effective in the source at the fork boundary; it refuses a head whose origin is not the source (`NotEffectiveInSource`), a head whose body a compaction retired (`HeadBodyRetired`), and the lineage body budget. |
| Planned-fork user (`crates/meerkat-mob/src/forked_participant/service.rs`), mob fork seam (`crates/meerkat-mob/src/runtime/session_service.rs`) | Before forking, the forked participant observes the planned child (`planned_child_evidence`: prefix count, prefix digest, execution evidence) and proves it against its record (`verify_planned_child`); a conflict archives the child and fails the activation. The seam `fork_planned_child` passes no `DurableSessionForkTarget`, so its child carries no member binding. |
| `DurableSessionForkTarget::member_binding` (`meerkat_core`) | The binding written onto a fork child at commit. The durable fork checks the comms name it derives against the persisted one and copies no key pair. |
| `source_session_for_transcript_edit_locked`, the session read (`persistent.rs`), `reject_if_archived_session` | The fork read and the session read of a source. Both refuse an archived session. |
| `reject_archived_persisted_session_without_live` (`crates/meerkat-rpc/src/session_runtime.rs`) | Answers `SESSION_NOT_FOUND` for an archived session that `turn/start` names. |
| `MemberCreationRecord`, `MemberCreationProvenance::Successor` (`crates/meerkat-mob`) | A member's creation id, kept by every rotation successor, including the fresh-session rotation a host uses to reset a member or to delete and recreate an identity (`handle.rs`, `rotate_member` docs). |
| `identity_entry_archived_sessions` (`crates/meerkat-mob/src/machines/mob_machine.rs`, `identity/created.rs`) | The session an archived identity entry's unarchive seats it on again. |
| `ControlScope::Retire` (`crates/meerkat-mob/src/runtime/scope_gate.rs`), `admit_member_host_door` (`handle.rs`) | The scope rotations, respawn and retirement require; the door rule that reaches an identity entry's member only with `AdminHost`. |
| `SubmitWork` guards, `ResolveSubmitWorkRejectionRetiringAsMemberNotFound` (`mob_machine.rs`) | `SubmitWork` refuses a `Retiring` incarnation; its rejection answers member-not-found today. `respawn_member` answers a retirement in flight `MobError::LifecycleOperationPending`, retryable. |
| Run-start hold (`RunStartHoldReason`, `HostPreparation`) | Holds run starts on a session. It does not refuse input: input admitted under it is queued in the session's ledger. |
| `MemberDeliveryScope`, `JournalFold` (`crates/meerkat-mob/src/runtime/delivery_scope.rs`) | A captured `{generation, fence, session}`; the replay fold that counts every `MemberSpawned` as a new membership. |
| Event router (`crates/meerkat-mob/src/runtime/event_router.rs`), `MemberStreamEpoch`, the per-member forwarder and `emit_mob_stream_event` (`crates/meerkat-rpc/src/router.rs`) | The mob-wide router re-attaches a member at stream end or when it becomes Live. The per-member forwarder stamps one epoch per stream. `mob/stream_event` carries `{ stream_id, sequence, epoch?, event }`, where `event` is an `AttributedEvent` on mob-wide streams and an `EventEnvelope<AgentEvent>` on per-member streams. |
| `end_member_identity_tool_grants` (`crates/meerkat-mob/src/runtime/actor.rs`) | When an identity ends, ends its identity-scoped grants and the session-scoped grants of the sessions MobMachine held for it then. An archive ends no grant. |
| `MemberReset` (`event.rs`) | A legacy row no live path writes; its replay arm survives for old journals. Not a model for new work. |

## Requirements

1. **Exact prefix.** The continued transcript is the source's messages before `message_index`, including the copied system prompt.
2. **Same identity continues.** After the rewind, `main` answers. `mob/member_status` and every member-targeted method resolve `main` to the rewound conversation.
3. **Lineage recorded.** The source session and boundary are durable and readable.
4. **Inherited restrictions.** The member keeps its tool access policy, dispatch admission and instruction policy.
5. **Idle source only.** A member that owes work is refused typed. Nothing is started, forked or rebound.
6. **Durable.** After a restart the member resumes on the rewound conversation; a crash at any point finishes the rotation, forward or at its defined failure end.
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

**(B), in rotation shape.** The maintainers have settled (B) as the direction. (B)'s costs (the dormant seat, comms identity, the fence on the old session, owed work) each map onto something that exists, and (B) leaves the old conversation intact as a whole session for history and redo.

## Proposed contract for (B)

**Method:** `mob/rewind_member`, replacing the ignored test's stand-in call.

```
MobRewindMemberParams  { mob_id, agent_identity, message_index, source_session_id? }
MobRewindMemberResult  { agent_identity, member_ref, session_id, source_session_id, message_count, generation }
```

`message_index` has its `session/fork_at` meaning. The source is the member's current session unless `source_session_id` names a session of the identity's rewind chain (see **Redo**); the result names the source the boundary was resolved to (see **Source resolution**). There is no `inherited_instructions` field: `mob/fork_member` deliberately has none. A rewind is the same member continuing, so the fork runs under **`ForkInstructionPolicy::ReactivateInherited`**, for activated keys and for skill loads alike: the child re-activates, before it commits, every key that was effective in the source **at the boundary**, not at the source's end. Keys activated in the cut tail are not carried; the copied system prompt is the prefix's. The successor's builds then apply the member's own instruction policy as for any rebuild.

**Owner and authority:** `MobHandle::rewind_member` in `crates/meerkat-mob/src/runtime/handle.rs`, admitted under `ControlScope::Retire` (a rewind retires the current incarnation, as rotations and respawn do), through `admit_member_host_door(Retire, identity)`. Under that door rule an identity entry's member can be rewound only by a principal that also holds `AdminHost`, which its host does; any other principal is refused `MobError::IdentityMember`. No agent-facing tool reaches it: it is not in the mob tool catalog, and the agent lane never acts on an identity entry's member.

**Rotation target.** `RotationTarget::CommittedFork` joins `FreshSession` and `SameSession`. Its record, `MemberForkRotationRecord { planned_session_id, source_session_id, message_count, expected_prefix, successor }`, is stored on the start row under a **new key** (`fork_rotation`), never in the existing record's key, because a release that supports rollback to 0.9.8 must not write a record 0.9.8 cannot decode (`rotation_intent.rs`). `expected_prefix` is the evidence the planned child must show: `{ prefix_message_count, prefix_digest, tool_access_policy, auth_binding, realm_id }`, the fields `PlannedChildEvidence` and `verify_planned_child` already compare, with the digest the one `transcript_prefix_digest(message_count)` gives on the source. A rollback fixture pins what the older reader does with the record: a start row it reads as a plain retirement, a terminal pair it reads as an ordinary retirement and spawn (the successor's `rewound_from` is ignored).

### Source resolution

A boundary inside a rewound session's copied prefix is read in the session it was copied from: when `message_index` is at or before the `message_count` the source was itself rewound from (its chain entry), admission moves the boundary to that entry's `source_session_id`, and repeats. The bytes are the same (requirement 1), and only there are the boundary's instruction heads the source's own, which `ReactivateInherited` requires (a head copied from an earlier session is `NotEffectiveInSource`). A boundary inside a fork's re-activation suffix is refused by the preflight.

### Sequence

1. **Admission** (no state change): the scope, the source resolution and its lineage check (**Redo**), the boundary check (`message_index` is a message boundary at or before the source's committed end), and the refusals in the owed-work table. A refusal here writes nothing.
2. **Begin** = the rotation start, one actor step:
   1. Stage the mob-owned hold on the current session `S0` (`RunStartHoldReason::MemberRewind`, a new mob-owned reason, as `HostPreparation` is). Unlike a plain run-start hold it also **refuses new input** on `S0` (see **Admission of turns**).
   2. Under the hold, re-check `S0` idle (no run in flight, no non-terminal input in its ledger) and run the **fork preflight** (below) against the resolved source at `message_count`.
   3. Compute `expected_prefix` from the source.
   4. `BeginMemberRotation` with target `CommittedFork`: record the intent (`member_rotation_targets`, the planned child id, `expected_prefix`), mark the incarnation `Retiring`, and write the start row `MemberRetirementStarted` with the record.

   If 2.2 refuses, release the hold and answer the refusal; nothing is written. The pending state is the rotation intent plus `Retiring`; there is no separate `member_rewind_pending` field.
3. **Fork** (outside the actor): `fork_durable_session_with_planned_identity` of the source at `message_count`, into the planned id, with target `DurableSessionForkTarget { member_binding: main's MobMemberBinding }`, policies `ReactivateInherited`, and no tool-access-policy argument (the child keeps the source's effective policy, as the forked participant's does). The mob seam passes no target today (`fork_planned_child` in `session_service.rs`), so the rewind needs a seam that carries the member-binding target; without it the child is born unbound. The child `S1` is born bound to `main`. Then carry the comms key pair (see **Comms identity**).
4. **Retire `S0`'s incarnation**: release its runtime (it is idle), settle `S0`'s owed compaction projections cold as the fresh-session rotation does (`settle_member_owed_compaction_projections`), end `S0`'s session-scoped tool grants explicitly (`end_tool_grants_of(ToolGrantSubject::session(S0))`, reason "member rewound"; see **What carries over**), and archive `S0` (disposal `Archived`, as a retirement does). Step 4 starts only once `S1` exists and matches `expected_prefix`.
5. **Commit** = the rotation terminal, one machine step: append `MemberRetired` and `MemberSpawned` in one batch; the successor's `MemberSpawned` carries `rewound_from { source_session_id, message_count }` and keeps the member's creation id (`MemberCreationProvenance::Successor`); seat the successor **Dormant** on `S1` at the next generation and fence, as an **existing** session: unlike the fresh-session terminal it writes neither `member_seat_session_unborn` nor `unborn_seat_sessions`, so the first wake resumes `S1` instead of creating a session; move the binding; append the chain entry (**Redo**); emit `MemberSessionBindingChanged { old_session_id: S0, new_session_id: S1 }` and the incarnation-change item (**Streams**); release the hold.
6. **Recovery**: replay re-records the intent and its `expected_prefix` from the start row (`RecoverMemberRotationIntent` gains a `CommittedFork` arm), re-stages the hold on `S0` while `S0` is not archived, and **observes the planned child** before anything else, as the planned-fork user does:
   - The child exists: verify it against `expected_prefix` and the member binding. A match continues at the key carry (the same bytes), then step 4 (an archive already complete is observed, as `retirement_archive_already_complete` does today), then step 5. A mismatch takes the failure end.
   - The child does not exist: `S0` is not archived yet (step 4 follows the verified child), so the fork read still accepts it, and step 3 runs again under the re-staged hold.

   Recovery never forks against an archived source. An archived `S0` with no child cannot arise from the order above; it would be store damage, and the rotation then stays stuck and re-drivable with that cause, as below.

A transient store failure at any step leaves the rotation stuck and re-drivable (`redrive_retirement`), as other rotations are.

### Fork preflight

Admission checks the scope, the source and the boundary; the durable fork can still refuse on more. Begin decides each of those under the hold, before the start row, with the answer the client gets:

| Fork refusal | Preflight at Begin, under the hold | Answer |
| --- | --- | --- |
| Busy guard: an active runtime admission on the source, or its turn boundary not free within `QUIESCENT_FORK_BOUNDARY_BOUND` | the idle re-check, then the same guard taken once | `SESSION_BUSY`, `data.kind = fork_source_unavailable`, `cause = running` |
| Pending input boundary at the source's end (`refuse_quiescent_fork_of_pending_boundary`) | the same check on the source at `message_count` | the same busy answer |
| Split tool group (`fork_at_complete_boundary`) | the same boundary check on the source | the durable fork's boundary refusal, mapped as `session/fork_at` maps it |
| Instruction policy (`apply_fork_instruction_policies` under `ReactivateInherited`: `NotEffectiveInSource`, `HeadBodyRetired`, `LineageBodyBudgetExceeded`, an unreadable activation row) | the same function on a clone of the planned prefix, as the fork applies it | `ForkInstructionRefused` with its cause and keys, mapped as `session/fork_at` maps it |
| Blob verification (`preflight_transcript_fork_blobs`) | read and content-verify every blob the prefix references (the inline images it would store are written by the fork itself) | the fork's store error, before anything starts |
| Binding and comms-name checks before commit: the source has canonical session metadata, the member binding derives a comms name, and the child's persisted comms name equals it | read the source's metadata, derive `member_binding.comms_name()`, compare it with the source's comms name | `UnsupportedForMode` ("member session has no canonical metadata") for a source without metadata; an internal error for a comms name that does not match the binding (a corrupt binding, not a client error) |
| Comms identity root for the key carry | resolve the comms identity root and `S0`'s key path | a key present with no resolvable root: `UnsupportedForMode` ("no comms identity root to carry the member's key"); no key: nothing to carry |

With these decided and the hold refusing runs and input, the fork's remaining failures are store faults, which are transient.

### The failure end

A non-transient failure after the start row (a fork refusal the preflight did not foresee, or a planned child whose evidence conflicts with `expected_prefix`) does not leave the member `Retiring` with only the mob's destroy as an exit. The rotation finishes as a **same-session successor on `S0`**: a conflicting child is archived (as `fail_activation_and_archive` does for forked participants), one machine step restates the recorded target from `CommittedFork` to `SameSession` (the same-session terminal's guard reads the target), and the existing same-session terminal (`CommitMemberRotationSameSession`, the `…Kickoff` form when one is owed) seats the member Dormant on `S0` at the next generation with its key, endpoint and edges, and releases the hold. The failure end is reachable only before step 4, so `S0` is never archived when it is taken. The caller gets `data.kind = rewind_not_committed`, with `cause` naming the fork's refusal and the generation the member continues at; a rotation finished by recovery has no caller, and the host learns the outcome from the incarnation-change item (rotation `same_session`) and `mob/member_status`. The chain gains no entry.

**No Build step.** The existing rotations build nothing: the successor is seated Dormant and its first delivery builds it. The rewind does the same, so a dormant member is not made live as a side effect, no second runtime ever claims the comms name while the old one holds it (the in-process registry would refuse it, `crates/meerkat-comms/src/inproc.rs`), and there is no `PendingProvision` to cancel: the wake path carries its own guard and rollback. The first build after the rewind resolves the dispatch admission and the per-spawn overlay through the spawn customizer, as for any rebuild (`RotationSuccessor` records neither).

**Fork-derived members cannot be rewound** in this contract. A dormant seat refuses a spec with `fork_source`, `fork_job` or a forked participant attachment, and a fork-derived member has no same-session successor. Begin refuses such a member `MobError::UnsupportedForMode` ("a fork-derived member cannot be rewound"), before anything starts, until a dormant seat can carry fork lineage. rkat-tui's `main` is host-root and unaffected.

**Autonomous members cannot be rewound.** Begin refuses any member whose runtime mode is not `TurnDriven` (`UnsupportedForMode`). An `AutonomousHost` member's loop and kickoff obligations are keyed to its session's conversation, and a rewound conversation would need a rule for re-owing them that this contract does not define.

### Comms identity

Choice: **carry the old session's key pair into the new session.** The comms key pair is stored per session (`<root>/<session>/identity.key`, `crates/meerkat-mob/src/runtime/tests/seat_comms_key.rs`), and the durable fork copies none, so without a carry `S1` gets a new key and endpoint while peers trust the old one. The two existing answers each lose something: a fresh-session successor records a new endpoint and drops the member's edges, re-wiring only declared ones (explicit `wire()` edges and external peer edges are lost); a same-session successor under its own role keeps key, endpoint and edges. The rewind takes the second answer's outcome: step 3 copies `S0`'s key pair to `S1`'s key path under the root the preflight resolved, the record names the member's existing endpoint, and the terminal keeps the member's endpoint, peer id and edges, as `CommitMemberRotationSameSession` does under the member's own role. This is safe only because `S0` is fenced: archived in step 4, so no runtime can come up on it with the same key, and its runtime was released before the successor can wake. A member whose session has no key pair (network comms before the first wake) carries none and owes its endpoint as a same-session successor does.

`S0` keeps its key file after the archive. The rewind therefore never records `S0` as an identity entry's archived session (`identity_entry_archived_sessions`): an entry archived after a rewind records its current session, `S1`, so an unarchive can never seat the identity on `S0` while `S1` holds the same key.

### Fence on the old session

`turn/start` does no owner check (`router.rs`), and a session whose mob owner no longer binds it "proceeds as before" (`crates/meerkat-rpc/src/session_runtime.rs`); session-targeted schedules (`resumable_session`, `crates/meerkat-schedule/src/tools.rs`) resume such a session too. Only archived sessions are refused. Step 4 therefore archives `S0` as part of the rotation, so after the terminal no door can run it and its ledger runs nothing. `session/history` still reads archived member sessions (`router.rs`, "archived mob-owned sessions should still route to session/history"), so requirement 7's read holds. Redo and source resolution read an archived session, so two reads that refuse archived sources today are lifted **for lineage-authorized sources only**: the fork read (`source_session_for_transcript_edit_locked`) and the session read (both `reject_if_archived_session` calls in `persistent.rs`'s `read`). The lift is a mob-internal entry that carries the chain proof; the generic fork and read surfaces keep refusing archived sources.

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
| Scheduled resumes of `S0` | While the hold stands, refused as other input is (below); after the archive, refused as an archived session. |

### Admission of turns and member work

`mob/turn_start` keeps its carrier (`turn/start`, which answers the turn's result and carries per-turn options); the generation check goes into the turn start's own admission. The handler resolves its target and checks the member in one actor-gated step: the identity's current generation, fence and session, and not `Retiring`.

A run-start hold holds runs, not input: a turn checked just before Begin would be queued in `S0`'s ledger behind the hold, and ended by the archive instead of refused, and could make the fork's busy guard fire. The rewind's hold therefore also **refuses new input** on `S0`, in the session's input admission, so nothing reaches `S0`'s ledger after the hold is staged. Input admitted before the hold is seen by Begin's idle re-check, which refuses the rewind busy. The answers:

| Door | Before the hold | Hold staged, rotation not committed | After the commit |
| --- | --- | --- | --- |
| `mob/turn_start`, `mob/member_send` and other member-targeted admissions | runs on `S0`; Begin then refuses the rewind busy | `MobError::LifecycleOperationPending`, retryable, the kind respawn uses for a retirement in flight. For a `CommittedFork` rotation, `SubmitWork`'s `Retiring` rejection answers this instead of member-not-found | targets `S1` |
| `mob/turn_start` resolved before the commit and started after it | | | `S0` answers `SESSION_NOT_FOUND` (`reject_archived_persisted_session_without_live`); the handler re-reads the member and maps it: the identity seated on another session answers `LifecycleOperationPending` (retry reaches `S1`), an identity gone answers `MemberNotFound`. The client never sees a session it did not name. |
| `turn/start` on `S0`, a scheduled resume of `S0` | runs; Begin refuses busy | `SESSION_BUSY`, retryable | `SESSION_NOT_FOUND`, as for any archived session |

Nothing is retargeted, and nothing ran on `S0` after the hold.

### Streams

The notice is a new mob-level item, **`MemberIncarnationChanged`**, emitted by **every rotation terminal**: the fresh-session terminals, the same-session terminals and the `CommittedFork` terminal (including the failure end). It is a machine effect beside the terminal's journal rows, not an `AgentEvent` variant. Its scope is rotations of a seated member; spawns, seats, retirements and destroys keep their existing lifecycle notices and `MemberSessionBindingChanged`, which is unchanged and still not emitted by a same-session terminal. The item serves respawn as well as rewind, since a respawn moves the member to a new runtime and generation on the same session.

**Wire shape.** On `mob/stream_event` the notification params are `{ stream_id, sequence, epoch?, event }` today. The item travels in a sibling field instead of `event`:

```
{ stream_id, sequence, incarnation_change: {
    agent_identity, member_ref,
    old_session_id, new_session_id,
    old_generation, new_generation,
    rotation: "fresh_session" | "same_session" | "committed_fork" } }
```

`event` and `epoch` are absent (the item belongs to no session's sequence); `sequence` is the stream's next. For `same_session`, `old_session_id` equals `new_session_id`.

- **Mob-wide.** The router forwards the item when the terminal commits. It attaches a member's stream only after an incarnation check: the stream it attaches must belong to the incarnation MobMachine binds now, so new-incarnation events never arrive before the item under the old runtime id.
- **Per-member.** On every rotation of the member the stream ends the old incarnation's source after its last event, yields the item, and attaches the successor at its next wake (it is Dormant), on the same `stream_id`. `MemberStreamEpoch` is reset at the item and set again at the successor's attach, so events from `S1` carry `S1`'s epoch.
- **Change for existing consumers.** A per-member consumer today decodes every `event` as an `EventEnvelope<AgentEvent>`, and a mob-wide one as an `AttributedEvent`. Both now receive notifications without `event`, and per-member streams now continue across rotations with a new epoch instead of following one incarnation. The change is a wire schema change, announced in the changelog; a consumer must branch on `incarnation_change` before decoding `event`.
- **Reattach.** No producer writes the item into `S1`'s retained sequence, and nothing is retained across a restart, so a client reattaching with an old-epoch cursor does not replay it. Such a client, and one with no open stream, learns of the switch from `mob/member_status` (`current_session_id`, generation, `rewound_from`), or from `mob/events`.

### Redo

After a rewind the current session holds only the prefix. Redo is `mob/rewind_member` with `source_session_id` set to a session of the identity's rewind chain, at its committed end (full redo) or any earlier boundary (partial redo). The same sequence runs: the hold and idle check are on the current session, which is the one replaced; the source is an archived session, read through the lineage-authorized reads.

- **The chain is machine state.** `member_rewind_chains[identity] = [{ session_id, source_session_id, message_count, generation }]`, appended by the `CommittedFork` terminal and restored on replay from its rows. The start step checks a requested source against it: it must be an entry's `session_id` or `source_session_id`, not the current session.
- **The boundary is the terminal kind.** The chain is kept across the same-session terminals (a respawn) and the committed-fork terminal, and dropped at every other terminal: a fresh-session rotation (how a host resets a member or deletes and recreates an identity, which discards the conversation), a retirement, a destroy. A predecessor's sessions, though bound to the same identity, are then refused `INVALID_PARAMS`, `data.kind = rewind_source_not_in_lineage`. The machine needs no creation ids for this; the creation id stays what it is today and plays no part in the chain.
- **Status reads never fold the journal.** `mob/member_status` carries `rewind_lineage`, newest first, from the roster's projection of the machine state.
- Re-seating a retired session as-is is not offered: it would bind one session to the identity in two generations. Every redo produces a fresh session.

### What carries over

The successor keeps the member's identity, creation id, profile, runtime mode, labels, spawner, wiring exemption, endpoint, edges and identity-scoped tool grants (subject `member(mob, incarnation, identity)`). An archive ends no tool grant: only an identity's end does (`end_member_identity_tool_grants`, over the sessions MobMachine holds for it then), and after the rewind `S0` is no longer among those. Step 4 therefore ends `S0`'s session-scoped grants explicitly; an end repeated changes nothing, so recovery may repeat it. They are not carried to `S1`: a session-scoped grant was a decision about `S0`'s conversation, and the host grants again on `S1` if it wants to. The roster's `fork_source` is unchanged and is never set to the member's own session; lineage lives only in `rewound_from` and the chain.

### Typed refusals

| Case | Refusal (nothing started unless stated) |
| --- | --- |
| Owes work (table above), or the fork's busy guard or a pending boundary at Begin | `SESSION_BUSY` / `fork_source_unavailable` / `running`, or `UnsupportedForMode` |
| The fork's instruction policy refuses at Begin | `ForkInstructionRefused`, mapped as `session/fork_at` maps it |
| A blob of the prefix does not verify at Begin | the fork's store error |
| No canonical metadata, or a key with no comms identity root | `UnsupportedForMode` |
| Rotation already in flight, or a turn or member send meeting a pending rewind | `LifecycleOperationPending`, retryable |
| A `mob/turn_start` that resolved `S0` and started after the commit | `LifecycleOperationPending` (mapped from `S0`'s `SESSION_NOT_FOUND`) |
| A non-transient failure after the start row | `rewind_not_committed` with the fork's cause; the member continues on `S0` at the next generation |
| Fork-derived, placed, not `TurnDriven`, or not runtime-backed with durable sessions | `UnsupportedForMode` |
| Seated without a session | `fork_source_unavailable`, `cause = no_session` |
| Unknown or retired identity | `MobError::MemberNotFound` |
| Principal lacks `Retire`, or `AdminHost` for an identity entry | `ScopeDenied` / `IdentityMember` |
| `source_session_id` not in the chain | `INVALID_PARAMS`, `rewind_source_not_in_lineage` |
| `message_index` not a boundary of the source | the durable fork's refusal, mapped as `session/fork_at` maps it |

**Mapping onto the ignored test:** the stand-in becomes `mob/rewind_member`. Its assertions hold, except that lineage reads `rewound_from` instead of `fork_source`.

## Acceptance tests

In `crates/meerkat-rpc/tests/member_session_fork_at_boundary.rs` unless noted:

- `a_member_session_forks_at_a_message_boundary`, un-ignored on `mob/rewind_member`; `main` is Dormant on `S1` after the call, `S1` is in neither unborn set, and its next turn resumes `S1` with the prefix instead of creating a session.
- Rotation rows (`crates/meerkat-mob/src/runtime/tests/rotation_intent_rows.rs` siblings): the start row carries the `fork_rotation` record, with `expected_prefix`, under its own key; the terminal appends `MemberRetired` and `MemberSpawned` in one batch with `rewound_from` and the kept creation id; a rollback fixture decodes both with the 0.9.8 reader.
- Fork preflight: a source with a key copied from an earlier session at the boundary (`NotEffectiveInSource`), a boundary inside a fork's re-activation suffix, a pending input boundary, a split tool group, a prefix blob missing from the store, and a member key with no comms identity root each refuse at Begin with no start row and the hold released.
- Instruction state at the boundary: a key activated before the boundary is effective on `S1`, a key activated in the cut tail is not; a second rewind into the first rewind's copied prefix resolves to the earlier session and carries its activated key.
- Failure end: a fork refusal injected after the start row, and a planned child whose prefix digest conflicts on recovery, each end with the member Dormant on `S0` at the next generation, not `Retiring`; the caller gets `rewind_not_committed`; the conflicting child is archived; the chain gains no entry.
- Restart finishes forward: crash after the start row (no child: the fork runs again on the same planned id), after the fork (the child is observed and verified; no second fork, no read of an archived source), and after the archive; each restart ends on the same `S1`.
- Archived old session: after the rewind `S0` is archived, `turn/start` and a session-targeted schedule on it are refused, `session/history` still reads it, and the generic fork and session reads still refuse it while the lineage-authorized ones read it.
- Grants: a session-scoped grant on `S0` has ended after the rewind and does not apply on `S1`; an identity-scoped grant still applies on `S1`.
- Identity entry: archive an identity entry after a rewind and unarchive it; it is seated on `S1`, never `S0`.
- Wired peer: a peer wired explicitly with `wire()` sends to `main` before and after the rewind, and the second message lands on `S1` with the same peer id.
- Owed-work refusals: a run waiting on an approval decision, a pending detached job, a queued input, an owed kickoff, a forked participant attachment and a member forked from `main` each refuse the rewind with nothing started.
- Turn admission races (`crates/meerkat-mob` runtime tests, paused after Begin): a `mob/turn_start` and a `mob/member_send` are refused `LifecycleOperationPending`, a `turn/start` on `S0` is refused busy, and `S0`'s ledger gains no input; a `mob/turn_start` resolved before the commit and started after it answers `LifecycleOperationPending`, not `SESSION_NOT_FOUND`; nothing ran on `S0` after the hold.
- Redo boundary: rewind, respawn, rewind again; redo from the first chain entry succeeds. Rewind, then `rotate_member` (a reset), and redo from the chain is refused `rewind_source_not_in_lineage`; the same after retire and recreate; `rewind_lineage` is empty in both. The same after a restart.
- Streams, both forms: a subscriber sees `incarnation_change` (rotation `committed_fork`, old and new session and generation) before any `S1` event; the per-member stream stays on its `stream_id` and carries `S1`'s epoch; a `respawn_member` yields `same_session` with equal session ids, and a `rotate_member` yields `fresh_session`; a reattach with an old-epoch cursor learns the switch from `mob/member_status`.
- Restrictions: the deny list and the host dispatch admission (#2112 lineage case) hold on `S1`, and a rebuild emits no fork-source warning.

## Consumers

- rkat-tui `/rewind` in attach mode: requirements 1 to 7.
- An interactive host runtime that offers rewind on a durable member.
- `rkat mob rewind <member> --at <index>` in the CLI.
