# Meerkat 0.8 -> 0.9 Migration Notes

> Scope: what an integrator changes when moving a 0.8.52 system to 0.9.0
> (released 2026-10-13). The source is the `## [0.9.0]` section of
> `CHANGELOG.md` (`### Breaking`, `### Changed`, `### Known issues`); read it
> for the full list. Coming from 0.8.50 or earlier, also follow the per-release
> sections of `docs/guides/upgrading.mdx` first.
> The last section covers moving a 0.9.3 system to 0.9.4.

## Before you upgrade

- Upgrade everything together: `rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`,
  PyPI `meerkat-sdk`, npm `@rkat/sdk` and `@rkat/web`, and every Rust
  `meerkat*` dependency exact-pinned to `=0.9.0`. The SDK handshake accepts any
  `rkat-rpc` of the same major and minor version, so a mismatched patch
  connects without complaint.
- 0.9.0 writes state that older binaries cannot read, and there is no
  downgrade path:
  - stored runtime input state is version 6 (rows at versions 3-5 are read and
    rewritten as 6; older binaries refuse version 6 rows), so stop every
    process that shares a runtime store and start 0.9.0 on all of them
    together;
  - a mob event log holding `OrchestratorResumeNotificationFailed`;
  - a realm or `mcp.toml` server definition carrying `access` or `secret_env`
    (upgrade every reader of a shared realm document before writing one);
  - a grant selection or work association naming a checked MCP route.
- Back up the realm state root and keep `rkat storage doctor` reports from
  before and after.

## Renames: MobKit is now Meta (no aliases)

| 0.8 | 0.9 |
|---|---|
| JSON-RPC `mobkit/jobs/{heartbeat,progress,checkpoint,complete,fail,cancel_ack}` | `meta/jobs/{heartbeat,progress,checkpoint,complete,fail,cancel_ack}` |
| Python `MeerkatClient.mobkit_job_*` | `meta_job_*` |
| TypeScript `MeerkatClient.mobkitJob*` | `metaJob*` |
| Rust/schema `MobkitJob{Heartbeat,Progress,Checkpoint,Complete,Fail,CancelAck}Params`, `MobkitJobMutationResult` | `MetaJob*Params`, `MetaJobMutationResult` |
| `ToolDeadlineOwner::MobkitPublicCallback` ("mobkit public callback") | `ToolDeadlineOwner::MetaPublicCallback` ("meta public callback") |

Meta 0.9 hosts use the new names; a host still calling the old ones gets
method-not-found.

## Hooks: config that no longer loads

Mandatory model-request hooks are now operation-local: a denying foreground
Guardrail at `pre_llm_request` / `post_llm_response` withholds only that model
operation, sends `mandatory_hook_prerequisite_failed` to the retained
controller, and the run continues. Configuration validation (and run
admission again) rejects:

- an enabled Guardrail at `pre_llm_request` or `post_llm_response` with the
  default `model_operations = "all"`;
- any enabled Guardrail at `run_started` or `turn_boundary`;
- `model_operations` on a point that carries no model operation (even on a
  disabled entry).

A config file with such an entry fails to load, so the host does not start
until it is migrated; a rejected config update keeps the previous valid
config. Run admission additionally rejects a model-request Guardrail on work
without a retained authorized controller (`AgentError::HookConfigInvalid`, no
`RunStarted`, no user append). That config loads, but every such run is
refused; ordinary (ungoverned) sessions have no retained controller. To
migrate:

- For each model-request Guardrail, decide whether its prerequisite also
  governs the separate controller-feedback request. If not, set
  `model_operations = "ordinary_only"` (nothing sets it for you) and run that
  work with a retained authorized controller. Without a controller, move the
  policy to `pre_tool_execution` / `post_tool_execution`, or change it to an
  `observe` hook that returns no decision (`{}`).
- For a `run_started` or `turn_boundary` Guardrail there is no enforcing
  migration yet: remove the gate, move the policy to a supported point (for
  example `pre_tool_execution`), or change it to `observe` and make it return
  no decision (`{}`). The engine still applies a `deny` returned by any
  foreground entry, `observe` included, and such a deny fails the run.

## MCP

- MCP OAuth credentials are per realm. A credential stored before 0.9 is left
  in place but unused: run `rkat mcp login <server>` (or the host login) once
  in each realm.
