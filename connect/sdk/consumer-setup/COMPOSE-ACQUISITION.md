# Acquire a selected container integration's images

`setup_acquire_compose` downloads immutable container images and prepares the
agent's selected graph for host review. It reuses the canonical Compose planner
and `ComposeRuntime.propose`; the setup host only binds inputs, runs the job and
verifies the returned files. The agent chooses the integration and writes its
selection. Downloading an image does not authorize running it.

## Ordinary library host

The `20261004-compose-onboarding` macOS arm64 library provides the runtime,
planner and setup bridge. Add these options to its `setup/prepare_host.py` or
`setup/bootstrap_host.py` command, along with the command's normal release,
Python, staging and fresh-output inputs:

```text
--compose-acquisition-host-profile /ABS/host/proposal-host.json
--compose-acquisition-host-profile-sha256 TRUSTED_PROPOSAL_HOST_SHA256
```

Supply the existing inner proposal host document described below. The helper
preserves its bytes and supplies the runtime/planner paths and hashes from the
archive; no wrapper profile needs to be assembled by hand. It uses the chosen
Python and shared staging root. The optional
`--compose-operation-timeout-seconds` defaults to 300 and accepts 1-3600.
It validates the host envelope and file bindings without contacting Docker.
The current Linux library lacks the planner and refuses these options.

This grants acquisition only. To execute a resulting plan, independently select
the backend profile and explicit actions as described in
[host preparation](HOST-PREPARATION.md#enable-container-acquisition-or-execution).
No execution tools are enabled by the acquisition pair alone.

## Existing direct host profile

For direct composition of the setup browser or MCP host, the existing outer
profile contract remains available independently of a catalog or execution
profile:

```text
--compose-acquisition-profile /ABS/host/compose-acquisition.json
--compose-acquisition-profile-sha256 EXACT_PROFILE_SHA256
```

The host profile contains exactly:

```json
{
  "schema_version": 1,
  "scope": "acquire_agent_selected_container_images",
  "staging_root": "/ABS/host/staging",
  "python_sha256": "SHA256_OF_THE_HOST_PYTHON_EXECUTABLE",
  "runtime": {"path": "/ABS/compose.py", "sha256": "RUNTIME_SHA256"},
  "planner": {"path": "/ABS/toolkit-compose-plan", "sha256": "PLANNER_SHA256"},
  "host_profile": {"path": "/ABS/host/proposal-host.json", "sha256": "HOST_SHA256"},
  "operation_timeout_seconds": 600
}
```

`--python` selects the runtime interpreter; the profile pins that exact executable.
The MCP transport interpreter separately needs the supported MCP SDK. All paths
are canonical absolute host paths. The staging root already exists and is
separate from the new setup workspace. Host custody of files, executables and the
Docker endpoint must remain exclusive for the operation.

The inner `host_profile` is the existing
[proposal host document](../python/COMPOSE-PROPOSALS.md), with scope
`prepare_agent_selected_local_container_plan_with_image_pull` and a required
`allowed_registries` list, for example `["docker.io", "ghcr.io"]`. It pins engine
identity, endpoint, tools, placement and project. Registry entries authorize exact
first-hop authorities; the agent cannot add another through its selection. The
engine owns transport, redirects, mirrors and proxy configuration. Ambient Docker
client credentials are excluded; this does not provide private-registry login or
an egress firewall.

## Agent flow

1. Research an integration and its exact image digests/platforms. Write the
   existing [proposal selection](../python/COMPOSE-PROPOSALS.md) as `selection.json`
   in one direct child of the staging root. Include nonsecret configuration only.
2. Call `setup_acquire_compose` with `request_id` (32 lowercase hexadecimal
   characters), `staged_id` (the child name), and `selection_sha256`.
3. Poll `setup_job` or `setup_state`. The runtime validates the entire selection
   before touching the engine, records pull intent, downloads each distinct
   registry image/platform, and verifies engine, images and tool pins.
4. Success returns `scope: compose_plan_for_host_review`, `proposal_directory`,
   `receipt_sha256` and the existing proposal receipt. Read its hash-bound files.
   The host reviews and admits the resulting plan through its normal policy.
5. Feed the resulting backend profile and inputs to the existing
   [Compose execution capability](COMPOSE.md). That separate capability prepares,
   starts, observes and explicitly stops/removes the graph.

Local image IDs remain inspect-only. Registry references must be immutable and
pass canonical validation; tags are not accepted as identities. The runtime uses
an empty private Docker client configuration. It never changes the selection to
fit an available image or starts a container during acquisition.

## Stop, replay and uncertainty

The existing Controller owns admission, jobs and exact live-session replay.
Request IDs are shared across Python, Node and Compose acquisition: a changed
kind or argument set refuses. Repeating an identical ID returns the original job,
including an unconfirmed outcome. Browser or MCP disconnection does not mean
stop. Use `setup_stop` for an explicit request.

The setup process gives the pinned runtime up to 10 seconds to handle TERM and
join its Docker child before forced termination. This grace is additional to the
configured job deadline. The runtime bounds each pull to 120 seconds. Neither
process exit nor cancellation proves that daemon cache changes were rolled back.
An entered operation that cannot be verified closes new Compose acquisition in
that setup session. The host must establish quiescent exclusive custody before
starting another session; existing files and cache effects remain available for
inspection. No automatic retry, image deletion or cache pruning is performed.

`image-acquisition.json` is the runtime's durable intent and observation record;
the final `proposal.json` is written last. A partial directory or image present
in Docker is not a successful proposal. Acquisition jobs are not restored across
setup-host restart. Native grants, authentication, attachment and original-agent
continuation retain their existing owners.

## Recover retained inputs

Keep the returned `recovery_request` and original workspace. A new readback-only
host can check the final receipt and acquired files without downloading or
pulling again. Follow [acquisition recovery](ACQUISITION-RECOVERY.md). A matching
file check leaves the historical job outcome unchanged and supplies no execution
or account authority.
