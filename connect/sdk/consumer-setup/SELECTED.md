# Install an agent-selected package through MCP

The optional selected-package tools expose the existing offline Python, Node and MCPB
installers through the consumer setup controller. The agent researches and
chooses an integration; the host fixes its installation scope. There is no
intent resolver or package-name matching service.

Use this path for a package outside the maintained library. It shares the
controller's jobs, exact replay and retained-request recovery with library setup.
It does not route a new package through an adopted family/profile parser.
Self-contained MCPB uses its existing installer through the same setup job flow.

For complete neutral JSON examples and the owner-computed package digest, see
[Prepare package and software metadata](METADATA.md). Read that guide before
authoring a selection; raw file hashes and typed content digests are different.

Public npm packages can be staged through [agent-selected Node acquisition](NODE-ACQUISITION.md),
which retains the exact lock and artifacts for this same installer.

## Choose the software document form

Plain local Python/Node service software can use `LocalSoftwareDeclaration`.
It retains the complete package/provenance and pinned artifact closure, one
service component/export and its source operation declarations. It does not
require host configuration/policy or lifecycle/enforcement references before
installation. Selected strategy requirements and unresolved details must be
empty; custom host requirements and selected-resource enforcement stay on the
full activation/custom path. Never delete requirements or broaden resource
access to make a selection fit. MCPB and frontend/custom inputs are unchanged.

For this form, submit `package_json` plus `declaration_json` inline. The host
derives `document_kind`; a caller-supplied discriminator on inline input is
refused. For staged metadata, use fixed `package.json` and `declaration.json`,
`package_sha256`, `selection_sha256`, and explicit
`document_kind: "local_software_declaration"`. Omitting the discriminator
selects legacy `activation.json`; there is no filename or content inference.
Both documents, mixed inline/hash fields and configuration fields are refused.
The planner uses `--declaration`; installation and readback use the same raw
`selection_sha256` plus `--document-kind local_software_declaration`.

New plans, receipts and readback carry exclusive `declaration_digest` and
`document_kind`; legacy `manifest_digest` and saved bytes remain unchanged.
Software approval, immutable request recovery, exact-job reconciliation and
read-only restart behavior remain the same owners. A declared raw operation map
can reach the existing protected public-service review after matched readback;
it does not prove actual schema observation, a credential, access or readiness.

## Configure the host

Keep `mcp_adapter.py` adjacent to `backend.py`. Run the adapter using a host Python
environment with MCP SDK 2.2.0 and its existing dependencies. The standalone agent
kit supplies the same offline verification environment used by the MCP checker.
The compatible Python/Node executables remain host-supplied.

Create a nonsecret JSON file with these required fields. Replace every placeholder
with a real canonical absolute path or SHA-256 digest:

```json
{
  "schema_version": 1,
  "scope": "standing_local_software_installation_only",
  "staging_root": "/absolute/canonical/staging",
  "installer": {
    "path": "/absolute/install-library-consumer.py",
    "sha256": "EXACT_INSTALLER_SHA256"
  },
  "validators": {
    "python": {
      "path": "/absolute/toolkit-python-plan",
      "sha256": "EXACT_PYTHON_VALIDATOR_SHA256"
    },
    "node": {
      "path": "/absolute/toolkit-node-plan",
      "sha256": "EXACT_NODE_VALIDATOR_SHA256"
    }
  }
}
```

For native CPython 3.11 on Linux x86_64, add the optional top-level
`"python_target": "linux-x86_64"`. This field requires a Python validator.
Omission or `"macos-arm64"` keeps the existing macOS arm64 behavior. Use
validator executables built for the selected host, and the same target in the
[acquisition profile](PYTHON-ACQUISITION.md) when acquisition is enabled.
The setup controller passes this host choice to both installation and readback.
It cannot be selected by an agent tool argument.

The nonempty `validators` map determines the permitted profiles. Omit a profile
to disable it. The installer and validators are copied into the private setup
workspace and their frozen bytes are checked before use. Runtime paths remain
host-owned; a path binding is not an executable-integrity attestation.

