# Durable frontend delivery

`meerkat_toolkit.frontend.egress.ProactiveEgress` is the shared SQLite output
owner, extracted from an existing host. Hosts supply their recipient policy, formatting
normalizer and provider adapters. Service connectors
remain a separate role: this module delivers an agent's output through a frontend;
it does not authorize a service action or admit an inbound request.

The host must authorize the exact recipient and payload before committing output.
An output row, sender selection, prompt key or provider receipt grants no authority.
Native admission and C1 integration are not implemented by this module.

## Current authorization before delivery

For a frontend that checks current permission, construct the owner with
`authorization_mode="enforced"` and `authorize_delivery=...`. The callback is
required in this mode and must return an exact `EgressAuthorizationDecision`
(`ALLOWED`, `DENIED` or `DEFERRED`) or an `EgressAuthorizationRefusal(reason)`.
It can be synchronous or asynchronous.
Supplying a callback in legacy mode is rejected, rather than silently leaving
it unused.

Before each initial, retried or recovered provider attempt, the callback receives
an immutable `EgressDeliveryContext`. It contains the retained output ID, issuer,
logical key, request digest, sender, recipient, optional thread and exact part
index/payload, plus recursively immutable retained `host_values` when a host
extension is configured.
The host resolves those associations through its existing request, audience,
subscription, approval and resource-policy owners. A string identifier or digest
is correlation data, never evidence that the request is authorized.
The digest identifies the original committed request; it is not recomputed from
the reconstructed part. Existing formatting recovery can discard corrupt optional
formatting. The callback sees the resulting candidate that will actually be
passed to the sender. A media reference identifies host-owned content, not frozen
media bytes; the host must retain its content and access-control guarantees.

Only `ALLOWED` permits an attempt reservation. `DENIED`, `DEFERRED`, an unavailable
callback, malformed result or invalid retained association leaves the part
queued without spending a provider attempt. Errors produce fixed diagnostics,
not policy exception details. Policy may later change, so denial alone does not
create a permanent cancellation record. The existing output freshness limit
still applies. Already accepted parts are never retransmitted. A completed
UNKNOWN can be authorized again only under its persisted remaining resend policy;
the new attempt still requires a fresh authorization and reservation.

`EgressAuthorizationRefusal(reason)` is a terminal refusal of this exact
candidate, distinct from a temporary `DENIED` or `DEFERRED` result. Its reason
must be a bounded ASCII code matching `[a-z][a-z0-9_]{0,63}`, not provider or
policy exception prose. Refusal prevents further entry without reserving an
attempt. It does not erase an earlier possible effect: raw `refused` can coexist
with retained uncertainty and public `delivery_state="unknown"`. A refusal is
never evidence that an earlier entered attempt was not sent.

The authorization await uses the configured dispatch timeout. Cancellation
before reservation propagates without creating delivery uncertainty. These are
cooperative asynchronous bounds; a blocking or cancellation-resistant host
callback requires host isolation. Authorization does not make an external policy
lookup and provider acceptance atomic or recall bytes already released.
When using `SenderAuthority`, route selection waits before authorization. A
changed selection or lifecycle transition during authorization defers entry;
there is no second sender-selection wait after the final check and reservation.

The host must keep enforcement enabled after every restart and prevent competing
legacy owners. Default `authorization_mode="legacy"` preserves existing callers
and makes no current-authorization guarantee. The mode is host configuration,
not a persisted grant or a downgrade-prevention marker. Enabling this callback
does not establish native admission, live provider visibility evidence or full
host adoption by itself.

## API and composition

Import `ProactiveEgress` and `EgressStatus` from
`meerkat_toolkit.frontend.egress`, and `SenderAuthority` from
`meerkat_toolkit.frontend.sender`.

The constructor takes a SQLite `path`, host `incident_sink` and `health_sink`, an
injected `sleep`, and sender implementations. A host using durable sender
replacement passes its existing `sender_authority` and an empty `senders` mapping.
The mapping form remains useful for a host with fixed sender composition. Sender
identities and eligible recipient candidates are host-owned; ordering candidates
is explicit host policy, not agent-intent inference.

