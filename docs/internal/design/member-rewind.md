# Same-identity member rewind

Status: draft for review. This note supplies the design note that the disposition of #2117 asked for before same-identity rewind is built. It recommends one option, but that recommendation is open to change: the maintainers own the decision. It is docs only: no API, wire type or test changes yet, and `a_member_session_forks_at_a_message_boundary` in `crates/meerkat-rpc/tests/member_session_fork_at_boundary.rs` stays `#[ignore]`d until the decision.

Source baseline: Meerkat `main` at `ad31c1fd31517c0c1908084fae459758808cfd2d`. Paths and names below describe that baseline.

## Problem

A host that runs an interactive agent as a durable mob member wants "rewind": the user picks an earlier message, and the member's own conversation continues from that boundary. The continuation must have the exact context (the prefix byte for byte, including the copied system prompt), keep the same identity, and record where it came from.

Today none of the member surfaces gives that:

- `mob/fork_member` (`MobHandle::fork_member`) copies the exact prefix, but seats it under a **new** identity. A fork onto the source identity is refused before anything is forked: `admit_fork_member` returns `MobError::MemberAlreadyExists` when `member.identity == source_identity` (pinned over RPC by `mob_fork_member_onto_a_seated_identity_commits_nothing`: "mob member already exists").
- `session/fork_at` refuses a member's session with `INVALID_PARAMS`, "mob-owned session transcripts cannot be edited through the generic session surface" (`session_fork_at_refuses_a_member_session`). The same refusal covers `session/fork_replace`, `session/rewrite_transcript` and `session/restore_transcript_revision`.
- Re-seating the identity on a fork child's session is refused. After the child and `main` are retired, `mob/spawn` of `main` with `launch_mode = resume` on the child's session returns `INVALID_PARAMS` with `data.kind = resume_session_bound_to_another_member`, naming the session and the member it is bound to (`main_cannot_be_reseated_on_another_members_session`). The fork child's session is durably bound to the child's identity at fork commit, and `crates/meerkat-mob/src/build.rs` refuses a resume whose current identity does not match the stored binding. Its sibling check, "persisted mob member binding ... contradicts persisted comms_name", fires only when a session's own records disagree with each other. A re-seat does not reach that check.

The consumer is rkat-tui's `/rewind` in attach mode: one durable host-root member `main` per workspace, driven over the daemon (see rkat-tui `docs/mob-split.md`). The user picks an earlier message, and `main` continues from that boundary.

## Current owners and APIs

