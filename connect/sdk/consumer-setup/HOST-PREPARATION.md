# Prepare a combined setup host

`prepare_host.py` writes the existing selected-package profile, optional
public-PyPI, npm acquisition or local recipe profiles, and ordinary MCP configuration from explicit
host inputs. Explicit family selection can also enable the release's official
GitHub and Playwright routes. It removes manual copying of the release's tool hashes and JSON
arguments. It does not bootstrap an environment, start setup, download a
package, launch a connector, authenticate an account or attach native tools.

For agent-selected public MCPB downloads, the explicit profile/hash pair in
[MCPB acquisition](MCPB-ACQUISITION.md) adds that capability to this same command.
It requires the MCPB runtime and inspection options; no network is used during
host preparation.

First obtain a trusted combined library archive and its SHA-256, extract it
into a host-owned directory, and use that release's root guide to create the
offline MCP Python environment. Keep the archive: preparation verifies the
extracted file bytes, permissions and directory membership against its exact
bytes. Private directory permissions and permission-restricted files from a
umask such as 077 are accepted. Files must keep owner read permission, executable
files must keep owner execute permission, and no permissions may be added beyond
the archive's modes. The command never changes these permissions. Select the original host
Python, the prepared MCP environment's `bin/python`, and any library Node/npm
runtimes using the same root guide. Hashes must come from the host's trusted
input selection, not from an agent's package metadata.

Run the preparation command with CPython 3.11 or newer. Replace the paths and
hashes below. The output and future setup workspace must not already exist;
their parents and the staging root must exist. Keep these directories and the
extracted release disjoint.

```sh
"$toolkit_python" -I -B /ABS/extracted-library/setup/prepare_host.py \
  --release-archive /ABS/downloads/toolkit-library-EDITION-macos-arm64.tar.gz \
  --release-sha256 TRUSTED_ARCHIVE_SHA256 \
  --release-dir /ABS/extracted-library \
  --target macos-arm64 \
  --output /ABS/host/new-setup-config \
  --workspace /ABS/host/new-setup-workspace \
  --staging-root /ABS/host/staging \
  --python /ABS/trusted/python3.11 --python-sha256 TRUSTED_PYTHON_SHA256 \
  --mcp-python /ABS/setup-client/runtime/bin/python \
  --mcp-python-sha256 TRUSTED_MCP_PYTHON_EXECUTABLE_SHA256 \
  --node /ABS/trusted/node --node-sha256 TRUSTED_NODE_SHA256 \
  --npm-cli /ABS/trusted/npm/bin/npm-cli.js --npm-cli-sha256 TRUSTED_NPM_CLI_SHA256
```

For Linux use the Linux archive and `--target linux-x86_64`. Preparation binds
that target to the release's selected-tool provenance and writes both target
fields where applicable. It does not probe the native runtime or assert version
compatibility. The existing setup/installer owners still perform those checks.

The default family selection is `agno,services,activepieces`; Node and npm are
required so that Activepieces is not silently removed. To deliberately select
only the Python libraries, pass `--families agno,services` and omit Node/npm
arguments. Supported family names are `agno`, `services`, `activepieces`,
`official-github` and `official-playwright`. Official families require their
component in the selected archive for its target. Optional Compose acquisition
and execution use the separate choices below.

The `20261005-host-paths` edition rejects overlapping paths using
Unicode-normalized, case-folded spellings in every preparation mode, including
ordinary fresh setup. This can
also reject distinct paths on a case-sensitive filesystem. Choose clearly
disjoint names so configuration publication cannot occupy a future workspace
on a case-insensitive filesystem. The frozen `20261005-release-guide` direct
preparer predates this correction; its bootstrap already applies the rule.

## Enable container acquisition or execution

The selected library must include the Compose runtime and planner and advertise
`compose-local-execution` in its selected-tool provenance. Linux archives through
`20261005-compose-browser` omit that planner and refuse these options. Read the
selected edition's platform release guide for its actual availability and
qualification. A host-installed substitute is not selected implicitly.

To enable image acquisition, add this pair to the preparation command, or to
the [offline bootstrap command](HOST-BOOTSTRAP.md):

```text
--compose-acquisition-host-profile /ABS/host/proposal-host.json
--compose-acquisition-host-profile-sha256 TRUSTED_PROPOSAL_HOST_SHA256
```