- New optional server fields: `access = { allowed_operations = [...] }` (an
  access ceiling; `rkat mcp add --allow-operation`) and, on stdio servers,
  `secret_env` (realm service secrets). Older readers reject documents that
  carry them.
- A grant covers an MCP server's installation and raw operations only when it
  lists the installation's connector approval, as the action
  `mcp_connector/<server>/<token>` or the domain
  `mcp-connector/<server>/<token>`; a wildcard approves no connector. Grants
  issued before 0.9 must be reissued to approve a binding.
- Stdio servers now close gracefully within bounds (stdin closed, then
  SIGTERM and SIGKILL to the process group after `eof_grace` 5 s and
  `term_grace` 3 s by default; hosts set them with
  `McpRouter::with_stdio_close_bounds`).

## Governed realms

- `rkat realm grant --controller` requires one or more `--route <key>`
  (`model-route/v1/<sha256 hex>`). A label-only controller grant issued before
  0.9 admits no route: issue a grant for each route (a refused governed prompt
  names the exact command).
- `rkat-rpc` enables `local-authorization` by default, so it serves a governed
  realm as governed JSONL over stdio instead of refusing it. Ordinary realms
  are unchanged and every stock operation stays R1 until review rules exist.
- Ordinary config writes (RPC, REST and MCP `config/set|patch`, the CLI) that
  would change, remove or restate the `governance` section are refused (RPC
  and MCP: invalid params with `data: {"type": "governance_administration_required"}`).

## Approvals

- RPC approval methods report store outcomes as `APPROVAL_STORE_UNAVAILABLE`
  (-32033, nothing committed), `APPROVAL_CONFLICT` (-32034) and
  `APPROVAL_COMMIT_UNKNOWN` (-32035, reconcile before reuse).
- The file approval store needs a Unix directory durability barrier and read
  access to every directory on its path, and refuses a symlinked store path.
  On other platforms (including Windows) a persisted `rkat-rpc` host's
  approval service is unavailable and fails closed; ephemeral hosts keep the
  in-memory store.
- `approval/decide` refuses a consent approval (`AuthenticationRequired`); R3
  consent is decided only through an authenticated host route.

## Mobs (Rust hosts)

- Admission closes once the last exported `MobHandle` clone drops and the mob
  is idle; later spawns, work submission, flow runs, wiring and resume answer
  `MobError::AdmissionClosed`. `read_handle()`, `events()` and the mob's own
  internal clones do not count as owners: keep a `MobHandle` clone for as long
  as the mob must admit work.
- A destroy keeps the mob's database files and answers the retryable
  `MobMcpDestroyError::Incomplete` (`mob_destroy_incomplete`) while a store,
  storage bundle, handle or reader in the process still holds the mob's
  observer, including a handle the caller kept. Drop them and retry; the same
  mob id cannot be re-created until the cleanup is done.
- `MobHandle::submit_work_with_mode_and_delivery_identity_bounded` takes a
  `&MemberDeliveryScope` (from `capture_member_delivery_scope`) instead of a
  runtime id and fence. A member session that moved answers
  `MobError::StaleDeliveryScope` (wire `STALE_DELIVERY_SCOPE`, -32036):
  re-capture the scope.
- A custom `MobProvisioner::admit_governed_input` takes a
  `custody: Option<GovernedInputCustody>` and returns
  `Result<MemberInputAdmission, MobError>`.

## Rust API

Struct literals and exhaustive matches need the new fields and variants named
under `### Breaking`. The common ones:

- `McpServerConfig` gains `access`, `account` and `connector` (use `None`);
  `McpStdioConfig` gains `secret_env`.
- `ToolDef` gains `audience` (`Default::default()`); `ToolProvenance` gains
  `artifact` (`None`, or build it with `ToolProvenance::new(kind, source_id)`).
- `SessionBuildState` gains `mcp_attachment_intents` (`Vec::new()`).
- `HookEngine::mandatory_topology` is a required trait method;
  `HookEntryConfig` gains `model_operations`.
- `ApprovalStore::put` is replaced by `put_if_current(expected, record)` plus a
  required `get`, and `ApprovalStoreError::Backend` by `NotCommitted`,
  `Contended`, `Conflict` and `CommitUnknown`.