`commit_output(issuer_id=..., logical_key=..., candidates=..., parts=...,
incident_delivery=...)` commits output before any provider call. Repeating the
same issuer/key returns the same output ID only for the same candidate list,
normalized parts and incident flag. Changing those inputs raises `ValueError`.
Each output's durable root is `<prefix>:<issuer_id>/<logical_key>`. The prefix
is host configuration, `root_prefix=` (default `notify-v1`): a host adopting an
existing database passes the prefix its rows already carry, on every reopening
and to `migrate_format`. The request digest does not depend on it.

`spawn_dispatch(output_id)` starts a retained task. A host with a short request
budget can stop waiting while that task continues; caller abandonment must not
cancel it or create a new output. `status(output_id=...)` reads the canonical
per-part states. `send(...)` commits and awaits dispatch inline. Recovery uses the
same rows through `recover_pending()`, `run_recovery_pass()` and the existing
recovery drainer, not another queue.

### Retain host fields in the same output

`EgressHostExtension` is a trusted host descriptor for an existing schema and
pure codecs. Keep schema and codec selection in trusted host Python code,
outside package manifests and model output. Import it,
`EgressAuthorizationRefusal`, `EgressDeliveryContext` and
`EgressDeliveryPolicy` from `meerkat_toolkit.frontend`. Pass the same descriptor
as `host_extension=` to `inspect_format`, `migrate_format` and `ProactiveEgress`.
Construct the owner with `schema_mode="managed"`; extension mode refuses the
default legacy initialization path.
This is an SDK source interface; existing wheels and installed hosts retain
their previous behavior until explicitly updated.

The descriptor declares `output_columns` as exact TEXT definitions,
`part_columns` as names of fields stored as canonical JSON, and `tables` and
`indexes` as exact declaration-only DDL. Core definitions cannot be replaced.
Undeclared or incompatible egress objects are refused. Toolkit executes no host
DDL, creates or repairs no host table, and does not write the host's auxiliary
rows. The host prepares its schema before explicit managed adoption; extension
construction requires a matching managed format and cannot create a missing
database or silently adopt an old one.

Its synchronous callable fields are:

| Callable | Input and result |
| --- | --- |
| `normalize_output(values)` | Frozen declared values to the complete output mapping; values are text or `None` and must satisfy the declared columns |
| `normalize_part(fields)` | Frozen declared extra part fields to their canonical mapping |
| `derive_output(output_id, values, parts)` | The newly allocated ID, frozen values and frozen part tuple to the complete output mapping, only on first insertion |
| `digest_fields(values)` | Frozen retained output values to request-digest fields named only by declared output columns; undeclared names and collisions with core digest keys are refused |

These codecs must be pure, bounded and synchronous. They receive no SQLite
connection or authority to perform effects. Classification, authenticated caller
identity, recipient policy, choice validation and ownership of any referenced
content stay with the host.

Supply new-output metadata through `commit_output(..., host_values=...)` or
`send(..., host_values=...)`. The output, normalized parts, derived fields and
selected delivery policy commit in the existing SQLite transaction. A codec or
insert failure rolls back the insertion. A same-key replay does not derive or
overwrite metadata again: it uses the validated retained values for the host
digest contribution while recomputing core candidates, parts and incident flag
from the request. Changed payload still conflicts. Concurrent first commits
converge on the winning row; replay cannot change its caller, recipient, policy,
receipt, uncertainty history or consumed quota.

For a host's historical encoding, its codec contributes
`{"delivery_class": "ordinary_reply"}` only for the retained ordinary-reply
class; its governed branch contributes nothing. The generic owner does not
choose that classification or accept either digest indiscriminately. Retained
`host_values` in authorization context are correlation facts to resolve through
current host policy, never an authorization grant.

The descriptor's `pass_output_id=True` opts into passing the committed ID to
the sender. Its default is false, preserving the old call shape. The callable
must explicitly name a keyword-capable `output_id` parameter; `**kwargs` alone
is insufficient. Choice-bearing parts require its `supports_choices` attribute
to be exactly `True`. Both checks use the current borrowed sender before attempt
reservation. `SenderAuthority.supports_choices(sender_id) -> bool` is an early
hint only: it cannot authorize entry or replace the check on the actual borrowed
selection.

### Detached task context

`spawn_dispatch` and the background recovery drainer start their tasks in a
fresh `contextvars.Context()`. This applies with or without a host extension.
They do not inherit ambient caller context from the task that starts them.
Hosts must resolve required caller and policy facts from the explicit retained
delivery context; callbacks that relied on unrelated ambient context must be
adapted before updating the SDK. Inline `send` remains an awaited call. Task
retention, Stop, attempt reservation and recovery ownership are unchanged.