This is the existing [proposal host document](../python/COMPOSE-PROPOSALS.md),
with scope `prepare_agent_selected_local_container_plan_with_image_pull` and
explicit `allowed_registries`. It fixes the engine, tools, placement and project;
the agent supplies the graph. The generated setup exposes
`setup_acquire_compose`, which may acquire images when called. These options
alone expose no `setup_compose_*` execution tools. See
[image acquisition](COMPOSE-ACQUISITION.md) for the staging and result contract.

To enable execution, select the exact existing backend profile separately and
choose actions explicitly:

```text
--compose-backend-profile /ABS/host/admitted-proposal/backend-profile.json
--compose-backend-profile-sha256 TRUSTED_BACKEND_PROFILE_SHA256
--compose-graph-workspace /ABS/host/new-persistent-graph
--compose-mode new
--compose-allowed-actions prepare,start,observe
```

The example permits only those three actions. Omitted actions are unavailable;
there is no default execution grant. To reopen an existing graph, select its
existing directory, use `--compose-mode reopen`, supply
`--compose-expected-plan-sha256 EXPECTED_RETAINED_PLAN_SHA256`, and choose actions
excluding `prepare`, for example `observe,stop,remove`. Reopening validates the
retained selection and exact profile through the canonical runtime projection when setup starts.
Preparing host configuration alone does not verify the graph or contact Docker.
New mode requires an absent graph and no expected-plan argument.

