# Changelog — meerkat-meta (Python SDK)

All notable changes to the Python SDK are documented here.

## Unreleased

- `MetaBuilder.provider_credentials_root(path)` sends
  `runtime_options.provider_credentials_root`: the gateway keeps every agent's
  provider credentials in that directory, an absolute path it opens once at
  init, instead of the process default `<config dir>/meerkat/credentials`,
  which every process of the OS user shares. Sent only when set; an empty
  path raises `ValueError`.

## 0.9.5

- Breaking: a tool's output moves from `ToolExecutionCompleted` to
  `ToolResultReceived`, following the gateway, which sends it once, on
  `tool_result_received`. `ToolResultReceived` gains `result`, its last field,
  and `ToolExecutionCompleted` no longer has it, so a positional pattern on
  `ToolExecutionCompleted` loses its `result` slot. Its later slots move up
  one: a 3-slot positional pattern now binds `is_error` where it bound
  `result`, without an error. Read a tool's output from the result event for
  the same `id`. An event from an older gateway still parses: a completion's
  `result` is ignored, and a result event without one reads `""`. Upgrade the
  SDK together with the gateway: a 0.9.4 SDK against a 0.9.5 gateway reads an
  empty `result` on the completion and loses the output.
- `LiveChannelHandle` and `PendingLiveChannelHandle` carry `advisories`, a
  tuple of what the live open went ahead without, empty when the gateway
  reported nothing. `"staged_prompt_refused"` means a run-start hook refused
  the member's staged create-time prompt, which stays staged for its first
  text turn. An advisory this SDK does not know is kept as the string the
  gateway sent; a field that is not a list of non-empty strings raises
  `ValueError`. `to_dict` writes the key only when it is not empty.
- `MobHandle.schedule_tools()` returns `ScheduleToolsResult`, the definitions
  of Meerkat's durable schedule tools from `meta/schedule/tools`: one
  `ScheduleToolDefinition` (`name`, `description`, `input_schema`) per tool.
  It raises `ValueError` on a malformed result.
- Mob announcements on `MobHandle`: `announce`, `announcement_get`,
  `announcement_subscription_{put,pause,resume,cancel,get,list}`,
  `announcement_delivery_{list,observe,reconcile}`,
  `announcement_source_{put,pause,resume,end,get,list,advance,record_failure}`
  and `announcement_audit_list`, one per `meta/mob_announcements/*` method.
  They return frozen result types (`AnnounceResult`, `Announcement`,
  `AnnouncementSubscription`, `AnnouncementBatch`, `AnnouncementSource`,
  `SourceAdvanceResult`, `AnnouncementAuditEntry`, ...) whose states, phases,
  statuses, outcomes, actions and kinds stay plain strings, so a newer
  gateway's value passes through; an announcer or route kind this SDK does
  not know keeps its object as `raw`. Optional parameters are sent only when
  set.
- `AnnouncementSourceView.source` is typed `AnnouncementSource | None`:
  `sources/get` answers `{"source": null}` for an id the mob does not have,
  so check it before use. The type is new in this release, so no earlier
  code relies on it being set.
- `MetaBuilder.mob_announcements(config=True)` sets
  `runtime_options.mob_announcements`: `{"enabled": true}` for `True`, a
  mapping unchanged.
- Typed refusals `MobAnnouncementsUnavailableError` (`-32044`),
  `AnnouncementConflictError` (`-32045`), `AnnouncementFrontierError`
  (`-32046`), `AnnouncementRefusedError` (`-32047`) and
  `AnnouncementNotFoundError` (`-32048`), with their `*_CODE` constants and
  the refusal's `kind`; a kind this SDK does not know keeps its code's class.
  A source's `stale_revision` raises `SourceStaleRevisionError`
  (`current_revision`, `current_version`, `generation`, `cursor`) and a moved
  cursor `SourceVersionConflictError` (`current_version`, `cursor`, `state`,
  `revision`), both `AnnouncementConflictError`s. A source method on a
  gateway without sources raises `RpcError` `-32601` saying "Meta does not
  support sources".
- `CapabilitiesResult.mob_announcements` (`MobAnnouncementsCapability`:
  `sources`, `local_delivery`, `local_delivery_kind`, `governed_admission`),
  `None` when mob announcements are disabled or the gateway does not report
  it, and `supports_announcement_sources(capabilities)`, true when
  `feature_capabilities` lists `mob_announcements.sources.v1`
  (`MOB_ANNOUNCEMENT_SOURCES_V1`).