- `SessionAgentMandate::new(executor, servers, represented_subject)` selects a
  grant lineage per MCP server (through 0.9.3 it also took a `catalog`
  lineage that nothing read).
- New variants on `McpError`, `MobError`, `MobEventKind`,
  `meerkat_contracts::ErrorCode`, `AuthError` / `TokenStoreError` (typed
  credential access) and many generated machine-authority enums. Exact-pin the
  crate family and rebuild against 0.9.0.

## Other behaviour changes

- `rkat-rpc` shuts down gracefully on SIGTERM or an interrupt, as on stdin
  EOF.
- Keyring access is typed: public status never reads the OS keychain, and a
  refusal is never reported as an absent credential (auth reasons
  `credential_access_denied`, `credential_interaction_required`,
  `credential_access_pending`, `credential_store_unavailable`).
- Explicit `agent.max_turns` values now survive realm config inheritance; `0`
  ends the run before its first model request.

## Known issues in 0.9.0

- In a governed realm, sessions a schedule materializes are not bound to an
  agent yet, so their configured MCP servers are unavailable.
- `rkat-rpc`: a mob's members reach callback tools only through the
  connection that holds the mob's callback route (its creator, or the
  connection that claimed it). A restored mob has no route until a
  connection calls `mob/claim_callback_route` with the mob's claim token
  (0.9.3: `mob/create` returns it).

## Checklist

1. Stop every process sharing the state root, back it up, and upgrade
   binaries, SDKs and crates together.
2. Rename `mobkit/jobs/*` calls to `meta/jobs/*` (and the SDK methods).
3. Fix hook entries: `model_operations = "ordinary_only"` on model-request
   Guardrails, and no Guardrails at `run_started` / `turn_boundary`.
4. Log in to OAuth MCP servers once per realm.
5. Reissue controller grants per route key and MCP grants with their
   connector approvals.
6. Rust hosts: keep a `MobHandle` clone while a mob must admit work, retry
   `mob_destroy_incomplete`, and update struct literals and matches.

## 0.9.3 -> 0.9.4

The source is the `## [0.9.4]` section of `CHANGELOG.md`; the operator steps
are in `docs/guides/upgrading.mdx` ("Upgrading to 0.9.4").

### Before you upgrade

- No downgrade to 0.9.3 once a 0.9.4 binary has written to a runtime store:
  stored input state is now version 7 (0.9.4 reads versions 3-6 and writes
  every row it stores at 7; 0.9.3 refuses a version 7 row and leaves the bytes
  in place). Stop every process that shares a runtime store, back up the
  state root, then start 0.9.4 on all of them together.
- 0.9.3 also cannot read back a session event log that holds a
  content-filtered `run_failed` event, a transcript holding a tool-scope,
  deferred-catalog or failed-closed notice that 0.9.4 wrote, or a session
  that recorded a failed realtime response (realtime sidecar event schema 4),
  or restore a grant owner whose journal holds an owner-scoped
  resource-domain restriction (`RestartRequired`).
- Mob stores: 0.9.4 migrates each mob's SQLite store to schema 12 (the
  creation record) on open and 0.9.3 refuses it, so keep a copy of the mob
  stores to roll back; 0.9.3 also refuses a version-2 member delivery scope.
- Mixed versions: a 0.9.3 peer that checks signatures drops a 0.9.4
  `reply_to_peer` reply (the sender sees an ambiguous outcome; other message
  kinds still reach it), and mob member hosts must be upgraded before their
  controller (fork instruction policy; a controller set to
  `reactivate_inherited` gets `member_host_unsupported` from a 0.9.3 member
  host). A temporary council record holding a
  participant refused by that policy cannot be read by 0.9.3.
- Upgrade the Python and TypeScript SDKs with `rkat-rpc`: a 0.9.3 SDK fails on
  every `tool_config_changed` event a 0.9.4 `rkat-rpc` builds, and does not
  know the `turn_failed` live observation. It runs a binary you supply
  (`MEERKAT_BIN_PATH`, an explicit path, or `rkat-rpc` on `PATH`) before it
  downloads its own matching one.

### Breaking changes

