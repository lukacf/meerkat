# Retain and configure a discovered hosted MCP

An agent can discover an HTTPS MCP endpoint outside the catalog, save its exact
recipe and export ordinary MCP configuration through the setup tools. It uses
the same recipe store and portable exporter as the local CLI. The agent chooses
the integration; the tools validate that concrete choice.

Use an archive that explicitly includes these tools, or a compatible source
build. The 20261004-combined-hosted macOS library includes them and offers
[host preparation](HOST-PREPARATION.md#enable-a-host-owned-recipe-store).
Earlier frozen kits retain their original tools.
No server is installed by this workflow. The endpoint already exists; the host
must separately connect, authenticate and permit operations through its MCP
client. A successful export does not resume a native original task.

## Enable the workflow

Use the existing [recipe profile](RECIPES.md#host-configuration), with scope
`retain_and_disclose_nonsecret_local_recipe_metadata` and an initialized
host-owned store. Its pinned `toolkit-local-recipes` executable must support
`save`, `read`, `export` and `check-export`. Add these options to a compatible setup MCP command:

```sh
--local-recipes-profile /absolute/host-owned/recipe-profile.json \
--local-recipes-profile-sha256 EXPECTED_PROFILE_SHA256 \
--allow-remote-recipes
```

The last flag explicitly enables incoming remote/guidance metadata and portable
export. Existing profiles do not acquire these operations automatically. The
host fixes the database, realm, owner and executable; tools cannot change them.
A recipe-only session can use `--families ''` with no consumer bundle.
The browser host accepts the same options; an MCP companion browser displays
export progress and resulting configuration from the shared Controller.

Only send nonsecret metadata suitable for the setup agent's audience. JSON
validation is not secret detection. Keep credentials with the host's credential
owner, outside recipe documents, tool arguments and results.

## Agent workflow

Use the complete [hosted metadata format](HOSTED-METADATA.md) to author the
portable documents and independent remote declaration. Its examples are inert syntax, not
a selected endpoint, decision or permission.

1. Research the user's full request with the host-provided discovery tools.
   Optionally inspect earlier choices with `setup_recipes` and `setup_recipe`.
   Assess the publisher, endpoint and documented authentication requirements.
2. Prepare the exact Agent Plugins `plugin.json`, optional `mcp.json` and
   Toolkit `recipes.json` documents using the existing
   [bundle contract](../../crates/toolkit-package/LOCAL-RECIPES.md#save-the-host-approved-documents).
   Call `setup_remember_remote` with their string contents as `plugin_json`,
   `mcp_json` and `recipes_json`. For the current investigation, omit
   `decision`: the updated pinned recipe owner retains this exact submission
   with a stable research reference in the same durable recipe record. An
   earlier decision record or additional human approval is not required for
   that provenance; the host's retention scope is still required. If an actual
   historical reference exists, optionally supply it unchanged as
   `decision: {owner, record, revision}`. It remains an unverified external
   claim. Neither form supplies approval, source authenticity or service access.
   Older frozen recipe owners do not support omitted-decision submissions or
   the new marked rows; use the updated owner instead of inventing a reference.
   The retained `mcp.json` must include
   `"$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"`.
   A bootstrap file accepted by a generic MCP client may omit that field;
   it still needs it when saved as an Agent Plugins document.
3. Read the returned recipe summary. Its `digest` identifies the retained
   bundle for lookup; `package_content_digest` is the package digest required
   by the remote declaration or a legacy activation. They are different fields.
4. Prepare a `RemoteExportDeclaration` using the existing
   [portable export contract](../../crates/toolkit-package/LOCAL-RECIPES.md#export-an-independent-mcp-endpoint).
   Call `setup_export_recipe` with `publisher`, `name`, `version`, the retained
   bundle `digest` and the exact declaration document as `declaration_json`.
   Include `selection_kind: remote_declaration`; omit host configuration/policy
   snapshots and the remote source's configuration revision. Preserve the full
   component/export independence contract and authentic source declarations.
   Schema hashes and lifecycle fields remain declarations, not live observations.
   Native schema observation and HTTP egress admission must come from their
   actual owners before protected publication. Historical documents confer no
   renewed authority. A legacy complete `ActivationManifest` remains accepted as
   `activation_json`; supply exactly one form and retain it unchanged for readback.
   For public-service connection, the selected `portable_mcp_entries` entry
   must also declare `operation_tools`, mapping every selected operation ID
   exactly once to its distinct actual raw MCP tool name. Keep those names
   grounded in retained source evidence. The exporter returns the map and
   authentication declaration in `declared_remote_service`; without the map,
   export alone does not supply public-service activation evidence.
5. Observe the returned job with `setup_job`. A successful `export` job returns
   `scope: portable_mcp_configuration_only`, the current selection digest,
   file hashes and sizes, the host-chosen paths and actual `mcp_configuration`.
   The Controller reads back both files and compares them with the canonical
   exporter's output before reporting success. The new form also returns
   `selection_kind: remote_declaration` and a separate `declaration_digest` in
   `declared_remote_service`; legacy results retain only their original
   `manifest_digest`. Neither result is an installed-artifact receipt.
6. Hand that ordinary configuration to the host's MCP client. Connection,
   protected sign-in, operation permission and the original task remain with
   their existing owners. Evaluate the task result after permitted calls.

Each document is at most 32 KiB and the complete encoded tool arguments are at
most 64 KiB. These bounds apply to the HTTP surface too. Larger documents need
the existing local CLI path, not splitting one setup operation into fragments.

For a public endpoint whose provider documents access without sign-in, use
the [explicit unauthenticated recipe mode](../../crates/toolkit-package/LOCAL-RECIPES.md#export-an-independent-mcp-endpoint)
and select `authentication: {"kind": "none"}` in its service declaration. The evidence
must refer to a retained source used by that strategy. Omitting the recipe's
authentication mode preserves the existing requirement for a credential
binding. Use a pinned CLI that supports the new mode; older frozen kits do not.
Anonymous access says nothing about the effects of the endpoint's tools.
Inspect actual schemas and admit only the operations needed for the task.

The portable form supports the canonical HTTPS `streamable-http` shape without
inline headers or URL query credentials. It exposes the **whole endpoint**;
selected operations in metadata do not become an MCP operation filter.
Where available, Connect's native protected attachment separately enforces the
host-approved subset of exact tool names. That can exclude separate write tools,
but does not constrain query semantics, prevent startup effects or provide
filesystem confinement. See [operation selection and runtime enforcement](METADATA.md#operation-selection-and-runtime-enforcement).
Packages needing Toolkit host services, state or custom runtime dependencies
cannot be made independent by exporting them. Guidance-only recipes can retain
explicit blockers, but cannot export a fictitious working endpoint.

## Replay, stopping and uncertain results

Saving uses the store's immutable identity rule. Exact document and decision
replay observes the same retained record. Changed bytes or a changed decision
under the same identity are a conflict; choose a new explicit version when
retaining a revised recipe.

When the current CLI rejects an incoming bundle before opening the recipe store,
`setup_remember_remote` returns a fixed `recipe_*_rejected` error and a
`diagnostic` containing `phase: before_store_open`, a code and repair guidance.
For example, `recipe_mcp_schema_rejected` explains the required `$schema` value.
The host may already have retained input files in its workspace; the diagnostic
concerns this save's store access. Correct the indicated document and submit the
revised input. No raw parser message, path or document value is returned.

Missing, malformed or older CLI error output remains `retention_unconfirmed`.
The same applies to errors after a successful save, including failed readback.
Inspect `setup_recipes` and `setup_recipe` before deciding what to do next;
uncertainty is not evidence that nothing was saved. Export failures retain their
existing unconfirmed job state.

An accepted export request maps to one job in the current Controller session.
Exact replay observes that job, including after a lost reply or a stopped job.
It never silently reruns the exporter. Each new accepted request has its own
host-chosen output directory. The bridge retains earlier files and never
overwrites an earlier export.

`setup_stop` targets the exact running export job. Wait until its worker settles;
files may already exist. Missing output, mismatched hashes and stopped work stay
unconfirmed. `setup_reconcile` checks settled exports through the pinned recipe
owner's `check-export` command. It never runs `export` again. The Controller keeps
exactly the admitted input form in its fixed `declaration.json` or legacy
`activation.json` file and reuses the matching canonical command flag. A restart
cannot reinterpret one form or digest as the other.
The generic job surface also reports a canonical validation refusal as an
unconfirmed job; it does not expose a separate typed export-failure state.
Graceful shutdown joins entered work. New export requests now retain and sync
their exact request, activation and profile binding before starting the worker.
The returned `recovery_request` identifies that record. This is an immutable
input record, not a durable scheduler or proof of export completion.

## Explicitly retry an unconfirmed export

In the same live setup session, call `setup_retry_export` with
`{"job_id": "EXACT_UNCONFIRMED_EXPORT_JOB_ID"}`, or choose **Try export again**
in the browser. The old worker must have settled. The host must still allow
remote recipes; a new attempt rechecks the pinned profile, exporter and exact
frozen request before reserving work.

The result is one new export job with `retry_of` pointing to the old job, the
same request digest and a fresh output directory. Earlier files, job status and
file-check observations remain intact, even if earlier output was complete.
This operation exports configuration only; it does not connect a server.

Repeating the same retry request observes that one successor, including while
it runs or after a lost reply. It does not revalidate mutable export files or
start another job. Current session admission and remote-recipe opt-in still
apply. The original `setup_export_recipe` request continues to return the
original job. To try again after a failed successor, explicitly use that
successor's ID. Observe it with `setup_job`; use `setup_stop` to stop that exact
running job and `setup_reconcile` to check its files.

The browser waits for a fresh status observation after an unconfirmed retry
response. Another click then replays the same predecessor ID. It does not
automatically repeat an export or infer that no files were written. Running or
successful jobs, other job kinds, unknown IDs and recovery-only sessions cannot
start a new export retry. Retry correlation is local to this live session;
after restart, use the successor's retained recovery request for read-only
inspection as described below.

## Inspect an export after restarting the host

First ensure the previous host and export worker have stopped and the files are
under exclusive host custody. Start a new setup MCP process with a new workspace,
the same separately pinned recipe profile and the previous job's recovery fields:

```sh
python -I -B /absolute/path/mcp_adapter.py \
  --workspace /absolute/new/recovery-workspace --python /absolute/path/python \
  --families '' \
  --local-recipes-profile /absolute/host-owned/recipe-profile.json \
  --local-recipes-profile-sha256 EXPECTED_PROFILE_SHA256 \
  --recover-workspace /absolute/previous/setup-workspace \
  --recover-request EXACT_PREVIOUS_JOB_ID \
  --recover-request-sha256 EXACT_RETURNED_REQUEST_SHA256
```

The host supplies these paths and hashes. Agent tool arguments cannot select a
different database, owner, profile or workspace. The current profile must match
the retained binding exactly. This readback session does not need
`--allow-remote-recipes`; only `setup_state` and `setup_reconcile` are exposed.

Call `setup_reconcile` with the recovered job ID. The separate observation has
scope `current_portable_export_files_only`, a revision and observation time.
Its `export_check` distinguishes matching, missing, mismatched and unavailable
files. Only matching canonical bytes return `configuration` with the MCP data.
The browser labels this action **Check exported files** and hides previously
displayed configuration after an unconfirmed check. Use `setup_state` to observe
a check whose response was interrupted.

The recovered job remains `unconfirmed` with `historical_outcome_unknown` even
when current files match. No configuration files are written, repaired or
deleted by inspection. SQLite may maintain WAL/sidecar files while reading the
existing recipe store. A missing database is not created. File comparisons are
bounded observations under host custody, not atomic snapshots against other
writers or guarantees that later client use sees unchanged bytes.

Older export jobs without the retained recovery record still need manual
inspection; these fields cannot be reconstructed into historical evidence.
Earlier frozen kits also retain their original CLI and setup code. Use the executable and setup code selected by the archive
that explicitly provides this recovery capability, or a matching source build.
