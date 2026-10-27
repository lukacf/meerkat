# Local Compose component lifecycle

`meerkat_toolkit.compose` runs an explicitly selected container graph through a host-pinned Docker Engine and Compose installation. The canonical `toolkit-compose-plan` executable validates the package, selected manifest, host profile and dependency order. Python retains that result and owns bounded CLI execution and current Docker observations.

The latest private runtime wheel has passed 74 installed-package tests using a fake Docker driver and real local subprocesses, with network access and source-tree reads denied. Actual canonical graphs have also completed retained-state lifecycle and adversarial checks: MCP operations, released Meerkat dispatch, stop/restart, exact removal and recovery after interrupted shutdown. See the linked verification reports for each artifact and its limits. The newer proposal and same-format successor operations are source additions; that previously frozen wheel does not include them. This interface does not supply native acquisition, account authentication, execution grants or domain readiness.

## Install and bind the host

Use Python 3.11 or later in an isolated environment. Install the reviewed local SDK wheel with no dependency resolution:

```bash
"$PYTHON" -m pip install --no-index --no-deps /absolute/path/to/reviewed-sdk.whl
"$PYTHON" -I -m meerkat_toolkit.compose --help
```

The Compose module uses the standard library. The complete private SDK wheel also contains other adapters with separate optional dependencies and licensing; installing it does not configure those adapters. The planner, Docker CLI and Compose plugin are separate host-supplied executables, not bundled by this module.

Supply a new private workspace, an exact planner path and SHA-256, and a canonical request. The request pins the selected package/manifest, nonsecret configuration and policy bytes, additional files and the host profile. The profile binds the explicit Unix socket, observed engine ID, Docker/Compose executable paths and hashes, project name, already-present image IDs/platforms, resource limits, mounts and health checks. Paths must name the actual regular executable files, not symlink launchers.

A host whose engine reports shared files under another root can explicitly set `engine.bind_source_prefix` to `{ "local_root": "/absolute/host/root", "engine_root": "/absolute/engine/root" }`. The optional mapping is part of the hashed host profile. A selected file must match its original host path or the exact corresponding mapped path at a path-segment boundary. No mapping is inferred from Docker output, and a changed profile requires a new plan. Observations retain both the host source and the path reported by the engine. The host attests this filesystem relationship; a matching prefix alone is not independent proof of shared bytes.

The host must control the workspace and its ancestors, selected binaries and engine. A digest identifies reviewed input; it is not execution permission. Obtain permission through the host's existing admission boundary before invoking an operation with effects. The workspace is retained input and correlation data, not a credential store.

See the [canonical request/profile contract](../../docs/verification/20261004-compose-lifecycle/canonical/CONTRACT.md) and its [illustrative request](../../docs/verification/20261004-compose-lifecycle/canonical/request.example.json). Example image and tool pins are placeholders that must be replaced by actual host-observed values. The planner never downloads or loads an image.

## Prepare, review and start