### Retain an explicit thread destination

Use `EgressCandidate(sender_id="telegram", recipient="-10077", thread_id="42")`
when the host has selected and authorized a particular thread. Import it from
`meerkat_toolkit.frontend`. Candidates with no thread, including the existing
`(sender_id, recipient)` tuples, retain the original digest and dispatch shape.
The generic owner treats a thread as exact nonblank UTF-8 text, at most 1,024
bytes, without control characters. Provider adapters validate their own form;
the Telegram adapter requires a canonical positive bounded integer string.

The selected thread is stored on the existing output row and included in the
original request digest. Changing only the thread conflicts with the same
issuer/key. Reopening the queue and removing refused formatting preserve the
destination. Each authorization callback receives `context.thread_id` along
with the recipient, so the host can check the pair against current policy.
Inbound thread metadata alone never grants outbound authority.

Thread-bearing output requires the managed `egress-sqlite-4` format. The sender
must explicitly declare a keyword-capable `thread_id` parameter. A generic
`**kwargs` wrapper alone is insufficient: it could silently discard the target.
Unsupported senders leave the output queued before any attempt reservation or
provider call. Compatible senders receive `thread_id` only when it is present;
they must forward it without falling back to the parent conversation. Signature
compatibility is not proof of correct adapter behavior.

### Stop one queued output

`cancel_output(output_id=..., issuer_id=...)` commits a permanent stop intent for
one existing output and returns `EgressStatus`. The host must authenticate the
actor and authorize control of that exact output. Matching its retained issuer
is an additional association check, not a grant. Unknown outputs and issuer
mismatches raise `ValueError` without mutation. Repeating the call is idempotent;
replaying `commit_output` with the same key and payload cannot resume it.

Cancellation and attempt reservation serialize in SQLite. After cancellation
commits, no new attempt can be reserved for that output, including later parts,
retries and recovery. Safely unentered or explicitly refused pending parts become
`cancelled` only if no earlier uncertain attempt exists. An already reserved attempt becomes `unknown`; its late receipt may
still refine that same attempt to `sent`, `unknown`, or, for an explicit refusal,
`cancelled` if no earlier uncertainty remains. Existing accepted, uncertain, failed and expired facts are preserved.
`cancel_requested` exposes intent separately from `part_states`; it does not
assert that every part was cancelled or recalled.

A reservation that committed first may still enter or complete after Stop.
The operation does not cancel its task, wait for the provider, recall released
bytes, stop the agent run, revoke a subscription, or authorize a replacement
send. The host owns those separate decisions. There is no resume or bulk-cancel
operation. Stopped outputs do not count as actionable pending backlog; their
per-part facts remain queryable.

Default parts support text or a media reference. Rich formatting is host-owned:
an optional `normalize_parts` callback validates and normalizes the complete
sequence, including `entities` and `caption_entities`. The owner freezes the
result before computing its digest and preserves the host's text and entity
offsets together, without further Unicode normalization. Without a host
normalizer, plain text retains its existing NFC normalization. Historical rows
are not rewritten; replay with a different rich payload is refused. An adopting host
supplies its original normalizer; provider-specific formatting and media storage
stay in the host.

## Outcomes and retry evidence

A sender receives `sender_id`, `recipient`, `part_index` and `part`, plus
`thread_id` for an explicitly threaded output. It returns a
mapping or object with an exact string `outcome`, optional string
`provider_message_id`, and optional `error`.

Each sender call must perform at most one provider send. In particular, provider
adapters must not hide a formatting retry inside that call: a second send needs
another current authorization and durable attempt reservation.

| Provider observation | Durable meaning | Automatic retransmission |
| --- | --- | --- |
| `sent` or `acked` | Provider accepted that part; receipt retained | No |
| `known_not_sent` | Provider adapter explicitly proves this attempt had no effect | At most four total attempts, with 5/30/120-second backoff, unless stopped |
| `unknown` | Attempt may have had an effect | None by default; only the output's explicit bounded policy permits a resend |
| Missing/malformed outcome, invalid receipt, exception, timeout or cancellation after entry | No proof of non-delivery | Same persisted policy as `unknown` |