- Delivery handling on `MobHandle`: `announcement_delivery_redeliver` and
  `announcement_delivery_dismiss(operation_id, batch_id)`, for
  `deliveries/redeliver` and `deliveries/dismiss`, return the batch's
  `AnnouncementBatchHandling` (`state`, `cause`, `redelivered`, `held`,
  `dismissed`, `read_at_ms`). A batch that holds nothing for an operator
  raises `AnnouncementRefusedError` with the kind `not_held`.
- `AnnouncementBatch.handling` is an `AnnouncementBatchHandling` instead of
  a dict, and `AnnouncementDeliveryObservation` gains `handling`. Both are
  `None` until the batch is admitted, and the state stays a plain string.
  The types are new in this release, so no earlier code relies on the
  dict.
- `AnnouncementAuditEntry.source_id` and `generation` are `None` for an
  entry of a plain announcement. The audit now also counts holds,
  redeliveries (`redelivered`) and dismissals (`dismissed`) of items
  whose turn ended unhandled (the reason `unhandled`), with or without a
  source.
- Mob announcement health on `MobHandle`: `announcement_member_get`,
  `announcement_member_list` and `announcement_escalations_list`, for
  `members/get`, `members/list` and `escalations/list`, and `escalated` on
  `announcement_source_list`. They return `AnnouncementMemberView`
  (`member`, `backlog` as `AnnouncementMemberBacklog`, `marker`,
  `escalations`) and `AnnouncementEscalationsPage` (`next`, and `events` of
  `AnnouncementEscalationEvent` or, with `open=True`, `escalations` of
  `AnnouncementEscalation`). Causes, events, evidence and subject kinds stay
  plain strings, and a subject is kept as sent.
- `AnnouncementSourceHealth` gains `escalations`, `quiet_monitoring`,
  `empty_coverage_monitoring` and `runner_scheduled_here`.
  `runner_scheduled_here` is `None` for a source without a runner, or from a
  gateway that does not report it.
- `StatusResult.mob_announcements` (`MobAnnouncementsStatus`: `sources`,
  `members`, `open_escalations`, `last_event_cursor`) is `None` in three
  cases: mob announcements are disabled, their database could not be read,
  or the gateway does not report it.
- `ErrorCategory.MOB_ANNOUNCEMENT_ESCALATED` and
  `ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED`, with their messages.
  The resolution is in `RESOLUTION_ERROR_CATEGORIES`, so
  `ErrorEvent.is_resolution` is true for it.
- Declarations and role templates on `MobHandle`:
  - `announcement_declarations_apply(declarations=None, *, revision=None)`
    and `announcement_declarations_get()`, for `declarations/apply` and
    `declarations/get`, return `AnnouncementDeclarationsView` (`revision`,
    `digest`, and `records` of `AnnouncementDeclarationRecord`).
  - `announcement_role_template_put(operation_id, template_id, members,
    topics, *, mobs=None, expected_revision=None)` sends
    `subscriptions/put` with `members` (`{"mob_id", "role"}` or an
    `AnnouncementRoleSelector`) and returns `AnnouncementRoleTemplate`.
  - The refusals `declaration_rollback`, `declaration_digest_conflict`,
    `role_template_limit` and `subscription_id_taken` keep
    `AnnouncementRefusedError`.
- `AnnouncementSubscriptionChange.subscription` is
  `AnnouncementSubscription | None`: the subscription verbs on a role
  template's id answer `template` and `children` instead.
  `AnnouncementSubscriptionView` gains `template`. The type is new in
  this release, so no earlier code relies on `subscription` being set.
- `announcement_subscription_put` takes `delivery`
  (`{"await_handling": bool}`), sent only when set: whether the member's
  next batch waits until the turn of its previous one has ended.
  `AnnouncementSubscription` gains `delivery` and `await_handling`, `None`
  when the subscription did not set them.
- `AnnouncementMemberView.handling` (`AnnouncementMemberHandling`:
  `pending`, `handled`, `handled_unverified`, `unhandled`, `held`,
  `oldest_unhandled_at_ms`) is `None` from a gateway that does not report
  it. `MobAnnouncementsStatus.handling` counts admitted batches by handling,
  and is empty from such a gateway. The escalation causes `unhandled` and
  `handled_unverified` read as sent.