The examples below use an installed SDK. An ordinary library archive instead
ships the same runtime as `setup/compose_runtime.py`, with its planner under
`selected/bin/toolkit-compose-plan`. Use the
[archive-local proposal command](COMPOSE-PROPOSALS.md#use-the-ordinary-library-archive)
to produce exact host-reviewable inputs, then the
[generated setup successor workflow](../consumer-setup/COMPOSE.md#explicit-same-format-successor-selection)
for admitted updates. No separate SDK installation is required for that path.

An agent can use [proposal preparation](COMPOSE-PROPOSALS.md) to produce the
exact backend profile from its own integration choice and observed installed
images. The host reviews that concrete result before admitting execution.

Set these values to reviewed host paths and a verified planner digest. Keep `request.json` and the response files outside the new workspace:

```bash
PYTHON=/absolute/isolated-environment/bin/python
PLANNER=/absolute/path/to/toolkit-compose-plan
PLANNER_SHA256=replace_with_reviewed_sha256
WORKSPACE=/absolute/private/new-deployment

compose() {
  "$PYTHON" -I -m meerkat_toolkit.compose \
    --workspace "$WORKSPACE" \
    --planner "$PLANNER" \
    --planner-sha256 "$PLANNER_SHA256" "$@"
}

compose prepare < request.json > prepared.json
PLAN_SHA=$("$PYTHON" -I -c 'import json; print(json.load(open("prepared.json"))["plan_sha256"])')
```

`prepare` requires an absent workspace. It writes the original request, canonical prepared result, generated Compose/configuration files, local operation record and isolated Docker configuration. It has no engine effects. Review the exact returned plan before admitting execution.

Once the host has admitted that plan:

```bash
compose start --plan-sha256 "$PLAN_SHA" > start-result.json
compose observe > observed.json
```

First start requires an absent project. Images must already exist at their pinned identities. The backend never builds or pulls images, publishes host ports, or inherits ambient Docker contexts or credentials. It starts deployment components in canonical dependency order and observes each prerequisite before starting a dependent. An unavailable prerequisite prevents that dependent from starting; independent selected components can still proceed. Each component's initial readiness wait is bounded to 30 seconds.

The profile's `image_id` is the result of inspecting the selected image with its explicit platform. Some Docker engines retain a registry index ID on the container instead. In that case, the backend requires the exact pinned registry digest and matching platform manifest descriptors from both image and container inspection. It records both identities; it does not accept another tag or an arbitrary image ID.

Running deployment containers must retain their selected service aliases on the private network. An endpoint cannot claim another selected service's alias. Alias observations are included in the observation digest, while stopped containers and request containers may lack a service alias. These checks establish the observed routing configuration, not an application response.

Every later operation reruns the pinned planner against the retained request and verifies the generated files. A changed request, profile, tool, engine or observed resource identity is refused. A new Python process can reopen the same workspace by supplying the same bindings.

## Read the result accurately

| Result | Meaning |
| --- | --- |
| `status: prepared` | Canonical files and their exact digest were prepared. No Docker deployment or permission is implied. |
| `status: observed` | The operation's relevant resource readback completed. Lifecycle operations include graph facts in `observation`; `run-stdio` returns only a request receipt and `observation: null`. Neither is a role-readiness certificate. |
| `status: unconfirmed` | An effect, request or subsequent observation could not be confirmed. Resources may exist. Inspect separately; do not infer rollback or retry automatically. |

`observe` returns the observation directly, without an operation-status wrapper. It includes `observation_sha256`, exact container/network references, named-volume metadata, per-component `available` values and `pending_operation`. Availability means the declared Docker running/health observation and its canonical deployment prerequisites currently match. It does not prove an API response, account access or domain readiness.

For JSON-returning lifecycle commands, a CLI exit code of zero means the response was emitted, including an `unconfirmed` response. It does not mean the deployment is ready. `run-stdio` instead exits successfully only when its receipt has `status: observed`, `request.stopped: true` and `request.returncode: 0`. Fixed refusal codes go to stderr with a nonzero exit. A disappeared, replaced or foreign resource must not be treated as a successful replay.

## Explicitly resume a partial start

Ordinary `start` replay does not repair an incomplete or uncertain attempt. After reviewing a fresh observation, the host can explicitly admit a new attempt using the same plan:

```bash
compose observe > observed.json
OBS_SHA=$("$PYTHON" -I -c 'import json; print(json.load(open("observed.json"))["observation_sha256"])')
compose resume --plan-sha256 "$PLAN_SHA" --observation-sha256 "$OBS_SHA"
```

`resume` retains ready running components, starts stopped components by their exact existing IDs, and can create selected components that have never been observed. It waits on the same canonical prerequisites. It refuses a previously observed component that has since disappeared or been replaced. The new attempt does not convert the earlier uncertain outcome into historical success.

## Adopt a same-format successor

This source interface adds `prepare_successor`, `adopt_successor` and
`reconcile_successor` to the existing `ComposeRuntime`, with matching CLI
commands. The source [generated setup host](../consumer-setup/COMPOSE.md#explicit-same-format-successor-selection)
can separately pin one successor and explicitly rebind its session after adoption.
Previously published library archives and SDK wheels remain unchanged.

`selection(transition_sha256=None)` and CLI `selection [--transition-sha256 X]`
read verified retained metadata under the existing workspace lock without Docker
entry. The `selected_plan` envelope carries a `prepared_summary` for the selected
plan, optional exact `prepared_successor_summary`, selected transition ID,
`pending_transition`, exact `pending_operation`, and sorted `state_namespaces`
from that selected plan's full retained inventory. The namespace list is metadata,
not deletion permission, and includes state belonging to removed components.
Each summary identifies the
full verified plan by hash but is not a complete canonical plan. File rows contain
hashes/modes only; raw configuration, contents, commands and environment are
omitted. The result grants no admission or readiness. A setup consumer must not
infer B execution authority from current selection or clear uncertainty by reading it.
The source setup bridge requires this exact internal selection envelope from its
pinned runtime. A mixed pair omitting the namespace inventory refuses; it does
not reconstruct custody from the active profile. Frozen matched archives remain unchanged.

Prepare B in A's existing workspace, passing the exact current A plan:

```bash
compose prepare-successor --predecessor-plan-sha256 "$PLAN_SHA" \
  < successor-request.json > successor-prepared.json
TRANSITION_SHA=$("$PYTHON" -I -c 'import json; print(json.load(open("successor-prepared.json"))["transition_sha256"])')
```

Preparation has no engine effects. The canonical planner checks the same logical
publisher/package name, engine, project, host owner/placement/capabilities, and
exact state namespace/component/owner/format/physical-volume mapping for every
surviving namespace. Package
version, manifest, configuration, policy and executable inputs may change. The
returned transition digest binds the exact original creation request, current
predecessor request and proposed successor request, including their profiles and
files. An old manifest acknowledgement cannot admit B, even when tool schemas
are unchanged. The host must review B's semantics and reevaluate grants through
its existing authority boundary before adoption.

After that explicit host admission, obtain a fresh A observation:

```bash
compose observe > observed.json
OBS_SHA=$("$PYTHON" -I -c 'import json; print(json.load(open("observed.json"))["observation_sha256"])')
compose adopt-successor --transition-sha256 "$TRANSITION_SHA" \
  --predecessor-plan-sha256 "$PLAN_SHA" --observation-sha256 "$OBS_SHA"
```

Adoption requires an attempted, confirmed predecessor with exact captured
resources and retained state. A separately prepared workspace cannot adopt a
project merely because its resource names match. The existing workspace lock
excludes requests and other commands throughout handover. Active request
streams must finish and release this lock before admission; unresolved request
cleanup refuses adoption. Retained stopped requests are removed by their exact
IDs, and their predecessor records remain retained.

Before any engine effect, the owner saves the transition journal and atomically
replaces the original request with a version-2 custody marker. Both previous
and current normal planner modes refuse that marker. Only the new runtime's
explicit successor planner mode reads retained selections. The marker is never
reverted to a legacy-compatible request, including after successful adoption.
A crash before marker replacement leaves A current and has performed no Docker
effects; reentry requires fresh A readback and a new explicit admission. If
replacement is visible but its directory fsync fails, no Docker effect has
entered; reopen follows the actual marker, retains pending custody and requires
fresh observation plus explicit transition reconciliation.

The same owner stops/removes the old graph in reverse dependency order and
starts B in dependency order. It never removes state volumes during adoption.
B uses external volume declarations while runtime observation continues checking
exact original creation labels and captured creation/mount metadata. Current
volume bytes, cursors and effect history belong to the declared state owner;
matching format names do not prove domain compatibility or permit rollback.

The source planner also supports `RetainedOnRemoval`: B may omit a stateful
component and its namespaces when that component is absent from B. A surviving
component cannot shed its state. The canonical plan's `volumes` map remains the
full original custody inventory, including dormant namespaces, while components,
service mounts and lifecycle orders describe only the active graph. The existing
per-plan records keep the exact volume creation and mount metadata. Observation
still checks dormant volumes and rejects replacements or foreign users; no
readiness is inferred for a removed component.

This inventory survives A-to-B-to-C transitions with the original owner, physical
name and creation labels. Its logical format follows the latest confirmed
selection; a migrated namespace retains that format when it later becomes dormant. Reintroducing a removed namespace or reusing
its original component ID is refused in this increment, even with the same state
format. Every transition retires the entire prior graph before starting the new
one, so surviving roles may restart. Interrupted retirement uses the same pending
transition and explicit fresh reconciliation described below. Default `remove`
continues to retain all inventoried state. Component removal grants no state
deletion permission and does not expand a generated host's explicitly admitted
`delete_state` list. The separate, explicitly authorized deletion operation is
unchanged. Ordinary library consumers need the `20261005-compose-retention`
edition or a later archive that selects this planner/runtime pair. Earlier
published archives and the previously frozen SDK wheel remain unchanged.

An interrupted or cancelled handover returns or retains uncertainty. Ordinary
start/resume/stop/remove/request commands refuse until explicit transition
reconciliation. `observe` then returns `status: successor_pending`, the phase,
current resource observation and a digest also binding retained custody. After
reviewing those facts and explicitly admitting continuation, use:

```bash
compose reconcile-successor --transition-sha256 "$TRANSITION_SHA" \
  --observation-sha256 "$CURRENT_TRANSITION_OBSERVATION_SHA"
```

Each B create/start intent is durable before provider entry. Reconciliation can
retain an already running, exactly matching attempted B resource. If its result
was lost and no such resource is present, current absence does not prove that B
never ran or produced effects. The operation stays unconfirmed and cannot
recreate that component, even with another admission. Resolving that case needs
the state/resource owner's evidence and is outside this increment. Cancellation
does not authorize automatic continuation.

The custody file retains up to 32 selections, predecessor records and completed
transition histories under one absolute workspace root. This remains local
exclusive host custody, not a distributed engine lock. State-owner transfers,
automatic rollback, exact prior-selection replay and automated recovery are not implemented; the accepted
`owner_reconciliation` declaration is preserved without executing it.

## Migrate one namespace with its existing state owner

This contract is paired in the `20261006-compose-migration` private library
candidate and current source. Previously published libraries and SDK wheels are
unchanged; the candidate handoff records its exact consumer qualification and
limits. Use a matching planner, runtime and setup bridge. The existing
successor APIs admit one same-owner namespace format change per edge; they do
not create a migration registry or infer a rollback from `reversible: true`.

B's state contract must declare the exact prior format, operation and artifact
digest. Its profile adds this optional binding (other profile fields stay as
shown above):

```json
{
  "state_migration": {
    "protocol_version": 1,
    "namespace": "data",
    "operation": "convert-data",
    "apply": {"executable": "usr/local/bin/migrate", "arguments": ["apply"]},
    "readback": {"executable": "usr/local/bin/migrate", "arguments": ["readback"]},
    "timeout_ms": 30000
  }
}
```

The operation's artifact must be the selected B state component's artifact.
The host admits these exact entrypoints, profile, manifests and plan identities
through the existing successor admission. An old acknowledgement or another
current admission cannot authorize a different migration. Ordinary fresh B
preparation/start uses B's ordinary entrypoint and never runs this capability.
Its presence on a later same-format selection also does not run it again.

Before retiring A, the runtime verifies the selected local migration image,
platform and bound mounts without pulling it. This does not inspect arbitrary
image filesystem paths or prove that an entrypoint works. After retirement, A
remains selected in `migrate`. The owner process receives only the target named
volume plus B's pinned read-only files, with no network; the readback process's
volume mount and root filesystem are read-only. Worker configuration and exact
container identities are observed under the existing workspace lock. An
unresolved or running worker remains visible in the pending observation and
blocks readback/B startup until quiescence is established.

Both entrypoints receive one JSON binding on stdin: protocol version, transition
and predecessor/successor plan digests, namespace, component, owner, prior/next
formats, operation, artifact digest, volume name, persisted `attempt_id`, and
`physical_volume_observation_sha256`. That last digest identifies observed
Docker volume creation/mount metadata, not the bytes stored in the volume.
The exact plan digests also bind the host profile and worker entrypoints.

Readback returns a closed JSON object, at most 64 KiB:

```json
{
  "protocol_version": 1,
  "status": "committed",
  "observed_format": "2",
  "receipt": {
    "binding": {"...": "the complete binding durably stored by this attempt"},
    "evidence_sha256": "<64 lowercase hexadecimal characters>"
  }
}
```

The example's `binding` placeholder represents the entire original object,
without extra keys. The state owner must atomically preserve it with conversion
and its cursor/effect state, then read that saved receipt back. Echoing the
current caller's binding with a format string is not this contract. Toolkit
checks the complete binding and receipt, while the trusted state owner owns
conversion semantics and the meaning of `evidence_sha256`.

Before first apply, readback may return `status: "ready"`, the exact prior
`observed_format` and `receipt: null` for this attempt. After any durable apply
entry, ready/absence/exit status cannot authorize another invocation. Only a
matching committed stored receipt resolves uncertainty. A response loss can be
resolved by explicit reconciliation after the exact worker is stopped; an absent
unconfirmed worker remains refused. Invalid reader output remains unconfirmed,
but the exact stopped reader is removed before decoding so a later explicit
readback can inspect repaired owner state. Readers are never application writers.

After confirmed readback and worker removal, the same journal switches once to B
and starts its services in dependency order. Partial B activation keeps normal
successor uncertainty; it does not undo the conversion or rerun apply. Component
removal still retains the latest confirmed logical format and original physical
provenance. Reintroduction, initialization, owner transfer and implicit rollback
remain refused.

Migration-bearing successor replies use schema 2 so old runtimes reject them
before adoption. Custody schema 3 is durable before engine effects and remains
in place after success, fencing prior readers. Later preparation folds the exact
ordered retained request history, not caller-supplied format assertions. Pure
legacy requests without migration preserve schema 1 output bytes. The browser
projects pending migration only; execution and reconciliation use the already
admitted SDK/CLI or generated-host tools.

### Return to an earlier application through a new selection

After B is settled, the host may select a distinct C that reuses A's application
bytes and declares an explicit same-owner reverse migration, such as format 2
to format 1. C needs its own exact manifest, profile, artifact and admission;
replaying A's request or B's admission cannot authorize it. The state owner must
convert the current state, preserving B's cursor, accepted effects and prior
receipts. Restoring A's old database would lose that progress and is not this
contract. Use the existing `prepare-successor`, `adopt-successor`, `observe` and
`reconcile-successor` operations. An unresolved B transition must be resolved
before preparing C; the new selection cannot bypass it.

The [archived Mac qualification](../../docs/verification/20261006-compose-reverse-migration/REPORT.md)
exercises this recipe with unchanged product bytes. A separate append-every-attempt
effect volume remains inaccessible to the migrator; B's progress survives the
conversion and C emits only the next event. Lost reverse-apply response recovery
uses the stored current-attempt receipt without reapplying. This controlled
observer does not establish live-provider delivery or arbitrary cross-store
atomicity, and adds no implicit rollback or automatic recovery.

## Use a request-lifetime stdio component

The selected component must have request lifetime. Its deployment dependencies must already be available:

```bash
compose run-stdio --plan-sha256 "$PLAN_SHA" --component selected-component-id --timeout 300
```

Use this command and its unchanged arguments as the host's explicit subprocess launch. It is a backend runtime binding, not a portable-MCP export or automatic native attachment. The host still owns tool admission and any protocol-specific checks.

Protocol bytes travel unchanged on stdin/stdout. Child diagnostics and the final lifecycle result use stderr. Input and each output stream are bounded to 16 MiB; the default duration is 300 seconds, with an explicit maximum of 3600 seconds. The backend uses `compose run --no-deps -T --pull never` with a fresh retained attempt name and never starts missing dependencies implicitly.

Cleanup checks the engine, selected image and exact matching request container. It saves the validated container ID before sending any stop, then reads back that same ID. Cancellation joins this cleanup before releasing the workspace lock. Cleanup does not stop shared deployment dependencies or refresh their observations.

The request receipt always has `observation: null`. Its `request.stopped` flag means the request was observed not running and that observation was durably saved. `request.returncode` is the Compose CLI stream's exit code, or `null` if no code was obtained. `status: observed` requires successful cleanup and return code zero. An input/output error, timeout or nonzero return remains `unconfirmed` even when `stopped` is true; successful cleanup can then clear lifecycle pending without claiming the protocol operation succeeded. Missing or changed resources, failed readback or failed persistence keep cleanup unconfirmed. Use a separate `observe` call for current graph availability. A direct child exit is not proof that engine work or all descendants ended.

A host can terminate the wrapper before its engine readback completes. The tested Python MCP 2.2 client gives the wrapper two seconds after closing stdin. If that is too short, a stopped request may still have a pending operation record. Reopen and inspect it, then explicitly resume using the fresh observation before requesting another stream; this does not convert the original unconfirmed close into success. The shorter request-scoped cleanup removes repeated whole-graph scans but does not guarantee completion within a host's shutdown grace.

The [SIGTERM correction](../../docs/verification/20261004-compose-lifecycle/runtime/attempt8/REPORT.md)
routes cooperative termination of the `run-stdio` CLI through that same joined
cleanup. Repeated SIGTERM does not release its workspace lock early. Interrupted
execution still exits nonzero; a subsequent observation determines whether
cleanup finished. SIGKILL cannot be handled and may leave pending state.
The [new private installed artifact](../../docs/verification/20261004-compose-lifecycle/installed/attempt8/REPORT.md)
passes 74 installed tests; older wheels retain their recorded behavior.

For the tested released Meerkat 0.8.50 static host, the per-server setting `connect_timeout_secs = 60` allowed connection, handshake and tool enumeration through this graph's full admission checks. Its default ten-second attempt failed before model entry. This is a host-selected startup budget, not a tool execution timeout or extra shutdown grace. See the [native host guide](../../docs/guides/meerkat-mcp-host.md) for the separate static configuration contract.

One workspace lock is held for the entire request stream, including cleanup. Other operations, including observation, return `operation_in_progress` while that stream holds the lock. This first interface serializes streams and lifecycle operations; it is not a concurrent request pool. Stopped request containers and their correlation records remain until explicit removal, and each workspace allows at most 128 request attempts.

## Stop and remove separately

Stop requires the exact plan and a fresh observation digest. It acts on matching container IDs in reverse canonical order, with the declared whole-second shutdown deadline, and preserves volumes:

```bash
compose observe > observed.json
OBS_SHA=$("$PYTHON" -I -c 'import json; print(json.load(open("observed.json"))["observation_sha256"])')
compose stop --plan-sha256 "$PLAN_SHA" --observation-sha256 "$OBS_SHA"
```

Removal is separately admitted and requires stopped resources. Obtain another fresh observation after stopping:

```bash
compose observe > observed.json
OBS_SHA=$("$PYTHON" -I -c 'import json; print(json.load(open("observed.json"))["observation_sha256"])')
compose remove --plan-sha256 "$PLAN_SHA" --observation-sha256 "$OBS_SHA"
```

By default, removal retains every state namespace. Add `--delete-state exact-namespace` for each namespace the host explicitly authorizes deleting. No prune, remove-orphans or unconditional volume deletion is used. Retained state is not automatically adopted by a new candidate. Removal is not a reset operation that enables recreation through the same workspace.

## Python API and limits

```python
from meerkat_toolkit.compose import ComposeRuntime

runtime = ComposeRuntime(workspace, planner, planner_sha256)
prepared = await runtime.prepare(request_bytes)
# The host reviews and admits the exact prepared plan before this call.
result = await runtime.start(prepared["plan_sha256"])
observation = await runtime.observe()
```

The same instance or a newly constructed instance supports `resume`, `stop`, and `remove` with the plan and observation hashes shown above. `remove` takes keyword argument `delete_state: set[str]`. `run_stdio` takes an asynchronous byte iterable as `stdin` and awaited byte callbacks as `stdout` and `stderr`; caller callbacks must not block the event loop. The module CLI supplies nonblocking pipe sinks.

The canonical profile is bounded to 16 components, 64 dependency edges, 16 volumes and 64 additional files. It supports deployment/request containers, explicit readonly configuration and declared named state. Other components may mount an owner's state readonly only through a declared dependency. Unsupported activation contracts, frontends, host services, event exports and external management are refused. State migration is limited to the same-owner protocol above.

The lock and identity checks assume exclusive local host custody, not distributed fencing against another Docker administrator. Image environment defaults plus selected overrides are checked exactly. Image-declared volume targets must each have an explicit selected bind or named-volume mount before any container is created; unbound anonymous state is refused. Mount/entrypoint overlap checks are lexical; the planner and runtime do not inspect image filesystem symlink topology. The host must review the pinned image's path layout. Named volumes expose names/creation metadata rather than immutable container-style IDs, so no stronger engine identity guarantee is implied.

Same-plan stop/restart preserves state. The source successor API above adds bounded same-format adoption and explicitly admitted same-owner migration in the same workspace, including the declared reverse-migration recipe above. Automatic reconciliation, implicit rollback, exact prior-selection replay, native authentication and native acquisition remain unsupported. The [current installed receipt](../../docs/verification/20261004-compose-lifecycle/installed/result.json) identifies the 65-test wheel and runtime. The [retained-graph evidence](../../docs/verification/20261004-compose-lifecycle/REPORT.md#completed-retained-graph-lifecycle) separately records the earlier runtime's actual lifecycle and explicit recovery, including its original shutdown uncertainty.

The same installed backend has also passed fresh actual-engine acceptance on two
different component topologies. It covers dependency loss, explicit partial-start
recovery, request cancellation, foreign and replaced resources, shutdown order,
and a real one-second stop timeout followed by separate stopped-state readback.
See the [acceptance report](../../docs/verification/20261004-compose-lifecycle/REPORT.md)
for exact artifacts and retained failures. The optional
[setup-agent interface](../consumer-setup/COMPOSE.md) exposes this backend under
an explicitly configured host profile.
