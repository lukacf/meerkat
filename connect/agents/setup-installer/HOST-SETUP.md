# Host setup from a new workspace

To give one setup agent the ready-made library and outside-catalog Python and
Node acquisition, use the combined [macOS library](../../dist/toolkit-library-20261005-browser-response-macos/toolkit-library-20261005-browser-response-macos-arm64.tar.gz)
or [Linux library](../../dist/toolkit-library-20261005-browser-response-linux/toolkit-library-20261005-browser-response-linux-x86_64.tar.gz).
Follow its extracted root guide and `selected/contracts/HOST-PREPARATION.md` to
generate the existing profiles and MCP configuration without hand-authoring
JSON. If a setup client does not yet exist, the optional
`<extracted-library>/setup/bootstrap_host.py` command installs this release's offline client and
then runs that same preparer. Follow the extracted root guide and
`selected/contracts/HOST-BOOTSTRAP.md`: supply the trusted archive/runtime
inputs, a fresh `--client-directory`, configuration output and workspace, and
the existing host scope options. The helper derives the MCP Python path; no
prior client environment is needed. It does not start setup, acquire a
connector, sign in or attach tools. Preserve partial directories after any
failure. With an existing verified client, use `prepare_host.py` directly.

For selected Node package installation, supply the complete runtime quartet:
`--selected-node`, `--selected-node-sha256`, `--selected-npm-cli` and
`--selected-npm-cli-sha256`. Library `--node`/`--npm-cli` inputs remain separate;
Linux Activepieces retains Node 22/npm 10 while Node acquisition uses Node
24/npm 12. The existing installers still validate their own runtime contracts.

Public npm acquisition additionally requires `--allow-public-npm`,
`--npm-archive` and `--npm-archive-sha256`, selecting the complete trusted npm
distribution. Public PyPI uses its independent opt-in and pinned pip zipapp.
The updated host helper also accepts `--enable-node-search` for public npm
research through `setup_search_node`. It uses the selected Node runtime and
complete pinned npm distribution. Search can be enabled without acquisition;
the helper records that distinction in the existing host profile. Queries go
to the public registry, so the agent must exclude private request details and
credentials. The agent chooses candidates; code supplies bounded metadata and
validates later concrete selections. Follow the selected archive's
`NODE-ACQUISITION.md` contract because earlier editions lack this flag.
Supplying selected runtimes enables no network acquisition by itself. All these
paths share one Controller and job history. The macOS combined library now also includes the Compose planner and runtime.
Use `--compose-acquisition-host-profile` with its hash to enable image acquisition;
use the independent `--compose-backend-profile` pair, graph path, mode and explicit
action list to enable execution. Follow the archived `HOST-PREPARATION.md` and
`COMPOSE.md` contracts. Acquisition permission does not permit starting containers.
Linux has no native Compose planner and refuses both opt-ins. The standalone kit
below retains its earlier profiles on their supported platforms. The current
combined macOS archive also supports MCPB inspection and installation as described
below; the Linux archive does not include an MCPB installer. Native attachment is an
explicit host step; the [released native host guide](../../docs/site/guides/native-mcp.mdx)
describes the supported public MCP path. Governed setup-agent delegation,
protected account authentication and durable continuation remain unfinished.

The current combined archives add `setup_guide`, which returns the enabled
metadata and installer contracts as frozen, bounded text through MCP. Their
root installation commands use the actual archived manifest hashes. This is
documentation only, not authentication or readiness. The
[current delivery review](../../docs/verification/20261005-host-preparation-review/REPORT.md)
pins the current archives, including ordinary plugin setup, optional archive
research after acquisition recovery, and macOS MCPB first-use setup.
On either platform, add `--enable-plugin-inspection` to expose the public-plugin
inspection and source-binding tools. Saved recipes retain exact nonsecret
plugin guidance after the original setup workspace is removed. Enable the
complete recipe scope options described in the handoff. Reuse requires
separately acquired artifact bytes and current host validation. The archived
helper also accepts the paired prior-request options for an explicitly reviewed
configuration proposal.

