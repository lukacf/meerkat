# Bootstrap an offline setup host

`bootstrap_host.py` prepares the local Python client needed to run Toolkit
setup, then generates its ordinary host profiles and MCP configuration. It
uses the selected release's existing offline, unconfigured services installer,
its `--verify-existing` readback, and `prepare_host.py`. It does not download a
Python distribution, start setup, select a connector, authenticate an account or
attach native tools.

The invoking host must trust this helper and its adjacent `prepare_host.py`,
the selected library archive and its digest, and the selected CPython 3.11
runtime. Obtain and extract the archive first. The helper checks the complete
extracted tree against the archive before executing a selected owner. Keep the
archive, extracted release, runtimes and all destination parents under exclusive
host custody throughout the operation.

```sh
"$toolkit_python" -I -B /ABS/extracted-library/setup/bootstrap_host.py \
  --release-archive /ABS/downloads/toolkit-library-EDITION-macos-arm64.tar.gz \
  --release-sha256 TRUSTED_ARCHIVE_SHA256 \
  --release-dir /ABS/extracted-library --target macos-arm64 \
  --python /ABS/trusted/python3.11 --python-sha256 TRUSTED_PYTHON_SHA256 \
  --client-directory /ABS/host/new-setup-client \
  --output /ABS/host/new-setup-config \
  --workspace /ABS/host/new-setup-workspace \
  --staging-root /ABS/host/staging \
  --families agno,services --timeout-seconds 300
```

For a compatible Linux archive use `--target linux-x86_64`. The existing
installer owns exact platform/runtime compatibility checks. The target option
does not make a runtime compatible.

The client, configuration output and future workspace must not exist; their
parents and the staging root must exist. These paths and the extracted release
must be disjoint. The helper also checks the real destinations against a
selected recipe database and its sidecar names before any installation.
Fresh paths with overlapping Unicode-normalized, case-folded spellings are
refused on every platform. This deliberately also excludes some distinct
paths on case-sensitive filesystems, so a configuration and its future
workspace cannot alias on a case-insensitive filesystem.

The optional family, runtime, inspection, acquisition, prior-request, prior-manifest and recipe
arguments follow [host preparation](HOST-PREPARATION.md). Their validation stays
with that owner. The default families remain `agno,services,activepieces`, so
omitting `--families` requires the library Node/npm inputs. Selecting only
`agno,services` deliberately avoids that requirement. Other explicit opt-ins
retain their existing platform restrictions. Public acquisition flags configure
later setup capabilities; this bootstrap still installs only from the retained
offline services closure.

`--enable-node-search` passes through the same
[explicit npm search configuration](HOST-PREPARATION.md#add-public-npm-search).
It requires pinned selected Node and a complete npm archive, but no selected npm
CLI or public acquisition opt-in. Bootstrap remains offline: the later
`setup_search_node` call sends the agent's literal query to the public registry.
Search does not confer Node acquisition or installation permission. Existing
Python selected-install and chosen library defaults remain unchanged.

Do not supply `--mcp-python` or `--mcp-python-sha256`: the helper derives them
from the successfully installed and verified client's `runtime/bin/python`.
It preserves that virtual-environment path while checking its resolved
interpreter against the original host runtime selection.

## Bootstrap a recovery connection

The same command accepts `--recover-manifest` and
`--recover-manifest-sha256` as described in
[host recovery preparation](HOST-PREPARATION.md#recover-selected-jobs-for-readback).
Choose a fresh client, output and workspace; the original workspace stays
read-only. The existing generated command is still single-use and refuses
an existing workspace.

The helper protects the original workspace, manifest and acquisition staging
paths against all three real destinations, not just the disposable preview.
It validates every selected request with the archived preparation owner before
installing the client, then repeats that full validation at effect checkpoints
and before publishing the final configuration. A predecessor preparer that does
not explicitly record recovery mode is refused before client installation.

Only the unconfigured offline client is installed. Starting its generated MCP
connection later exposes recovery readback and selected inspection, never replay
or new installation authority. Use `--families ''` for acquisition-only recovery
without library runtimes. Live acquisition, Compose execution and prior-request
authoring options cannot be combined with recovery. A failure after client
installation retains that client and its receipts; it does not repair or mutate
the original jobs.

## What the result means

To reuse settings from a saved recovery file in a fresh setup, supply
`--prior-manifest`, `--prior-manifest-sha256` and `--prior-request-id` as described
in [prior configuration preparation](HOST-PREPARATION.md#reuse-a-prior-configuration).
This requires approval of the selected request's full contents for the new
setup audience. Only that request is copied. Current family and runtime
choices stay explicit, and setup presents a new configuration for review.
These options cannot be combined with readback recovery.

Both this bootstrap helper and the selected archive's preparer must support
the handoff. The preview must confirm the exact selected request before client
installation; an older preparer that ignores the options is refused. The
manifest and request bindings are rechecked before client effects and before
the final configuration is published.

Before installation, a private disposable preview calls the selected release's
unchanged preparer with the host Python as a placeholder. This validates
configuration and scope, not MCP availability. The actual destination overlap
checks happen independently of the preview path. The preview is never returned
as usable configuration or retained as an installation receipt.

Next, the canonical installer creates the fresh unconfigured client. Its
offline pip closure and payload checks remain unchanged. The helper requires
its successful receipt and successful `--verify-existing` observation of the
same client, original selection, interpreter path and receipt digest. Readback
retains that owner's exclusions, including generated metadata, bootstrap trees
and source-associated caches; it is not a new environmental attestation.

Finally, it rechecks release/runtime inputs and calls the preparer again with
the installed client. The JSON result has scope
`offline_setup_client_and_host_configuration_only` and references the existing
client `installed.json` and host `prepared.json` receipts and their hashes. Use
the generated `mcp.json` through the consuming host's existing MCP mechanism.
The future setup workspace remains absent until setup is actually started.
The result is not a connector installation, account grant, MCP initialization,
native attachment or runtime readiness claim.

## Cancellation and failures

`--timeout-seconds` is an integer from 1 to 900, default 300. The deadline and
stop signal are checked between bounded local validation steps and while an
installer/readback process runs. Synchronous preparation and archive validation
finish their current step before observing a stop; this is not a strict
whole-command wall-clock limit. SIGINT, SIGTERM and SIGHUP request cancellation.
The helper first allows the canonical installer to settle its own child
sessions, then uses bounded termination of the direct process if needed.
Cleanup may take up to six additional seconds. This is not containment of
arbitrary detached descendants.

On failure, no success result is returned. The client and any partially
published configuration remain untouched for inspection; the installer retains
its own inputs and step logs. A late cancellation may therefore leave completed
owner receipts even though bootstrap reports an unconfirmed outcome. The helper
does not infer success from those files, retry, delete them, repair them or
reuse the destination. Inspect the exact owner evidence before choosing a new
destination or a separate supported recovery operation. Existing
`prepare_host.py` remains available when a verified client already exists.

The explicit [MCPB acquisition profile](MCPB-ACQUISITION.md) and its SHA-256 can
also be passed to bootstrap with the normal MCPB runtime/inspection choices.
The preview validates this scope before client installation, then the final
configuration forwards it to setup. Bootstrap itself downloads no MCPB bundle.
