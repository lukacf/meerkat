# Install a selected self-contained Node MCPB

`toolkit-mcpb-install` materializes an explicitly selected MCPB archive and writes
a standard `mcp.json` that an ordinary MCP client can launch. It composes the
existing package validator, MCPB importer and local artifact preparer. It does
not discover a package, fetch dependencies or attach tools to a native agent.

The operator supplies a trusted executable and exact package, activation,
configuration and archive files. An agent can research and propose these files;
the host still selects and admits the concrete installation. A serialized plan
or self-consistent receipt is not authorization.

## Install and use

Keep these inputs in a directory controlled by the operator:

- `package.json`: an `IntegrationPackage` with the selected `local_mcp` strategy.
- `activation.json`: an `ActivationManifest` bound to that package's canonical
  content digest. Select one independent local Node component, one service
  export and one portable MCP entry. The component source is the exact archive
  with `dependencies.kind` set to `self_contained`.
- `configuration.json`: explicit nonsecret string values for every MCPB
  `user_config` field. Its raw SHA-256 is also the activation's
  `configuration.content_digest`.
- The original `.mcpb` archive, bound by the component's artifact digest.

The selected entrypoint names a relative `.js`, `.mjs` or `.cjs` file in the
archive and contains the complete literal arguments after that script. The
bundled manifest must agree after the supported substitutions. For Geoprims
0.1.0, the explicit configuration `{"toolsets":"navigation"}` corresponds to
`server.mjs` with the single literal argument `--toolsets=navigation`. The
installer never chooses or applies a default. The donor declares an empty
default, but its CLI rejects the resulting `--toolsets=` argument; the retained
first execution demonstrates why installation verification alone cannot prove
that a server starts. The explicit navigation selection exposes six meta tools
and 40 direct tools, not only the declared unit-conversion operation.

To check those exact inputs before installation, use a fresh scratch destination:

```sh
env -i PATH=/usr/bin:/bin \
  /ABS/toolkit-mcpb-install check \
  --package /ABS/inputs/package.json \
  --selection /ABS/inputs/activation.json \
  --selection-sha256 HOST_SELECTED_RAW_ACTIVATION_SHA256 \
  --component SELECTED_COMPONENT_ID \
  --artifact /ABS/inputs/server.mcpb \
  --config /ABS/inputs/configuration.json \
  --config-sha256 HOST_SELECTED_RAW_CONFIGURATION_SHA256 \
  --destination /ABS/new-check-directory
```

`check` accepts the same arguments as `install` except `--node`, which is
rejected. It uses the same package and selection validation, bounded artifact
preparer, verified reopen and MCPB plan derivation. It writes `inputs/` and
`prepared/` in the new check directory, including the frozen archive and
materialized tree. It executes no runtime probe, server or other child process,
and creates neither `ready/` nor an MCP launch configuration.

Standard output is the canonical JSON serialization of `McpbInstallPlan`, with
`schema_version: 1`, `package`, `package_content_digest`, `manifest_digest`,
`component`, `artifact_digest`, `mcpb_manifest_digest`, `configuration_digest`,
`prepared_evidence_digest`, `entrypoint`, `mcp_name` and `minimum_node_version`.
The output is bounded to 8 MiB. It contains no observed runtime or installation
readiness. In particular, the minimum Node version is a package requirement,
not evidence that any selected runtime satisfies it.

The host must freeze and bind the original inputs to its reviewed proposal.
Serialized plan data is not authorization or a reusable validation receipt.
Install into a different fresh destination using those exact inputs; `install`
repeats validation and preparation before probing the selected runtime. A check
directory cannot be installed into or verified as an installation. Retain it
after success, failure or lost output for inspection; checks never overwrite
or repair an existing destination. Check failures can leave partial files or a
complete `prepared/` directory without any installation readiness claim.

Pin the raw activation and configuration hashes in the host's review, then run:

```sh
env -i PATH=/usr/bin:/bin \
  /ABS/toolkit-mcpb-install install \
  --package /ABS/inputs/package.json \
  --selection /ABS/inputs/activation.json \
  --selection-sha256 HOST_SELECTED_RAW_ACTIVATION_SHA256 \
  --component SELECTED_COMPONENT_ID \
  --artifact /ABS/inputs/server.mcpb \
  --config /ABS/inputs/configuration.json \
  --config-sha256 HOST_SELECTED_RAW_CONFIGURATION_SHA256 \
  --node /ABS/trusted/node \
  --destination /ABS/new-installation
```

All supplied paths must be absolute, and the destination must be new with an
existing parent. Symlink inputs and a symlink destination are refused. The
canonical destination must be expressible as exact UTF-8 MCP strings. Directory
custody remains the host's responsibility; these checks are not a filesystem
sandbox against a concurrent hostile local user.

On success the destination contains:

```text
inputs/package.json
inputs/activation.json
inputs/configuration.json
prepared/payload
prepared/tree/...
prepared/dependencies/...
prepared/evidence.json
ready/installed.json
ready/mcp.json
```

Use `ready/mcp.json` in an ordinary MCP client. It names the selected absolute
Node executable, the verified absolute script path and the literal arguments.
The installer does not execute the server. The client owns process lifetime,
environment, protocol initialization and shutdown. Generic MCP configurations
can inherit environment variables from their client; no environment isolation
is implied. The installed directory is not relocatable because the emitted
launch paths are absolute.

An ordinary client importing this bare configuration sees the entire server
endpoint. Declared operations, effects, resource
limits and lifecycle contracts remain metadata for their respective host
owners. The configuration is not an operation filter, grant, account-readiness
receipt or proof that the endpoint obeys those declarations.