For an unfamiliar self-contained Node MCPB on macOS, add `--allow-mcpb` and
`--enable-mcpb-inspection` to host preparation. Supply `--selected-node` and
`--selected-node-sha256`; MCPB alone requires no selected npm runtime. The host
stages `artifact.mcpb` and supplies its exact hash. The agent reads its manifest
and selected public files with `setup_inspect_mcpb`, chooses configuration and
metadata, then uses the existing check/install/reconcile path. Inspection is
not permission, authentication or a readiness claim. Linux refuses this opt-in
because its archive has no MCPB installer.

Earlier immutable archives retain stale guide text and bootstrap pins; the
[guide erratum](../../docs/verification/20261004-setup-contract-guidance/PRIOR-GUIDE-ERRATUM.md)
records those corrections without changing the old artifacts.

Use the [current macOS arm64 kit](../../dist/agent-install-kit-20261004-acquisition-recovery/README.md)
with an existing agent host that supports MCP stdio and permits the required
research tools. This guide composes the kit's existing commands. It does not
start an agent, install host runtimes or implement native `find_and_install`.

First-use discovery, installation and a permitted task call need **no recipe
database**. Retention is optional and is described separately below. The shortest
setup here enables selected Python packages; the kit's
`contracts/SETUP-MCP.md` covers enabling Node and supported MCPB profiles.

## Linux Python and Node acquisition

The [current Linux x86_64 kit](../../dist/agent-install-kit-20261004-node-linux/README.md)
supports selected Python and Node acquisition, installation and retained-file
readback. Verify archive SHA-256
`a5a75067b5e9c554dac3e2bde843152a8e9ceedd1c9b6bbc91c2eaa4be34cb46`,
then follow its extracted guide and exact tool hashes. Its unchanged offline
MCP bootstrap requires native 64-bit CPython 3.11.16 and glibc 2.39, with working
`venv` and `ensurepip`. Node acquisition additionally needs host-supplied native
Node 24.15 or later within major 24 and the complete pinned npm 12.2.0
distribution. Set its acquisition profile's `target` to `linux-x86_64`.
Python acquisition uses its separately pinned host pip zipapp and Linux target.
Use the Linux guide's commands and examples, not the macOS examples below.

The predecessor's [Python acceptance](../../docs/verification/20261004-selected-python-linux/REPORT.md)
covers acquisition, host exit, network-denied recovery, offline installation and
an installed Fetch MCP fixture call. The [new Node path](../../docs/verification/20261004-node-linux/REPORT.md)
also passes actual Linux source and extracted-kit acquisition, fresh-process
recovery, offline installation and installed filesystem MCP use, with an exact
outside-directory refusal. Its archive passed independent byte-level review.
These checks do not establish live account authentication, fresh-agent research,
native attachment or continuation. MCPB and Compose executables remain outside
this Linux kit.

## Prepare the kit and client environment

Supply absolute paths for the archive, compatible CPython 3.11 executable and
two new directories whose parents already exist. The host controls these paths.
The Python installation needs working `venv` and `ensurepip` support.

```sh
KIT_ARCHIVE=/absolute/path/agent-install-kit-macos-arm64.tar.gz
KIT_PARENT=/absolute/new/kit-directory
HOST_WORK=/absolute/new/toolkit-host
PYTHON=/absolute/path/python3.11

printf '%s  %s\n' 6c3c6c7d47aefbbcbe1c6694b756011fb4ad94d645f6eb8d8ee9e9bb71b889b1 "$KIT_ARCHIVE" | shasum -a 256 -c -
```

Continue only after verification succeeds. This digest identifies the
`20261004-acquisition-recovery` edition; it is not a publisher signature.
Run each preparation command only after the previous one succeeds. Preserve
partial directories after an error instead of repeating an installation there.

```sh
mkdir -m 700 "$KIT_PARENT" "$HOST_WORK"
tar -xzf "$KIT_ARCHIVE" -C "$KIT_PARENT"
KIT="$KIT_PARENT/agent-install-kit-macos-arm64"
mkdir -m 700 "$HOST_WORK/scratch" "$HOST_WORK/staging"

env -i PATH=/usr/bin:/bin TMPDIR="$HOST_WORK/scratch" \
  "$PYTHON" -I -B "$KIT/bin/install-library-consumer.py" \
  --bundle "$KIT/verification-environment" \
  --contents-sha256 8d89196ba7ef13e32b2aee320e1ffb325fd692917108ea69c80d24c36b372881 \
  --family services --python "$PYTHON" --destination "$HOST_WORK/client"

MCP_PYTHON="$HOST_WORK/client/runtime/bin/python"
```

