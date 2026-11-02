Covers: governed realms (`rkat realm govern`/`grant`/`revoke-grant`,
`--as-agent`, route keys, limits, listing grants, governed `rkat-rpc`,
authorization changes while work runs), review tiers R1/R2/R3 and
`[[governance.review.rules]]`, R3 consent and approvals (-32033 to -32035),
native authorization, Rust host composition (stock prompts, host outcomes),
application tool policy, secret redaction. Other topics: references/security.md

# Security in Meerkat 0.9: governed realms, review and approvals

How a governed realm decides whether an agent may perform an operation, when a
model or a human must review it first, and how approval decisions are
recorded. Process sandboxing, the shell command policy, credential storage,
MCP boundaries, audit and the full errors table are in
`references/security.md`.

## Governed realms and grants

Preview. A governed realm runs agent work only under grants that its local
operator issued. Authorization crates: `meerkat-authorization` (grant owner,
policies, audit) and `meerkat-authorization-contracts` (shared vocabulary).

Requirements and limits:

- Feature `local-authorization`: on by default in `rkat` and `rkat-rpc`; off by
  default on the `meerkat` facade. `rkat-rest`, `rkat-mcp` and every ordinary
  opener refuse a governed realm (`realm '<id>' is governed and cannot be
  opened as an ordinary realm`).
- Unix only: the operator is the local OS user (`uid`); elsewhere `govern`
  fails with "the local operator identity is unavailable on this platform".
- The realm must already exist (`RealmNotMaterialized` otherwise) and must not
  use the `memory` backend.
- Governing is permanent: there is no command to ungovern a realm, and no
  flag or run option selects governance. Every later process opens it governed.
- `govern` issues no grants; until the operator grants, governed prompts are
  refused.

### Set up a governed realm with rkat

```bash
rkat realm create team-alpha
rkat -r team-alpha mcp add github --scope realm --allow-operation search_issues --allow-operation get_issue -- /usr/local/bin/github-mcp
rkat realm govern team-alpha
rkat realm grant team-alpha --catalog                              # read the session tool catalog
rkat realm grant team-alpha --mcp github=search_issues,get_issue   # or --mcp github=* for every operation
# Learn the controller route key: grant a placeholder route, then run a prompt.
rkat realm grant team-alpha --controller --route model-route/v1/0000000000000000000000000000000000000000000000000000000000000000
rkat -r team-alpha run "hello"   # refused; the error names the exact command:
#   rkat realm grant team-alpha --agent rkat-agent --controller --route model-route/v1/<sha256 hex>
rkat realm grant team-alpha --agent rkat-agent --controller --route model-route/v1/<sha256 hex>
rkat -r team-alpha run "triage today's issues"
rkat realm show team-alpha        # prints "governance: governed"
```

A prompt needs a controller grant; without one it is refused before any model
call ("the operator has not selected the realm's stock grants"). What governed
work can use in 0.9.0:

- the controller model, on the granted route keys;
- the session tool catalog, with a `--catalog` grant;
- configured MCP servers that have an access ceiling (`access`, written by
  `--allow-operation`; see "MCP security boundaries" in
  `references/security.md`) and a `--mcp` grant; a server without a ceiling
  or a grant is unavailable while the session continues;
- tool bundle operations a Rust host registered, with their own grants.

Builtin tools (shell, files, tasks and the like) and provider-hosted tools have
no stock grant target and are refused (`operation_authorization_unavailable`).
The builtin `web_search` names its own operation target, which stock never
grants, so a per-target stock selection refuses it (`operation_refused`)
before any owner is asked.

### Commands

```bash
rkat realm govern <REALM_ID>
rkat realm grant <REALM_ID> [--agent <AGENT>] (--controller --route <ROUTE_KEY>... | --catalog | --mcp <SERVER>=<OP>[,<OP>...] | --mcp <SERVER>=*) [--expires-in-secs <N>]
rkat realm revoke-grant <REALM_ID> [--agent <AGENT>] (--controller | --catalog | --mcp <SERVER> | --worker)
rkat run --as-agent <AGENT> "..."
```

- `--agent` defaults to `rkat-agent`. One grant per target; a new grant
  supersedes only that target's previous grant, which is retired once no
  unfinished work references it. New selections apply to realms opened
  afterwards.
- `--controller` takes one or more `--route` keys
  (`model-route/v1/<sha256 hex>`, covering provider, model, binding, endpoint
  and wire model) and has no expiry. Changing the model or endpoint changes the
  key and needs a new grant. Controller grants from before 0.9 name no route
  and admit nothing.