To let the agent compute package content digests without a shell, optionally
add a top-level `package_inspector` with the same `{"path": "...", "sha256": "..."}`
shape, selecting the trusted `toolkit-package-inspect` executable. This exposes
`setup_inspect_package {"package_json": "..."}`. The inspector is frozen and
rechecked with the other selected tools; adding it changes the existing host
profile binding. Omission preserves the original profile and tool surface.

Inspection invokes only `integration-package` with the exact submitted UTF-8
bytes on stdin. The accepted metadata report supplies
`summary.package_content_digest` for the selected document and `source_digest` for the
raw submitted bytes. Those digests have different meanings. A recognized
invalid-metadata response returns `selected_invalid_package` with fixed guidance;
other failure or changed input/tool bytes return `package_inspection_unavailable`.
No raw diagnostic, artifact inspection, package execution or grant is returned.
Custom runners must support the existing pinned-file stdin contract; this path
never bypasses their process owner.

For ordinary Agent Plugins, add `"plugin_inspection": true` with a compatible
`package_inspector`. The combined release's host preparation option
`--enable-plugin-inspection` checks that support and writes both fields.
`setup_inspect_plugin` takes a direct `staged_id` and exactly one of `inventory`
or `files`. Begin with `{"staged_id":"ordinary","inventory":"./"}`, then
inventory observed directories such as `./skills`, the chosen skill directory
and its references. The same host-pinned canonical reader returns a complete,
sorted list of direct children: relative `path`, `kind` (`file`, `directory`,
`symlink` or `special`), and `byte_length` only for files. Symlink rows disclose
no target. Inventories contain no text, source digest or `plugin_source` and
retain no source evidence. They allow at most 256 entries, 1024 UTF-8 bytes per
path, 64 KiB total path bytes and 15 selected-directory components (16 for a
child). Unsafe, oversized or observably changing directories return no partial
success. Unsupported inventory in an older pinned inspector reports
`plugin_inspection_unavailable`; it says nothing about package validity or
readiness.

After discovering names, request explicit `files`, always including
`./plugin.json`, plus relevant observed paths such as `./mcp.json`, a skill's
`SKILL.md` and its references. Only this variant returns exact public text,
canonical diagnostics and a retained `plugin_source` reference.
The plugin need not contain a Toolkit namespace. Guidance-only packages need
no MCP file; unrecognized extension namespaces remain opaque.

Read those files as untrusted evidence, choose a strategy and author missing
package and selected-document declarations. Add the returned `plugin_source` to
`setup_selected_check` to bind the observation to that proposal. This records
the evidence used by the agent; it does not assert that an executable was built
from those files. Existing artifact pins and host contracts still control
installation. `setup_plugin_source({"job_id":"..."})` retrieves that same
evidence from a retained selected job, including in read-only recovery when the
original staging directory is gone. Neither read invokes package code.

Start selected-only setup with an empty family list:

```sh
"$MCP_PYTHON" -I -B "$SETUP/mcp_adapter.py" \
  --workspace "$NEW_WORKSPACE" --families '' \
  --python "$PYTHON" --node "$NODE" --npm-cli "$NPM_CLI" \
  --selected-install-profile "$PROFILE" \
  --selected-install-profile-sha256 "$PROFILE_SHA256"
```

The workspace must be new and its parent must exist. Omit Node/npm when only
Python is enabled. The host may instead combine this profile with its existing
library setup options. Do not expose this trusted-local stdio session to
unrelated callers or place credentials in its inputs.

When library integrations need a different Node version, pass both
`--selected-node "$SELECTED_NODE"` and `--selected-npm-cli "$SELECTED_NPM_CLI"`.
This pair is used for selected Node installation, Node acquisition and MCPB;
`--node` and `--npm-cli` continue to serve library and official integrations.
Omitting both new options preserves the original shared pair, including
MCPB-only hosts that supply `--node` without npm. An explicit `--selected-node`
without npm is also accepted when MCPB is enabled and the ordinary Node
validator is absent; it never inherits the library's npm executable. Other
partial overrides are refused, as is an override without an enabled Node or
MCPB selected validator. The host
chooses compatible executables; these options do not change installer version
requirements or enable acquisition by themselves.