- `announcement_role_template_put` takes `delivery`
  (`{"await_handling": bool}`), sent only when set: every child's delivery.
  `AnnouncementRoleTemplate` gains `delivery` and `await_handling`, `None`
  when the template did not set them.
- `MobHandle.run_flow(flow_id, params, *, role_targets=None)` binds every
  role the flow's steps use to existing members, for example
  `{"reviewer": ["rev-a1", "rev-a2"]}`; each step then dispatches only to the
  bound members of its role. Before sending, it reads `meta/capabilities` and
  raises `CapabilityUnavailableError` when the gateway does not list
  `flow_role_targets` (`FLOW_ROLE_TARGETS`, `supports_flow_role_targets`),
  since an older gateway would run the flow role-wide. A binding the mob
  refuses raises `FlowRoleTargetsRefusedError` (`flow_id`, `refusal`,
  `refusal_kind`; kinds in `FLOW_ROLE_TARGET_REFUSAL_KINDS`). When the result
  does not echo the binding, it cancels the run and raises
  `FlowRoleTargetsNotAppliedError` (`run_id`, `requested`, `echoed`,
  `cancel_error`). `MobRun.role_targets` and `MobRunSnapshot.role_targets`
  carry a run's binding, `None` for a role-wide run.
- `MobHandle.run_flow(..., idempotency_key=None)` admits the run once under
  a caller key: a replay with the same key, `params` and `role_targets`
  returns the same run, and other params or another binding under the key
  raise `FlowRunIdempotencyConflictError` (`run_id`, `flow_id`;
  `FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND`). It raises
  `CapabilityUnavailableError` before calling a gateway that does not list
  `flow_run_idempotency_key` (`FLOW_RUN_IDEMPOTENCY_KEY`,
  `supports_flow_run_idempotency_key`).
- Keyed sends: `MetaRuntime.send(identity, content, idempotency_key=...)` and
  `MetaRuntime.agent(identity).send(content, idempotency_key=...)` commit the
  content as a member continuation under the key and return a
  `KeyedSendResult` (`delivery_id`, `committed_at_ms`, `status`). A retry
  under the same key returns the first receipt with the delivery's current
  status, after a respawn, reset or gateway restart too; other content under
  the key is refused (`data.kind` `input_idempotency_conflict`). Keys never
  expire. A key does not combine with `track_turn` or `expected_spec_digest`
  (`ValueError`), and it raises `CapabilityUnavailableError` before sending
  when the gateway does not advertise `meta/send_status`, since an older
  gateway would ignore the key. `MetaRuntime.send_status(identity,
  session_id, input_id)` and `MetaRuntime.agent(identity).send_status(
  session_id, input_id)` return a `SendStatusResult`, whose `state` is `None`
  when the session holds no such input.
- Keyed steers: `MetaRuntime.send(identity, content, idempotency_key=...,
  handling_mode="steer")` and the agent handle's `send` commit the content as
  a steer under the key: it joins the identity's running turn at its next
  boundary, or starts a turn when the identity is idle. `handling_mode` is
  `"queue"` (the default) or `"steer"`; a steer needs a key, and an unkeyed
  `"queue"` sends as before. A replay of the key in another mode is refused
  as other content is. A steer raises
  `CapabilityUnavailableError` before sending when the gateway does not list
  the `keyed_send_handling_mode` feature capability, since an older gateway
  would ignore the mode and queue.
- `MetaRuntime.gateway_shutdown_horizon_ms` is the shutdown horizon the
  gateway advertised in `meta/init`, as the transport accepted it, in
  milliseconds; the transport exposes it as `advertised_shutdown_horizon_ms`.
  It is `None` before `meta/init`, or when the gateway advertised no valid
  horizon and the transport waits its own default, and a replacement gateway
  process never inherits an earlier value. A process that supervises an
  application can use it to size its stop window.
- `MetaBuilder.shutdown_in_flight_drain_ms(window_ms)` sends the `meta/init`
  parameter `shutdown_in_flight_drain_ms`: on the SDK's shutdown handshake or
  a SIGTERM, the gateway lets its members' in-flight turns finish for up to
  the window before it shuts down. The gateway's advertised shutdown horizon
  grows by the window plus 10 seconds, and the transport waits that much
  longer on close. 0, the default, drains nothing; the most is 300000.

### Fixed

- An identity customizer's `after_create` gets the context the gateway
  sends: the model, labels and system prompt beside the identity and the
  session. It read a nested `context` object, which the gateway does not
  send, and passed an empty context; a nested object is still read when one
  is present.