| Owner | What it does today |
| --- | --- |
| `MobHandle::fork_member` / `fork_member_with_source_admission` / `fork_source_session` (`crates/meerkat-mob/src/runtime/handle.rs`) | The durable fork path. Admits control scope `SendCommand`, refuses a source that owes work (`refuse_fork_source_with_admitted_work`: `ForkSourceUnavailable { cause: Running }`), resolves the source's `ForkBuildInheritance` and dispatch admission lineage, then calls `SessionService::fork_persisted_session`. |
| `admit_fork_member` (same file) | Builds the `meerkat_core::DurableSessionForkTarget`. Its `member_binding` (`MobMemberBinding { mob_id, role, member }`) is written onto the child at fork commit, so the child is born bound to the identity it will be seated under. It also resolves `inherited_instructions` from the mob builder's `ForkInstructionPolicy` against the path's `ActivationWindow`. |
| `seat_forked_member` (same file) | Seats a committed fork through the ordinary resume pipeline (`MemberLaunchMode::Resume`, `SpawnSource::PersistedForkResume`). A seat failure after commit is `ForkMemberProvisionFailed { fork_session_id, .. }`, which keeps the committed session id in the refusal. |
| `MobMemberBinding` and `comms_name()` (`crates/meerkat-core/src/connection.rs`) | A member's durable identity on its session. The comms name `mob/role/member` is derived from the binding. `build.rs` (`ResumedIdentityRejection`, `member_binding_proves_comms_components`) enforces it on every resume. |
| `ForkBuildSource` (`crates/meerkat-core/src/service/mod.rs`), `MemberSpawned.fork_source` (`crates/meerkat-mob/src/event.rs`), `RosterEntry::fork_source` (`crates/meerkat-mob/src/roster.rs`) | Lineage: the source member's binding and the session the transcript was cut from. Every rebuild carries it as the build's `fork_source`. |
| `ForkBuildInheritance`, `fork_overlay_rule`, `follow_fork_link`, `order_fork_sources_first` (`crates/meerkat-mob/src/runtime/fork_build.rs`) | Build inheritance from the source, including the overlay and admission lineage. A child follows its source's overlay only while the source is seated **and bound to `fork_source.source_session_id`**. Otherwise the link is `ForkLinkBroken::OtherSession` and a warning names the missing source. |
| `observe_fork_source_dispatch_lineage` / `resolve_seat_dispatch_admission` | The admission-lineage record from the #2112 fix: MobMachine decides the child's tool dispatch admission from the source's lineage before anything is forked. |
| `MemberReset`, `MemberSessionBindingRecovered` (`event.rs`) | The existing journal facts for "same identity, new generation and fence token" and for a machine-authorized bridge-session rebind. |
| `mob/fork_member` (`handle_fork_member`, `crates/meerkat-rpc/src/handlers/mob.rs`; `MobForkMemberParams` / `MobForkMemberResult`, `crates/meerkat-contracts/src/wire/mob.rs`) | Request `{mob_id, source_member_id, agent_identity, message_index?, profile?, runtime_mode?}`, response `{agent_identity, member_ref, session_id, source_session_id, message_count}`. A running source is `SESSION_BUSY` with `data.kind = fork_source_unavailable`, `cause = running`. |

## Requirements

From the rkat-tui regression list and #2117:

1. **Exact prefix.** The continued transcript is the source's messages before `message_index`, including the copied system prompt (`assert_exact_prefix`).
2. **Same identity continues.** After the rewind, `main` is the member that answers. `mob/member_status` and every member-targeted method resolve `main` to the rewound conversation.
3. **Lineage recorded.** The source session and boundary are durable and readable.
4. **Inherited restrictions.** The member keeps its tool access policy, dispatch admission and instruction policy. A deny list in force before the rewind is in force after it.
5. **Idle source only.** A member that owes work is refused with the typed cause (`SESSION_BUSY`, `fork_source_unavailable` / `running`). Nothing is committed, seated or rebound.
6. **Durable.** After a process restart, the member resumes on the rewound conversation, and replay never seats it on the old one.
7. **Redo.** The previous tail stays readable (for example through `session/history` of the retired session), so a host can offer "redo".

## Options

### (A) Re-seat the identity on a prefix fork

`mob/fork_member` into a staging identity, retire `main`, then seat `main` on the staging child's session by rewriting the child's persisted binding (and so its comms name) to `main`.

- Mechanics: needs a binding rewrite on an existing session, which is exactly what `ResumedIdentityRejection::bound_to_another_member` refuses today, plus a retire of the staging identity.
- Trade-offs: the binding check guards identity integrity across restarts, and a rewrite path weakens it for every caller. The operation spans two identities and three journal facts (staging spawn, retire, re-seat). A crash between them leaves `main` unseated, or two sessions claiming it. It is also unnecessary: `DurableSessionForkTarget::member_binding` can already bind the child to any identity at commit (see (B)).

### (B) Prefix fork as a new session of the same identity

A mob-owned method forks the member's own session at the boundary into a new session that is **born bound to the same identity**, then moves the identity's seat onto it in one journal fact.