The per-attempt reservation is committed before provider entry. A process that
exits after an external effect but before recording a receipt therefore leaves
uncertainty. Ordinary recovery after reopening retires that unresolved pending attempt as
`unknown`; it does not send again while the reservation remains open. A missing sender before reservation leaves the
output queued without consuming an attempt.

### Select a bounded uncertainty policy

`EgressDeliveryPolicy()` defaults to `at_most_once` with zero UNKNOWN resends.
This is the ambiguity policy: proved `known_not_sent` attempts can still retry.
A host that accepts possible duplicates can explicitly select
`EgressDeliveryPolicy("at_least_once", unknown_resends=1)` for one output with
`commit_output(..., delivery_policy=...)` or `send(..., delivery_policy=...)`.
The constructor policy is only the default for newly committed outputs when the
argument is omitted or `None`; an explicit `EgressDeliveryPolicy()` can select
conservative behavior on an owner with an opted-in default. The bounded profile accepts 1-3 resends,
always inside four total provider attempts per part and the existing
5/30/120-second backoff. It is a bounded delivery effort, not a guarantee that
the recipient receives a message. A host can select exactly one, for example
for conversational Telegram replies only. It does not cover consent- or
approval-governed effects, which still require resolved uncertainty or applicable
idempotency. This frontend policy is not a retry policy for connector writes.

The mode and limit are committed atomically with each new output. Reopening with
a different constructor setting, or supplying a different selection on the same
issuer/key, cannot change that output's saved policy, consumed quota, history or
receipt. Concurrent first commits converge on the policy saved by the winning
insert. Read `status(output_id=...).delivery_policy` to observe the retained
selection rather than infer it from the latest request. Policy selection does
not change the historical request digest. Only a new opted-in output requires
the managed format: an unchanged replay of an existing conservative or old NULL
policy row remains valid with a wider requested selection. Invalid policy values
are rejected before persistence, including on replay. Policy selection also
does not enable authorization: hosts requiring every-entry policy checks must
explicitly compose `authorization_mode="enforced"`, `authorize_delivery`, and
their current `SenderAuthority` selection. A retry re-enters the same owner
selection, authorization, freshness, cancellation and reservation path.

This local host policy supplies no native consent or post-UNKNOWN exception.
The ordinary conversational reply scope is settled separately from protected
effects; no native-consent exception or relaxation is authorized. Native
integration/adoption remains unproven. The host must authorize each actual
provider entry under current policy, and a retained budget cannot bypass it.

The UNKNOWN resend quota is consumed atomically when reserving the new attempt,
not when its result arrives. Later refusals or reopening do not replenish it.
`status(...).uncertain_attempts` retains at most four typed observations per part,
including their attempt numbers, fixed reason codes and optional provider receipt
ids. It stores no provider prose, raw response or exception representation.
Earlier uncertainty remains after later refusal, Stop or expiry. A later accepted
attempt can establish provider acceptance while this history still records the
possible duplicate. The current `error` field keeps its existing host diagnostic
contract; adapters must sanitize sensitive provider diagnostics before return.

Completed UNKNOWN observations may resume under the retained policy through
ordinary recovery. An open reservation needs
`recover_pending(previous_dispatches_quiesced=True)`: an exact-bool assertion by
the host that every previous writer and dispatch has stopped. This is not a
lease, process fence or fact inferred from age/local locks. The owner captures
at most 1,000 exact open `(output_id, part_index, attempt)` identities before its
first provider await, conditionally records `result_lost`, then uses ordinary
retry admission. It never sweeps newly reserved attempts into that captured set.
Any remaining rows stay conservative until a later correctly quiesced pass.
Default and background recovery never grant an open reservation retry authority.

Explicit `cancel_output` always prevents new reservations. An asyncio task's
cancellation after provider entry records uncertainty and propagates cancellation;
it does not infer user Stop. A host handling user Stop must commit the durable
intent. Old rows with no saved policy expose `delivery_policy=None`: they retain
the prior conservative behavior, receive no new UNKNOWN resend authority, and
do not manufacture missing historical observations during migration.

When a closed, opted-in part ends uncertain, the existing `incident_sink` receives
`egress_delivery_unknown` with output/part identifiers and counters. Stop and
incident-delivery outputs do not generate recursive exhaustion incidents. A
competing live handle cannot report an open reservation as exhausted. The part
and its uncertainty history commit before callback entry and survive a failed
callback. This callback is not an incident outbox: a return does not prove durable
host acknowledgement, and recovery does not promise callback redelivery. The
host must use its durable incident owner and reconcile missing acknowledgements
from retained delivery observations.

