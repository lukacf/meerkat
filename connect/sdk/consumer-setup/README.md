# Local integration setup workbench

Browse the pinned integration catalog, choose an installed library profile, fill in its nonsecret configuration,
validate it and install a new MCP server configuration. The workbench uses the
existing consumer installer and the component's installed `profiles`,
`config-help` and `check-config` commands. No frontend build or integration glue
is required.

For ordinary local adoption, use the [newcomer quickstart](../../docs/site/quickstart.mdx)
and the exported `start.py` helper. It selects the current platform archive,
verifies it and reuses the existing bootstrap, with one new destination instead
of manually assembled runtime hashes and profile files. Advanced hosts retain
the direct preparation and bootstrap interfaces.

## Maintaining the ordinary starter

`current-distribution.json` is the current-selection authority. It points to a
frozen publication manifest and binds its exact hash; the manifest owns platform
filenames, sizes and archive digests. The starter never uses a manifest supplied
by a downloads directory and never chooses an archive by modification time.

After reviewing an accepted successor's platform support and operation evidence,
update that one pointer and export a new starter:

```sh
python3 scripts/export-setup-starter.py --destination /absolute/new-starter
```

Run this command from the repository root. The export contains the scripts and
an exact projection of the selected manifest. A previously delivered starter
retains its old selection. Do not silently retarget it, mutate a frozen manifest
or interpret installing a new environment as permission to replay provider writes.
Retiring an unsafe version means withdrawing its distribution and notifying its
host owners; no starter background process revokes or updates installations.

Before offering a successor, run the focused starter/export checks and validate
the generated MCP connection on each platform claimed. Keep installed/synthetic
operation evidence distinct from live account evidence. Native releases,
credential custody and attachment remain with their existing owners.

## Using the workbench

To keep selected work for later inspection, use **Add to recovery file** on each
job and **Save recovery file**. An agent uses `setup_recovery_manifest` with the
same explicit job IDs. Both return the existing recovery manifest without
hand-assembling hashes or configuration. Preserve the original workspace and
follow [multi-request recovery](MULTI-RECOVERY.md); saving this file does not
stop work, authenticate an account or resume a task.