- Skill frontmatter: the YAML parser is serde-saphyr. It refuses a duplicated
  key at any level, a null `description` or a null value under `metadata`, an
  unrecognized tag (`!custom`), nesting deeper than 64 levels, 100 or more
  aliases that outnumber their anchors more than ten to one, and a `...` end
  marker line. A block scalar ending the frontmatter keeps its final
  line break. Extension values render through the new parser (`'1.10'`
  becomes `"1.10"`; unquoted, `1_000` becomes `1000`, `0755` becomes
  `755.0`, `tRUE` becomes `true`, an integer past u64 becomes a float,
  `1e999` becomes `.inf`, and a `!!binary` value is decoded): quote values
  that must stay text.
- Mobs: an admitted `MobHandle::shutdown` closes admission before teardown
  (`MobError::AdmissionClosed`, reason `shutdown`): spawn, respawn, work,
  flows, wiring, host binds, live commits, resume, reset, reload and owner
  leases are refused, and stay refused if the Shutdown fails. Other requests
  during the drain get the retryable `MobError::ShutdownInProgress` (wire code
  `SessionBusy`). `acquire_owner` is also refused after an admitted destroy,
  a durability fail-stop, a lost actor or a dispatch failure (reasons
  `destroyed`, `durability fail-stop`, `actor lost`, `dispatch failure`).
  `MobError` is `#[non_exhaustive]`.
- Refusals and content filters fail the run as `content_filtered` with a
  typed stop (`refusal` or `content_filter`) and the provider's detail;
  `error_report.reason` of that `run_failed` event is an object
  (`reason_type: content_filtered`), no longer `null`. OpenAI refusal text is
  never committed as an answer. `web_search` reports a refused, filtered,
  context-full or cancelled search as `failed`.
- Tool configuration changes: the lifecycle bit is `settled`, not
  `persisted` (`tool_config_changed` payloads, stored `tool_config` notices,
  `mcp` notice blocks); rows stored before 0.9.4 keep `persisted`, so readers
  accept either key. SDKs expose `ToolConfigChangedPayload.settled`; Rust reads
  `ToolConfigChangedPayload::settled()` and
  `SystemNoticeBlock::lifecycle_settled()`. The `persisted` result of
  `mcp/add`, `mcp/remove` and `mcp/reload` is unchanged.
- Child tool bundle opt-in (Rust hosts): a child mob's inline profile mounts
  a bundle the host offers with the new
  `ChildToolBundleAvailability::ChildOffered` by naming it in `tools.bundles`
  (an unoffered name is refused with `MobError::ToolBundleNotOffered`).
  `ToolConfig` and `MobToolConfigInput` gain `bundles`;
  `ChildToolBundleAvailability` and `ChildMcpServerRegistrationError` are
  non-exhaustive.
- Mob Shutdown: `MemberShutdownOutcome::UnregisterPending` carries a
  `settlement` (`MemberUnregisterSettlement`: `try_state()`,
  `settle_within(deadline)`; states `Settled`, `Pending`, `Failed`,
  `HandedOver`) and is non-exhaustive; match it with `{ stage, .. }`.
- Forks: under the default fork instruction policy
  (`require_explicit_activation`), a fork whose copied prefix carries
  activated host instructions and that starts the child's first turn itself
  (`fork_off`, `fork_member_then_run_*`, `fork_member_at_turn_boundary`,
  council participants) and the RPC forks `session/fork_at` /
  `session/fork_replace` are refused before commit with
  `FORK_INSTRUCTION_REFUSED` (-32037, HTTP 422, exit 57); set
  `reactivate_inherited`
  (`MobBuilder` / `MobMcpState::with_fork_instruction_policy`),
  or pass `inherited_instructions: "reactivate_inherited"` on the RPC fork, to
  carry the activations. Rust:
  `SessionError::ForkInstructionRefused`, `MobError::ForkInstructionRefused`,
  `AgentError::InstructionActivation` (was `ConfigError`), new
  `inherited_instructions` fields and `reactivated_instruction_count`, council
  `ParticipantForkInstructionRefused` / `Refused`.
- Comms: in-turn steers (prompts, peer messages, requests, notices, events,
  flow steps) are written into the running turn at a model boundary or taken
  by one follow-up turn, so history holds them; the peer notice text gains
  `Delivery ID:` and `In reply to:` lines. Rust: `delivery_id` and
  `in_reply_to` fields on the comms notice block, `PeerContentIngested`,
  `CommsCommand::PeerMessage`, `InteractionContent::Message`,
  `MessageKind::Message` and `PeerInput`.