The distinct health event `egress_terminal_unknown` reports a persisted,
settled terminal uncertainty fact for either saved delivery policy, including
conservative delivery. It is separate from an immediate attempt-UNKNOWN event
and from the existing opt-in exhaustion incident callback. Its safe identifiers,
state, policy and counters let the host route the fact through its own incident
policy. Alongside `kind`, its top-level fields are `output_id`, `part_index`,
`state`, `attempts`, `unknown_resends_used` and `delivery_policy` (a mapping with
`mode` and `unknown_resends`). No schema declaration selects an incident severity
or repeat window.
Incident-delivery outputs and open provider reservations are excluded. Settled
historical uncertainty remains eligible after Stop; requesting Stop alone never
settles an open reservation.

Neither a successful health callback nor a failed callback proves durable host
acknowledgement. Exactly-once incident handling requires the host's existing
durable deduplication and reconciliation, not a synchronous Toolkit return.
The host owns its outbox and incident drain cycle/repeat-window keys. Callback
failure cannot authorize another business send.

For host reconciliation, `report_terminal_uncertainty(output_id) -> int` re-emits
the currently settled terminal facts through `health_sink` using the same
predicate. Its count records callbacks that returned, not durable acknowledgements;
callback failures propagate with delivery rows unchanged. Repeating the call can
emit the same fact again. The host callback can have effects, so this is a
state-preserving operation, not a read-only one. It does not dispatch or invoke
the legacy incident callback. During ordinary reporting, an eligible
legacy incident callback remains independent of a failure in the new health
sink.

A sender may return the exact boolean `retry_without_formatting=True` with
`known_not_sent` when the provider explicitly refused the formatted request.
The owner records that refusal and removes only `entities` and
`caption_entities` from the retained next candidate in one conditional SQLite
update. Text, caption, media reference, destination and the original request
digest do not change. The normal retry loop then checks freshness, selects the
current sender, authorizes the plain candidate and reserves its own attempt.
The existing four-attempt budget and backoff still apply. Recovery uses the
saved plain candidate; it does not repeat the refused formatted request.