Require a successful installation before using its interpreter. This offline
step provides MCP SDK 2.2.0 and its dependencies through the included, unconfigured
services 0.1.7 base package. Optional Google authentication dependencies are not
included. It starts no service and requests no credentials. Your agent host can
keep its existing Python/MCP environment: the setup server runs as an isolated
stdio child. The [catalog adoption handoff](../../docs/verification/20261004-current-setup-kit/INSTALLER-HANDOFF.md)
records a verified Python 3.12/MCP 1.30 client and the exact catalog-only host
dispatch restriction.

## Configure the setup agent's MCP connection

Write `$HOST_WORK/selected-profile.json` using the existing profile below.
Replace the three example paths with the actual staging, installer and validator
paths above. The two executable hashes are for this exact kit edition.

```json
{
  "schema_version": 1,
  "scope": "standing_local_software_installation_only",
  "staging_root": "/absolute/new/toolkit-host/staging",
  "installer": {
    "path": "/absolute/new/kit-directory/agent-install-kit-macos-arm64/bin/install-library-consumer.py",
    "sha256": "c04a86a9bbd9b6384be1c03bea29b1f816fe8965c070da34237e46ad31dc2481"
  },
  "validators": {
    "python": {
      "path": "/absolute/new/kit-directory/agent-install-kit-macos-arm64/bin/toolkit-python-plan",
      "sha256": "0436d6682899f33fc6eefb9246d49397a8f63c401017db261a68e4618b4e9f55"
    }
  }
}
```

Compute the profile's raw file hash with
`shasum -a 256 "$HOST_WORK/selected-profile.json"`. Set `PROFILE_SHA256` to the
returned digest. Configure the agent host to launch this command over MCP stdio;
use actual argument values in its configuration, not unresolved shell variables:

```sh
"$MCP_PYTHON" -I -B "$KIT/setup/mcp_adapter.py" \
  --workspace "$HOST_WORK/session" --families '' --python "$PYTHON" \
  --selected-install-profile "$HOST_WORK/selected-profile.json" \
  --selected-install-profile-sha256 "$PROFILE_SHA256"
```

`session` must not exist; the adapter creates it. Keep the host's process,
filesystem and network restrictions in effect. The profile enables local
software installation in this session, not account access or server execution.
MCP connection configuration is owned by the chosen agent host; this kit does
not register itself. Optional `--browser-port 0` exposes the same controller's
local companion through the private launcher it reports on stderr.

## Optional: let the agent acquire its selected artifacts

The current kit includes all three acquisition tools below. Each requires a
separate host profile; the basic command above enables none of them. Read the
named contract in the extracted `$KIT/contracts` directory, create the exact
host profile it describes, hash its raw bytes, and add its profile path and hash
arguments before starting a new setup session. Use `setup_state` to confirm the
enabled capabilities. Do not add profiles to an already running session.

| Tool | Host preparation | Contract in the kit |
| --- | --- | --- |
| `setup_acquire_python` | Keep the selected Python profile above. Supply and pin a trusted pip zipapp, and permit public PyPI access. Add `--python-acquisition-profile` and `--python-acquisition-profile-sha256`. The supported target is CPython 3.11 on macOS arm64. | `PYTHON-ACQUISITION.md` |
| `setup_acquire_node` | Enable the selected Node validator using `SETUP-SELECTED.md`, with the validator hash from `$KIT/SHA256SUMS`. Supply `--node` and `--npm-cli`, and pin the complete npm 12.2.0 distribution in a profile for Node 24.15 or later within major 24 on macOS arm64. Add `--node-acquisition-profile` and `--node-acquisition-profile-sha256`. | `NODE-ACQUISITION.md` |
| `setup_acquire_compose` | Supply the host-selected engine, Docker/Compose tools, allowed registries and proposal settings. Use `$KIT/setup/compose_runtime.py` and `$KIT/bin/toolkit-compose-plan`, with their hashes from `$KIT/SHA256SUMS`. Add `--compose-acquisition-profile` and `--compose-acquisition-profile-sha256`. This can also run without a selected Python/Node profile. | `COMPOSE-ACQUISITION.md` and `COMPOSE-PROPOSALS.md` |