- The SSE reader behind the agent and mob event streams (`parse_sse_stream`)
  decodes each complete line as UTF-8 instead of decoding each network chunk
  with replacement, so a multi-byte character that a chunk boundary split no
  longer arrives as U+FFFD. Bytes that are not UTF-8, and a stream that ends
  inside a character, raise `TransportError` (caused by the decode error)
  instead of being replaced with U+FFFD.

## 0.9.4

- `MemberReloadRequiredError` (an `RpcError`), with
  `MEMBER_RELOAD_REQUIRED_CODE` (`-32029`) and `MEMBER_RELOAD_REQUIRED_KIND`
  (`"mob_member_reload_required"`): `MobHandle.force_cancel_member` raises it
  for a member whose runtime registration must be reloaded, when the refusal
  carries both the code and the kind. It has `member_id`, `reason` and
  `required_action` (`"reload_member_registration"`), each `None` when the
  gateway sent no such field. Any other `-32029` stays a plain `RpcError`.
- `MobHandle.force_cancel_member` raises meerkat's typed mob refusal: an
  unknown member now arrives as an `RpcError` with code `-32602` (`mob member
  not found: <id>`) instead of `-32000`.

### Fixed

- A request waiting on a gateway process that exits while a process it
  started still holds the gateway's stdout fails with
  `GatewayChildEndedError` (its `exit_code` set) about a second after the
  exit, instead of waiting out its request timeout (60 seconds by default)
  or, for the shutdown handshake, the gateway's whole shutdown horizon.

## 0.9.3

- Breaking: the draft results of `meta/mobpacks/list`, `create`, `save`,
  `undo`, `redo` and `delete` type `rows` as the new `MobpackDraftSummary`
  (id, name, version, stage, trigger, source, description, revision, etag,
  `updated_at_unix_ms`, `can_undo` and `can_redo`): Meta lists drafts
  without their documents. A write's `row` is still a whole
  `MobpackDraftRow`; `meta/mobpacks/get` returns a draft's document.
- `MobpackDraftSaveResult`, `MobpackDraftHistoryResult` and
  `MobpackDraftDeleteResult` carry `durability_warning`, set when Meta wrote
  the draft store but could not sync its directory: the write took effect
  and may not survive a crash.
- `MobHandle.spawn_helper` and `MobHandle.fork_helper` take
  `transport_timeout` (seconds): how long the call waits for the helper's
  result. It defaults to the transport's request timeout (60 seconds) and is
  not sent to the gateway. When it runs out the helper's outcome is unknown
  and it may still be running: reconcile it before any retry. A retry under a
  fresh `agent_identity` starts another helper (it neither cancels nor
  deduplicates the first); one under the same identity queues behind the
  first on identity-first gateways.
- Test doubles that replace the private `MetaRuntime._rpc` must accept the
  `transport_timeout` keyword: `spawn_helper` and `fork_helper` now always
  pass it (`None` when unset).

### Fixed

- A gateway whose stdout closes while `meta/init` is in flight (it exits, or
  writes a fail-closed init refusal and exits) no longer leaves asyncio
  logging "Future exception was never retrieved" with a
  `TransportReaderFailedError`. Closing the init watch retires its
  settlement future on the host loop; the caller's typed error is unchanged.
- A request timeout longer than `threading.TIMEOUT_MAX` is refused before the
  request is written: the call raises `ValueError`, and `connect()` with such a
  `gateway_init_timeout` raises `TransportError` caused by it, sending no
  `meta/init`. The transport used to write the request and only then fail its
  wait with `OverflowError`, so the gateway ran the call with nobody waiting
  for the reply.

## 0.9.2

- No SDK API changes; released with Meta 0.9.2 on Meerkat 0.9.2.

## 0.9.1

- `MetaBuilder.gating("")` and `MetaBuilder.access_control("")` raise a
  `ValueError` that says an empty path no longer switches the feature off and
  names how to keep it off: do not call the method, and keep nothing at
  `config/gating.toml` / `config/access.toml` under the working directory.

## 0.9.0