Selected proposals retain the effective pair in their existing host runtime
binding. Retry and read-only selected-install recovery require that same pair,
including for Python proposals in the shared selected profile. Acquisition-only
recovery reads retained files without resolving former Node/npm paths. No new
profile field or receipt format is introduced.

Enabling the profile supplies standing authority for local software installation
within this session. Manifest policy, lifecycle and configuration references
remain declarations; they do not grant permission. The host separately supplies
any references needed for the selected document; full activation still requires truthful host composition. The profile
does not authorize selected-server execution, credentials, account access,
frontends, native attachment or business calls.

## Select an explicit Node build

This optional development path is not part of Connect 0.9.0. It uses the existing check, software-effect review, install job and
readback owners. There is no new build agent or Rust manifest field. The exact
[version-1 plan and host profile](../../crates/toolkit-package/NODE-INSTALL.md#build-selected-source-with-an-explicit-plan)
define inputs, literal commands, outputs, limits and raw hash conventions.

The host adds an optional top-level field to `selected-install.json`:

```json
{
  "node_build": {
    "path": "/absolute/host/node-build-profile.json",
    "sha256": "EXACT_RAW_BUILD_PROFILE_SHA256"
  }
}
```

This is a fragment of the existing selected profile, not a replacement for its
required fields. It requires `installer` and the `node` validator, plus the
host-selected Node/npm pair. The profile bytes are snapshotted and rechecked;
referenced tool paths remain on the execution host. Preparation can supply this
field through the [optional flag pair](HOST-PREPARATION.md#add-selected-node-build-support).
Omitting it leaves the original no-build behavior. Agents cannot replace this
host profile through setup tool arguments.

1. Read `setup_state.node_build`. When `available` is true, `planning` contains
   executable/file IDs, hashes and useful names, exact target, limits and
   environment key names. It omits host paths and environment values. An
   unavailable toolchain requires a host correction; an in-progress operation
   may temporarily prevent this read. The projection does not execute a build.
2. Author the plan from those IDs and actual package instructions. Use
   `profile: "node"`. With inline `package_json` and `activation_json` or
   `declaration_json`, also send the exact `build_plan_json` string. With staged
   metadata/hash fields, put the plan at `closure/node-build-plan` and send its
   `build_plan_sha256`. Mixed inline/hash forms and Python/MCPB build plans refuse.
3. For ordinary staged or inline source metadata, bind that same digest as
   `node-build-plan` in the existing pinned closure. For live `acquired_source`,
   keep the existing minimal source form: the Controller reobserves the original
   acquisition and adds this separately authored plan digest to its materialized
   declaration. The original acquisition files and receipt remain unchanged.
4. Review the returned `build` projection with the existing package, entrypoint
   and destination. It includes literal commands, exact input/tool hashes,
   fixed nonsecret environment, target, output mappings and limits. These are
   retained in the same effect identified by `validation_id` and `effect_sha256`.
   Only the separate `setup_selected_install` call admits that checked effect.

Approved commands may execute selected package build scripts as trusted local
code. npm lifecycle scripts remain disabled; the installer does not implicitly
choose or enable a script. An explicit build is not MCP server launch, native
attachment, authentication or operation permission. Qualification includes one
actual local source-addon build and fresh Node consumer on Linux, alongside the
focused setup controls. General npm build support remains unqualified.

### Node build receipt binding

The original successful installer reply is captured under the host's selected
execution deadline before any mutable `ready/installed.json` read. Output and
its canonical parsed form must each fit the existing 2 MiB capture bound;
overflow, a missing or malformed reply, mismatch or failed process leaves the
job unconfirmed, retaining its files. A truncated reply cannot become success.

A successful live result has `build_receipt_binding: "original_installer_reply"`
and `installer_reply` with these observations:

- `raw_sha256` and `bytes`: the actual installer-returned stdout.
- `semantic_sha256`: SHA-256 of the parsed receipt serialized with the compact,
  sorted-key, ASCII JSON plus newline convention shown in the Node contract.
- `receipt_sha256`: the actual raw ready-file hash, recorded only after matching
  semantics and successful verification. The result also retains that file pin.

The installer's indented receipt file and stdout need not have identical bytes.
The first readback compares semantics against the original reply, then retains
the raw file pin. If that first readback fails, a later successful live
`setup_reconcile` stores the verified raw pin back into the same owned intent.
Subsequent live observations check both bindings. No observation rewrites the
historical unconfirmed job as a success.

A live attempt that never observed a valid original reply cannot later claim a
matched build through same-session reconciliation. Replaying its install call
returns the same job. The existing explicit retry proposes a fresh destination
and review; it does not install or erase the old evidence. Explicit cold recovery
can report `build_receipt_binding: "current_receipt_consistency_only"` after the
existing inert checks, because the recovered request does not preserve the live
stdout observation. That is current file consistency, not historical provenance.


## Enable self-contained MCPB bundles

Before authoring the four installation inputs, the optional
`setup_inspect_mcpb` tool can read an unfamiliar staged `artifact.mcpb`. Enable
`mcpb_inspection: true` alongside the MCPB validator and pinned
`package_inspector` in the host profile. The generated host helper exposes this
through `--allow-mcpb --enable-mcpb-inspection` in a supporting release.

Call with `staged_id`, exact raw `artifact_sha256` and `files: []` to inspect
the manifest, or include explicit `./` public member names. The canonical
reader returns exact source text, configuration prerequisites, declared
entrypoint and arguments, plus a bounded inventory. It checks the entire ZIP
without extracting or executing it. Paths retain the installer's 240-byte
portable-path limit even though the transport permits longer strings.

The agent supplies the semantic choices. An accepted inspection is not
installation permission or evidence that sensitive, authenticated or other
unsupported bundle contracts can run. Bind the subsequent activation to that
same artifact hash. Existing selected installation and recovery retain the
frozen inputs; this reader creates no second receipt or lifecycle registry and
is unavailable in recovery sessions.

Add an `mcpb` validator entry pointing to a trusted `toolkit-mcpb-install`
executable with the `check`, `install` and `verify` commands. The same pinned
executable owns all three operations. A host enabling only MCPB may omit the
Python/Node `installer` field:

```json
{
  "schema_version": 1,
  "scope": "standing_local_software_installation_only",
  "staging_root": "/absolute/canonical/staging",
  "validators": {
    "mcpb": {
      "path": "/absolute/trusted/toolkit-mcpb-install",
      "sha256": "EXACT_MCPB_INSTALLER_SHA256"
    }
  }
}
```

Use an optimized release build of `toolkit-mcpb-install`. The retained real-bundle
trial exceeded the inspection deadline with a development build; raising the
budget or skipping verification is not the supported workaround.

Pass `--node` with the actual trusted Node executable. MCPB needs no npm CLI
or dependency download in the original shared-runtime mode. If a separate
selected pair is supplied, MCPB uses `--selected-node`; its paired npm CLI is
not invoked by MCPB. The host Python is still required to run setup. Earlier
MCPB executables without `check` are incompatible with this setup route.

Stage `package.json`, a current `activation.json`, explicit nonsecret
`configuration.json`, and `artifact.mcpb` in one immediate staging child. Call
`setup_selected_check` with `profile: "mcpb"` and the usual fields below, plus
`configuration_sha256` for the raw configuration bytes. The installer inspects
the actual archive in a new private scratch directory. It checks the manifest,
configuration, selected script and literal arguments without executing Node
or the server. This may leave scratch files on failure; it is not installation
or permission to execute. The supported archive/runtime/configuration profile
is defined by the [MCPB owner](../../crates/toolkit-package/MCPB-INSTALL.md).
MCPB check and readback have a 120-second process budget because they verify
the archive and its materialized tree. Other captured metadata checks retain
their 30-second budget. The calling MCP client must allow the longer archive
check to return; a client timeout alone does not establish the host outcome.
Expiry signals the owned inspection process group and waits for its direct
child. Scratch evidence is retained; expiry does not authorize a retry or repair.

Review the result and use the same `setup_selected_install` call. The owner
revalidates the four frozen inputs, probes `node --version` and publishes its
ordinary MCP configuration. It never launches the selected server. The setup
browser displays this job and its configuration digest through the same
current/history view; it does not supply an editable MCPB configuration form.

`setup_reconcile` invokes the same owner's `verify`. For MCPB this includes
checking the retained Node executable hash and version, as well as archive,
materialized files and launch configuration. It does not prove MCP protocol
compatibility, tool filtering, authentication or native attachment. The endpoint
still exposes every tool the selected server supplies.

## Let the agent prepare and install

1. Read `setup_state` for the enabled profiles and staging contract. Research with
   the host's ordinary permitted tools. Use the existing selected
   [Python](../../crates/toolkit-package/PYTHON-INSTALL.md) or
   [Node](../../crates/toolkit-package/NODE-INSTALL.md) contract to prepare exact
   `artifact.whl` or `artifact.tgz` and the locked `closure/` in one immediate
   child of the staging root. For the legacy full activation form, author `package.json` and `activation.json` as
   nonsecret metadata. They may remain in memory for inline submission or be
   written beside the artifacts for the original file-based path. For MCPB, use
   the four-file layout [above](#enable-self-contained-mcpb-bundles), including
   `configuration.json` and `artifact.mcpb`, without a dependency closure.
2. Call `setup_selected_check` with `profile` (`python`, `node` or `mcpb`), `staged_id`
   (that child name), raw `package_sha256`, raw `selection_sha256`, and
   `component`. MCPB additionally requires the raw `configuration_sha256`. The
   tool creates a private copy and invokes the pinned canonical validator.
   Alternatively, replace the two hash fields with `package_json` and
   `activation_json` strings containing the complete authored documents. For
   MCPB also replace `configuration_sha256` with `configuration_json`. Never mix
   inline documents and hash fields in one request. The host computes the raw
   hashes and writes exact inline bytes directly to the private proposal. It
   neither reads nor overwrites staged metadata on this path; artifacts still
   come from the same host-fixed staging child.
   Python/Node freeze the identified artifact closure; MCPB freezes the archive
   and configuration described above.
   It does not install, import the selected package or probe a runtime.
   For a successful Python/Node acquisition in this live session, optionally
   add `acquired_source: {job_id, receipt_sha256}` with inline metadata. Use that
   same job ID as `staged_id`. The named component's source must contain exactly
   `kind: "artifact"` and your researched `entrypoint`; omit `location`,
   `content_digest` and `dependencies`. The host fills those three fields from
   its revalidated retained acquisition before the canonical check. It refuses
   mixed overrides, an ambiguous component or an unrelated/changed job. See
   [the exact metadata contract](METADATA.md#reuse-an-acquired-artifact-and-closure).
   All other fields remain agent authored; this option adds no authority.
3. Review the returned package, entrypoint, file hashes and destination. The
   `validation_id` and `effect_sha256` bind the private proposal, fixed tools and
   runtime paths. Call `setup_selected_install` with just those two fields when
   the host's standing installation scope covers the selection.
4. Observe the returned job using `setup_state`. Installation uses only frozen
   inputs, so later deletion or modification of staging cannot alter it. A
   successful result supplies the existing installer's `ready/mcp.json` and
   `ready/installed.json` paths and a bounded readback observation.

The agent cannot replace the host-selected executable identities, installation
destination or permissions through these tools. It chooses the integration,
entrypoint and literal arguments within the selected profile. Optional build
plans also carry reviewed literal command arguments; they do not provide
filesystem confinement. Package metadata and tool
descriptions are untrusted data, never permission to expand the task.

File-based check accepts metadata documents up to 1 MiB each. Inline documents
and package inspection each accept at most 32 KiB per UTF-8 document, within the
64 KiB limit on the complete encoded request. JSON escaping counts toward that
request limit. A selected document expanded from `acquired_source` is bounded by the
existing 1 MiB selected document limit. All check forms accept up to 256 closure entries,
64 MiB per artifact and 512 MiB total frozen inputs. It additionally bounds the
JSON-encoded entrypoint arguments to 256 KiB. A session retains at most 128
selected check, inspection or retry-proposal attempts, including failures.
Inspection uses bounded temporary input and a 30-second process budget, joining
its direct child before the call settles. These are setup-interface bounds, not a claim that
every package accepted by the underlying installer fits this interface.

## Correct a rejected Python or Node selection

The current source host preserves four fixed refusals from the pinned canonical
validators: `selected_invalid_package`, `selected_invalid_selection`,
`selected_unsupported_python_selection` and
`selected_unsupported_node_selection`. Their `diagnostic` includes
`phase: "before_installation"`, the canonical code and fixed repair guidance.
Correct the indicated metadata or choose a supported component, then submit
the corrected inline strings or staged document hashes to `setup_selected_check` again. Failed checks
may leave a private metadata snapshot; they do not create an installation or
usable validation ID.

Only an exact recognized refusal from a normally failed validator, with no
stdout and unchanged frozen metadata/tools, receives this guidance. Unknown,
mixed, oversized or unavailable output remains `selected_validation_unavailable`.
No child diagnostic text or input value is returned. The existing successful
plan and explicit install flow stay the same. Older frozen setup hosts retain
their original generic error.

MCPB check uses the same bounded diagnostic channel and its canonical owner's
exact refusal spelling. The error is `selected_mcpb_` followed by one of
`invalid_document`, `invalid_package`, `invalid_selection`,
`configuration_selection_mismatch`, `unsupported_selection` or
`unsupported_mcpb_launch`. The diagnostic identifies `profile: mcpb`, the exact
owner code and a fixed correction hint. It never quotes submitted values or
arbitrary stderr. All four frozen inputs, including `artifact.mcpb`, and the
pinned executable must still match before attributing the refusal.

A refused MCPB check may already have materialized files in its private check
directory. The hint therefore promises no installation, not no local effects.
Correct the declared input and call `setup_selected_check` again; it creates a
new proposal in a fresh check directory. Do not retry an installation into the
old directory. The MCPB inspection and readback budget remains 120 seconds;
this change supplies no new process, retry or authentication authority.

## Replay, readback and recovery

Repeating the same successful check verifies the current artifact bytes and the
submitted metadata before
returning its existing proposal. Changed or incomplete staging does not silently
replace that proposal. Inline and file-based checks with identical document
bytes share the same proposal; whitespace changes produce different raw hashes.
The existing effect and retained-request formats remain hash-based. Recovery
uses the frozen metadata, not the original inline strings or mutable staging.
Repeating the exact install call returns the original job,
including an unconfirmed outcome. It does not install again. Caller cancellation
does not undo an admitted install; graceful host closure joins admitted work.
An explicit [stop request](MCP.md#stop-one-local-setup-job) can interrupt the local
worker without removing its files or restoring installation authority. Observe
settlement before requesting readback; an interrupted effect remains unconfirmed.

### Explicitly review another installation attempt

After a selected installation settles as `unconfirmed`, call
`setup_selected_retry {"job_id": "FAILED_JOB_ID"}` in the same live session.
The browser offers **Review another installation attempt** for the same action.
This creates a proposal, not an installation. The Controller verifies the
original frozen inputs and current host bindings, then copies those inputs to
a fresh proposal with a new destination, validation ID and effect digest.
Mutable staging is not consulted. Prior files, job results and observations
remain intact, including any files left by an interrupted or completed but
unconfirmed first attempt.

Review the returned package, entrypoint, literal arguments, file hashes and new
destination. Then explicitly call `setup_selected_install` with the new
`validation_id` and `effect_sha256`, or choose **Install reviewed attempt** in
the browser. This uses the existing installer and installation authority.

Repeating `setup_selected_retry` for the same predecessor returns that same
proposal. Its `retry_of` identifies the predecessor; `job_id` is null before
installation and identifies its one job once admitted. A lost reply can be
retrieved without creating another attempt. A concurrent first request may
receive `operation_in_progress` until the proposal is published. To retry a
failed successor, explicitly select that successor's job ID. Repeating the
original install still returns its original job; repeating the original check
still requires valid original staging before returning its original proposal.

Running, successful, foreign and recovered jobs cannot create a retry proposal.
The worker must release its operation lock first. Changed frozen inputs or
host tools prevent a fresh proposal. A retry's optional durable `retry_of`
field survives read-only recovery as lineage only; it neither follows an old
job nor restores installation authority. Older retained requests remain valid.
Existing distribution archives predate this source addition.

### Check retained installation files

`setup_reconcile {"job_id": "ORIGINAL_JOB_ID"}` uses the original private
proposal and the trusted installer. Python/Node use `--verify-existing`, which
executes the pinned metadata validator on temporary copies. MCPB uses the
distinct `verify` behavior described above. Neither path
executes the selected server, install, repair or rewrite historical job
status. A matching observation establishes retained payload and launch binding
only. Python/Node readback does not attest runtime/bootstrapping integrity. No profile
authenticates an account or establishes a usable native binding.

The controller reads installation receipts up to 64 MiB; captured validator and
verifier output is bounded to 2 MiB. If installed proof output exceeds that bound,
the outcome remains unconfirmed and the files are preserved. A check cannot
predict the final installed proof size. There is no automatic reinstall or
truncation that could turn incomplete proof into success.

Before dispatching installation, the controller synchronizes the private
proposal and an immutable selected-install request. After a forced restart,
the host can explicitly supply the original workspace, request ID and independently
retained request digest using `--recover-workspace`, `--recover-request` and
`--recover-request-sha256`. Supply the same selected profile and runtime choices
in a fresh workspace. Staging need not still exist. The original workspace and
proposal must remain at their original paths under exclusive quiescent custody.
Only `setup_state` and `setup_reconcile` are available; historical state does not
restore installation authority. Relocating an existing installation is unsupported.

The browser host (`backend.py`) accepts these same recovery arguments and
selected profile. Its private launcher opens the recovered request with setup
controls disabled. **Check installation** displays the existing verifier's
current file/launch observation, selection type, component and exact bindings;
it does not change the historical job outcome or retry installation. The stdio
and browser command-line entrypoints each create their own controller. Stop
the prior owner and ensure quiescence before recovery; starting the browser
entrypoint is not a simultaneous viewer for an active stdio session.

## Share the active session with a browser

Add `--browser-port 0` to the stdio adapter command to open an optional loopback
companion on an available port. Keep the adjacent `static/` directory from the
distribution. The private `start.html` path appears on stderr; open that file and
choose **Open local setup**. The launch credential stays in that local file.
The bare loopback URL does not grant access. Keep this single-operator session
and launcher private.

This companion borrows the stdio adapter's Controller. It displays agent-started
installations and shares **Check installation** results with `setup_state`.
Closing the browser tab does not cancel work. Closing the adapter stops browser
admission and joins admitted setup work. Do not start a second `backend.py`
process to view an active stdio session: that creates a different Controller.

The library form accepts nonsecret setup values and credential reference names only.
It does not provide OAuth sign-in, collect tokens or attach native tools.

The complete native `find_and_install` path, protected authentication, attachment
and continuation remain unfinished product work. These local tools implement the
software-installation step of that path.

## Acquire Python inputs through setup

The optional [Python acquisition tool](PYTHON-ACQUISITION.md) performs public
PyPI wheel resolution and staging for an agent-selected exact distribution. It
uses the same host staging root and job owner. The agent then authors the usual
package and selected document and continues through this check/install path.