The corresponding source contracts are [Python acquisition](../../sdk/consumer-setup/PYTHON-ACQUISITION.md),
[Node acquisition](../../sdk/consumer-setup/NODE-ACQUISITION.md) and
[Compose acquisition](../../sdk/consumer-setup/COMPOSE-ACQUISITION.md).
The kit contains no pip zipapp, npm distribution, host runtime or Docker engine.
The host supplies those trusted tools and any required network isolation.

The agent chooses exact packages or immutable images and polls the returned job.
Python and Node acquisition retain artifacts for `setup_selected_check` and
`setup_selected_install`; they do not author the agent's package metadata.
Compose acquisition returns a graph proposal for separate host admission and
the existing execution flow in `COMPOSE.md`. Acquiring an image does not start
its container. These steps do not connect accounts or attach native tools.

Save each acquisition's `recovery_request` with its workspace. After that host
exits and its worker has stopped, follow `contracts/ACQUISITION-RECOVERY.md` to
open a readback-only session. **Check acquired files** verifies the retained
inputs without another download or image pull. A matching check can feed normal
setup in a separately authorized session; it does not mark the old job completed.

## Give the agent the task and let it finish

Supply the complete request, `$KIT/agent/instructions.md`,
`$KIT/contracts/METADATA.md` and `$KIT/contracts/SETUP-MCP.md`. Give it the actual
host-approved research tools, staging path and metadata references. Use
`$KIT/host-profile.example.json` as a checklist only: its placeholders are not
authority or a parsed host protocol. The host supplies real configuration,
policy, lifecycle and execution facts; the agent must not invent them.

The agent researches and chooses a compatible integration, prepares its exact
package/artifacts and calls `setup_selected_check`, then
`setup_selected_install`. It observes the job with `setup_job` or `setup_state`
and uses `setup_reconcile` for a separate current file readback. An unsupported
selection needs another host-enabled profile; validation must not be bypassed.

After an unconfirmed selected-package installation settles, the agent can call
`setup_selected_retry` with that exact job ID. It returns one fresh proposal,
not an installation. Review its new destination and effect digest, then use
`setup_selected_install` explicitly. Repeating the retry request returns the
same proposal and preserves the original files and job history.

To stop a running local preparation or installation, call `setup_stop` with its
exact job ID, or use **Stop local setup** for the displayed job in the companion
browser. Observe `setup_job` until the worker settles. Existing files and effects
remain, so use a separate readback before assuming an installation outcome.
See the kit's `contracts/STOP-SETUP.md` for process and retry limits. This action
does not cancel the original business task or disconnect an account.

After the host separately permits the concrete server launch and task calls,
a Meerkat host can use its [existing native MCP configuration](../../docs/guides/meerkat-mcp-host.md).
This avoids introducing another execution adapter. For a standalone check,
the agent uses `contracts/MCP-EXECUTION-CHECK.md` to prepare a discovery request,
inspect actual schemas and prepare its chosen call request. Each request uses
the existing checker:

```sh
"$MCP_PYTHON" -I -B "$KIT/bin/toolkit-mcp-check.py" \
  --request "$REQUEST" --request-sha256 "$REQUEST_SHA256"
```

`REQUEST` is the actual reviewed request file and `REQUEST_SHA256` its raw hash.
The host supplies the admitted launch, environment and resource access. The
checker does not authenticate or authorize the request. The agent evaluates
the observations and answers the original task; installation alone is not task
completion. Uncertain effects remain visible and are not silently retried.

## Optional: retain a choice for another session

Configure this before starting a recipe-enabled session. The current kit includes
`toolkit-local-recipes init`; no separate source build is needed. Use a dedicated
host-owned database path. Initializing the store does not add a placeholder
recipe or identify an authenticated caller.