- Breaking (access control): `MetaBuilder.access_control(path)` now needs
  an existing, valid access file. The gateway refuses to start with a
  missing path, a directory, or an unreadable or invalid file instead of
  starting with access control disabled, and an explicit empty path raises
  `ValueError`. A configuration that names no admin grants administration
  to nobody; name the first admin in the file. Omitting the call leaves
  access control off only when nothing is at the conventional
  `config/access.toml` (a missing or dangling parent directory counts as
  nothing there). Discovery now forwards any other entry there (even a
  directory or a dangling `access.toml` link) and any lookup error other than
  "not found", and the gateway's strict loader refuses initialization when
  it cannot open, read, parse or validate the file. The loader does not check
  the file type, so a FIFO there blocks startup instead of refusing it.
- Breaking (gating): `MetaBuilder.gating(path)` raises `ValueError` for an
  explicit empty path, and the runtime forwards any explicit gating path
  instead of dropping an empty one, so the gateway refuses it. Omitting the
  call leaves gating off only when nothing is at the conventional
  `config/gating.toml` (a missing or dangling parent directory counts as
  nothing there). Discovery now forwards any other entry there (even a
  directory or a dangling `gating.toml` link) and any lookup error other than
  "not found", and the gateway refuses initialization when it cannot open or
  read the file or rejects its contents. The loader does not check the file
  type, so a FIFO there blocks startup instead of refusing it.

## 0.8.19

- Added `MetaBuilder.role_migrations([...])`, which authorizes member role
  migrations for the life of one boot. A durable member whose role changed
  refuses to resume until the host declares the migration, so an unintended role
  edit cannot silently restamp that member's durable role, comms name and
  binding. Accepts `RoleMigrationDeclaration` dataclasses or plain dicts and
  validates both; declaring one identity twice with conflicting `from_role`
  values raises `ValueError`, an identical repeat is accepted, and omitting the
  call emits no `role_migrations` init parameter at all. Exports
  `RoleMigrationDeclaration` from `meerkat_meta`.

## 0.8.16