- Mobs: a host's refusal of a restored member's registration (also one the
  boot recreates on a fresh session) removes the registration and leaves only
  that member Broken with `MobError::MemberRegistrationRefused` until a
  repair; Stop and Shutdown report a member needing a durability reload as
  `ReloadRequired` (`MemberStopRun` / `MemberRunStarts`) instead of failing,
  and a Resume refuses with `MemberReloadRequired` while a local member needs
  a durability reload (reload it, then Resume; a reload while Stopped answers
  `Discarded` and the Resume revives the member);
  an uninstalled destination is `MobError::MemberDestinationNotInstalled` (was
  `WiringError`; schedule delivery records `runtime_rejected`);
  `force_cancel_member` / `cancel_all_work` on a member needing a durability
  reload answer `MemberReloadRequired` (-32029, HTTP 409, exit 49): reload it
  with `reload_member_registration`, then cancel again.
- Respawn: a respawned local member keeps its predecessor's launch tool access
  policy and inherited visibility ceiling (never widened) and its recorded
  auth binding when its profile stays on that provider; a respawn whose
  restrictions cannot be established (a lost session included) is refused
  before anything is retired with
  `MobError::RespawnPredecessorRestrictionsUnavailable`; retire and spawn
  instead. A custom `MobSessionService` delegates
  `predecessor_tool_restrictions` (default `Unsupported` refuses respawns).
- Mob incarnations: a mob keeps its `MobIncarnationId` across resume (stored
  in a creation record; pre-0.9.4 mobs get one at their first resume as
  `Unrecorded`), so operator grants survive a resume and a creating session
  manages its mob again after a restart. Member delivery scopes are version 2
  and name the incarnation (`MobError::IncarnationMismatch`, also from
  `retire_at_scope`; `ScopedRecoveryUnresolved::IncarnationNotHosted`), and
  so do member delivery addresses (`rt:mob-member:...`, which 0.9.3 does not
  read): a delivery owed to a destroyed mob's member strands instead of
  reaching the same identity in a mob created again, and
  `meerkat::member_delivery_address` takes the incarnation (in-memory mob
  storage strands deliveries owed to the previous start's incarnation).
  Stores whose creation record and log disagree are not resumed
  (`IncarnationInconsistent`;
  `IncarnationDestroyed` for a finished destroy) and stay unrestored without
  failing the others (`MobMcpState::unrestored_mobs`). Session creates record
  provenance and answer `ProvenanceUnavailable` until the mob set is restored
  (`declare_mob_set_restored`). Rust: the operator grant functions take
  `LiveMobIncarnation` (`handle.live_incarnation()`); `MobIncarnationId`
  serializes as its UUID; new `MobError` / `MobStoreError` variants.
- Runtime: a boundary commit with no durable outcome within the store's
  deadline (120 s for SQLite) marks the session reload-required; waiters get
  `CompletionWaitError::DurableOutcomeUnknown` (a rejected commit
  `BoundaryRolledBack`), and `CompletionWaitError` is non-exhaustive. Read the
  input after the reload instead of resubmitting it (resubmit only with an
  idempotency key); a scheduled delivery waits for the input's own outcome
  for up to 5 lease periods. A custom `ScheduleTargetDelivery` resolves a
  deduplicated replay's completion from the original input's outcome. In
  0.9.4 a delivery whose executor crashed is reclaimed only inside its
  misfire window (known limitation: under the default `skip` grace and the
  60 s lease it ends `misfired`; keep crash recovery with a `catch_up_within`
  `window_seconds` comfortably longer than the lease). 0.9.5 lifts it: an
  occurrence whose earlier attempt recorded its dispatch is reclaimed past
  the deadline too and ends with the admitted input's outcome (see
  `docs/guides/upgrading.mdx`, "Upgrading to 0.9.5"). An
  input admission whose write never answers is bounded by the same deadline
  (then `RecoveryRepairBlocked`, `runtime_admission_deadline`; resubmit with
  the same idempotency key); registration, unregister, control writes and
  recovery still wait for the store (known limitation).
- Realtime: a response the provider ends without completing it arrives as the
  non-terminal `turn_failed` live observation (route on `failure.status`;
  nothing of it is committed, the transcript records `assistant_turn_failed`,
  no retry); 0.9.3 committed its partial text as a reply. Rust:
  `RealtimeSessionEvent::TurnFailed`,
  `RealtimeTranscriptEvent::AssistantTurnFailed` and
  `RealtimeTranscriptStopReasonKind::Failed` are new arms of exhaustive enums,
  and `LiveProjectionSink` gains the required `signal_turn_failed`.
