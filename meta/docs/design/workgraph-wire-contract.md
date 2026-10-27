# WorkGraph wire contract (mobkit 0.7.30)

Historical binding contract for the `meta/workgraph/*` JSON-RPC group, the
experience payload section, capabilities, ABAC actions, and SDK method names,
with current qualifications called out below. The methods table and as-built
notes retain the 0.7.30 account; they are not a complete current API inventory.
All wire fields snake_case. The original results serialized meerkat 0.7.23's
typed results VERBATIM (serde of `WorkItem`, `WorkGraphSnapshot`, `WorkAttentionBinding`,
`GoalCreateResult`, etc. — see docs/design/workgraph-integration.md for the
upstream shapes).

`expected_revision` is required by revision-checked mutations of existing
work items or attention bindings, not every mutation. `create`, `goal/create`,
`link`, and `attention/prune` have no caller-supplied CAS token; their
transactional invariants still apply. Item mutations and
`goal/confirm`/`goal/request_close` use the work item's `revision`;
`attention/pause`/`attention/resume`/`attention/reassign` use
`attention.machine_state.revision`.

## Current scope qualification (Meerkat 0.8.40)

The audited MobKit 0.8.39 checkout pins Meerkat 0.8.40. Its scope contract
supersedes the historical realm/namespace behavior described below:

- Stock gateway and default Meta construction derive the canonical
  `mob.<mob_id>` realm and use `WorkNamespace::default()`, not the bare mob
  definition ID as a realm.
- All WorkGraph reads and mutations stay within the service's one immutable
  namespace grant, including ordinary item operations, not only goals and
  attention. An optional `namespace` may name only that granted namespace.
  The tool surface is pinned to the same grant.
- `all_namespaces: true` on `list`, `snapshot`, or `events`, and any
  out-of-grant namespace, are refused with JSON-RPC `-32602`. The optional
  fields in the historical table do not grant cross-namespace access.
  `realm_id` remains caller-forbidden on every RPC method.
- Member-bound attention is realm-bound (Meerkat 0.8.41). A member resolves
  bindings only in `mob.<mob_id>`; `goal/create`, `attention/reassign` and
  `attention/break_glass_reassign` classify the lowered target by its typed
  owner key (`WorkOwnerKey::as_mob_agent`) and refuse a member of another mob
  before the write (`-32602`, `data.kind = "attention_target_realm_mismatch"`).
  Bindings written before the rule are migrated at bootstrap
  (`workgraph_realm` module; report on `meta/capabilities`).
- Library embedders can inject a composed `WorkGraphService`; its own fixed
  grant is authoritative rather than a universal requirement to use the stock
  namespace. This is not a caller-controlled grant-widening mechanism.

## Methods

Historical inventory; apply the current scope qualification above.

Read (ABAC `workgraph.view`):