Connect's native protected attachment can separately enforce an approved subset
of exact MCP tool names through the host's current grant and MCP access
configuration. Separate write tools do not, by themselves, rule out that
protected read subset. An operation-name allowlist does not constrain query
semantics, prevent startup effects or provide filesystem confinement. Inspect
the allowed tools and startup behavior, and require the host's separate resource
controls. See [operation selection and runtime enforcement](../../sdk/consumer-setup/METADATA.md#operation-selection-and-runtime-enforcement).

## Verify, uncertain results and restore

The artifact preparer verifies the archive and materialized file tree, then the
installer cross-checks the bundled manifest and probes the selected runtime.
Only after those checks does it atomically publish `ready/`. Standard output is
the same canonical installation receipt stored at `ready/installed.json`.

If output is lost after publication, inspect the destination and verify it
against the original selected inputs:

```sh
env -i PATH=/usr/bin:/bin \
  /ABS/toolkit-mcpb-install verify \
  --package /ABS/inputs/package.json \
  --selection /ABS/inputs/activation.json \
  --selection-sha256 HOST_SELECTED_RAW_ACTIVATION_SHA256 \
  --component SELECTED_COMPONENT_ID \
  --config /ABS/inputs/configuration.json \
  --config-sha256 HOST_SELECTED_RAW_CONFIGURATION_SHA256 \
  --node /ABS/trusted/node \
  --destination /ABS/new-installation
```

Verification reopens the retained payload through `LocalArtifactPreparer`,
checks every expected input and ready file, derives the launch again and probes
`node --version` again. It executes no donor code and does not repair or complete
an interrupted publication. It also refuses a changed runtime binary even if
its new version is otherwise compatible.

A failed install can leave `inputs/`, a fully published `prepared/`, or
`pending-ready/`. A failed final directory sync or lost output can also leave a
complete `ready/` with an unconfirmed caller outcome. No error implies that
these files were rolled back. Repeating `install` into that destination is
refused. Preserve the files for inspection; a restore is a separate installation
into a new directory using the same exact selected inputs and trusted runtime.
This restores software, not provider state or native activation.

## Supported profile

This backend deliberately supports a concrete MCPB 0.3 profile:

- macOS or Linux local materialization, with the current platform explicitly
  present in the manifest's compatibility list. Windows execution is not
  implemented, even when an archive declares it.
- A self-contained archive accepted by the existing bounded ZIP32 preparer.
  The original archive and all materialized bytes stay under that owner's
  digest, path, file-type, size and recovery checks.
- `server.type: node`, `mcp_config.command: node`, one explicit relative script,
  and the first argument exactly `${__dirname}/<selected-script>`.
- Later arguments are literal strings, optionally containing exact
  `${user_config.NAME}` substitutions. No shell, environment substitution,
  additional directory substitution, control characters or recursive expansion.
- Explicit string configuration only, with no sensitive, multiple, numeric,
  file or directory fields. Optional fields must still be explicitly supplied.
  A field's nonsecret declaration is not secret detection; the host must keep
  credential values out of these files and arguments.
- Exactly one Node requirement in the form `>=MAJOR.MINOR.PATCH`, plus a known
  platform list. Other range syntax, runtime requirements, platform overrides,
  environment maps, setup behavior and unknown executable metadata are refused.
- No authentication requirement, frontend or event export, dependent component,
  Toolkit-only host contract or externally managed lifecycle.

The operator selects the actual trusted Node executable, not a runtime wrapper.
The installer hashes that regular executable before and after a bounded
`--version` probe with an empty environment. It allows at most 128 output bytes
and a three-second deadline, killing and joining the direct probe on ordinary
failure or timeout. It does not supervise arbitrary descendants or guarantee
cleanup after abrupt termination of the installer. Trust and custody of the
runtime and its ancestor directories remain host responsibilities. A compatible
version is a software prerequisite, not protocol or account readiness.

## Implementation and verification scope

The feature is `local-artifacts`; no new dependency resolver, native registration
API or credential owner is introduced. `McpbInstallPlan` is a derived projection
from the existing validated selection and prepared artifact. The existing npm
and Python-wheel profiles retain their own pinned-closure restrictions.

Focused Rust tests use a synthetic archive and an inert version-probe fixture.
They exercise cross-binding, refusals, byte tampering, publication, reopening
and fresh-directory restore. Those tests do not establish behavior of a real
Node runtime or donor server. The retained 2026-10-03 package run passed 149 tests, including 14 MCPB
cases, and strict Clippy. The 2026-10-04 setup increment separately passes the 19 current MCPB CLI tests and focused strict Clippy; it does not claim a new complete package run. The real Geoprims 0.1.0 proof under
`docs/verification/20261003-mcpb-geoprims/attempt3` uses Node 24.19.0 on macOS
arm64 with process-scoped network denial. Two separate installations expose the
expected 46 tools through their emitted configurations. Each standard client
checks the conversion schema, converts 100 cm to 1 m, observes two input-error
cases and exits its context. A third installation verifies after its success
response is lost; archive, argument and materialized-file tampering are refused.

That donor requires an explicit toolset and the client's `legacy` mode,
negotiating MCP `2025-11-25`. Its advertised `2026-07-28` automatic mode fails
tool-list validation because its response lacks `resultType`. The two original
failures remain retained. The installer does not add client-specific protocol
options to standard MCP JSON; the consuming host must select and verify its
protocol configuration. License and notice materials are retained beside the
unchanged donor archive. This local proof does not clear public redistribution,
certify scientific accuracy or prove retirement of every possible descendant.

Native agent-led acquisition, host admission, authentication, original-task
continuation and publication remain separate work. This path does not change
the existing adopted library count.