- Rust: `LlmDoneOutcome::Success` is non-exhaustive (build it with
  `LlmDoneOutcome::success(..)` / `success_with_detail(..)`);
  `AgentError::ContentFiltered` gains `stop` and `detail`; the exhaustive
  `AgentErrorReason` gains `ContentFiltered { stop, detail }`, so every
  `match` on it needs the arm.
- `Agent::replace_client` (and `replace_client_with_request_policy`) and
  `SessionService::set_session_client` refuse a client, or a request-policy
  model, whose provider or model differs from the session's durable identity
  (`AgentError::ClientModelChangeRequiresHotSwap`); change models with
  `hot_swap_llm_identity` / `hot_swap_session_llm_identity`. A session
  without a durable identity still accepts any client.
- Custom compactors: `Compactor::prepare_for_summarization(messages, budget)`
  returns a `SummarizationSource` and has no default. Return
  `SummarizationSource::whole(meerkat_core::materialize_latest_system_prompt_versions(messages))`
  to keep the old behaviour. `SessionCompactionCadence` and
  `CompactionContext` gain `last_rewrite_history_tokens`;
  `AgentEvent::CompactionCompleted` and `CompactionProjectionIntent` gain
  `summary_source` and `estimated_history_tokens_after`;
  `CompactionSummarySource` is non-exhaustive.
- Governed realms (Rust hosts): `StockGovernance::bind_prompt` binds only
  operator prompts (`StockGovernanceError::RequesterUnsupported` otherwise);
  bind host-reported outcomes with `StockGovernance::bind_host_outcome`. The
  stock ingress principal is refused as a requester.
  `SessionAgentMandate::new(executor, servers, represented_subject)` must
  select an MCP server. `ExecutionRestrictions.resource_domains` is a
  `DomainRestriction` (`unrestricted`, `exact`, `owned_by`, `unresolved`).
  `AuditObservation` is non-exhaustive. `OperationRefused` equality includes
  `dispatch()` (whether the tool code ran); compare `kind()` and `dispatch()`.
  An R2-reviewed tool call whose owner publication changes during review is
  reviewed once more under the same `tool_call_id`: an observer can see an
  attempt retired as `ContextChanged` before the second attempt, so do not
  treat that retirement as final. A helper admission fence or child issuance
  that refuses to rebuild answers `Unavailable`, not `Denied`.
- Runtime (Rust hosts): MCP attachment providers take the exact
  `RuntimeSessionRegistrationWitness`; `retained_original` becomes
  `retained_custody` (`RetainedCustody`); `ContinuationAdmitError` and
  `BlockedDeliveryReason` gain `NotReady(..)` (`BlockedDeliveryReason` loses
  its defined discriminants, so `as` casts stop compiling); the wire
  readiness failure gains `original_admission_pending`;
  `RuntimeDeliveryOwnerRetirement::join` returns a `#[must_use]` report. GPT
  Live adapter enums gain `FinishedPending`, `NoteDelegationFinishedContext`
  and `InstructionsContextAppendWithdrawn`. Generated machine state structs
  gain fields (`stored_input_state_migration_v6`,
  `identity_generation_highwater`, `identity_generation_highwater_sealed`),
  and mob machine enums gain the `RetireAtScope` and host-refused restore
  variants.

### Other behaviour changes

- Compaction sizes its summary source for the summarizing model's context
  window and stops paying for a summary at every boundary when no rewrite can
  get under the threshold. `compaction_completed` carries `summary_source` and
  `estimated_history_tokens_after`.
- Multiprocess hosting accepts a store whose file notifications cannot be
  registered (the sweep is the wake).
- Fenced runtime-store writes: the SQLite runtime store waits up to 2 s for
  the mob database's write lock inside the identity fence (the new defaulted
  `execute_if_current_blocking`), and a fenced write that meets `Backoff`
  while holding an async lock is tried again for up to 2 s with the lock
  released, so an intent-managed identity on slow disks no longer stays in
  `Backoff`. A fence caller in another crate that holds an async lock retries
  `Backoff` the same way (`execute_if_current` never waits beyond its own
  I/O); a decorator that forwards fenced calls may forward
  `execute_if_current_blocking` too.