| Method | Params | Result |
|---|---|---|
| `meta/workgraph/snapshot` | `{namespace?, all_namespaces?, statuses?: string[], labels?: string[], include_terminal?, limit?}` | `WorkGraphSnapshot` |
| `meta/workgraph/list` | same filter | `{items: WorkItem[]}` |
| `meta/workgraph/get` | `{id, namespace?}` | `{item: WorkItem}` |
| `meta/workgraph/ready` | `{namespace?, labels?, limit?}` | `{items: WorkItem[]}` |
| `meta/workgraph/events` | `{namespace?, all_namespaces?, after_seq?, limit?}` | `{events: WorkGraphEvent[]}` |
| `meta/workgraph/attention/list` | `{namespace?, status?}` — `status` accepts plain strings `active\|paused\|superseded\|stopped` (or upstream's tagged `{state: ...}` form) | `{attention: WorkAttentionBinding[]}` |
| `meta/workgraph/goal/status` | `{binding_id, namespace?}` | `{item, attention}` |

Mutate (ABAC `workgraph.manage`; all honored on unified + console surfaces,
console additionally gated by `can_mutate`):

| Method | Params | Result |
|---|---|---|
| `meta/workgraph/create` | `CreateWorkItemRequest` fields (`title` required; `description?, priority?, completion_policy?, labels?, due_at?, not_before?, snoozed_until?, external_refs?, evidence_refs?, status?("open"\|"blocked"), namespace?`) | `{item}` |
| `meta/workgraph/update` | `{id, expected_revision, title?, description?, priority?, labels?, due_at?, not_before?, snoozed_until?, namespace?}` | `{item}` |
| `meta/workgraph/claim` | `{id, expected_revision, owner: {kind, id, display_name?}, lease_seconds?, namespace?}` | `{item}` |
| `meta/workgraph/release` | `{id, expected_revision, namespace?}` | `{item}` |
| `meta/workgraph/close` | `{id, expected_revision, status?("completed" default\|"cancelled"\|"failed"), namespace?}` | `{item}` |
| `meta/workgraph/block` | `{id, expected_revision, namespace?}` (verified upstream: `block(realm_id, namespace, id, expected_revision)` — no blocker-linkage field; use `link` with kind=`blocks` for blocker edges) | `{item}` |
| `meta/workgraph/link` | `{kind, from_id, to_id, namespace?}` | `{edge: WorkEdge}` (upstream returns a bare `WorkEdge`; meta wraps it) |
| `meta/workgraph/evidence/add` | `{id, expected_revision, evidence: {kind, id, label?, summary?}, namespace?}` | `{item}` |
| `meta/workgraph/policy/escalate` | `{binding_id, id, expected_revision, completion_policy, namespace?}` — witness (`AttentionContextProjection`) fetched SERVER-SIDE from `binding_id` | `{item}` |
| `meta/workgraph/goal/create` | `{title, description?, target: {kind:"session", session_id} \| {kind:"identity", identity} \| {kind:"owner", owner_key:{kind,id}}, mode?("pursue" default), completion_policy?, delegated_authority?, namespace?}` — `identity` lowered via `lower_agent_identity_attention_target` with the runtime's mob id | `{item, attention}` |
| `meta/workgraph/goal/confirm` | `{binding_id, expected_revision, evidence?: {kind, id, label?, summary?}, namespace?}` — console surface promotes authenticated principal via `with_trusted_principal` | `{item, attention}` |
| `meta/workgraph/goal/request_close` | `{binding_id, expected_revision, status?, namespace?}` | `{item, attention}` |
| `meta/workgraph/attention/pause` | `{binding_id, expected_revision, until?, namespace?}` | `{attention}` |
| `meta/workgraph/attention/resume` | `{binding_id, expected_revision, namespace?}` | `{attention}` |
| `meta/workgraph/attention/reassign` | `{binding_id, expected_revision, target: GoalAttentionTarget-or-identity form (as goal/create), namespace?}` — witness fetched server-side | `{previous, attention}` |
| `meta/workgraph/attention/prune` | `{updated_before?: RFC3339, namespace?}` — deletes TERMINAL (superseded/stopped) binding rows only; the event stream keeps the audit history (meerkat 0.7.25, ask 24) | `{pruned}` |
| `meta/workgraph/attention/break_glass_reassign` | CONSOLE SURFACE ONLY. `{binding_id, expected_revision, target (as reassign), reason (required, non-empty), namespace?}` — principal is the AUTHENTICATED console principal (never a wire param; unauthenticated consoles get `-32030`); bypasses the mode-derived authority for the one unrecoverable state (binding stuck on a wedged/retired agent with no coordinator); upstream records principal+reason in the workgraph event stream + WARN log (meerkat 0.7.25, ask 23). Not present on the host stdin surface or in its capabilities catalog. | `{previous, attention}` |

## Errors

- Service not configured: JSON-RPC error code `-32041`,
  `data.kind = "workgraph_unavailable"` (memory-backend-unavailable pattern).
- CAS/revision conflict (upstream Conflict): code `-32042`,
  `data.kind = "workgraph_conflict"`, `data.detail` carries upstream message.
  Both SDKs raise `WorkGraphConflictError`; neither automatically retries.
  Callers must inspect the conflict, refetch the relevant item/binding state,
  reconsider the operation, and explicitly retry with the appropriate current
  revision if still valid. A revision refresh does not resolve every conflict
  (for example, target occupancy). The console displays the failure and
  best-effort refreshes the inline card for the next operator action; it does
  not replay the failed write.
- Other WorkGraphError: `-32000` internal with `data.kind = "workgraph_error"`,
  full detail (K2 disclosure posture).
- Invalid params: `-32602` (standard).
- Member of another mob as attention target: `-32602` with
  `data.kind = "attention_target_realm_mismatch"` and the typed fields
  `owner_key`, `mob_id`, `required_realm_id` (`mob.<mob_id>`), `realm_id`
  (this runtime's realm). Raised by Meta before the write and by Meerkat's
  `WorkGraphError::AttentionTargetRealmMismatch` if a write reaches it.
- ABAC denial (console): `-32030` `access_denied` (standard).

## Capabilities + experience

- `meta/capabilities` result gains `"workgraph": true|false` (service
  configured) beside `identity_first`; workgraph methods appear in the
  advertised methods list only when configured.
- `GET /console/experience` gains:
  `"workgraph": {"available": bool, "can_view": bool, "can_manage": bool}`
  (available = service configured; can_* = ABAC-intersected for the caller,
  true when access control disabled).

## ABAC

New actions in the vocabulary: `workgraph.view`, `workgraph.manage`.
Reads map to view; mutations to manage. Deny-by-default when enabled; admins
bypass as usual.

## SDK method names

Python (MobHandle): `workgraph_snapshot, workgraph_list, workgraph_get,
workgraph_ready, workgraph_events, workgraph_attention_list,
workgraph_goal_status, workgraph_create, workgraph_update, workgraph_claim,
workgraph_release, workgraph_close, workgraph_block, workgraph_link,
workgraph_add_evidence, workgraph_escalate_policy, workgraph_goal_create,
workgraph_goal_confirm, workgraph_goal_request_close,
workgraph_attention_pause, workgraph_attention_resume,
workgraph_attention_reassign, workgraph_attention_prune` (prune returns the
pruned row count as `int`).

TypeScript: same set camelCased (`workgraphSnapshot`, ...,
`workgraphAttentionPrune`).

`attention/break_glass_reassign` is deliberately NOT in either SDK: it is a
console-operator recovery act whose principal comes from console
authentication; the SDK stdin surface has no principal to attribute.

Typed results both SDKs: `WorkGraphItem`, `WorkGraphEdge`,
`WorkGraphAttentionBinding`, `WorkGraphSnapshotResult`, `WorkGraphGoalResult`
(= item+attention), `WorkGraphAttentionReassignResult` (= previous+attention),
`WorkGraphEventEntry` (fields: seq, realm_id, namespace, item_id?, kind, at, payload — upstream `WorkGraphEvent`), `WorkGraphItemsResult`. Tolerant parsing (`.get()` /
optional fields); retain `revision` fields verbatim.

Builder opt-out both SDKs: Python `MetaBuilder.workgraph(enabled: bool)`,
TS `.workgraph(enabled: boolean)` → `runtime_options.workgraph` (bool,
default true; the gateway also accepts a STRING directory for an explicit
durable store location (`workgraph.sqlite3` created inside) —
identity-first launches without a state dir are otherwise memory-backed,
warned at boot).

Typed `CapabilitiesResult` in both SDKs carries the `workgraph: bool` flag.

## Console

Headless command names (console/src/lib/headless.ts):
`workgraphSnapshot`, `workgraphGet`, `workgraphGoalStatus`, `workgraphClaim`,
`workgraphRelease`, `workgraphClose`, `workgraphGoalConfirm`,
`workgraphGoalRequestClose`, `workgraphAttentionPause`,
`workgraphAttentionResume`, `workgraphAttentionReassign`,
`workgraphEvents` → the RPC methods
above (CONSOLE_RPC_METHODS entries in console/src/lib/contract.ts).
`workgraphGet`/`workgraphGoalStatus` also back the operator-action CAS
resolution path (console/src/lib/workgraph-actions.ts): when the UI never
observed a revision it fetches the live one instead of guessing 0.

Inline card entry kind: `"workgraph"` (`ConversationWorkGraphEntry`), one
evolving entry per goal/root work-item, aggregated from workgraph tool-call
frames. Operator actions on the card gated by
`experience.workgraph.can_manage` + `!consoleReadOnly`.

## Implementation notes (as-built, 0.7.30)

These are historical notes. In particular, the bare realm and narrower
namespace restriction below are superseded by the
[current scope qualification](#current-scope-qualification-meerkat-0840).

- `claim.owner`: the wire accepts BOTH the flat `{kind, id, display_name?}`
  form and upstream's nested `{key: {kind, id}, display_name?}`; results
  always serialize the nested upstream shape (canonical).
- `goal/confirm` with absent `evidence`: the evidence kind is derived from
  the goal's completion policy (matching the machine's confirmation
  admission). Wire-supplied evidence is restricted to
  `{kind, id, label, summary}` — `confirmation_kind` /
  `confirming_owner_key` are rejected (reserved-classification smuggling);
  the service stamps the canonical classification. PrincipalConfirmed
  policies confirm only on the console surface (authenticated principal);
  the host-trusted stdin surface has no principal.
- `realm_id` is rejected (-32602) on every method — the service is
  realm-scoped at construction (mob definition id), and agent tool calls are
  scope-pinned (`ScopePinnedWorkGraphTools`) for the same reason.
- Upstream request fields the tables omit (e.g. `goal/create`
  `projection_policy`, `update` `external_refs`, `claim`
  `lease_expires_at`) are accepted verbatim.
- Library-mode spec constructors and `UnifiedRuntimeBuilder` also wire
  workgraph (not just the gateways) — a profile with `tools.workgraph=true`
  and no dispatcher is a fail-closed member-build error upstream.
- ONE BINDING PER TARGET, enforced across `goal/create`,
  `attention/reassign` (the new target), and `attention/resume`: a second
  ACTIVE — or PAUSED, since pauses expire — binding whose target aliases the
  same member (session and identity forms are unified through the roster)
  returns `-32042` naming the existing binding. As of meerkat 0.7.25
  (ask 25) the uniqueness invariant is OWNED UPSTREAM: the store enforces
  at-most-one-ACTIVE-binding-per-target transactionally inside the same
  write that mints or revives a binding, with a typed `Conflict` naming the
  occupant (mapped to `-32042` here), and `MultipleActiveBindings` is gone —
  legacy duplicates arbitrate newest-wins with a loud diagnostic instead of
  hard-failing every turn. Meta's admission (`WorkGraphAdmission`) is now
  DEFENSE-IN-DEPTH plus the pieces upstream does not own: session↔identity
  target aliasing through the roster/shared-session-store (upstream
  uniqueness keys the literal target), Paused-occupancy, and friendlier
  pre-write conflicts on the RPC and tool planes. The check-then-act is
  serialized per runtime, and — for SQLite-backed stores, which two
  processes may share — cross-process via a `workgraph.admission.sqlite3`
  sidecar lock beside the store (candidate for removal in 0.7.31 now that
  the store owns the invariant for literal targets).
- `goal/create`, `attention/*`, and `policy/escalate` reject non-default
  `namespace` (`-32602`) — upstream turn-overlay resolution only reads the
  service default namespace, so goals elsewhere would be silently inert.
- `attention/reassign` is restricted by upstream's authority model to
  COORDINATE-mode bindings (the witness's `can_link_derived_from` derives
  from mode); other modes get a precise error, and the console hides the
  affordance. The host-plane escape hatch shipped in meerkat 0.7.25 as
  `break_glass_reassign_attention` and is exposed ONLY on the console
  surface (see the methods table) — never to agents.
- Upstream semantics consumers should know: `due_at` is an ELIGIBILITY time
  (claims guard-reject until `due_at <= now`), not a deadline; an
  attention-bound member turn's provider-visible tools are hard-filtered to
  the binding mode's workgraph allow-set for that turn; there are no push
  events at meerkat 0.7.23 — poll `snapshot`
  (`event_high_water_mark`) + `events(after_seq)`.