- Breaking (mobkit 0.8.16, tracking meerkat 0.8.22's exact helper contract):
  `spawn_helper` and `fork_helper` now require keyword arguments
  `result_label: str` and `max_text_bytes: int`, mirroring the required wire
  fields; the byte bound is validated upstream before admission. The response
  regains `session_id` (exact turn session identity), populating the existing
  optional `HelperResult.session_id` that older releases left `None`.

- Breaking: removed `MetaBuilder.scheduling(...)`, static schedule-file
  discovery, and `MobHandle.scheduling_evaluate` / `scheduling_dispatch`.
  Create durable schedules through Meerkat schedule tools or
  `ScheduleService`; `host_runnables` and `callback/schedule_fire` remain.

## 0.8.9

- Gateway stderr is now INHERITED by default, so tracing lines, panic hooks,
  and storage-migration progress reach the host process's stderr. The old
  default sent it to `/dev/null`, which cost one production fleet a week of
  panic-hook lines and made a working migration read as a hang. Opt out with
  `META_GATEWAY_STDERR=devnull`, or redirect with the existing
  `META_GATEWAY_STDERR_FILE=<path>`.
- Added durable detached callback tools. `register_tool(..., execution=...)`
  declares runner identity, restart and idempotency policy, submission timeout,
  and credential scopes; the host accepts short `start`/`reconcile`/`cancel`
  control callbacks and reports fenced progress, checkpoints, and terminal
  results asynchronously.
- Added `runtime.jobs` and `runtime.monitors` domain handles for the stable
  job/monitor RPC surface. Reconnect reconciliation reuses the exact committed
  attempt, fence, lease, checkpoint, and handle; it never advances authority
  merely because the gateway reopened.
- Detached credentials are resolved from the runner's bound SDK profile for
  each committed attempt and are never serialized into job state or reports.
- Persistent gateway shutdown now negotiates an explicit bounded horizon and
  keeps stdin/provider callbacks open until the gateway acknowledges runtime
  cleanup. The stock 335-second horizon covers provider operations that meet
  their public 120-second contract, a dedicated 125-second Python host
  completion deadline, and the gateway's 130-second hard wire deadline.
  Provider callback timeout cancels the event-loop future, malformed cleanup
  acknowledgements fail closed, older/custom gateways retain the EOF protocol,
  and blocking child-process waits remain off the asyncio event loop.
- Failed gateway bootstrap now detaches and reaps its child process before a
  reconnect can start a replacement, including provider callback failures and
  cancellation while cleanup is in progress.

## 0.8.0

### Identity bootstrap

- Added `IdentityBootstrapMode` and
  `Meta.builder().identity_bootstrap_mode(...)` for eager, lazy, and bounded
  background-warm gateway startup.
- Every explicit bootstrap mode now requires `.roster(...)`, including eager;
  omission continues to preserve the classic gateway when no roster is set.
- A partially failed eager gateway bootstrap now completes ordered runtime
  shutdown and exact lease release before returning the initialization error.
- Added typed `identity_bootstrap_status()` and `wait_identity_bootstrap()`
  runtime methods, including materialized and startup-ready barriers with
  per-identity progress and terminal error details.
- Documented the lease-renewal commit boundary: provider exceptions are
  pre-commit and must leave every input grant unchanged; returned renewed or
  lost results are committed per identity.

## 0.7.17

### Identity-first agent memory

- Added `.agent_memory(...)` on `Meta.builder()` for gateway-backed
  identity-first runtimes with persistent state.
- Added `MobHandle.remember_agent_memory(...)`,
  `MobHandle.recall_agent_memory(...)`, and
  `MobHandle.forget_agent_memory(...)` for durable per-identity memory
  records backed by `meta/agent_memory/*`.
- Added strict typed results `AgentMemoryRecord`,
  `AgentMemoryRecallResult`, and `AgentMemoryForgetResult`, exported
  from `meerkat_meta`.

## 0.6.29

### Console fetch policy

- Added `Meta.builder().console_fetch_timeout_ms(ms)` for gateway-backed
  runtimes. The value is sent as `runtime_options.console_fetch_timeout_ms`
  and projected through `/console/experience` for the bundled console.

## 0.6.28

### Runtime state builders

- `.persistent_state(path)` no longer creates a local redb mob store.
  Gateway-backed SDK runtimes keep mob storage in-memory while sessions,
  runtime state, metadata, console logs, and binary blobs remain under
  the configured state directory.

## 0.6.6

### Multimodal attachments

- Added `MobHandle.upload_blob(...)` for efficient binary image upload
  through the console multipart endpoint.
- `MobHandle.send(...)` accepts image attachments and sends multipart
  placeholders instead of inlining large base64 payloads in JSON.

### Streaming structural events + durable cursors

Meta's structural-events surface is now durable end-to-end. Consumers
can checkpoint a `cursor` from any `MobStructuralEvent` and resume
across meta gateway restarts on SQLite-backed deployments.

- `MobStructuralEvent.cursor` is now the **meerkat ledger cursor** —
  not a per-process `AtomicU64`. The 500ms poller is gone; meta's
  subscription task streams from `MobEventsView::subscribe_after`
  directly. Restart-resume is automatic when the runtime is built
  with `.persistent_state(path)` (a `SqliteMetadataStore` is
  auto-installed at `<path>/meta_metadata.sqlite`).
- New `MobEventsStaleError(RpcError)` raised by `query_mob_events()`
  / `subscribe_mob_events()` when `after_seq` is past the current
  ledger frontier. Carries `after_cursor` and `latest_cursor` so
  callers can rewind. JSON-RPC code is `-32010`
  (`MOB_EVENTS_STALE_CURSOR_CODE`).
- `meta/mob_events/subscribe` now returns a `subscribe_url`
  pointing to `/meta/mob_events/stream?after_seq=...&...` — a
  per-client SSE route that opens its own
  `MobEventsView::subscribe_after`, eliminating the gap between
  snapshot and live tail. Original filters are echoed back.
- `meta/mob_events/query` always returns a numeric
  `next_after_seq` even when the filter matches nothing (was
  `null`); a polling SDK keeps a valid resume anchor.

### `MobHandle.list_runs(flow_id?)`

New SDK method wrapping `meta/list_runs` over `MobHandle::list_runs`.
Returns `list[MobRun]` carrying the **full meerkat ledger projection**
— `step_ledger`, `failure_ledger`, `frames` (map keyed by frame id),
`loops` (map keyed by loop id), `loop_iteration_ledger`, `flow_state`,
`activation_params`, `schema_version`, `root_step_outputs`,
`loop_iteration_outputs`. Meerkat-internal sub-shapes (kernel state,
output blobs) pass through opaquely as `Any`.

New types in `meerkat_meta.types`:
`MobRun`, `MobRunStatus`, `StepRecord`, `FailureRecord`,
`FrameRecord`, `LoopRecord`, `LoopIterationRecord`. All exported
from `meerkat_meta`.

### Bug-hunt fixes (PR #69)

- `RpcError.data` propagates the JSON-RPC `error.data` payload —
  used by `MobEventsStaleError` to surface typed cursor info.
- `AsgiApp.__call__("/rpc", ...)` no longer masks `RpcError` as
  `-32603 / AttributeError`: the handler used a non-existent
  `exc.message` attribute and fell through to the generic
  `except Exception`. Real RPC code/message/data are now returned.
- `_transport.send_sync` rejects empty / duplicate request ids
  (`ValueError`) instead of deadlocking concurrent callers on the
  shared `_pending[""]` slot for the full 60s timeout.
- `EventEnvelope.from_dict` / `KeepAliveConfig.from_dict` coerce
  numeric fields (`timestamp_ms`, `interval_ms`) via `_coerce_int`.
  Pre-fix `None`, `"15"`, or floats survived into `int`-annotated
  fields and broke arithmetic far from the parse site.
- `MobEvent.from_sse` / `AgentEvent.from_sse` preserve non-dict
  payloads as `UnknownEvent(type="non_dict_payload", data={"raw":
  ...})`. Pre-fix the raw value was silently dropped.
- `subscribe_mob_events` accepts bare-list responses (not just dict
  envelopes), matching `query_mob_events`.
- Memory backend failures (`meta/memory/index`,
  `meta/memory/query`) now use distinct JSON-RPC code `-32012`
  (`MEMORY_BACKEND_UNAVAILABLE_CODE`). Pre-fix they shared `-32010`
  with the mob_events stale-cursor contract; SDKs branching on the
  code misclassified.

### Cross-mob signed-peer surface

- New `MobHandle.peer_pubkey()` → returns the local gateway's Ed25519
  signing pubkey as a base64 string. Wraps the new `meta/peer_pubkey`
  RPC. Use this to bootstrap trust before populating a peer meta's
  contact directory with this gateway's pubkey.
- `MobHandle.wire_local()` and `MobHandle.unwire_local()` accept an
  optional keyword `remote_pubkey_b64`. Non-inproc transports (`tcp://`,
  `uds://`) require it: the gateway rejects unsigned descriptors on real
  transports so meerkat-comms can verify envelope signatures at ingress.
  Inproc-only callers keep the existing four-positional-arg shape.
- `meta/cross_mob/directory` entries gain an optional `pubkey` field
  (base64) when the contact-directory TOML uses the new table form
  (`{ transport = "...", pubkey = "ed25519:..." }`). Bare-string entries
  remain backward compatible and report `pubkey = None`.

## 0.6.0 — Meerkat 0.6 wire rename (BREAKING)

### Structural mob events surface

Added `MobHandle.query_mob_events()` and `MobHandle.subscribe_mob_events()`,
backed by the new `meta/mob_events/query` and `meta/mob_events/subscribe`
JSON-RPC methods. The structural surface preserves the full
`MobEventKind` vocabulary from meerkat-mob (25 variants — `flow_started`,
`step_dispatched`, `members_wired`, `supervisor_escalation`, ...) along
with the `mob_id`, `run_id`, `step_id`, and `agent_identity` fields that
the legacy lossy `UnifiedEvent::Agent` projection discards.

- New typed envelope `MobStructuralEvent` (frozen dataclass with
  `from_dict`) carries `event_id`, `cursor`, `mob_id`, `timestamp_ms`,
  `kind`, `run_id`, `step_id`, `agent_identity`, `data`.
- `EventQuery` extended with `mob_id`, `run_id`, `step_id`, `identity`
  filters. `after_seq` is the cursor — pass the highest-seen `cursor` to
  paginate strictly-newer events.
- Both methods accept either an `EventQuery` instance or a plain dict.



Aligns the Python SDK's wire-level field names with the meerkat 0.6 platform.
Consumers upgrading from 0.5.x need to rename a handful of field references —
see the table below. No behavioural changes.

### Renamed fields

| Before | After | Surfaces |
|--------|-------|----------|
| `meerkat_id` | `agent_identity` | `SpawnResult`, `MemberSnapshot`, `DiscoverySpec`, `meta/*` RPC params that accept a member identity |
| `profile` (as a mob-member role) | `role` | `SpawnResult`, `MemberSnapshot`, `DiscoverySpec`, `meta/ensure_member`, `meta/attach_existing_session`, `meta/spawn_helper` / `fork_helper` options, admin-console live snapshots |

Rename in your code:

```diff
- spec = DiscoverySpec(profile="worker", meerkat_id="agent-1", labels={...})
+ spec = DiscoverySpec(role="worker", agent_identity="agent-1", labels={...})

- await handle.ensure_member("agent-1", role="worker")
+ await handle.ensure_member("agent-1", role="worker")

- await handle.spawn_helper("h1", "task", profile="worker")
+ await handle.spawn_helper("h1", "task", role="worker")

- snap.meerkat_id       # MemberSnapshot / SpawnResult
+ snap.agent_identity

- snap.profile
+ snap.role
```

### Other wire shifts that ride the same rename

- `MemberSnapshot.state` is now `"Active"` / `"Retiring"` (title-cased from
  meerkat's native `MemberState` enum) where 0.5 emitted `"active"` /
  `"retiring"`. If you pattern-match on these strings, update the cases.

- `HelperResult.session_id` is now always `None`. Meerkat 0.6's
  `MobHandle::spawn_helper` / `fork_helper` retire the helper before the
  call returns, so meta cannot recover the bridge session id post-hoc;
  the field is preserved on the Python dataclass (so `data.get("session_id")`
  returns `None` cleanly) but meta no longer emits the key on the wire.
  If your code correlated helper history via this id, you need an
  alternative route (subscribe to the helper's events during the call, or
  wait for an upstream `HelperResult.bridge_session_id` field).

### Reconcile error semantics + report shape

- Native Meerkat `MobHandle::reconcile(...)` may now return a roster report
  with a non-empty `failures` list (meerkat 0.6 collects per-identity failures rather than
  returning `Err` on the first failure). `UnifiedRuntime::reconcile()` in
  the Rust layer re-lifts this into `UnifiedRuntimeReconcileError::PartialFailure`
  so callers using `?` see the same propagation behaviour they had pre-0.6.

- The `spawned` field in that native roster report's wire projection is now
  an array of
  `{ "agent_identity": str, "member_ref": str }` objects (the canonical
  `MobSpawnReceiptWire` shape from meerkat-contracts) rather than a list
  of identity strings. `member_ref` is a server-resolved opaque handle
  for subsequent member-targeted control calls.

These are native Rust roster changes, not Python SDK module-reconcile
results. The SDK's `MobHandle.reconcile(modules)` is a different operation:
it calls `meta/reconcile` and returns `ReconcileResult` with
`accepted`, `reconciled_modules`, and `added`, not `failures` or `spawned`.

### Lightweight roster

- `meta/list_members`, `meta/get_member`, `meta/find_members` no
  longer inject a `session_id` field per entry. Aligns with meerkat's
  lightweight-roster design. Use `meta/member_status` to bridge a
  member to a realtime session — its `MobMemberSnapshot` carries
  `current_session_id` natively.

### Removed RPCs

- `meta/member_current_session_id` and `meta/member_session_ref`
  are gone. Both were one-liners over an internal session-id lookup
  and are subsumed by `meta/member_status`. The corresponding Python
  SDK methods (`MobHandle.member_session_id`,
  `MobHandle.member_session_ref`) and the `MemberSessionRef` dataclass
  have been removed.

### New: server-side readiness

- `MobHandle.wait_ready(timeout=...)` — blocks until all current mob
  members are startup-ready for orchestration, then returns
  `{"ready": [...], "timeout": False}`. On deadline expiry returns
  `{"ready": [], "timeout": True}` instead of raising. Relays meerkat
  0.6's `MobHandle::wait_for_ready`. Replaces ad-hoc client-side
  polling loops on `member_status`.

### New: flow enumeration + start

- `MobHandle.list_flows() -> list[str]` — returns the flow IDs declared
  by the mob's `[flows.*]` tables. Relays meerkat 0.6's
  `MobHandle::list_flows` via the new `meta/list_flows` RPC.
- `MobHandle.run_flow(flow_id, params=None) -> str` — starts a flow run
  and returns its `run_id`. Relays `MobHandle::run_flow` via
  `meta/run_flow`. Pair with `flow_status`/`cancel_flow` to drive the
  run end-to-end.

### Unchanged

- The `DurableAgentSpec` identity-first surface keeps its `profile` field (it
  mirrors meerkat's internal Rust `DurableAgentSpec`, which is a different
  surface from `MobMemberListEntry`).
- RPC method names (e.g. `meta/ensure_member`, `meta/spawn_helper`) are
  unchanged. Only their parameter and response field names shifted.
- `CatalogEntry.profile` (model-capability dict) is unrelated to mob-member
  roles and is unchanged.
