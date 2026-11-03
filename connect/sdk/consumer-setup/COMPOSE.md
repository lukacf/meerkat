# Container-backed integration setup

The optional Compose setup capability lets an agent prepare and run an explicitly
permitted local integration with supporting containers. The existing canonical
planner validates its package and manifest, and `ComposeRuntime` remains the
owner of deployment observations and recovery. Native `find_and_install`, account authentication and original-task
continuation remain separate work.

The earlier direct setup-host capability has
[actual setup-MCP and agent evidence](../../docs/verification/20261004-compose-setup/REPORT.md).
The selected archive must include its runtime, planner and setup bridge and
explicitly declare `compose-local-execution`. An archive without that capability
refuses Compose host preparation; shared contract files alone do not enable it.
The release guide states the actual platform qualification.

## Host setup

The host can obtain the exact backend profile through
[agent-selected proposal preparation](../python/COMPOSE-PROPOSALS.md). This
keeps the initial host facts separate from the integration's component choices.
If images are absent, the optional [image acquisition capability](COMPOSE-ACQUISITION.md)
can download them while preparing that proposal. Review and admit the resulting
concrete plan before enabling its execution here.

For the ordinary library, add these options to `setup/prepare_host.py` or
`setup/bootstrap_host.py`, retaining that command's normal release, Python,
staging and fresh-output inputs:

```text
--compose-backend-profile /ABS/host/admitted-proposal/backend-profile.json
--compose-backend-profile-sha256 TRUSTED_BACKEND_PROFILE_SHA256
--compose-graph-workspace /ABS/host/new-persistent-graph
--compose-mode new
--compose-allowed-actions prepare,start,observe
```

The helper retains the exact backend profile and generates the outer profile
using the archive's runtime/planner and selected Python. The example enables
only preparation, start and observation. Choose the actions appropriate to the
host's existing authority; there is no implicit action grant. Image acquisition
is a separate pair of options and need not be enabled for execution.

For an existing graph, use the same backend profile, its existing graph path,
`--compose-mode reopen`, and the exact
`--compose-expected-plan-sha256`. Supply an explicit action list excluding
`prepare`, for example `observe,stop,remove`. This does not rerun preparation or
restore historical setup jobs. The existing bridge verifies the retained
request/plan at setup startup before later engine observation or action.