Reopen mode can also pin one exact successor using
`--compose-successor-inputs` and its `--compose-successor-inputs-sha256`, plus
`--compose-successor-backend-profile` and its matching SHA-256 option. The
optional `--compose-successor-transition-sha256` admits one already retained
transition for explicit recovery; without it, reopening requires A still selected with no pending successor transition.
Ordinary pending operations remain observable but suppress runtime bindings.
The helper retains `compose-successor-inputs.json` and
`compose-successor-backend.json` byte-for-byte. These source options require an
archive containing the matching successor-aware runtime and setup bridge; older
published archives do not gain them. See the [explicit successor sequence](COMPOSE.md#explicit-same-format-successor-selection)
for host-enabled actions, pending-state refusal and the separate mutating rebind.

`--compose-delete-state-namespaces` is an optional comma-separated list; it
defaults to empty. New mode requires a subset of the backend profile's volume
keys. On reopen, the setup host validates the explicit list against the exact
admitted selected plan's canonical state inventory, including dormant namespaces
retained from a removed component. Preparing configuration does not perform this
readback or grant deletion permission. Namespace names use canonical package Id
syntax; the request must remain a subset of the host's explicit list.
The separate `remove` action must also be allowed. The optional
`--compose-operation-timeout-seconds` defaults to 300, accepts 1-3600, and applies
to both Compose profiles when both are selected. Acquisition and execution may
share one setup host or be configured independently. A proposal never turns
acquisition permission into execution permission.

The helper preserves exact inner profile bytes and generates the existing
outer profiles using this archive's runtime/planner, the selected Python and
the same staging root. Docker and Compose executable paths/hashes remain in
the host-selected inner profile. All profiles and tool files must be canonical,
hash-matched and separate from mutable host paths; the graph must also be
separate from the release, configuration, setup workspace and staging.
Ambiguous case/Unicode aliases are refused conservatively. Bootstrap additionally
protects its fresh client directory. Preparation validates the profile bindings
without running the planner, Docker, acquisition or graph lifecycle. The
[Compose execution guide](COMPOSE.md) defines those later operations.

## Recover selected jobs for readback

A generated setup command has a single-use workspace. Starting the same command
again still refuses an existing workspace. To inspect prior work, create a new
configuration and new workspace with an explicit recovery selection:

```text
--recover-manifest /ABS/host/history/recovery.json
--recover-manifest-sha256 TRUSTED_RECOVERY_MANIFEST_SHA256
```

The manifest uses the existing [multi-request recovery format](MULTI-RECOVERY.md).
It names one canonical original workspace and 1-256 exact request IDs and hashes:

```json
{
  "schema_version": 1,
  "workspace": "/ABS/host/original-setup-workspace",
  "requests": [
    {"id": "0123456789abcdef0123456789abcdef", "sha256": "REPLACE_WITH_RETAINED_REQUEST_SHA256"}
  ]
}
```

Select these anchors from the original jobs' `recovery_request` records. The
helper does not search for jobs, infer a completed installation, or rewrite the
original workspace. Keep that workspace and its retained request inputs
available. An old staging tree may be absent; recovery preserves its canonical
path without recreating it, and later reconciliation reports unavailable
artifacts as unconfirmed. The manifest itself is copied byte-for-byte into private
`recover-manifest.json` output; its original path is no longer needed after
successful preparation. Protect its contents for the intended setup audience.

Use the same library payload, selected tools, runtime bindings and recipe
profile required by the selected requests. The archive's existing per-kind
recovery readers validate these bindings and all retained request inputs before
publication, and setup validates them again at startup. A changed configuration,
proposal, tool binding or anchor refuses preparation. This is not an upgrade or
a migration to another library version.

For configured library requests, explicitly select their families and runtimes,
for example `--families services`. For acquisition-only recovery, `--families ''`
omits library routes and does not require old pip, npm, Node or acquisition
profiles. Without optional readers, no selected-install profile is generated.
Recovery rejects public acquisition flags/profiles, Compose acquisition or
execution options, remote-recipe authoring and `--prior-request`; none can be
used to reopen installation authority.

Optional recovered archive inspection still requires
`--enable-acquired-inspection` and the current shipped inspection tools. The
generated selected profile must exactly match the profile recorded by the
original acquisitions, including staging root and inspection choices. For
Node acquisition-only inspection the retained Node kind selects the shipped
Node validator without requiring a Node/npm runtime. MCPB inspection additionally
requires `--allow-mcpb --enable-mcpb-inspection`; an acquisition-only readback does
not require Node. Mixed selections containing selected installs retain their
normal runtime requirements. Inspection remains unavailable until each job has
a matching reconciliation observation.

The original workspace, manifest and all historical acquired staging paths
cannot overlap the new output or workspace. Recipe database and sidecar paths
must remain separate from the original workspace, manifest and currently
selected staging root. Bootstrap additionally protects every historical
acquired staging path against its client destination and recipe database paths.
Case and Unicode aliases are refused conservatively. Preparation
rechecks the full manifest and every selected request immediately before
publication. The generated MCP exposes readback, history and explicitly opted-in
readers only. Reconciliation observations do not change historical jobs from
`unconfirmed` to `succeeded`, and no install, retry or stop tool is restored.

## Reuse a prior configuration

To start a fresh setup using settings saved through **Save recovery file**, add
these three inputs to the preparation command:

```text
--prior-manifest /ABS/host/history/toolkit-recovery.json
--prior-manifest-sha256 TRUSTED_RECOVERY_MANIFEST_SHA256
--prior-request-id EXACT_CONFIGURED_REQUEST_ID
```

Select one configured-install request explicitly, even when the file contains
only one request. The helper uses the existing recovery reader to verify the
manifest and its request anchors, then copies only the selected request into
the new configuration. It does not search the workspace or choose an
integration. Acquisition, selected-package and recipe-export requests cannot
seed this configured-library flow.

Keep the original workspace available until preparation finishes. The manifest,
original workspace and selected request must be canonical and separate from
the selected release, new configuration, workspace, staging and any recipe
database or sidecars. Bootstrap also protects its fresh client directory.
Preparation rechecks the manifest and its request bindings before publication.
The resulting private `prior-request.json` needs no original workspace after
successful preparation. Other requests and the recovery manifest are not
copied into the new setup.

The host must approve the selected request's whole contents as nonsecret and
suitable for the new setup agent's audience before supplying these inputs.
Saving a recovery file or being allowed to inspect an old job does not supply
that approval. Choose current families and runtimes as for any fresh setup;
the old request cannot enable them. After startup, **Reuse prior configuration**
or `setup_successor` presents settings against current help for review, with
operations selected again explicitly.

These options require the updated host helper. Previously frozen archives keep
their existing command-line interface. Do not combine this selection with
`--recover-manifest` or the raw request pair below: inspection-only recovery and
fresh configuration review are distinct sessions.

If the exact historical configured-install request is already available, use
this existing alternative:

```text
--prior-request /ABS/host/history/configured-install-request.json
--prior-request-sha256 TRUSTED_REQUEST_SHA256
```

The host must approve the whole file as nonsecret and suitable for the setup
agent's audience. Format validation cannot detect secrets in ordinary strings.
Use a canonical absolute path without symlink aliases, outside the release,
configuration output, new workspace and staging root. The source must also be
separate from any selected recipe database and its SQLite sidecars.

Preparation uses the selected release's existing historical-request parser,
accepting version 1 library requests, version 2 official GitHub requests and
version 3 official Playwright requests. It verifies the exact file hash and
internal configuration hash, then preserves the original bytes in private
`prior-request.json` output. It rechecks the source before publication and
forwards the copied path and hash through the existing MCP startup arguments.
The original file is not modified and need not remain available after successful
preparation. Old workspace and runtime paths inside it are not opened or run.

Include the intended family explicitly in `--families` when it is not enabled
by default, with any current official-family inputs described below. The source
does not enable a family or choose a replacement profile. Once setup starts,
`setup_state` identifies the historical selection and `setup_successor` proposes
values against the current profile's help. The agent reviews unapplied fields
and operations, chooses current operations again, and uses the existing check
and install tools. See [prior configuration reuse](README.md#reuse-prior-configuration-in-a-new-setup)
for the complete review flow.

This input records historical intent, not proof of a successful old installation,
semantic compatibility or current permission. Preparing or installing the new
selection does not switch an active installation, restore grants or implement
rollback. Omit both options for the existing setup behavior.

## Enable official GitHub or Playwright

Official families are disabled by default. Add `official-github` to
`--families` to enable the archived GitHub route. For GitHub alone, use
`--families official-github` and omit the library Node/npm arguments. Keep the
other required preparation inputs above, including the staging root. The
generated configuration retains catalog browsing and selected Python setup.

Preparation binds `official/CONTENTS.json` and
`setup/install-library-consumer.py` from the verified archive and emits the
existing official-controller arguments. It also selects that setup installer
as the shared verifier for library and official readback. No hashes need to be
copied from another release. GitHub is available in the current macOS arm64
and Linux x86_64 archives; a missing component or mismatched target is refused.

Playwright requires `official-playwright` in `--families` and a host-selected
profile with exactly these four fields:

```json
{
  "schema_version": 1,
  "chrome_executable": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "chrome_sha256": "REPLACE_WITH_SHA256_OF_SELECTED_EXECUTABLE",
  "process_parent": "/private/tmp/my-playwright"
}
```

Pass the profile and its exact file hash:

```text
--playwright-host-profile /ABS/host/playwright.json
--playwright-host-profile-sha256 TRUSTED_PROFILE_SHA256
```

The current Playwright route supports macOS arm64 with Node 24.19.0 and
npm 11.12.1. Supply those through the library `--node` and `--npm-cli`
arguments and their SHA-256 arguments. Playwright shares this pair with
Activepieces; it does not use `--selected-node` or `--selected-npm-cli`.
The latter pair remains independent for selected-package acquisition.

Choose an existing Chrome executable and an existing, short, host-controlled
process parent using canonical absolute paths without symlink aliases.
Preparation checks the profile's exact hash, version-1 envelope and 64 KiB
limit, then preserves its bytes in private `playwright-host.json` output.
The existing installer remains responsible for Chrome, runtime, path and Unix
socket-length validation. Preparation does not run those executables or
create the browser's process directories. Supplying a profile without selecting
Playwright, or selecting Playwright without a complete profile and library
Node/npm pair, is refused.

These options configure the existing setup tools. They do not install a
connector, start GitHub or Chrome, request a token, authenticate an account or
establish readiness. Playwright's selected operations describe exercised
coverage; the emitted server exposes its whole endpoint.

## Enable a host-owned recipe store

Recipe access is disabled by default. When the selected archive includes
`selected/bin/toolkit-local-recipes`, add all four inputs explicitly:

```text
--recipes-database /ABS/host/recipes.sqlite3
--recipes-realm HOST_SELECTED_REALM
--recipes-owner HOST_SELECTED_OWNER
--recipes-scope read
```

The host derives the realm and owner from its authenticated operator and
approves that store's nonsecret metadata for the setup agent's audience.
Preparation does not choose a namespace or sanitize stored documents.
`read` maps to the existing `disclose_nonsecret_local_recipe_metadata` scope.
Use `retain` for `retain_and_disclose_nonsecret_local_recipe_metadata`, which
also permits the existing recipe retention operations.

For hosted MCP retention and configuration export, use `retain` and add the
separate `--allow-remote-recipes` flag. It is refused with `read` or without a
complete recipe selection. See [hosted recipes](HOSTED-RECIPES.md) for the
existing tools and limits. An export still requires real current host
references and an agent's concrete choice. Preparation creates neither
decision records nor admission references, and it does not contact an endpoint,
authenticate, attach native tools or grant permission to call exported tools.

The database must already exist at a canonical absolute path without symlink
aliases. Before using a fresh store, the host explicitly runs the shipped
canonical initializer as a separate action:

```sh
/ABS/extracted-library/selected/bin/toolkit-local-recipes init \
  --database /ABS/host/recipes.sqlite3 \
  --realm HOST_SELECTED_REALM --owner HOST_SELECTED_OWNER
```

Its parent must exist. Follow the [initialization contract](RECIPES.md#host-configuration)
for existing databases and failure behavior. The preparation helper never
invokes the executable, opens SQLite, initializes the database or saves a
recipe. Its profile validator checks the database's regular-file path; actual
database contents and schema remain the canonical store owner's responsibility.

Keep the database and its `-wal`, `-shm` and `-journal` paths outside the release,
configuration output, setup workspace and staging root. None may overlap those
directories, even when a sidecar does not exist yet. Existing sidecars must be
regular files without symlink aliases. Preparation leaves them unchanged.
The `20261005-recipe-paths` edition also applies the existing Unicode-normalized,
case-folded overlap rule to these database and sidecar paths in ordinary fresh
preparation. Earlier direct preparers applied that normalized recipe check only
with Compose, recovery or a prior-settings manifest; bootstrap already applied it in every
mode. Use disjoint names such as `configuration`, not an alias of a future
`recipes.sqlite3-wal` sidecar path.

The generated private `local-recipes.json` binds the exact shipped executable
path and SHA-256 from the verified archive. The existing profile reader checks
it before publication. Missing executable support, including a Linux archive
without this binary, returns `local_recipes_not_in_release`; preparation never
substitutes a host binary or enables recipes implicitly. Other family and
selected-package options can be used in the same command.

## Enable public acquisition

Public PyPI acquisition is disabled by default. To authorize that existing
scope, add all three options explicitly:

```text
--allow-public-pypi
--pip-zipapp /ABS/trusted/pip.pyz
--pip-sha256 TRUSTED_PIP_ZIPAPP_SHA256
```

Supplying pip without the flag, or the flag without a complete pip selection,
is refused. Preparation itself makes no network call. A later setup session
started with the generated configuration may run trusted pip and download
public wheels when the agent calls `setup_acquire_python`. This is not a network
firewall. See [Python acquisition](PYTHON-ACQUISITION.md).

## Enable MCPB installation and investigation

With a release that includes `selected/bin/toolkit-mcpb-install`, add
`--allow-mcpb` and the pinned `--selected-node` / `--selected-node-sha256`
inputs. MCPB alone does not require npm. A complete selected Node/npm pair
continues to enable the ordinary Node package path; partial npm inputs are
refused. Library Node/npm selections remain separate.

Add `--enable-mcpb-inspection` to let the agent investigate a bundle before
authoring its configuration. This requires `--allow-mcpb`, the pinned package
inspector and the release capability `mcpb-archive-inspection`. Older archives
refuse this option. The generated selected profile includes
`mcpb_inspection: true`; installation permission alone does not enable reads.

Place only the public `artifact.mcpb` in a direct staging child. The agent calls
`setup_inspect_mcpb` with its `staged_id`, exact `artifact_sha256` and `files: []`
for the manifest, then names additional public text members if useful. The
canonical reader validates the whole archive without extraction or execution.
The agent chooses configuration and entrypoint values from that evidence, then
uses the unchanged selected check/install tools. Sensitive prerequisites can
be reported, but the existing installer still requires its nonsecret,
self-contained Node profile. No authentication or activation is implied.

## Read ordinary Agent Plugins

Add `--enable-plugin-inspection` when the release advertises the
`agent-plugin-files` capability. Preparation pins the canonical inspector and
enables `setup_inspect_plugin` plus `setup_inspect_package`. Availability follows
the selected archive's capability and exact inspector, not the host operating
system. Earlier Linux archives without that capability refuse this option.

An agent supplies a direct staging child and explicit public files, for example
`{"staged_id":"my-plugin","files":["./plugin.json","./mcp.json","./skills/example/SKILL.md"]}`.
The tool returns their exact bounded text, canonical metadata diagnostics and a
`plugin_source` reference. It reads below the selected child without running
plugin code. Public guidance is untrusted research material, not host authority.
A skill-only plugin may omit `mcp.json`.

The agent reads the guidance and supplies any missing Toolkit declarations.
It can pass the returned `plugin_source` to `setup_selected_check` with its
chosen package and activation documents. The existing proposal retains those
source bytes for subsequent installation and recovery. This does not infer
an entrypoint, approve a contract or alter the plugin's original files.

## Submit investigated package metadata through MCP

For Python and Node, to let the agent inspect the primary archive after acquisition, add
`--enable-acquired-inspection`. This requires an explicitly enabled public PyPI
or npm acquisition path and the shipped inspection worker. It adds
`setup_inspect_acquired`, which lists archive members and reads bounded UTF-8
text from an exact completed acquisition. It does not extract files, import
package code, choose an entrypoint or grant server execution. The agent uses
that evidence to author its package and activation documents.

For MCPB acquisitions, the same opt-in enables the exact job/receipt form of
`setup_inspect_mcpb` through its existing canonical reader. See
[MCPB acquisition](MCPB-ACQUISITION.md); this path does not need the Python/Node
inspection worker.

The flag is independent of metadata validation below. Both are disabled by
default; inspection grants no additional network scope. See
[acquired archive inspection](ACQUIRED-INSPECTION.md) for the file, text and
job-binding limits.

Add `--enable-package-inspection` to pin the archive's existing
`selected/bin/toolkit-package-inspect` in the selected profile and expose
`setup_inspect_package`. The agent can then obtain the canonical package digest
through MCP and submit its exact `package_json` and `activation_json` to
`setup_selected_check`. The same validator freezes the proposal for the
separate installation call. Metadata files do not need to be written into
staging; package artifacts still come from the existing acquired or host-staged
inputs. See [selected setup](SELECTED.md).

This flag is independent of public acquisition and is disabled by default.
It starts no inspector during preparation, adds no registry access and grants
no selected-server execution. Research tools and truthful host configuration,
policy and lifecycle references are still needed. The profile continues to
bind the same existing package and installation owners.

## Use the prepared files

### Add selected Node packages

The library and selected-package tools can use different Node/npm runtimes.
For example, the Linux library uses Node 22.23.2/npm 10.9.8, while npm acquisition
requires Node 24.15 or later within major 24 and npm 12.2.0. Keep the library
`--node` and `--npm-cli` inputs above. To enable selected Node installation,
supply a complete separate pair:

```text
--selected-node /ABS/trusted/node24
--selected-node-sha256 TRUSTED_SELECTED_NODE_SHA256
--selected-npm-cli /ABS/trusted/npm12/bin/npm-cli.js
--selected-npm-cli-sha256 TRUSTED_SELECTED_NPM_CLI_SHA256
```

The pair adds the shipped Node validator to the existing selected profile. It
does not enable network acquisition or change the library runtime. No runtime
version is inferred or downloaded. The host must retain the complete npm
installation associated with the chosen CLI.

To enable public npm acquisition as well, supply all three options:

```text
--allow-public-npm
--npm-archive /ABS/trusted/npm-12.2.0.tgz
--npm-archive-sha256 TRUSTED_FULL_NPM_ARCHIVE_SHA256
```

The complete npm archive belongs to the acquisition owner; the separately
selected npm CLI belongs to the offline installer. The existing owners validate
their respective inputs. Python and npm acquisition opt-ins are independent.
Partial pairs, incomplete opt-ins and older Python-only releases without the
Node tools are refused before output. See [Node acquisition](NODE-ACQUISITION.md).

### Add selected Node build support

This optional development path is not in Connect 0.9.0. With the complete selected Node/npm pair above and a matching archived
installer/Node validator, supply both options:

```text
--node-build-profile /ABS/host/node-build-profile.json
--node-build-profile-sha256 EXACT_RAW_PROFILE_SHA256
```

The nonempty profile is an exact regular file of at most 65536 bytes. Its
[version-1 shape](../../crates/toolkit-package/NODE-INSTALL.md#version-1-host-profile)
selects named executable/input files, an exact Node target, a fixed nonsecret
environment and limits. The pair alone cannot enable Node or network acquisition.
Omission keeps the existing profile and no-build behavior.

Preparation snapshots the unchanged bytes as `node-build-profile.json`, adds
`node_build: {path, sha256}` to `selected-install.json`, and binds the snapshot in
the existing prepared receipt. Referenced executable/input paths are not rewritten
or provisioned; they must remain valid on the execution host. The pinned installer
owns their schema and byte validation during planning/review. Preparation does
not run a compiler, Node, npm or a package build.

`package-connect-resources.py` accepts the same pair and places the exact profile
in the existing resource inventory. The product preparer discovers that entry
and forwards the pair through the starter and archived host preparer. Do not add
a new top-level `resources.json` field or supply a build profile through agent
tool arguments. See the [selected setup sequence](SELECTED.md#select-an-explicit-node-build).


### Add public npm search

To let an agent research npm candidates before choosing a package, enable the
separate read-only search capability:

```text
--enable-node-search
--selected-node /ABS/trusted/node24
--selected-node-sha256 TRUSTED_SELECTED_NODE_SHA256
--npm-archive /ABS/trusted/npm-12.2.0.tgz
--npm-archive-sha256 TRUSTED_FULL_NPM_ARCHIVE_SHA256
```

This uses the same pinned Node 24.15-or-later major-24 runtime and complete npm
12.2.0 distribution. Search does not require `--selected-npm-cli` or
`--allow-public-npm`. Omit those to leave Node installation and acquisition
disabled. The preparer's existing Python selected-install and chosen library
capabilities remain as configured; the search switch does not change them.
To support an exact candidate's later acquisition and Node installation, supply
the separate complete selected npm pair and `--allow-public-npm` described above.

The generated `node-acquisition.json` includes exact Boolean
`allow_acquisition:false` unless public npm acquisition was also selected.
`mcp.json` adds `--allow-node-search`, and `prepared.json` records
`node_search_enabled` separately from `public_npm_enabled`. The shared owner
validates all runtime and archive inputs before publication, with no network or
process execution during preparation. The agent sees `setup_search_node`; its
query is later sent publicly and its results grant no selection or installation
authority. [The search contract](NODE-ACQUISITION.md#research-public-npm-candidates)
defines literal-query restrictions, evidence limits and joined cancellation.
Recovery-only preparation refuses the search opt-in.

### Generated configuration

A successful output directory contains:

- `selected-install.json`, using the existing standing local-software scope.
- `node-build-profile.json`, only with the complete optional build-profile pair.
- `python-acquisition.json`, only when public PyPI was explicitly enabled.
- `node-acquisition.json`, only when public npm acquisition or search was explicitly enabled.
- `compose-acquisition-host.json` and `compose-acquisition.json`, only with the
  explicit Compose acquisition profile pair.
- `compose-backend.json` and `compose-execution.json`, only with the explicit
  Compose execution profile and choices.
- `playwright-host.json`, only when the official Playwright family was selected.
- `local-recipes.json`, only with the complete explicit recipe selection.
- `prior-request.json`, only with the complete approved historical-request pair.
- `mcp.json`, containing the ordinary `mcpServers.toolkit-setup` entry with
  absolute paths and hashes of the exact generated profiles.
- `prepared.json`, written last, recording the archive binding, selected target,
  runtime input hashes and output file hashes. Its scope is
  `host_setup_configuration_only`.

Load `mcp.json` through the host's existing MCP configuration interface when
ready to start setup. The command uses `/usr/bin/env -i` with a fixed minimal
environment and keeps the library and selected tools in one Controller. Follow
[combined setup](COMBINED.md) for the agent flow and included metadata inspector.
The host remains responsible for admission of the later setup process.

The prepared MCP interpreter retains its virtual-environment invocation path.
Its resolved executable hash and symlink association are checked separately;
replacing that invocation path with the base Python would lose the environment.
Executable hashes do not attest a Python environment's installed dependencies,
Node's supporting files, dynamic libraries or runtime compatibility. Preserve the
previously verified environment and the release under exclusive host custody.

## Publication and failure

Preparation reads bounded ordinary archive files, rejects unexpected extracted
members and modes, and checks the generated profiles through the release's
existing profile readers. These trusted Python owner modules are loaded for
validation; no Controller, runtime subprocess, installer, pip or provider is
started. Input hashes and release files are rechecked before publication. This
is a point-in-time check, not protection against a concurrent hostile writer
or a future change before MCP startup.

Validation failures leave no final output directory. Publication exclusively
creates a new directory and writes files with mode 0600, with `prepared.json`
last. An interruption or I/O error during publication can leave incomplete files
without a completed receipt. Such a directory is not a prepared configuration;
retain it for inspection and choose a different new output path. The command
never overwrites an existing output, retries setup, or creates the future setup
workspace. A prepared receipt proves configuration output only, not installed
software, usable accounts, native attachment or completion of the original task.
