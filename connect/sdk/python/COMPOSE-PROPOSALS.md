# Prepare a container integration chosen by an agent

The optional `propose` command turns an agent's explicit selection into the
exact profile and request accepted by the existing Compose backend. The host
supplies its environment; the agent chooses the integration and component graph.

The host supplies a JSON document with exactly `schema_version: 1`,
`scope: "prepare_agent_selected_local_container_plan"`, `owner`, `placement`,
`host`, `project` and `engine`. The last five fields have their existing
[Compose profile meanings](../../docs/verification/20261004-compose-lifecycle/canonical/CONTRACT.md):
ownership, placement, declared host capabilities, an isolated project name,
and explicit engine/socket/tool bindings. Optional `engine.bind_source_prefix`
retains its existing meaning. These facts describe preparation; they do not
authorize execution of an agent's selection.

The agent supplies the existing six staged fields: `schema_version`,
`package_json`, `manifest_json`, `configuration`, `policy` and `files`. It adds
`components` and `volumes` with their existing profile shapes. Each component
omits `image_id`: Toolkit observes that value from the explicitly selected image
and platform. The agent chooses the service name, numeric user, environment,
mounts, resources and health check. Volumes have explicit names under the host's
project prefix. Configuration and files are nonsecret input; this workspace is
not a credential store.

## Use the ordinary library archive

When the selected archive declares `compose-local-execution`, it contains the
standalone `setup/compose_runtime.py` and `selected/bin/toolkit-compose-plan`.
From the extracted archive root, use a trusted native Python 3.11 and the exact
planner digest recorded in `selected/provenance/selection.json`:

```bash
python3.11 -I -B setup/compose_runtime.py \
  --workspace /ABS/host/new-proposal \
  --planner /ABS/library/selected/bin/toolkit-compose-plan \
  --planner-sha256 TRUSTED_PLANNER_SHA256 \
  propose --host-profile /ABS/host/host-profile.json \
  --host-profile-sha256 TRUSTED_HOST_PROFILE_SHA256 \
  < /ABS/host/selection.json > /ABS/host/proposal-receipt.json
```