- Mechanics: reuse `fork_source_session` with the source identity as the target identity. Lift the `member.identity == source_identity` guard for this path only, and set `DurableSessionForkTarget::member_binding` to the member's existing binding (same mob, role and member). No staging identity, no binding rewrite. Then quiesce the old seat and seat the identity on the new session as a new generation, as `MemberReset` does: new fence token and `AgentRuntimeId`, same `AgentIdentity`.
- Trade-offs: a new seat transition in MobMachine ("rebind a seated identity to a committed fork of its own session") and a new journal fact. In exchange, no window exists in which two sessions claim the identity, and the binding/comms-name invariant is untouched: the new session's binding proves the same comms name.

### (C) New identity, host follows

Use `mob/fork_member` as merged and let the host switch to `main-2`.

- Trade-offs: identity churn becomes visible to every surface: the spawn customizer (keyed by identity), admission and grants, operator methods, wiring, `mob/member_status` consumers, and rkat-tui's conversation model, which names one durable `main` per workspace. The maintainers chose this shape for forks, and it is right for forks. It does not meet "same identity".

### (D) Same-session rewind through the transcript revision graph

The engine already has a same-session, append-only edit: `session/rewrite_transcript` (`SessionService::rewrite_session_transcript`) commits a rewrite and keeps the parent revision, and `session/restore_transcript_revision` restores it. A mob-owned variant could cut the tail `[message_index, end)` and record reason kind `rewind`. Redo would then be a restore.

- Mechanics: route the rewrite through the mob, under an owner's authority. `crates/meerkat-mob/src/control_policy.rs` lists a `RewriteTranscript` control scope as deferred to v2.
- Trade-offs: no seat change, no new session id, and lineage comes from the revision graph. Before this could be chosen, these must be settled: whether an ordinary edit may have an empty replacement (a pure tail cut); how live mob state keyed to that session's messages reacts to a rewrite (instruction activation rows in the cut tail, fork jobs' `prefix_message_count`, children whose `fork_source` names this session at a later boundary); and the new control scope. It also fails requirement 3 as the ignored test states it (a new `session_id`, `fork_source`). Recorded here so the decision is made against it, not by default.

### Recommendation

**(B).** It reuses the durable fork path, its typed refusals and its admission lineage unchanged. It keeps the binding invariant, and it makes the switch a single journal fact. (A) is not needed as a fallback: the seat path can already bind an identity at creation through `DurableSessionForkTarget::member_binding`. (D) is the alternative to weigh if the maintainers would rather extend the revision graph than add a seat transition.

## Proposed contract for (B)

**Method:** `mob/rewind_member`. The ignored test's stand-in calls it `mob/fork_member_at`; either name works. The maintainers pick.

```
MobRewindMemberParams  { mob_id, agent_identity, message_index, inherited_instructions? }
MobRewindMemberResult  { agent_identity, member_ref, session_id, source_session_id, message_count }
```

`message_index` has its `session/fork_at` meaning: the new transcript is the source's messages before it. If `inherited_instructions` is absent, the path uses the mob builder's `ForkInstructionPolicy` with `ActivationWindow::HostDriven`, as `mob/fork_member` does: the host drives the next turn.

**Owner:** `MobHandle::rewind_member` in `crates/meerkat-mob/src/runtime/handle.rs`, admitted under control scope `SendCommand`, like `fork_member`.

**Typed refusals** (nothing committed, seated or rebound unless stated):