- Known limitation: after a restart, graceful or not, an adopted identity runs
  again only once the previous process's identity lease expires (at most
  30 s, `IDENTITY_LEASE_MAX_TTL_MS`); until then its convergence status reads
  `await_lease` (condition `suspended`).
- Work naming an original whose admission is still pending reports
  `original_admission_pending` and stays retryable.
- Fixed: the 0.9.3 known issue "First concurrent durable delivery" (a store
  refusing to open while another process creates the same schema domain);
  macOS FSEvents registration no longer stalls the first mob storage open.
- Governed tool refusals: the `operation_refused` tool result carries
  `data` `{"kind", "tool_invoked"}` for the model; a mob member upcall
  refusal with no known kind reads as `operation_authorization_unavailable`.
- Fixed: in a governed realm, input whose model key comes from the
  environment (`OPENAI_API_KEY` and the other provider and `RKAT_*` keys, no
  realm binding or none that resolves) now reaches controller authorization;
  0.9.3 refused it at admission with
  `controller credential is not currently usable`. Admission holds no
  credential lease for that route, and every other governed check applies.
  The route has its own route key, so a stock controller grant admits it only
  if one of its `--route` keys names it, the build-time fallback from an
  unresolved configured binding included; otherwise the input is refused with
  a typed policy refusal. A host with its own controller policy decides there.
- Fixed: the SQLite session store lays out a whole-blob session compacted more
  than `history_retained_rewrites` times; `@rkat/sdk` `close()` no longer
  keeps Node alive for 5 seconds, and both SDK clients settle on the
  `rkat-rpc` child's exit; public GPT Live sends a finished notice for a
  delegation result held behind the user's question.
- Fixed: a resume finishes an interrupted Stop or Complete before it hands
  the mob back (an attempt still running after 5 s, held on a member or a
  placed completion, goes on in the background: wait for `MobStopped`); a
  session 0.9.2 or 0.9.3 left stuck at a compaction finalizes its last turn
  at its next start; WebSocket and WebRTC voice keeps flowing while another
  turn runs (rows queue off the media path).
- SQLite: release binaries build the bundled SQLite without its three
  process-wide locks; a build outside the repository (`cargo install rkat`,
  an embedding application) sets `LIBSQLITE3_FLAGS` itself (value in
  `docs/guides/deploying.mdx`, "SQLite Lock Profile"), and an embedding
  application calls the `unsafe fn`
  `meerkat_sqlite::disable_memory_statistics_at_process_start()` first in
  `main`, before any other thread or SQLite connection exists.
  `rkat storage doctor` reports `sqlite-lock-profile`. Kept store connections
  hold `<file>.mpresence` presence locks, which maintenance waits for.
- Fixed: a child of `session/fork_at` or `session/fork_replace` can run a turn:
  it gets a build state derived from its source's, with the source's session
  configuration but without its `shell_env`, MCP attachments or mob operator
  authority (`docs/api/rpc.mdx`, "What a fork child inherits"). A child 0.9.3
  forked that never ran a turn still fails, so fork its source again.
  Published crates pass `cargo test` in their unpacked packages, and `rkat`
  builds without default features.

### Checklist

1. Stop every process that shares a runtime store, back up the state root,
   and upgrade everything together, SDKs included.
2. Fix skill frontmatter the new parser refuses, and quote extension values
   that must stay text.
3. Handle `AdmissionClosed` and `ShutdownInProgress`; add wildcard arms.
4. Read `content_filtered` failures by their typed reason.
5. Use `hot_swap_llm_identity` for model changes; update custom compactors.
6. Governed hosts: use `bind_host_outcome` for host-reported outcomes,
   treat a `ContextChanged` review retirement as not final, and grant an
   environment key's controller route by its route key if governed work
   should run on it.
7. Realtime clients: handle `turn_failed`; Rust: add the new arms and
   `signal_turn_failed`.
8. Forks: handle `FORK_INSTRUCTION_REFUSED`; upgrade member hosts before their
   controller and comms peers together.
9. Mobs: back up the mob stores (schema 12), pass `live_incarnation()` to the
   operator grant functions, and call `declare_mob_set_restored` when the host
   restores mobs by inserting handles.