Use the host's absolute archive path and verified digests. This uses the archived
runtime directly; it needs no Toolkit checkout or separate core SDK wheel.
It only inspects already present, explicitly selected images. After reviewing
the resulting `backend-profile.json` and `inputs.json`, pin those exact files in
[host preparation](../consumer-setup/COMPOSE.md#host-setup). For an admitted
same-format successor B, use them as the explicit
[successor target](../consumer-setup/COMPOSE.md#explicit-same-format-successor-selection).
The host separately approves the new manifest, configuration, policy, profile
and allowed actions. Shared state format never transfers A's authority to B.

## Use an installed Python SDK

With a reviewed SDK installed into the selected Python environment, the equivalent
module entrypoint is:

```bash
"$PYTHON" -I -m meerkat_toolkit.compose \
  --workspace /absolute/new-proposal-directory \
  --planner /absolute/toolkit-compose-plan \
  --planner-sha256 "$PLANNER_SHA256" \
  propose --host-profile /absolute/host-profile.json \
  --host-profile-sha256 "$HOST_PROFILE_SHA256" \
  < selection.json > proposal-receipt.json
```

Replace paths and hashes with actual reviewed host bindings. The first planner
pass validates the complete selection and lowers image references. Only then
does Toolkit issue bounded `info` and `image inspect` calls to the selected
engine, using isolated empty Docker configuration. The second planner pass binds
observed image IDs. Image facts and the engine are checked again before
publication. Images must already be present; no images are pulled and no
containers are started.

A successful receipt has `status: proposed`,
`scope: compose_plan_for_host_review`, the final `plan_sha256`, and hashes of
six files:

| File | Purpose |
| --- | --- |
| `host-profile.json` | Exact supplied host bytes |
| `selection.json` | Exact supplied agent selection bytes |
| `backend-profile.json` | Existing profile with observed image IDs and file hashes |
| `inputs.json` | Six staged fields accepted by the setup bridge |
| `request.json` | Exact existing backend request |
| `prepared.json` | Final canonical plan for host review |

The receipt is written to `proposal.json` last. Missing receipt means publication
did not complete. Existing destinations are refused; partial attempts remain
inspectable. Stdout contains only the receipt, not configuration, file bodies
or raw engine diagnostics. A proposal is not a deployment workspace: it contains
no runtime operation record or live-resource claims.

After reviewing and admitting the concrete plan, the host can feed `request.json`
to `prepare` in a separate new deployment workspace. Alternatively, pin
`backend-profile.json` in the existing [setup Compose profile](../consumer-setup/COMPOSE.md)
and copy `inputs.json` into a selected staging directory. The request encoding
matches the bridge, so unchanged bytes produce the same canonical plan hash.
Execution rechecks tools, engine and images. Proposal success does not grant
permission or promise later readiness.

The default Python API is `await runtime.propose(host_bytes, selection_bytes)`, where
`runtime` is a `ComposeRuntime` pointing at a new proposal directory and pinned
planner. The caller owns these input snapshots and the ordinary host review
and admission policy. Native acquisition, authentication and continuation
retain their existing owners.

## Explicitly acquire selected images

When selected registry images are not already present, the host may authorize
image acquisition as part of proposal preparation. This is a separate opt-in:
the host profile uses
`scope: "prepare_agent_selected_local_container_plan_with_image_pull"` and adds
the required `allowed_registries` list, for example `["docker.io", "ghcr.io"]`.
All other host fields remain the same. Registry entries are exact lower-case
first-hop authority names, without a scheme, path, port or credentials. An empty
list permits only local image references when acquisition is requested. The
ordinary preparation scope does not accept this extra field.

Call `await runtime.propose(host_bytes, selection_bytes, acquire_images=True)`
or add `--acquire-images` to the `propose` command above. Possessing the extended
host profile does not itself trigger a pull: omitting the API option or CLI flag
preserves the no-pull behavior.

The canonical planner validates the entire graph and immutable image references
before any engine request. The proposal then checks every selected registry
against the host list before pulling anything. Each distinct registry
image-and-platform pair receives one explicit `docker image pull --platform`
call, even if the engine already has it. Cache reuse belongs to Docker. Local
`docker-image:///sha256/...` selections remain inspect-only and must already
exist. No tag resolution, build, image load, container start, cache removal or
automatic retry is introduced.

Acquisition uses the same pinned CLI, selected engine endpoint, isolated empty
Docker configuration and existing subprocess owner as the runtime. It does not
borrow the user's registry login. The host may deliberately list a private
registry reachable anonymously; the list does not constrain daemon-configured
mirrors, registry redirects or network destinations. The host remains responsible
for the trusted engine and its network configuration. This is not a network
sandbox or native permission grant.

Before the first pull, the proposal retains the exact `host-profile.json` and
`selection.json` bytes and writes `image-acquisition.json`. That record binds
both input hashes, the canonical draft plan hash and the complete image/platform
intent. Each image changes from `not_started` to `pending` durably before its
pull call, then to `client_completed` when the CLI returns successfully. A
successful CLI return is not yet a verified image observation. The ordinary
second planner pass, complete image checks, engine identity and tool-pin checks
must pass before the record becomes `observed` and binds the final plan hash and
observed image IDs. Source manifest digests and engine image IDs retain their
distinct meanings.

If acquisition or its image verification fails or is cancelled, the record
remains `unconfirmed`; its per-image states preserve which calls may have affected
the engine. A hard kill may leave
the record `pending`. Neither state establishes that no data was downloaded or
cached. Normal cancellation and CLI SIGTERM join the owned pull client through
the existing subprocess cleanup path. Each standalone pull has a 120-second
deadline; the calling host may impose a shorter total deadline. This does not
promise atomic rollback of daemon effects.

A completed acquisition adds `image-acquisition.json` to the proposal receipt's
file hashes. The other six outputs and the setup handoff encoding are unchanged.
An `observed` acquisition record without `proposal.json` means image observation
completed but proposal publication did not. Existing or interrupted workspaces
are refused on replay; inspect the retained attempt and make any later retry an
explicit choice in a new workspace. Starting the graph still requires the usual
separate host admission and current runtime checks.