| Case | Refusal |
| --- | --- |
| Member owes work (turn in flight, admitted input, unanswered boundary) | `SESSION_BUSY`, `data.kind = fork_source_unavailable`, `cause = running` (`refuse_fork_source_with_admitted_work`, then the durable owner's `SessionError::Busy`) |
| Member seated without a session | `fork_source_unavailable`, `cause = no_session` |
| Unknown or retired identity | `MobError::MemberNotFound`, mapped as other member-targeted methods map it. Today `fork_member` reports an unseated source as `no_session` because `resolve_bridge_session_id` finds nothing; a rewind should name the missing member instead |
| `message_index` beyond the committed end, or not a message boundary the durable fork accepts | the durable fork owner's refusal, mapped as `session/fork_at` maps it (`INVALID_PARAMS`) |
| Fork committed, seat failed | `ForkMemberProvisionFailed { fork_session_id }`. The **old seat stays bound**: the switch is only journaled after the new seat succeeds, and the orphan session carries the identity's binding but no roster fact names it. |

**What is persisted:** the fork commit, written by the session store with the member's own binding. Then one mob journal fact, `MemberRewound { agent_identity, previous_generation, new_generation, fence_token, agent_runtime_id, session_id, source_session_id, message_count }` (the name is illustrative). Replay feeds it to MobMachine as a seat rebind and updates the roster's bridge session. Because it is a single row, a crash leaves either the old seat (fact absent) or the new one (fact present), never neither.

**Lineage:** record it on the rewind fact and project it on the roster entry as `rewound_from { source_session_id, message_count }`. **Do not** put it in `fork_source`. `fork_source` means "built with the source's `ForkBuildInheritance`". A rewound member is built as itself (its own labels, context, customizer resume answer and overlay). A self-referential `fork_source` would also send `fork_overlay_rule` through `follow_fork_link` to the member itself, now on another session, so every rebuild would hit `ForkLinkBroken::OtherSession` and warn. If the maintainers prefer `fork_source`, these three functions need a self-link case: `fork_overlay_rule`, `follow_fork_link` and `order_fork_sources_first`.

**Recovery:** restart restore resumes the identity on the session the latest seat fact names, as for any member. The retired session is a plain durable session that no seat names. It is not disposed, so `session/history` on `source_session_id` still reads the tail. Redo is a second rewind with a `message_index` over the retired session: a new method, or (B) generalised to `{source_session_id}` restricted to sessions in the identity's own rewind lineage. That generalisation is left open.

**Afterwards:** `mob/member_status` shows the same `member_ref` and a new `current_session_id`, and the generation advances. `mob/events` carries the rewind fact, so a second attached client sees the switch without polling.

**Mapping onto the ignored test:** `fork_member_at` becomes the real call. Its assertions hold as written: `session_id != source`, `source_session_id`, `message_count == BOUNDARY`, exact prefix, the source history unchanged, `member_session(MAIN) == child`, the deny list kept, and `SESSION_BUSY` with nothing rebound while a turn is held. One exception: the lineage assertion reads `rewound_from` instead of `fork_source` if the recommendation above stands. The test's doc comment also cites `today_the_exact_prefix_member_fork_exists_only_in_process`, a test that no longer exists in that file. Fix that reference when the test is un-ignored.

## Acceptance tests

All in `crates/meerkat-rpc/tests/member_session_fork_at_boundary.rs` unless noted:

- `a_member_session_forks_at_a_message_boundary`: un-ignored, the stand-in replaced.
- A restart test: rewind, drop the runtime, restore over the same stores, and `main` resumes on the rewound session with the prefix plus later turns. The old session is still readable.
- A two-client test: client B, attached to the same mob, observes the rewind fact on `mob/events`, and its next `mob/turn_start` for `main` lands on the new session.
- Running refusal: a held turn, then a rewind, gives `SESSION_BUSY`. Roster, session list and binding are unchanged (already in the ignored test). Add an admitted-but-queued input case, as `fork_member` pins.
- Lineage and restrictions: `rewound_from` survives a restart. A member seated under a host dispatch admission keeps it (the #2112 lineage case: a `crates/meerkat-mob-mcp/tests/fork_dispatch_admission_lineage.rs` sibling). A rebuild after a rewind emits no fork-source warning.
- Seat failure: an injected seat failure after the fork commit gives `ForkMemberProvisionFailed`, and `main` is still seated on the old session.

## Consumers

- rkat-tui `/rewind` in attach mode: the motivating case. It needs requirements 1-7, and redo needs 7.
- An interactive host runtime (for example Meta's) that offers rewind on a durable member.
- `rkat mob` CLI could expose it as `rkat mob rewind <member> --at <index>`.