- Since 0.9.4 governed admission holds no credential lease for a controller
  route on the host's environment key (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
  the other provider and `RKAT_*` keys, with no realm binding), and every other
  governed check still applies. That route has its own route key, so a
  controller grant admits it only if one of its `--route` keys names that
  route; otherwise the prompt is refused with a typed policy refusal. The same
  holds when the build falls back to the environment key because no binding is
  named and the configured one does not resolve (for example a logged-out OAuth
  binding).
- `--expires-in-secs` sets a lifetime for catalog and MCP grants.
- `revoke-grant` exits successfully only when revoked. Otherwise it reports
  why: unfinished work holds the controller, cleanup deferred because work
  references the grant, custody unavailable, refused, or uncertain.
  `--worker` revokes an older selection's single worker grant.
- `--as-agent <AGENT>` is a global flag and works before or after the command
  name (`rkat --as-agent x run ...` or `rkat run --as-agent x ...`); 0.9.0
  accepted it only after the command name.
- MCP grants pin the server's connector approval. A rebound account or a
  connector added to a no-connector server is denied; grants issued before 0.9
  must be reissued.
- Tool bundle operation grants and checked-route grants exist only through the
  Rust API (`grant_stock_agent` with `StockGrantTarget::ToolBundleOperation` or
  `CheckedMcpRoute`); `rkat realm grant` does not issue them.
- Listing grants: no CLI or RPC command lists them (there is no
  `rkat realm grants`); the CLI only grants and revokes. A Rust host reads a
  `StockGrantObservation` (`meerkat::governed_realm::stock::issuance`: realm,
  agent, executor, coverage and one entry per requested `StockGrantSlot`,
  such as `Controller`, `Catalog`, `Mcp(server)`). The offline
  `observe_stock_agent_grants(realms_root, realm_id, agent, slots)` reads the
  persisted selection and is refused while a host serves the realm; that host
  calls `StockGovernance::observe_retained_grants(slots)` instead. An
  observation reports grants only, not credential or account readiness.
- Files: grant owner `<realm root>/authorization/grant-owner.jsonl` and the
  selection `<realm root>/authorization/stock-grants.json`. A missing or
  corrupt grant owner refuses governed work; it is never recreated silently.
- `rkat run` (including `--resume`) works in a governed realm. Known issue in
  0.9.0: sessions created by schedules have no bound agent, so their MCP
  servers are unavailable.
- Restart recovery (0.9.0): a governed input recovered after a restart keeps
  the scoped destination it was admitted under, even where scoped
  destinations are optional. Its destination-scoped operations and the
  disclosure of its result are refused until that same destination is bound
  again (fail-closed if it never is): REST 403 `RESULT_DISCLOSURE_REFUSED`
  (`ApiError::ResultDisclosureRefused`), JSON-RPC -32025 `SCOPE_DENIED` with
  `data.reason` `result_disclosure_refused`.

### When authorization changes while work runs (0.9.4)

Every owner mutation of the serving process (a grant issued or revoked, a
policy, identity, destination or route-account change) is published as an
owner change. Work that reads the authorization while a change is being
published is refused to rebuild (`ReprepareRequired`): a stale read, not a
denial. Since 0.9.4:

- A model request, a governed tool dispatch (at its prepared entry, its body
  entry and the call-level policy gate), the `web_search` helper's model
  request and a compaction summary wait for a change in progress to complete,
  then rebuild under the same operation. Each wait state allows at most 2 s
  (less when the run's budget or the tool's remaining time is shorter) and 3
  free rebuilds; a reviewed tool call has at most 3 wait states (6 s, 9
  rebuilds), inside the tool's timeout. Past those bounds the refusal takes its
  ordinary path: one rebuild, then `operation_refused` feedback to the model
  (a compaction summary still fails its compaction). A run whose budget runs
  out while waiting ends with the time-budget terminal. Owner changes while a
  session runs therefore no longer turn ordinary calls into refusals.