The updated [host preparation helper](HOST-PREPARATION.md#reuse-a-prior-configuration)
can also use this file and one explicitly selected configured request to seed
a fresh setup. The host approves the saved settings for the new setup audience;
then **Reuse prior configuration** reviews them against current help and asks
for current operation choices. This avoids locating an internal request file
by hand. Inspection-only recovery remains a separate session. Older frozen
archives require their existing raw prior-request inputs.

With services 0.1.11 or later, a rejected Slack or Gmail configuration can point
to the setting that needs correction. For example, channel restrictions and
`list_conversations_page` cannot be selected together. The browser explains the
conflict and focuses the operations while keeping every selection intact.
Correct it explicitly and validate again before installing. Older validators
and unrecognized failures keep the generic error. This checks configuration,
not credentials or account access.

The optional [container setup capability](COMPOSE.md) lets an agent
prepare and run supporting containers under a separate explicit host execution
scope. Consult the selected kit's inventory for its included capabilities.

For an agent-operated workflow, the optional [stdio MCP adapter](MCP.md) exposes
these same installed help, validation and installation operations as tools.
The host still selects trusted software inputs, runtimes and the output directory.
The optional [local recipe bridge](RECIPES.md) recalls previous investigations.
An explicitly enabled retention scope also lets the agent save a settled
selected-package job's historical documents with its real decision and
verification references. Retention is not an installation-success or account
readiness claim, and the original disclosure-only scope cannot save.
For researched integrations outside the library, the optional [selected-package workflow](SELECTED.md) accepts pinned Python, Node and self-contained MCPB inputs. MCPB check, installation and readback use the same owner CLI and appear in this session view.
With acquired inspection enabled, `setup_inspect_acquired` lists and reads
bounded text from the exact retained Python/Node primary archive. Its optional
`archive_sha256` selects a dependency from the same job's `artifacts` array.
This [archive research](ACQUIRED-INSPECTION.md) executes no package code and
grants no additional installation or runtime authority.
Add `--browser-port 0` to that adapter to serve this same view over its existing
controller. See [the shared-session workflow](MCP.md#show-the-same-session-in-a-browser).
The browser observes agent-started jobs automatically without replacing an
unfinished local form. The session job picker also lets you revisit earlier
preparations and installations, including their own result and installation
check. Viewing a previous job leaves current work running; the current job's
status stays visible. The list loads at most 50 summaries per page.
Setup agents can read the same history through `setup_jobs` and `setup_job`.
This lets a multi-integration request retain each installation's result and
current check observation after another job becomes latest, without repeating
the install or check. These source tools are available in normal live sessions
and in explicitly selected [multi-request recovery](MULTI-RECOVERY.md).

In development source, `setup_job` can observe a running job with optional
`wait_ms` from 0 to 10000, for example
`setup_job {"job_id":"0123456789abcdef0123456789abcdef","wait_ms":10000}`
using the returned job ID. Omitted or `0` keeps the immediate response. The wait
returns the current snapshot after a terminal observation or interval expiry;
existing caller and read deadlines still apply. Cancelling this observation
leaves installation running. Use `setup_stop` explicitly to request a stop; a
terminal observation proves neither Toolkit readiness nor worker cleanup.

This history belongs to the running setup host. A browser reload can reconnect
to those retained jobs, but restarting the host does not automatically restore a
session. The host can pin one request or a manifest of requests from one original
workspace for readback. Each imported job retains an unknown historical outcome.
Browsing does not reinstall software or establish account access. Older frozen
distributions retain their original history and recovery behavior.

The source workbench also offers **Stop local setup** for a running job. It
targets the job currently displayed, including a selected-package installation.
Stop acceptance is shown separately from settlement: current changes may have
already occurred, files are retained, and the interrupted outcome remains
unconfirmed. **Check installation** can inspect those files after the worker
settles. This does not cancel the original task or disconnect an account.
Closing the page still leaves work running. See the
[stop contract](MCP.md#stop-one-local-setup-job) for process and recovery limits.

The companion also displays agent-started Python, Node and Compose acquisition
jobs, including their status, exact-job Stop and earlier results in history.
Successful package acquisition shows retained inputs for a later installation;
successful image acquisition shows a proposal for separate host review. Neither
means software is installed or a tool is ready. These jobs do not offer
**Check installation**. Detailed download percentages are not reported by the
host, and acquired files or image-cache changes remain after a stopped job.

After preparation fails or stops, **Try preparation again** starts a fresh
attempt for that library in the same session. It preserves the earlier job and
files, and publishes the new profiles only after all preparation checks pass.
Agents use `setup_retry_prepare` with the exact failed preparation job ID.
See [preparation retry](RETRY-PREPARATION.md) for replay and recovery limits.
This does not retry a configured installation or perform a provider operation.

An operator selects a trusted consumer bundle, its expected `CONTENTS.json`
digest, compatible runtime executables and a fresh controlled output directory.
The browser then supplies only the explicit profile, operations and configuration.
It cannot select executable paths or installation destinations.

## Browse before preparing software

The source workbench displays all entries in the selected bundle's `catalog.json`
before preparing a library. Filter names and descriptions literally, then select
an exact publisher/name/version to read the complete record. Every strategy,
including alternative Slack and Gmail routes, remains visible with its original
provenance, requirements, unresolved details and assessments. These are catalog
claims, not current availability or account-readiness checks. Catalog text is
displayed as data; it cannot authorize an operation or change host policy.

Catalog browsing reads only the copied, hash-pinned metadata. It does not call an
installer, import integration code or run a profile inspector. Both list and
detail responses bind the catalog SHA-256 and the host-selected `CONTENTS.json`
SHA-256. A missing catalog is explicitly unavailable, rather than an empty
catalog. Recovery sessions expose only selected requests and their readback,
including history when the host selects a recovery manifest.

Use a bound option's explicit setup button, or select an enabled family manually after reading the strategies. Catalog
identity is not an installed profile ID, and no name matching or strategy-text
parser maps one to the other. Preparation supplies the actual installed profile
choices. These setup controls support the host-enabled Agno, Activepieces and
Slack/Gmail service families. Official GitHub can be enabled with the separate
explicit pins below; the optional Playwright route uses an additional host profile. Outside-catalog choices use the optional selected-package workflow described below. This source addition is not included in earlier
published local archives until a successor archive explicitly selects it.

Agents with the optional [hosted recipe tools](HOSTED-RECIPES.md) can also retain
newly discovered HTTPS MCP metadata and export standard MCP configuration.
The browser displays those jobs as configuration exports with their actual
files; it does not label them as installations or connected accounts.

## Start the local host

Use a current accepted consumer bundle and the runtimes required by its
`README.md`. From a Toolkit checkout, run:

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  /absolute/path/python3.11 -I -B sdk/consumer-setup/backend.py \
  --bundle /absolute/path/toolkit-consumers \
  --contents-sha256 EXPECTED_DELIVERY_DIGEST \
  --workspace /absolute/new/setup-workspace \
  --families agno,services,activepieces \
  --python /absolute/path/python3.11 \
  --node /absolute/path/node \
  --npm-cli /absolute/path/npm/bin/npm-cli.js \
  --port 0
```

The parent of the new workspace must exist. The server prints its loopback
origin and the path to a private launcher file. Open that file in your browser
to enter this local session. Keep the launcher private; the bare origin does
not confer access. There is no automatic browser opener.

`--families` enables only the listed component families. Omit the Node and npm
arguments when Activepieces is not enabled. Preparing a family explicitly
installs an inspection environment from the pinned offline bundle. Its installed
help commands supply the form. Each later configured installation uses a new
directory and the same selected software inputs. Preparation does not retrieve
provider credentials or call a provider.

To enable **Check installation**, add both host-only options to either the
browser host or the stdio MCP adapter command:

```sh
--verifier /absolute/path/install-library-consumer.py \
--verifier-sha256 EXPECTED_VERIFIER_SHA256
```

Select a trusted installer edition that supports library `--verify-existing`,
such as the accepted library-reconciliation add-on. The host freezes that exact
script separately from the selected bundle's installer. It never substitutes
the script for installation or discovers another executable. Without both
options reconciliation is disabled. An older or incompatible verifier can
only yield an unconfirmed check; it cannot establish a match.

The workbench itself needs only Python's standard library and its adjacent
`static/` files. It has no npm build step. Runtime compatibility and dependency
realization stay with the unchanged consumer installer. Changing platform means
selecting the appropriate accepted bundle and runtimes; it does not mean using
the other platform's wheel files.

## Use the form

1. Choose an exact profile from an enabled library family. Agno and Activepieces
   provide 99 adopted service profiles; the services family offers alternative
   Slack and Gmail operations. These alternatives do not increase the library's
   integration count. When explicitly enabled, `official-github` provides the fixed
   GitHub route from a separately pinned distribution. The separately enabled `official-playwright` family uses host-selected Chrome.
2. Read its prerequisites and unresolved account or registration requirements.
   Select the operations you need, paying attention to write effects.
3. Fill in the required fields. Credential reference fields take environment
   variable names such as `MY_SLACK_TOKEN`. The consuming MCP host supplies the
   actual value later. Do not enter tokens, passwords or OAuth codes here.
4. Validate the configuration. The selected route's validator decides whether it
   is valid. Field guidance is not a complete schema and may leave semantic
   constraints to that check. Validation uses the same 30-second inspection
   deadline and bounded output capture as profile help. A stuck validator is
   retired before validation or installation can proceed.
5. Review the exact selection and start installation. The existing installer
   validates and copies the configuration, then publishes `ready/mcp.json` with
   its software installation receipt. Library families also validate through
   the newly installed component. Official GitHub never executes its binary; official Playwright installs software without launching its endpoint or browser.
6. Load that file through your MCP host's ordinary configuration interface.
   Supply the host-owned credential environment and authority for that process.

An installed configuration is not an authenticated connection, an agent tool
binding or a completed business task. The workbench does not start the resulting
MCP server, choose an integration from natural language or replace native
`find_and_install`.

## Ownership and recovery

For Python, Node and Compose acquisitions, follow [acquisition request
recovery](ACQUISITION-RECOVERY.md). It checks retained inputs in a new readback-only
session without repeating downloads or image pulls. The installation recovery
contract below remains separate.

To inspect several retained installations, exports or acquisitions together,
use [multi-request recovery](MULTI-RECOVERY.md). A host-pinned manifest selects
exact requests from one original workspace; it does not resume a setup session.

This is a local single-operator surface, bound to loopback. Keep the browser and
server on the same machine and keep the bundle, runtime executables and output
directory under the operator's control. Its temporary browser session does not
implement remote users, application grants or protected authentication. Do not
expose it through a reverse proxy or a shared listener.

Navigation and validation do not install a configured server. Starting an
installation is an explicit effect. Closing the tab does not cancel it. The
installer's files remain the source for determining what was published if an
HTTP response or the workbench process is interrupted. Preserve incomplete
destinations for inspection; do not load them or retry into them. Software
restore and switching a live host binding remain separate operations.

While this host process is still running, **Check installation** can reconcile
a settled configured-install job, including one whose installation response was
lost. It runs the explicitly selected verifier with the original host-retained
bundle, runtime, configuration hash, server name and destination. It does not
rerun installation or the installed configuration validator. A separate current
observation reports whether retained package files and launch settings match;
the original job outcome is unchanged. A match exposes the actual `mcp.json`
and observed receipt hash, with runtime, bootstrap and generated-file integrity
excluded for library and Python/Node profiles. MCPB readback also checks the Node executable hash and version. This is not an authentication, native-readiness or safe-execution check.

Library, official and Python/Node verification uses the existing 30-second, bounded-output inspection path. MCPB archive check and verification use 120 seconds, as described in the [selected-package contract](SELECTED.md).
Timeout, malformed output, changed inputs or a verifier refusal produce an
unconfirmed observation without child diagnostics. A disconnected browser does
not cancel this check; refresh status to observe its result. The host joins
entered work on graceful shutdown. Keep files under exclusive host custody
during the check; its before/after reads are not an atomic filesystem snapshot.
Jobs, validation tokens and current observations remain in memory. Before a
configured install worker starts, the host now retains one immutable nonsecret
request at `requests/<job_id>.json`. The job's `recovery_request` contains its
`id` and SHA-256. Configuration and request files and their containing directories
are synchronized before dispatch, as are the new workspace and its parent
directory entries. A failed write or synchronization starts no
worker. A retained request proves neither that installation began nor that it
finished. Ordinary configuration fields must still be nonsecret; this is not a
secret scanner.

### Recover one request after a host restart

This legacy single-request mode remains unchanged. Use the
[manifest mode](MULTI-RECOVERY.md) for more than one retained request.

First establish exclusive, quiescent host custody of the original workspace.
A dead controller, absent PID, request file or receipt does not prove that the
installer or its descendants have stopped. This feature does not detect or
terminate them. Do not check while any previous writer may still be active.

Start a fresh host session with the original bundle digest and the same selected
runtime executable paths. Add these host-only arguments to either `backend.py`
or `mcp_adapter.py`, along with the existing explicit verifier path and digest:

```text
--workspace /absolute/new/session
--recover-workspace /canonical/original/workspace
--recover-request ORIGINAL_JOB_ID
--recover-request-sha256 HOST_PINNED_REQUEST_SHA256
--verifier /absolute/trusted/install-library-consumer.py
--verifier-sha256 HOST_PINNED_VERIFIER_SHA256
```

The three recovery arguments are all required together. The original workspace
must use its canonical path without symlink aliases. The host chooses the
request and pins its exact bytes; the current verifier is independently selected
and pinned by the host. No verifier need have been selected before the original
installation. Runtime and installer choices cannot be taken from an untrusted
request or receipt. No executable is discovered or substituted automatically.

The host may retain the request hash from the original job response or status.
If that response was lost, the operator can explicitly select the request from
its protected original workspace and supply its hash. That trust comes from
host custody and selection, not from a hash stored beside an untrusted file.
There is no automatic scan, trust decision or import from `installed.json`.

Recovery imports only the request into a new `mode: recovery` session. Its job
is `unconfirmed` with `historical_outcome_unknown`; no old validation token,
prepared inspection environment, success result or observation is restored.
The UI disables setup controls, and MCP advertises only `setup_state` and
`setup_reconcile`. Direct setup requests refuse; hidden MCP tools are unknown.
Import performs no check, installer or installed parser call. Only an explicit
**Check installation** / `setup_reconcile` runs the existing bounded verifier
and yields a separate current observation. It never changes the historical
outcome, retries, repairs or launches a connector. Original files stay untouched.
The Activepieces configuration binds its profile and values; the retained single
operation string is original request data, not a fresh operation-schema check.

The current source host also recovers bounded string-list configuration values,
including Slack channel and Gmail recipient limits. Omitted, empty and explicit
lists retain their original bytes and meaning. Recovery validates their generic
shape using the same list bounds as setup; it does not rerun provider validation
or derive a new grant. Older frozen hosts with scalar-only recovery must adopt
the corrected host code before reopening those requests. Existing installations
and request files need no migration or rewrite.

This is request recovery for local file reconciliation, not a durable acquisition
workflow, runtime grant, authentication ceremony or original-task continuation.
The [local macOS request-recovery edition](../../dist/toolkit-library-20261003-setup-request-recovery/README.md) includes the original scalar request-recovery path, not the current string-list correction. Earlier archives retain their original implementation.

The workbench does not read credential values or inherited user package-manager
settings. It is not a secret scanner: ordinary text fields still require the
operator to supply nonsecret configuration. Provider sign-in, refresh, account
verification and tool authorization stay with the existing host owners.

## Validated credential reference handoff

The setup host carries the installed validator's optional
`credential_environment_references` through configuration review and a successful
install result. Each result binds the rows to `family`, `profile`,
`configuration_sha256` and the selected bundle's `contents_sha256`. Rows contain
only literal `field_path` keys and an `environment_variable` name, following the
[existing launch contract](../../docs/adoption/LAUNCH-CONSUMER-CONTRACT.md#selected-credential-environment-references).
The host checks every row against the exact configuration and the installed
help's declared credential fields, including completeness. It preserves two
fields referring to the same name and omits an absent optional field.

An empty array means the supporting validator reported no selected references.
A missing array means the older validator does not report them; requirements
remain unknown. Malformed output is refused without exposing the child's raw
output. Pre-install validation must retain the same projection and configuration
before installation can begin. These are names, including any selected reserved
host names, not permission to read or inject environment values.

The browser shows this handoff in the review and final install result. MCP
returns the same fields. File reconciliation and recovery do not run the
configuration validator, restore a reference list or present an earlier list as
current. They continue to report only their separate file-check observation.
The [local credential-handoff edition](../../dist/toolkit-library-20261003-setup-credential-handoff/README.md) includes this behavior. Earlier archives retain their original implementation.

## Verification scope

The [component acceptance](../../docs/verification/20261003-consumer-setup/REVIEW.md)
records 23 backend tests, 12 frontend tests, all 101 installed help paths and
three real configured installations on the accepted macOS consumer profile.
Ordinary MCP clients consume all three emitted launch configurations; the
SearXNG client also performs a real operation against a local HTTPS fixture.
The protected browser flow remains unverified pending a human opening the
private launcher. Linux workbench execution, provider sign-in and native
agent-led acquisition are separate unfinished acceptance work.

The [validation deadline correction](../../docs/verification/20261003-setup-validation-deadline/REPORT.md)
adds actual hung-validator and retained-stdout subprocess regressions. The
predecessor fails those bounded probes; the successor passes 11 focused methods
and independent review. Installer lifetime and the earlier acceptance scope
are unchanged. Previously frozen release archives retain their original code.

The reconciliation action described above is a source successor. Previously
frozen workbench and agent-setup archives do not gain it automatically.

## Optional official GitHub route (source candidate)

The host can explicitly enable `official-github` without enabling any Python/npm
library family. It requires a separately selected official bundle for macOS arm64
or Linux x86_64 and an installer version supporting that target, each independently pinned.
These source changes do not update any previously delivered archive.

```sh
/absolute/path/python3.11 -I -B /absolute/path/backend.py \
  --workspace /absolute/new/setup-workspace \
  --families official-github --python /absolute/path/python3.11 \
  --official-bundle /absolute/path/official \
  --official-contents-sha256 EXPECTED_OFFICIAL_CONTENTS_SHA256 \
  --official-installer /absolute/path/install-library-consumer.py \
  --official-installer-sha256 EXPECTED_SUPPORTING_INSTALLER_SHA256
```

The same flags apply to `mcp_adapter.py`. To also offer library families, retain
`--bundle`, `--contents-sha256` and their existing runtime arguments, and list the
chosen families alongside `official-github`. The official installer selection
never replaces the installer inside the library bundle. Optional reconciliation
still requires the separately pinned `--verifier` and `--verifier-sha256`; select
a version with the official readback contract.

Prepare validates official metadata only, without copying or starting GitHub.
The installer owns the one `selected-reads` profile and its help/check behavior.
Select all three operations, in their displayed order: `get_me`, `list_commits`,
`get_file_contents`. Supply `token_env` as exactly `GITHUB_PERSONAL_ACCESS_TOKEN`.
This is a variable name, never a token. Custom GitHub hosts, operation subsets,
extra argv and alternate environment names are outside this fixed route.

Installation copies the pinned executable, all 78 notices, template and retained
guide, then publishes the actual `ready/mcp.json` and receipt. It never invokes
the GitHub binary, including for version or help. The launch configuration fixes
GitHub.com, read-only mode and these three tools. `env: {}` does not clear inherited
environment variables. The consuming host must inject the selected account's
existing token and exclude unrelated `GITHUB_*` toolset/app/OAuth/log options and
proxy settings, as explained by the retained official guide. No secret lookup,
authentication, connectivity check or MCP launch occurs in setup.

Distinct requests receive distinct job IDs and destinations even if their fixed
variable names match. A variable name is not an account identity: the consuming
host chooses the account separately for each child. Repeating the same validation
request returns the original job and never reinstalls it.

Official requests use schema version 2 with `official_component: "github"`, their
original official bundle/installer pins, config digest and host runtime paths.
Library version-1 records remain unchanged. Recovery imports only the original
request under explicit quiescent host custody, keeps historical outcome unknown
and permits state/readback only. It never restores validation or starts a new
install. Readback verifies retained binary/notices/configuration bytes and launch
binding; it does not run GitHub or establish present account access. The
[Linux setup successor](../../dist/toolkit-library-20261003-linux-github-setup/README.md)
includes explicit target metadata and passes an actual archive bootstrap,
MCP installation, replay and fresh-process recovery on CPython 3.11.16,
Linux x86_64/glibc 2.39. Its target is reported by help, check, install and readback;
the installer rejects a different host before destination work. This test does
not launch GitHub, sign in, or establish browser UI support.


## Optional official Playwright route (source candidate)

The host can enable `official-playwright` using the same official bundle and
installer pins above, plus `--node`, `--npm-cli`, `--playwright-host-profile` and
`--playwright-host-profile-sha256`. This candidate supports macOS arm64 with
Node 24.19.0 and npm 11.12.1. It accepts an existing Chrome executable, whose
compatibility or current sandbox launch permission is not established by checking
its file hash and executable mode bits. No browser is bundled
or downloaded. Older distributions do not acquire this route automatically.

The host profile is an exact four-field JSON object. All paths must be absolute
and canonical, with no symbolic-link components. Its parent directory must
already exist under the host's exclusive control:

```json
{
  "schema_version": 1,
  "chrome_executable": "/absolute/path/to/Google Chrome",
  "chrome_sha256": "HOST_VERIFIED_SHA256_OF_EXECUTABLE",
  "process_parent": "/private/tmp/short-host-owned-parent"
}
```

The host hashes these exact bytes and supplies the digest separately. Neither
agent tools nor form fields accept executable paths, browser arguments, environment
settings, cookies or profiles. The only profile is `isolated-chrome`, with empty
`values` and the fixed ordered coverage list `browser_navigate`, `browser_snapshot`,
`browser_fill_form`, `browser_click`, `browser_close`. The full Playwright endpoint
is exposed: these five operations are adopted coverage, not a tool filter.

Prepare/help/check read pinned metadata and file identities without invoking
Node, npm, Playwright or Chrome. Install uses only the selected Node/npm, a new
cache and the three original archives with the unchanged lock. It disables scripts,
network acquisition and browser downloads, verifies installed package payloads,
and emits `ready/mcp.json` without starting Playwright or Chrome.

Install exclusively creates one process directory under `process_parent`, named
`p-` plus 16 hexadecimal characters from the canonical installation path's SHA-256.
The receipt retains the full hash. Workspace, output, HOME, TMPDIR and socket
folders stay under that directory, outside the immutable software installation.
An existing child or a socket path over the conservative 103-byte budget (including
`/sockets/dashboard/` and a 16-character socket basename) refuses the attempt. A failed attempt
may leave partial directories; setup never reuses or cleans them. The host owns
serving, joining all children and cleanup. Check the emitted paths before launch.

The consuming host must launch with a minimal environment. In particular it must
exclude unrelated `PLAYWRIGHT_MCP_*`, `NODE_OPTIONS`, `NODE_PATH` and proxy settings;
the emitted MCP `env` object does not remove inherited variables. Headless and
isolated flags do not provide a file or network sandbox. Snapshot links require
host file access. No browser readiness, authentication or safe browsing claim is
made by installation or check.

Version-3 request records bind the exact host-profile digest, original official
bundle/installer and selected runtime paths. Recovery imports those same paths
without allocating another process directory or restoring validation. Readback
checks retained software/configuration and launch binding, and checks the process
root is a nonlink directory if it still exists. It neither requires nor traverses
mutable serving contents and never recreates a deleted root. Node/npm/Chrome are
not executed; complete Chrome bundle, generated npm cache/log/hidden-lock provenance
and browser readiness remain excluded. Version-1 library and version-2 GitHub
request behavior is unchanged. Linux browser execution remains unsupported here.


## Explicit catalog setup handoff

Bundles may include `routes.json`, authenticated by their selected `CONTENTS.json`.
The exporter compiles exact maintainer inventory associations against the exact
catalog and component bytes, using pinned accepted profile captures as generation
evidence. No display name, description or instruction prose determines a route.

For a mapped option, choose **Set up this option**. This clears the previous form,
prepares the specified library through the existing job when needed, then checks
that the exact profile exists in the installed package before displaying its help.
Choose operations and enter nonsecret configuration yourself. Credential fields
still accept reference names only. Preparation that is pending or unconfirmed is
never automatically repeated. Changing selection invalidates delayed responses.

GitHub and Playwright currently have no mapping in this index. Their explicit
manual controls and separately pinned official bundles remain available when the
host enables them. Old bundles without an index retain manual setup. A present
but invalid or changed index is refused, not silently treated as absent.

The index's runtime target binds the chosen bundle metadata; it does not establish
physical host compatibility. The existing installer retains target admission.
Neither the mapping nor profile help establishes account access or native attachment.

The route index is trusted maintainer build output authenticated by the host's
bundle pin. The exporter owns the association with the installer's consumed
artifact; the controller verifies the index's declared component files against
actual frozen bytes. It does not reimplement package selection. Route lookup
returns `scope: "catalog_setup_handoff"` because a prepared lookup runs the
existing installed help inspector; ordinary catalog list and detail stay
`catalog_metadata_only`. Neither route lookup nor help installs software.


## Reuse prior configuration in a new setup

A host may select one nonsecret configured-install request as historical input:
version 1 for library integrations, version 2 for official GitHub, or version 3
for official Playwright. Add the paired startup arguments to either `backend.py` or
`mcp_adapter.py`:

```sh
--prior-request /absolute/path/configured-install-request.json \
--prior-request-sha256 EXPECTED_REQUEST_SHA256
```

Use a fresh setup workspace and the separately selected current bundle. The
controller verifies the exact request digest and its configuration digest, then
freezes the original bytes in the new workspace. It never follows recorded old
workspace, runtime, configuration or output paths. The original file and any
original installation stay untouched. No old artifact is needed, and a relocated
request is allowed. Official routes use the newly selected official bundle,
installer and runtimes; Playwright also uses the current host profile. Old pins
and profile hashes are historical metadata, not current authority. Recovery
mode is separate and cannot use this path. Omit both arguments to retain the
existing setup behavior.

The host must designate the whole source file as nonsecret and suitable for the
setup audience. Format checks cannot detect secrets in ordinary strings. A
request records historical intent before installation runs; it does not prove
that an old install succeeded, that a credential is available, or that any old
grant still applies. These requests contain no historical field schemas or operation
effects, so this feature does not establish semantic compatibility.

Choose **Reuse prior configuration** to open the exact historical family and
profile. The action uses the existing preparation job if necessary and refuses
pending or unconfirmed preparation rather than retrying it. A missing current
profile is reported without choosing a replacement. The current installed
profile supplies all help and field definitions. Current fields receive only
exact structurally representable prior values; new fields remain blank. Prior
operation names are shown. Select current operations again for selectable
profiles. Official profiles display their complete current fixed list for
review; Playwright exposes the whole endpoint. A historical subset does not
filter the endpoint or restore an earlier permission. Both cases require an
explicit current check and install.

Unknown fields, removed operations, type mismatches, invalid reference names and
out-of-bound values appear as named unapplied items. Their original raw values
remain in the frozen source and are not returned in the proposal. Explicitly
replace an affected current value or acknowledge its omission before browser
validation. A prior conditional value that is not used by the current operation
selection also requires an explicit decision. Omitting it clears the old value;
selecting the operation later does not restore it. These are edits to a new
selection, not migration of the old install. The installed current parser still
owns validation, including new required fields and constraints that the form
cannot establish. Only a subsequent explicit install creates the separate new
installation. No active installation, credential grant or native binding is
switched by this workflow.


## Agent-selected packages outside the library

The optional [selected-package MCP tools](SELECTED.md) let an agent install its
own researched Python, Node or MCPB selection through this controller. The host pins
the installer, validators, staging root and local installation scope. Check
freezes exact inputs; install uses existing jobs, replay and retained-request
recovery. This route is exposed through MCP and the controller API; the browser
form continues to present configured library/official families. No selected
package browser form or native acquisition is implied.