```sh
RECIPE_TOOL="$KIT/bin/toolkit-local-recipes"
"$RECIPE_TOOL" init --database "$HOST_WORK/recipes.sqlite3" \
  --realm "$RECIPE_REALM" --owner "$RECIPE_OWNER"
```

The tool, realm and owner are host-selected values. Require
`status: "initialized"`; this does not assert emptiness or grant a namespace.
Follow the [recipe profile contract](../../sdk/consumer-setup/RECIPES.md) with
scope `retain_and_disclose_nonsecret_local_recipe_metadata`, the database path
above and the exact selected executable hash. Save that profile as
`$HOST_WORK/recipe-profile.json`, hash it and add these existing flags to the
MCP command:

```sh
--local-recipes-profile "$HOST_WORK/recipe-profile.json" \
--local-recipes-profile-sha256 "$RECIPE_PROFILE_SHA256"
```

If a session has already started, its profile is frozen. Start a new session
with a different new `--workspace` path. The agent can explicitly retain a
settled selection with `setup_remember`, supplying actual decision and
verification references. A later session can recall it through `setup_recipes`
and `setup_recipe`. Call `setup_recipes` with `{"limit":20}` for bounded
traversal of a large store. Pass the returned `next_cursor` as `cursor` with
the same limit to continue. Historical metadata is not current permission
or readiness.

## Connect an already hosted MCP

For a remotely hosted service, the agent can retain its researched endpoint
and export a configuration without installing a local server. Initialize the
optional recipe store above, then also add `--allow-remote-recipes` to the
setup adapter command. The agent uses `setup_remember_remote`, reads the saved
record, supplies a current host-supported activation and calls
`setup_export_recipe`. Observe the returned job and pass its exact configuration
to the host's ordinary MCP client.

The current kit also supports explicit `setup_retry_export` after an
unconfirmed export and separate fresh-session export recovery. Follow
`contracts/HOSTED-RECIPES.md`; preserve the original request and prior files.
Readback observes current files and does not establish historical success.

The extracted `contracts/HOSTED-RECIPES.md` and `HOSTED-METADATA.md` include
complete document shapes, current-reference requirements and repair guidance.
Explicit anonymous recipes are supported for documented public endpoints;
authenticated recipes keep their credential-binding requirement. Export is
configuration, not authentication or operation permission. The host still
admits actual calls, and the agent evaluates their results against the task.

## Enable library preparation and retry

The delivered verification environment also supports preparation of its bundled
services profiles. This is a small setup environment using services 0.1.7, not
the complete 101-integration library. To enable it, use a fresh session path and
replace `--families ''` in the setup command above with:

```sh
--families services --bundle "$KIT/verification-environment" \
--contents-sha256 8d89196ba7ef13e32b2aee320e1ffb325fd692917108ea69c80d24c36b372881
```

The agent can call `setup_prepare`, inspect profiles and choose operations
through the standard setup tools. A failed preparation can use
`setup_retry_prepare` with its current job ID; see
`contracts/RETRY-PREPARATION.md`. That explicit retry uses a new directory and
preserves the previous attempt. It does not repeat an installation, exported
configuration or business call.

For Slack, `setup_help` exposes optional `allowed_channel_ids` as a JSON string
array. Omitting it adds no channel restriction; `[]` rejects every channel call;
an explicit list permits only those exact channel IDs. The host must authorize
the configuration. The installed parser rejects broad conversation listing
when this fixed ceiling is set. This routing limit is not an account grant.

## Remaining host work

This local composition needs the existing agent host and its real permissions.
The kit does not supply native delegation, protected provider login, native
attachment or continuation between original and setup agents. The current
[kit readback and acquisition recovery evidence](../../docs/verification/20261004-acquisition-recovery/REPORT.md)
binds this edition to its exact source and predecessor payloads. Earlier linked
trials establish their named behavior on their own recorded editions; they are
not fresh runs of every feature in this kit. Verification is on the recorded
macOS arm64 host with compatible CPython 3.11, not every macOS version or runtime.
These limitations remain when the commands above succeed.