- A grant resolution that meets an owner change is a stale read everywhere: a
  preparation rebuilds once; a prompt's controller admission, and the
  admission of a scoped original (its own mandate and each named
  contributor's liveness), are refused as not ready
  (`ControllerReadinessUnavailable`, reason `PolicyChanged`), so resubmit the
  input; a retained resume answers the same and stays retryable, and
  `require_source_original_live` outside an exclusion of owner changes
  answers readiness `PolicyChanged`; a review read refuses as stale
  (`operation_refused`, `ReprepareRequired`) and a fresh attempt reads again;
  a configured MCP server's per-agent grant check answers `NotCurrent`. A
  poisoned publication, failed grant custody and a clock failure still answer
  unavailable.
- A governed helper's admission fence and a helper child issuance answer
  `Unavailable` (a later attempt may succeed), not `Denied`, when their check
  refuses to rebuild. A denial and malformed facts stay `Denied`.
- Each wait is one `debug` event of target `meerkat::authz::owner_change_wait`
  with `site` (`model_request`, `compaction_summary`, `tool_dispatch`,
  `tool_entry`, `tool_policy_entry` or `web_search_delegate`, and since 0.9.9
  `tool_step`, `tool_child_issuance` or `helper_admission`),
  `operation_id`, `waited_ms` and `settled`. Each refused attempt keeps its
  own audit record.
- What the model sees (0.9.4): the `operation_refused` row of the error table
  in `references/security.md` gives its `data.kind` and `data.tool_invoked`.
  The kind carries no policy, owner or audit detail. A mob member upcall still
  carries the bare kind, so its receiver reads `"unknown"`, and an upcall
  refusal whose data is absent or names no known kind now decodes as
  `operation_authorization_unavailable`, not `operation_refused`.

#### Retrying a governed helper admission (0.9.9)

The helper admission fence never waits: while another writer holds the
authorization publication (a reservation that has not published) or an owner
change is in progress, it refuses at once as `Unavailable`, with nothing run or
committed. That contention is retryable at the host level:
`GovernedHelperAdmissionFence::create_idempotent_after_owner_change` makes the
same idempotent create through a new fence for each attempt and waits between
attempts outside every lock (`OwnerChangeChurn::wait_after_stale_read`: 2 s and
three free rebuilds, clamped to the budget passed, read right before the call;
site `helper_admission`). Await it holding no lock or store transaction of your
own. On a dispatch or entered fence it retries every refusal to rebuild, as
the waiting dispatch steps do, and a fence built from a dispatch reports its
waits to the host's refusal observer on the call's surface, without an
operation id. It answers success, `Denied` and every other `Unavailable` (a
refusal to rebuild inside the prepared path's own exclusion, clock or custody
unavailable, a poisoned publication) at once, and the last `Unavailable` once
the bounds are spent. An installed owner's own `Unavailable` is final instead
(at a controller's model request it ends the run), so an owner never answers
its own transient lock contention that way.

### Serving a governed realm over JSON-RPC

`rkat-rpc --realm <id>` serves a governed realm only as governed JSONL over
stdio, acting as `rkat-agent` (it has no `--as-agent`). `--tcp`, `--live-ws`
and `--live-webrtc` are refused, skills and schedules are disabled, and the
realm needs a configured `default_binding` (the controller). Configured MCP
servers connect per granted server.

## Operation review (R1/R2/R3)

Preview. In a governed realm every operation the grants allow runs at R1 (no
review) unless a rule raises it:

- R2: the configured reviewer model reviews the exact operation before it runs.
- R3: a human consents to the exact operation (see the next section).

Rules live in the governed realm's own `config.toml`:

```toml
[[governance.review.rules]]
target = { kind = "mcp_operation", server = "mail", operation = "send" }
tier = "r3"
consent_assurance = "person_authenticated"   # or "device_trusted"

[[governance.review.rules]]
target = { kind = "mcp_server", server = "calendar" }
tier = "r2"

[governance.review.reviewer]
identity = { model = "claude-sonnet-5-5", provider = "anthropic" }
max_output_tokens = 1024
```

- `target.kind`: `mcp_server` (every operation of a configured server),
  `mcp_operation` (one raw operation) or `tool_bundle_operation` (`bundle`,
  `operation`). Names are the configured or registered names, never display
  names. Each target appears once.
- A rule only raises a tier; it never permits what grants refuse. The strictest
  matching rule applies.
- `consent_assurance` is required for `r3` and refused for `r2`.
  `device_trusted` accepts a declared local-operator posture or a person;
  `person_authenticated` needs a person authenticated on the decision event. A
  delegated service credential never decides consent.
- The reviewer `identity` takes `model`, `provider`, optional
  `self_hosted_server_id` and `auth_binding`; `provider_params` is refused.
  There is no default reviewer: an R2 operation without one is refused, never
  run at R1. Unknown fields anywhere in the section are refused.
- Reviewed operations must state the account they act with: an MCP server's
  connector binding, its `oauth_account`, or the host declaration
  `account = "none"` on the server entry. An operation with no stated account
  is refused, never reviewed without it.
- Administration: `config set`/`patch` over RPC, REST, the MCP server tool and
  the CLI cannot change, remove or restate the `governance` section. RPC and the
  MCP server answer invalid params with
  `data: {"type": "governance_administration_required"}`. Edit the realm's
  `config.toml` directly (for a project-local realm,
  `<project-root>/.rkat/realms/<realm>/config.toml`); rules take effect when the
  realm next opens. A malformed or unreadable section refuses the realm's
  governed work.
- Rust hosts observe refusal dispositions with
  `AgentFactory::with_operation_refusal_observer` /
  `AgentBuilder::with_operation_refusal_observer` (`OperationRefusalObserver`):
  the typed refusal, its surface and what Core did with it, host-only and
  never in the event stream or a model context.
- Rust hosts attach review with `AgentFactory::with_operation_review` /
  `AgentBuilder::with_operation_review` (`BoundOperationReview`). A tool
  without a reviewed entry point (for example `web_search` and comms `send`)
  never runs under a required review: R2 settles as `reviewer_missing`
  without a reviewer and as `unsupported_entry` with one; R3 settles as
  `human_consent_required` without a human consent route and as
  `unsupported_entry` with one.
- What the reviewer sees (0.9.0): for an R2 or R3 operation the owners state
  its action account and current mandate (the grant that admits it, with its
  bounds and expiry), read once after R1 is ruled out and sent to the R2
  reviewer model or shown in the R3 consent presentation. This material
  grants nothing, and each field is bounded to 64 KiB (4 KiB before 0.9.7),
  never truncated. An owner that cannot supply it settles the call as
  `review_unavailable` (`context_unavailable`) before any review; material
  that changes makes a retained allow stale (`review_unsatisfied`,
  `context_changed`) and a fresh attempt is reviewed again. In a stock
  governed realm the mandate text depends on who requested
  the work (0.9.4): for an outcome bound with `bind_host_outcome` it says the
  stock ingress itself requested it, that no person did and that it is not
  the operator's consent, and it attributes the event's content to the ingress
  instead of asserting it; an operator prompt's and a host-authenticated
  caller's texts are unchanged. A person sees the mandate as the
  `stock.review_mandate` consent label, and the requester a consent surface
  presents is the person the approval is presented to. Rust owners answer
  through `WorkAuthorization::review_operation_context`
  (`LocalWorkPolicy::review_context`, `AdmittedWorkPolicyOwner::review_mandate`,
  `OperationPolicyOwner::review_action_account`); every default fails closed.
- Owner changes during an R2 review (0.9.4): when one owner change lands
  during the review, at its verdict's commit or at the dispatch's entry
  staging before the tool runs, and the call is still permitted at R2 with the
  same review material, the dispatch reviews it once more from the start as a
  second attempt for the same `tool_call_id`; the earlier verdict is never
  used. At most one re-review per call, only after an allow or a reviewer
  failure, and only with time left for a whole review deadline. A review
  observer can therefore see a `context_changed` retirement followed by a
  second attempt (a second `ReviewAttemptStarted` audit record): do not treat
  the retirement as the call's outcome. A change still in progress when the
  verdict is checked is waited out (within the review deadline, 2 s at most).
  A second change, a denial or escalation, an expired review, a changed tier
  or material, and a change once the tool runs settle as before.

Feedback is an ordinary tool result: `review_unsatisfied` (data `kind`:
`denied`, `escalated`, `context_changed`, `human_consent_required`,
`already_consumed`, plus `approval_id` when a consent request was committed) or
`review_unavailable` (`deadline_expired`, `reviewer_failed`,
`reviewer_missing`, `unsupported_entry`, `owner_unavailable`, `dispatch_ended`,
`store_conflict`, `store_commit_unknown`, `context_unavailable`).

## Human consent and approvals

### Approval records (JSON-RPC)

`rkat-rpc` exposes approval audit records: `approval/request`,
`approval/list` (`filter`), `approval/get` (`approval_id`), `approval/decide`
(`approval_id`, `decision` = `approve`|`deny`, `actor`, optional `reason`,
`provenance`). Statuses: `pending`, `approved`, `denied`, `expired`,
`cancelled`. SDKs: Python `request_approval`, `list_approvals`,
`get_approval`, `decide_approval`; TypeScript `requestApproval`,
`listApprovals`, `getApproval`, `decideApproval`. REST, the MCP server and the
CLI have no approval methods.

These records gate nothing and execute nothing. `requester` and `actor` are
audit strings that `rkat-rpc` does not authenticate. Do not store secrets in
them. `approval/decide` refuses a consent approval (`AuthenticationRequired`).

Storage: one approval issuer per persistence. A persistent host keeps
`approvals.json` under the realm's store path; an explicitly ephemeral host
keeps them in memory. The file store holds an OS lock on `<path>.lock`, syncs
file and directory on every commit, and works only on Unix: elsewhere (for
example Windows) it refuses to open, so a persisted `rkat-rpc` host's approval
service is unavailable and fails closed. Symlinked store paths are refused.

| Code | JSON-RPC | HTTP | CLI exit | Meaning |
| --- | --- | --- | --- | --- |
| `APPROVAL_STORE_UNAVAILABLE` | -32033 | 503 | 53 | nothing committed; a fresh, rechecked retry is valid |
| `APPROVAL_CONFLICT` | -32034 | 409 | 54 | another owner changed the approval first |
| `APPROVAL_COMMIT_UNKNOWN` | -32035 | 500 | 55 | commit may have happened; reconcile, never resend |

### R3 human consent on a governed rkat-rpc pipe

When a governed realm's rules require R3, `rkat-rpc` makes the stdio pipe's
operator (the OS user who started it, assurance `device_trusted`) the human
route. A `person_authenticated` rule therefore refuses that operator.

1. The call needing consent settles locally with `review_unsatisfied`
   (`human_consent_required`, with `approval_id`); other work continues.
2. The pipe sends `approval/consent_notice` with `kind` = `requested` and a
   `presentation`: `action`, `risk`, `account`, `mandate`,
   `accepted_assurance`, `expires_at`.
3. The operator decides:

```json
{"jsonrpc":"2.0","id":31,"method":"approval/consent_decide",
 "params":{"approval_id":"01936f8a-7b2c-7000-8000-0000000000c1","decision":"approve"}}
```

   Only `approval_id` and `decision` are accepted; any other field is an
   invalid parameter. The method is listed by `initialize` only on a pipe that
   serves consent, and the pipe decides only approvals it presented.
4. Later notices report `approved`, `denied`, `used` or `withdrawn`.
5. An approval reaching its still-open run is spent once, at the run's next
   boundary, on a fresh attempt of the exact retained call; the outcome arrives
   as a `consent_attempt` runtime notice. Consent is spendable only in memory:
   when the run closes it is parked or withdrawn, and a restored row is
   history only.

The pipe's authority ends at EOF. SDKs receive notices with Python
`on_consent_notice` (`parse_consent_notice`) and TypeScript `onConsentNotice`
(`parseConsentNotice`); they have no consent-decision wrapper, so send
`approval/consent_decide` as a raw request.

Rust embedders build their own route: `BoundOperationReview::with_human_consent`,
`ApprovalService::decide_consent` with an `AuthenticatedConsentDecision`, or
`MethodRouter::dispatch_with_consent_event` with an
`AuthenticatedConsentEvent` built after the host authenticated the decision.

## Native authorization for Rust hosts

Preview. Enable the facade feature `local-authorization`. A host installs real
owners in `NativeGrantWorkConfiguration`: `grants` (`LocalGrantAuthority`),
`ingress` (`NativeIngressCheck`), `invocation_owner`
(`AdmittedWorkPolicyOwner`) and `operation_owner` (`OperationPolicyOwner`),
then `MeerkatMachine::ephemeral().with_local_grant_authorization(owners)`
before the machine is shared. Persistent governed construction requires
exclusive execution custody. Other entry points: `meerkat_rpc::governed_jsonl`
(`serve_governed_jsonl`, one fixed-host JSONL connection),
`meerkat::governed_realm::stock_host` (a host's own stores under a stock
governed realm), and `StockCallerPosture::HostAuthenticated` for callers a
host authenticated.

The builtin `web_search` is two operations, each mapped from typed facts and
never from the tool name. The call's plan carries the owner-produced
`ToolOperationTarget::BuiltinOperation { operation:
BuiltinToolOperation::WebSearch }`; its helper's model request is an
`Inference` whose binding carries a `ToolDelegateAttribution` (role
`HostedSearchInference`, naming the call), with the helper's own route and its
`WebSearch` or `GoogleSearch` capability, never the controller's. In a
`TargetedGrantLineages` basis select `GrantTarget::BuiltinToolOperation` for
the call and `GrantTarget::DelegatedHostedSearch { route, capability }` (the
helper's own route key) for the helper; the base, catalog and controller
lineages never cover either. The attribution grants nothing: revoking the
call's lineage refuses later calls but not one already entered, whose helper
still needs its own current lineage.

Typed outcomes (`OperationAuthorizationError`): `Refused` (ordinary denial,
local feedback), `Unavailable` (owner could not decide; repair it, do not
retry unchecked) and `ObservationUnavailable` (required audit failed; an
infrastructure error, never a denial). Grant issuance refusals:
`GrantRefusal::Denied`, `Duplicate`, `ControllerInUse`, `Unavailable`.
Continuations are refused on governed runtimes (`NoAdmissibleWorkBinding`).
Contract and limits: `docs/guides/authorization-integration.mdx` and
`docs/rust/native-authorization.mdx`.

## Rust host composition

Brief pointers for trusted Rust hosts; the stock `rkat` flows above need none
of this.

- Realm posture: the host picks `meerkat::governed_realm::RealmGovernance`
  before a realm opens: `Ungoverned`, `Governed(GovernedRealmBootstrap)` (the
  host's own owners), `Stock(StockGovernance)` (the composition `rkat` uses)
  or `GovernedWithoutOwners` (every governed effect refused as unavailable).
  The posture is read from the realm manifest, never inferred from a grant
  file. Errors: `PersistenceError::RealmGoverned` (an ordinary opener met a
  governed realm) and `StoreError::InvalidRealmGovernance` (the manifest's
  posture and format disagree; the realm does not open).
- Callers: `StockCallerPosture::LocalOperator` (default: the OS operator is
  the only requester) or `HostAuthenticated(StockCallerAccess)`, built with
  `StockGovernance::host_authenticated` or `host_authenticated_for_agent`.
  Each input is bound with its authenticated caller (`bind_caller_prompt`),
  the host decides each exact caller and session, and a refused caller gets
  `StockGovernanceError::CallerNotAdmitted`. In that posture the operator
  stays the owner of every session and is no requester, and since 0.9.4 the
  composition's own stock ingress principal (`StockPrincipals::ingress_actor`)
  is refused as a requester too, with a Denied verdict before the host is
  asked.
- Operator inputs (0.9.4): `StockGovernance::bind_prompt` binds only a prompt
  of the local operator (`Input::Prompt`, from the CLI or a governed JSONL
  connection; labels `prompt-<event>`, `prompt-events`,
  `rkat-prompt-<event>`). Any other input, an external event included, is
  refused with `StockGovernanceError::RequesterUnsupported` before anything
  is written; 0.9.3 bound any input with the operator as its requester. An
  administrative outcome the operator's own process reports (for example a
  connector finalization) binds with `StockGovernance::bind_host_outcome`: an
  `ExternalEventInput` of origin `InputOrigin::External` and durability
  `InputDurability::Durable`, whose requester is the stock ingress itself,
  never the operator, under the operator's grant-lineage mandate at the
  operator's session (labels `host-outcome-<event>`, `host-outcome-events`,
  `rkat-host-outcome-<event>`). Each outcome runs its own turn. Both return
  the input to submit and a `PendingOriginal` to admit once the runtime
  accepted that input; an uncertain delivery is resubmitted, never bound
  again. A host-authenticated composition refuses both.
- Owner-scoped resource domains (0.9.4): `ExecutionRestrictions.resource_domains`
  is a `DomainRestriction` (`unrestricted`, `exact`, `owned_by`,
  `unresolved`), read through `bound()` as the `#[non_exhaustive]`
  `DomainBound` (`Unrestricted`, `Exact`, `OwnedBy`). `owned_by(owners)`
  matches every domain whose authority is one of the owners, so a parent
  grant can fix the application while each child names its exact session
  domain. A reader that cannot interpret a bound kind must treat it as naming
  nothing. Unrestricted and exact bounds keep their wire bytes; a 0.9.3
  binary refuses to restore a grant owner whose journal holds an owner-scoped
  restriction (`RestartRequired`).
- Session agent mandate (0.9.4): `SessionAgentMandate::new(executor, servers,
  represented_subject)` has no catalog argument and needs at least one MCP
  server (`AgentMandateError::NoSelection`); the catalog grant stays in the
  work's own basis (`AgentGrantRefs::prompt_selection`). A stock selection
  that grants only the catalog binds no session agent, so configured MCP
  servers stay unavailable, as before.
- Observed grant resolution (0.9.4): `LocalGrantAuthority::resolve_lineage_observed`,
  `resolve_controller_lineage_observed` and `resolve_lineage_ceiling_observed`
  resolve inside the caller's own observation of the grant publication and
  return `LineageResolutionRefusal::Changed` (an owner change published
  during the resolution: classify it as a stale read) or `Refused`. A caller
  with no enclosing observation whose answer can say "ask again" uses them
  too and classifies `Changed` as a change of policy, as native admission
  does. The standalone `resolve_lineage` still reports a concurrent change as
  `GrantRefusal::Unavailable`.
- Owner-change waits for custom owners (0.9.4):
  `WorkAuthorization::owner_change_in_progress` (default `None`) reports a
  change in progress as a waiting hint, never a permission;
  `LocalAuthorizationPublication::change_in_progress` answers it for the local
  owners. `meerkat_core::authorization::OwnerChangeChurn` is the bounded wait
  (`OWNER_CHANGE_WAIT_BOUND` 2 s, `OWNER_CHANGE_FREE_REBUILDS` 3);
  `wait_for_settled_change` takes the wait's trace site and operation, records
  the debug event and reports the wait to the host's refusal observer. A
  delegate operation of a tool call (such as the `web_search` helper's model
  request) takes that call's waiting state from
  `ToolDispatchContext::owner_change_churn` and bounds its wait by
  `ToolDispatchContext::remaining_tool_time`.
- Refusal observations (0.9.4; the observer is installed as in "Operation
  review (R1/R2/R3)"): each `OperationRefusalObservation` names the session,
  run and operation, the surface (`ModelRequest`, `ToolCall { tool_use_id }`,
  `Compaction`), the outcome (`Refused { kind, dispatch }` or
  `AuthorityUnavailable`) and the disposition
  (`WaitedForOwnerChange { waited_ms, settled }`, `Rebuilt`,
  `ControllerFeedback`, `ToolResult`, `CompactionSkipped`, `RunFailed`). It is
  a diagnostic at decision time, not exactly-once across a crash, and the
  observer is called synchronously and must not block. Review outcomes and
  exact-control refusals (MCP attachments, tool applications, context
  controls) are not observed.
- Whether the tool ran (0.9.4): `RefusalDispatch` (`NotDispatched`,
  `Unknown`; non-exhaustive) and `OperationRefused::dispatch`. The fenced
  dispatch (and a retained consent attempt) certifies `NotDispatched` for a
  refusal it meets before it invokes the dispatcher, and the execution policy
  gate for one before its inner dispatcher; every other refusal is `Unknown`,
  which makes no claim. A dispatcher layer that runs tool code of its own
  before delegating forwards `OperationRefused::new(refusal.kind())`.
  `OperationRefused` equality now includes the dispatch fact, so compare
  `kind()` and `dispatch()` instead of `OperationRefused::new(kind)`.
- Audit types (0.9.4): `AuditObservation` is `#[non_exhaustive]`,
  `AuditObservation::Refused` gains `read: Option<AuditRefusalRead>` and
  `AuditPolicyRead` gains `publication_instance` (see "Audit and security
  events" in `references/security.md`).
- Prepared construction: `AgentFactory::prepare_agent` (or
  `prepare_agent_with_controller`) returns a `PreparedAgent` whose
  `tool_surface()` is the final composed tool surface. No agent exists and
  provider-native tools stay off until `finish_agent(prepared, selection)`
  binds the host's `ToolSurfaceSelection`; a selection made from another
  surface is refused. `MeerkatMachine::bind_session_tool_surface_gate`
  installs such a gate for a session before its agent is built.
- Grant issuance beyond the CLI: `grant_stock_agent` (including
  `StockGrantTarget::ToolBundleOperation`), `revoke_stock_agent` (including
  `StockGrantSlot::ToolBundleOperation`) and
  `grant_stock_agent_for_reviewed_installation`, which grants an MCP server
  only while the realm config still holds exactly the reviewed server at the
  reviewed generation (`StockGovernanceError::InstallationChanged`
  otherwise). `LocalGrantAuthority::issued_reference` returns the reference
  of an issued grant, which does not say the grant is usable;
  `revoke_expecting` refuses with `ExpectedRevokeRefusal::NotCurrent`
  (nothing changed) or `Grant(..)` (which does not prove that);
  `GrantRefusal::Duplicate` means the grant id was already issued, since ids
  are never reused.
- Reviewed connectors: `governed_realm::stock::finalize::finalize_reviewed_connector`
  is the last step of a reviewed connector install. Under one administrative
  custody it rechecks the reviewed account, stored credential and client
  profile without using a token, writes the exact reviewed MCP server to the
  realm config and issues the agent's `--mcp` grant with its connector
  approval. Outcome: `Refused` (nothing written), `PersistUncertain` or
  `Persisted`.

## Application tool policy

A host can attach a per-member application tool (consequence) policy that
decides each call the tool access policy already allows. It needs a Rust host
that installs a `ToolConsequencePolicyRegistry`
(`AgentBuildConfig.tool_consequence_policy_registry`); the stock binaries
install none. The policy content stays with the host's provider and is never
copied into specs.

- Binding (`application_tool_policy`, or `application_policy` in a member
  tool declaration): `{"kind":"unmanaged"}` (default, no policy),
  `{"kind":"inherit"}` (resolved from the parent when spawned) or
  `{"kind":"provider","provider_id":"...","policy_id":"..."}`.
- Set it per mob member with JSON-RPC `mob/apply_member_tool_declaration`
  (read it with `mob/member_tool_declaration`) or REST
  `PUT /mob/{id}/members/{agent_identity}/tool-declaration`, or on a portable
  member spec or `AgentBuildConfig` in Rust.
- The policy document is a `CompiledApplicationToolPolicy` in canonical JSON
  (`parse_canonical_json`): `default_deny` must be `true`, a tool absent from
  the member's grants is denied, and unknown fields, non-canonical bytes and
  digest mismatches are refused.
- At call time a denial is `policy_denied`; `policy_indeterminate` means the
  policy produced no authoritative verdict, so the call is refused. Tools
  stay visible, and a governing policy disables provider-native tools.
- Child mobs that agents create on such a host run under the host's child
  policy or are refused (`child_tool_policy_required`).

## Secret redaction in Debug output

Since 0.8.50, `Debug` output, and so any log line or error that prints it,
keeps names but shows `<redacted>` for values:

- MCP, skill-repository and hook configuration (`McpServerConfig`,
  `McpConfig`, `McpAddParams`, `SkillRepoTransport`, hook command and HTTP
  runtime configs): argument, env and header values, and URL userinfo, query
  and fragment.
- Auth types: inline secrets, command credential arguments and env values,
  refresh and id tokens, API keys, OAuth token results and the stored MCP
  OAuth client secret.
- Live sessions: WebRTC and WebSocket bootstrap tokens and the bootstrap URL
  query; WebRTC SDP offers and answers print only their length (they carry
  ICE credentials).

`Serialize` and equality are unchanged, so config files, wire payloads and
anything you serialize still contain the values. Rust hosts can reuse
`meerkat_core::redact::REDACTED` and `RedactedUrl` for their own types.

## Common questions

- Why is every prompt refused after `rkat realm govern`? `govern` issues no
  grants. Grant a controller route (learn the key with the placeholder route
  shown above), plus `--catalog` and `--mcp` grants for tools.
- Can I ungovern a realm? No. Governing is permanent; use a separate,
  ungoverned realm for ungoverned work.
- Why is the shell (or another builtin tool) refused in a governed realm?
  Builtin tools have no stock grant target in 0.9.0 and are refused with
  `operation_authorization_unavailable`.
- What can run work in a governed realm? `rkat run`, `rkat-rpc` (stdio JSONL
  only) and Rust hosts through the governed entry points above. `rkat-rest`,
  `rkat-mcp` and ordinary Rust openers refuse it.
- How do I list the grants my agent holds? No CLI or RPC command does; the
  CLI only grants and revokes. A Rust host reads a `StockGrantObservation`
  (`meerkat::governed_realm::stock::issuance`) through
  `observe_stock_agent_grants` or, while serving, `observe_retained_grants`.
- Do logs show my tokens? `Debug` output redacts them (since 0.8.50), but
  serialized config and wire payloads still carry them.
- How do I require approval before an action? Grant the operation, add an
  `r3` rule for it with `consent_assurance = "device_trusted"` (the stock
  pipe operator's assurance), serve the realm with `rkat-rpc` and answer the
  `approval/consent_notice` with `approval/consent_decide`.