Optional `--compose-delete-state-namespaces` defaults to empty;
`--compose-operation-timeout-seconds` defaults to 300 and accepts 1-3600.
See [host preparation](HOST-PREPARATION.md#enable-container-acquisition-or-execution)
for path, capability and publication checks. Configuration preparation performs
no Docker or planner invocation and makes no claim that the graph is ready.

### Direct host profile contract

Enable the capability with `--compose-profile /absolute/host-profile.json` and
`--compose-profile-sha256 EXPECTED_SHA256` on the existing browser or MCP host.
The browser displays the host's current phase and retained job results, including
pending successor transitions and an explicit rebind requirement. Use the
authorized setup MCP tools or host API to prepare, adopt, reconcile or rebind;
the browser has no successor action buttons. Refresh and reopening observe the
host's state without repeating an action. Historical launch descriptors remain
evidence of their original result, not current execution authority.
Compose job history belongs to the current host session. A fresh host reopen
reads current deployment state and does not restore previous-session Compose jobs.
The profile is a host input, never a tool-call argument. Its required fields are:

```json
{
  "schema_version": 1,
  "scope": "execute_agent_selected_local_container_plan",
  "staging_root": "/absolute/staging",
  "graph_workspace": "/absolute/persistent-graph",
  "mode": "new",
  "expected_plan_sha256": null,
  "python": {"path": "/absolute/python3.11", "sha256": "..."},
  "runtime": {"path": "/absolute/compose.py", "sha256": "..."},
  "planner": {"path": "/absolute/toolkit-compose-plan", "sha256": "..."},
  "backend_profile": {"path": "/absolute/backend-profile.json", "sha256": "..."},
  "allowed_actions": ["prepare", "start", "observe", "resume", "stop", "remove"],
  "delete_state_namespaces": [],
  "operation_timeout_seconds": 600
}
```

Replace every example path and digest with an actual host-selected input. Python
must support the POSIX runtime. The separate backend profile specifies the exact
Docker endpoint, engine identity, installed Docker/Compose binaries, existing
images, component slots, mounts, resources and file digests. See the
[canonical contract](../../docs/verification/20261004-compose-lifecycle/canonical/CONTRACT.md)
and [backend guide](../python/COMPOSE.md).

The scope explicitly delegates trusted local container-code execution. A fixed
backend profile does not constrain the selected program's arguments, dependency
structure or all configuration contents. The agent may choose those within the
canonical profile constraints. This authority is distinct from the workbench's
software-only installation permission. Manifest policy text never grants it.
The host retains exclusive custody of the selected files, workspace, tools and
Docker endpoint. No image download, native credential injection or account login
is supplied by this capability.

One profile names one persistent graph workspace. Use a separate configured host
for another graph. `mode: new` requires an absent graph directory. For explicit
reopen, select `mode: reopen` and the expected non-null plan hash. A new host
checks the retained request against its exact backend profile before observing
or acting on the engine. It does not restore the previous setup session's jobs.

## Agent workflow

Read the enabled capability in `setup_state`. Stage a direct child directory of
the permitted staging root containing `inputs.json`. Its exact fields are
`schema_version`, `package_json`, `manifest_json`, `configuration`, `policy` and
`files`, following the canonical request contract. The first two document values
are JSON strings; all contents must be nonsecret. The host injects its own exact
`profile_json` and `profile_sha256`.

1. Call `setup_compose_prepare` with a fresh 32-character lowercase hexadecimal
   `request_id`, the direct-child `staged_id` and the exact `inputs_sha256`.
   Preparation validates through the existing planner before creating the graph
   workspace. Review the prepared summary; it is not a started integration.
2. Call `setup_compose_start` with a new request ID and exact `plan_sha256`.
   Poll the returned job through the existing job tools. A backend observation
   can remain unconfirmed even if the local process exited successfully.
3. Use `setup_compose_observe` for current graph facts. If a partial operation
   requires recovery, explicitly call `setup_compose_resume` with a new request
   ID and the current plan and observation hashes.
4. Hand the returned `compose_backend_stdio` stdio binding to an ordinary MCP
   client. It is a backend-specific launch, not a portable MCP package or proof
   that the endpoint has authenticated or answered a tool call.
5. Explicitly stop the graph with `setup_compose_stop`, a new request ID and the
   current plan and observation hashes. Stop retains its state. Removal is a
   separate `setup_compose_remove` action with the exact state namespaces to
   delete, limited by the host's `delete_state_namespaces`.

A fresh host reopening the exact B plan may explicitly admit a dormant namespace
retained from A even when B's active backend profile has no volume for it. The
admission correction requires the paired runtime and bridge selected by the
`20261005-compose-retention` edition or a later release that retains this support;
earlier published archives remain unchanged.
The matched runtime supplies only its canonical namespace inventory; the bridge
validates the host's explicit list against that selected plan. An empty grant
still forbids deletion. The removal request also needs current plan/observation
hashes and the existing owner checks. No state deletion is inferred from removal
of a component, inventory readback or a previous host session.

Only host-enabled actions are exposed, and the backend checks that permission
again on dispatch. Repeating a mutation request ID with identical arguments
returns the original job. Changed arguments refuse. A deliberate later action,
including restarting the same plan after a stop, needs a new request ID.
This correlation exists only in the current setup session.

## Explicit same-format successor selection

The successor interface uses the existing graph owner and setup jobs. Use an
archive whose release guide explicitly selects successor support; older archives
remain unchanged. A transition grant and ordinary B execution are separate host choices.
An A-only host never acquires B authority by observing the graph.

For a reopen host pinned to A, the ordinary preparer accepts exactly one optional
B target in addition to the existing profile A, expected A plan, and graph path:

```text
--compose-successor-inputs /ABS/host/selected-b-inputs.json
--compose-successor-inputs-sha256 TRUSTED_B_INPUTS_SHA256
--compose-successor-backend-profile /ABS/host/selected-b-profile.json
--compose-successor-backend-profile-sha256 TRUSTED_B_PROFILE_SHA256
--compose-allowed-actions observe,prepare_successor,adopt_successor,reconcile_successor,rebind
```

The input file has the same fields as staged `inputs.json` above. Inputs and their
assembled request are bounded to 2 MiB; profile limits remain unchanged. The
preparer copies both files exactly and pins their hashes in the generated outer
profile's optional `successor` object. That object contains `inputs` and
`backend_profile` path/hash references, `predecessor_plan_sha256`, and
`transition_sha256` (initially null). It is accepted only in reopen mode.
Agents supply IDs and hashes to tools, never replacement target files or profiles.

1. Read `setup_state.compose.successor.inputs_sha256` and the admitted A plan.
   Call `setup_compose_prepare_successor(request_id, predecessor_plan_sha256,
   inputs_sha256)`. The runtime validates the exact target and retains proposal X;
   no container effect or B binding follows. The setup session admits this X.
2. Read `setup_compose_observe`, then explicitly call
   `setup_compose_adopt_successor(request_id, predecessor_plan_sha256,
   transition_sha256, observation_sha256)` with X and the fresh observation.
   Poll the existing job. Successful selection of B still returns no B bindings.
3. When the owner reports B selected with no pending transition or ordinary
   operation, call the host-enabled `setup_compose_rebind(request_id,
   plan_sha256, transition_sha256)`. This mutates the session's admitted plan and
   bindings, without an engine effect. Its summary identifies profile B and keeps
   the original host-profile hash as provenance. Ordinary B actions remain limited
   to the host's unchanged action list.

For an interrupted transition, first establish that earlier processes have ended.
Prepare a fresh compatible host with the same exact A/B pins and
`--compose-successor-transition-sha256 X`. Null X refuses pending or already
selected B; reopening never discovers and admits a transition. Observe the pending
owner state, then explicitly call `setup_compose_reconcile_successor(request_id,
transition_sha256, observation_sha256)`. Settling B still requires explicit rebind.
Alternatively a new ordinary B host may pin expected B and profile B directly.

The runtime's read-only selection projection owns selected/pending facts. Public
setup summaries contain hashes and closed topology metadata only. Pending
transitions, ordinary pending operations, stale admitted plans, and outer worker
uncertainty suppress all bindings. Rebind cannot clear uncertainty. Same-session
request-ID replay returns the original job without repeating a transition or
rebind, renewing admission, or clearing a later uncertainty fence. Selection and
`pending_operation: false` do not assert engine readiness.

### Same-owner state migration in the 20261006-compose-migration candidate

The `20261006-compose-migration` private library candidate pairs the planner,
runtime and setup bridge for the bounded [state migration contract](../python/COMPOSE.md).
Previously published archives and SDK wheels are unchanged. Use this matched
candidate or matching current source before selecting the contract in a host;
its release handoff records the exact consumer qualification and limits.
The host pins the migration entrypoints through the existing exact B profile.
Preparation remains inert. A schema 2 summary may describe retained migration
history or a same-format edge, so its version alone does not report migration.

During the owner's `migrate` phase, A remains selected and no current runtime
bindings are exposed. The browser reports a pending migration and provides only
its existing observation controls. After any outer uncertainty, establish that
earlier processes ended, reopen with the exact A/B/X admission above, observe,
and use `setup_compose_reconcile_successor` explicitly. Reconciliation cannot
authorize another attempted migration apply. Only the runtime's confirmed owner
receipt and worker settlement permit B selection; explicit host rebind remains
separate. Migration entrypoints, worker commands and receipts stay with the
canonical owner, outside the closed setup summaries. No rollback action is added.

To return to earlier application bytes after B has settled, the host can admit a
new C selection with an explicit reverse owner migration using the
[existing migration contract](../python/COMPOSE.md#return-to-an-earlier-application-through-a-new-selection).
Open a fresh host pinned to selected B and exactly admitted C, then use
`setup_compose_prepare_successor` and `setup_compose_adopt_successor`. Earlier
A/B admission does not authorize B/C. After a lost response, first establish
process settlement, reopen with exact B/C/X, observe and explicitly reconcile.
Pending migration keeps B selected with no bindings; a confirmed C still needs
explicit host rebind. A pending B transition cannot be bypassed with C.

The [archived Mac recipe qualification](../../docs/verification/20261006-compose-reverse-migration/REPORT.md)
uses actual emitted MCP commands and unchanged product bytes. The owner converts
current state while retaining B's cursor, effects and both full migration
receipts, rather than restoring an old database. Its separate append-only
effect observer records every attempt and is not mounted by the migrator. This
is a controlled owner recipe, not a live-provider delivery or general rollback
guarantee.

## Results and interruption

Prepared, observed and unconfirmed are backend outcomes, not labels for connected
accounts or completed user tasks. The job retains that outcome. Current graph
facts come only from explicit backend observation, not the setup job history.

Preparation returns hashes and topology metadata, not configuration, policy,
file contents, container commands or environment values. The complete canonical
plan remains in the graph workspace for exact validation and recovery. Summary
projection does not make secret-bearing inputs supported.

The generic **Stop local setup** action is unavailable for container lifecycle
jobs. Killing only the setup wrapper could leave its Docker subprocess running.
Admitted operations therefore settle under their bounded deadline; closing a
browser or cancelling an MCP wait does not cancel them. Graph stop is a separate
explicit backend action and can itself return an uncertain result.

If the outer worker times out, loses its runtime result, or receives a nonzero
runtime exit, the current host
refuses further engine dispatch. The operator must establish that previous work
has settled before explicitly reopening the graph with a fresh host. Reopening
is not an automatic retry. Partial files and resources remain available for
inspection; the bridge never deletes them to conceal an interrupted operation.

The first bridge treats every nonzero runtime exit conservatively, including a
runtime rejection of a stale observation. Errors detected before runtime entry
do not require reopening. This avoids guessing whether a runtime diagnostic was
produced before or after an effect; no automatic repair or retry follows it.
Recognized fixed runtime error codes may accompany an unconfirmed job. They
help diagnose the failure but do not establish whether an effect occurred or
authorize a retry. Arbitrary subprocess error output is not returned to agents.

A returned stdio binding has its own client-owned process lifetime. Successful
tool output or MCP-client context exit does not prove that its container cleanup
settled. Read the backend again before the next operation. If it is pending,
establish that the previous invocation has ended and explicitly resume the
observed graph; do not repeatedly launch the binding. A runtime with the
[SIGTERM correction](../../docs/verification/20261004-compose-lifecycle/runtime/attempt8/REPORT.md)
joins existing cleanup on cooperative termination, but cannot prevent a client's
hard kill. Select that runtime's actual installed digest in the host profile.