This hint is neither permission nor an arbitrary replacement payload. It has
no effect on an accepted, queued, unknown or malformed observation. The
[Telegram presenter](TELEGRAM.md#portable-interpretation-and-presentation)
produces it for a complete explicit 400 refusal of a request that carried valid
formatting. A composing sender must forward the hint and translate Telegram's
`provider_accepted` to `sent`, while preserving `known_not_sent` and mapping
`provider_queued` to `unknown`. A wrapper that drops the hint cannot obtain this
plain-formatting retry. Hosts must adopt the presenter and owner contract
together; older already-built distributions are unchanged.

`EgressStatus.delivery_state` is `provider_accepted` only when every part is
`sent`; any `unknown` part or prior uncertain attempt in a part without later
acceptance makes it `unknown`, otherwise pending parts make it
`queued`, otherwise any cancelled part makes it `cancelled`, and remaining
terminal failures make it `failed`. `part_states` retains mixed outcomes and
`cancel_requested` independently records a stop request. For example, a fully
accepted output remains `provider_accepted` after Stop. Provider acceptance does
not prove transport delivery or that a person read the message.

A host's durable notification result can carry the same all-parts-accepted
meaning, with `delivery_state`, `output_id` and `part_states`. A host facade that
keeps an older return signature should report unconfirmed durable output as a
non-retryable error with those observations and same-key guidance. Callers may
observe the same key and exact payload; they must not automatically manufacture
a fresh send after uncertainty or partial acceptance.

## Persistence and lifecycle boundary

The existing `egress_outputs` and `egress_parts` tables remain authoritative.
For unmarked databases, default construction performs schema inspection, table/index creation, column upgrades
and legacy row backfills in one `BEGIN IMMEDIATE` transaction. A failed migration
rolls back that whole change; competing constructors wait before inspecting the
schema. This is local SQLite migration atomicity, not deployment quiescence or
permission to roll back shared state. A host that pinned an earlier wheel retains
the predecessor until it builds and adopts an explicit successor.
See the [failure and crash verification](../../docs/verification/20261003-egress-atomic-migration/REVIEW.md).
Migration adds nullable `last_outcome`; it never backfills a refusal for old
attempted rows. Existing receipts, uncertainty counters, errors and terminal rows
are preserved. Freshness expiration applies only to eligible unattempted output
or recorded known-not-sent attempts. A stale sweep cannot overwrite a concurrent
accepted receipt. Default freshness is 900 seconds, dispatch timeout 60 seconds
and recovery interval 30 seconds.

### Managed state format

Hosts integrating package updates use the same owner's explicit format API:

- `ProactiveEgress.inspect_format(path=...)` reads the actual schema and any
  committed format evidence. It distinguishes an absent database, absent
  namespace, recognized unmarked legacy/current layouts and managed state.
  It creates no namespace and performs no recovery dispatch or backfill.
- `ProactiveEgress.migrate_format(path=..., expected=..., binding=...)` adopts
  a recognized unmarked/absent namespace into `egress-sqlite-4`, or explicitly
  upgrades a validated `egress-sqlite-1`, `egress-sqlite-2` or `egress-sqlite-3` namespace. The host supplies
  the prior observation and an `EgressMigrationBinding` containing target
  manifest, owner artifact and migration artifact digests, with an optional
  previous manifest digest. The host must first verify those selections and
  quiesce every relevant writer. The binding is historical correlation, not
  proof of verification or authority. Adoption also requires every existing
  output root to carry the host's `root_prefix=`.
- `ProactiveEgress(..., schema_mode="managed")` requires an existing valid
  committed v4 format and performs no DDL or backfill. Subsequent connections
  cannot recreate a removed database. Default construction also validates a
  managed record when present; it cannot silently repair incompatible managed
  state.

The fixed namespace is `channels/egress`, its owner is `frontend/egress`, and the
current adoption operation is `adopt-egress-sqlite-4`. The owner recognizes a bounded
set of its own schema layouts; a marker alone cannot establish compatibility.
Stale observations, unknown formats and schema/record disagreement are refused.
Explicit adoption commits schema, backfill and the `egress_format` row together.
The v3 upgrade adds nullable output policy and per-part reservation, quota and
uncertainty fields without inventing old history or retry permission. A v2
upgrade also adds a nullable thread destination; a v1 upgrade also adds the
cancellation flag, initially false. All preserve prior delivery fields. The
transition records `source_state="managed_v1"`, `"managed_v2"` or `"managed_v3"` and the
exact predecessor observation digest, including the prior transition and binding.
This is a link, not storage of the full earlier record; retain the prior
observation in the host's migration evidence. An already managed v4 namespace
cannot be relabeled or migrated again. After a lost return, inspect it and compare
the committed binding. Inspecting valid v1/v2/v3 remains read-only; either constructor
mode refuses to open it for delivery until explicit migration.

The host retains activation, writer exclusion, startup and rollback authority.
An observation does not freeze business rows or fence old code. A removed format
table loses history for legacy callers, so managed startup must refuse unmarked
state. Fresh Toolkit predecessor constructors reject v4, but old host code can
ignore new fields or alter old counters while leaving history stale; quiesce and
fence every writer before migration. Additive SQL readability does not prove
semantic rollback compatibility. A host's own channel database migration and
old/new read-write rollback remain host adoption requirements, separate from
this Toolkit format contract. Compatible code
rollback retains the progressed database and its transition evidence. Fresh-file
failure can leave an empty file. SQLite journal and WAL behavior remains subject
to the filesystem environment.

For host extensions, a supported transition policy covers only a stopped-writer
domain with a known retained `egress_format` and admitted legacy writes.
Re-adoption must preserve unchanged managed state, merge consumed quota by
maximum, union uncertainty/history, and keep cancellation sticky. A terminal
managed row contradicted by later legacy state must refuse. The format marker
and counters alone may not reveal overwritten terminal facts: the host must
retain enough owner evidence to detect those contradictions or refuse the
unsupported domain, for example with an adoption-time per-output snapshot. The
descriptor and generic migration API do not implement that transition, prove
the old/new reader roundtrip, or authorize deployment.

[64 source cases and two real process-kill checks](../../docs/verification/20261003-egress-owner-format/REVIEW.md)
verified the predecessor v1 owner contract. A separate private wheel passed all 64 owner
cases after offline installation with verified parent and child imports.
Existing library archives retain their earlier wheels, and a host's
activation/startup integration is its own work. This component
verification does not validate every SDK change in the new wheel.

Timeout and shutdown behavior assumes cooperative asynchronous provider adapters.
The host owns process shutdown and must retain/join dispatched tasks or allow
recovery of their durable uncertainty after process loss. SQLite calls run
synchronously. This is not a hard wall-clock isolation boundary for blocking or
cancellation-resistant provider code.

One host lifecycle owner must compose each live sender. Atomic row reservations
protect an individual part from duplicate entry across SQLite handles, and the
existing `SenderAuthority` coordinates replacement within its owning instance.
These facts do not establish distributed process fencing, global multipart order
across independent owners or provider-side exactly-once delivery. A second live
process cannot infer lifecycle ownership from these rows.

### Replacing a sender

`stage_activation(sender_id=..., to_owner_id=..., sender=...)` retains a candidate
without changing the active dispatch route, including when the owner ID stays
the same. Keep the exact returned `SenderActivation` object. It belongs to that
authority instance and candidate; copied or reconstructed handles are refused.
Dropping an unused or superseded handle releases Toolkit's reference to that
candidate. A published sender stays attached even if its handle is dropped.
This object lifetime rule does not deactivate providers or undo external effects.

`await commit_activation(handle)` activates that candidate, checks the durable
selection in a SQLite transaction, synchronizes the database file and then
publishes the new local attachment. A newer local attachment invalidates older
handles. Staging several candidates does not let one handle activate or close
another candidate. Exact-handle replay calls `activate` again, so implementations
must support the existing lifecycle replay contract. Reopening the database
retains selection but loses live attachments and handles: the host explicitly
attaches the selected implementation or stages a fresh candidate in that instance.

If `activate` fails or is cancelled, the prior local route is retained. This does
not prove that the candidate's lifecycle had no external effects. A file-sync error
after SQLite commit can leave a new durable owner selected but unpublished and
therefore detached. If the owner ID is unchanged, the previous local attachment
remains available. Only the exact still-current handle can retry that transition;
no rollback or lifecycle compensation is implied.

`await close_activation(handle)` removes its candidate's exact local dispatch
route before awaiting provider deactivation. Durable selection and owner history
remain intact. Failed or cancelled deactivation propagates to the host and leaves
that route detached; it does not prove external cleanup. Already entered provider
calls can finish on their original implementation and retain their receipts.
The existing egress owner skips detached senders without spending attempts or
cancelling pending output. Its normal expiry and explicit output cancellation
policies still apply.

Lifecycle operations serialize per stable sender, so closing one sender does not
block or invalidate dispatch through an independent sender. Explicit
`await commit_activation(handle)` can reactivate the still-current candidate and
waits for an in-flight close of that sender. Synchronous `attach_active` refuses
the same implementation object while cleanup awaits deactivation, with
`sender implementation is still deactivating`; use the async activation path to
wait. This also applies when an activation loses its durable owner comparison:
it fences any matching local route before deactivating its candidate, without
changing the winning durable selection. A different synchronous attachment can
supersede the closing candidate,
and the old close cannot remove that replacement. Superseded handles remain
invalid. Attachment status remains a local routing fact, not readiness or
provider cleanup proof.
This scheduling independence assumes independently owned implementations. If
multiple stable sender IDs intentionally share an implementation or provider
resource, the host must coordinate that shared dependency's lifecycle.
The [sender replacement verification](../../docs/verification/20261004-sender-staging/REPORT.md)
records local SQLite, failure, cancellation and stale-handle evidence. Existing
host installations retain their previous bytes until explicitly updated.

## Caller adoption by a host

Approval prompts persist one delivery key per actual pending occurrence. Duplicate
observations reuse it. Before sending, the existing ledger owner synchronizes and
validates the exact captured key and argument fingerprint. Keyless legacy rows
remain unresolved; a failed append or sync cannot authorize a send. Prompted events
carry the captured key, so an old acknowledgement cannot mark a replacement
occurrence under a reused native pending ID. This bookkeeping does not decide an
approval or replace native gating.

Incident delivery observes/resumes an existing pending output instead of minting
a new cycle. Accepted, unknown and expired cycles do not authorize an automatic
new send. Only the existing known-failure cycle path, backed by explicit refusal
receipts for all failed parts, can create the next occurrence.

This extraction is implemented and locally tested. It does not establish live
provider delivery, publication, installation, native C1 availability or completion
of the Toolkit's full end-state requirements.
