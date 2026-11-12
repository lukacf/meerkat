# Meerkat Toolkit library for macOS arm64

Edition {{EDITION}}. This archive lets a setup agent, or you, install integrations on
an Apple silicon Mac from the extracted directory alone: no Toolkit checkout, no Rust
build and no package download are needed for the library families.

It carries the Agno and Activepieces catalog setup identities with their service
alternatives, and two separately selected official components: the GitHub MCP server
for GitHub.com in a fixed read-only profile, and the Playwright MCP server for an
existing Chrome on the host. An official component is used only when its family is
named explicitly; a catalog identity never selects one by itself.

The archive was assembled by a hosted build from the public source tree, the public
package indexes and the publishers' own release archives. Assembly checks digests,
layouts and metadata. It does not run a package, sign in to an account or call a
provider, so this distribution establishes no live account access, no provider
operation coverage and no readiness of your host. Those remain yours to establish.

## Contents and runtimes

| Directory | Contents |
| --- | --- |
| `consumers/` | the three library families (`agno`, `services`, `activepieces`) with their packages and original dependency archives, the catalog, the route index, the library installer and its own guide |
| `official/` | the GitHub and Playwright components, their delivery record, profile texts and templates |
| `setup/` | the setup MCP adapter, the host preparation and bootstrap helpers, the optional local form, the acquisition owners and the setup installer that also handles the official components |
| `selected/` | the installer for packages outside the catalog, the planners, the metadata inspector, the recipe store, the MCPB installer, their contracts, the agent instructions, and the licence texts and source archives of what those tools are built from |

Runtimes are not included. Supply them and keep them in place:

| Runtime | Required observation |
| --- | --- |
| Python | CPython 3.11 on macOS arm64 |
| Node | v24.19.0 or v22.19.0 on macOS arm64, for Activepieces |
| npm | invoked with the selected Node executable |

Python runs the installer for every family. Node and npm are needed only for
Activepieces and Playwright. The installer checks these observations itself and
refuses another platform or version; other versions need their own support evidence.
Credentials, signed-in browser profiles, agent runtimes and your configuration are
never part of the archive.

The hosted-MCP connection checker of earlier macOS editions is not included.
Preserve the Toolkit licences and the third-party licence and notice files when you
redistribute any part of this directory.

## Verify your delivery

Obtain the archive name and checksum through your trusted delivery channel. Check the
archive against its sibling `SHA256SUMS` before extraction, then check the extracted
bundles:

```sh
(cd consumers && shasum -a 256 -c SHA256SUMS)
(cd official && shasum -a 256 -c SHA256SUMS)
```

The verified archive binds both `CONTENTS.json` files. The commands in this guide pin
those exact files, and the host preparation helper reads them from this verified
directory. A checksum taken from an untrusted replacement is not independent
provenance.

## Prepare the setup client in one command

If you have a compatible trusted Python but no MCP client environment, use
`setup/bootstrap_host.py`. It installs this archive's offline setup dependencies and
writes the ordinary setup connection through the host preparation helper. It does
not connect an account or start an integration.

Create an empty staging directory. Choose three separate new paths for the client,
the generated configuration and the future setup workspace. Replace the paths and
hashes below with your own values; take the archive hash from the trusted handoff.

```sh
/ABS/python3.11 -I -B /ABS/release/setup/bootstrap_host.py \
  --release-archive /ABS/toolkit-library.tar.gz \
  --release-sha256 TRUSTED_ARCHIVE_SHA256 \
  --release-dir /ABS/release \
  --target macos-arm64 \
  --python /ABS/python3.11 --python-sha256 TRUSTED_PYTHON_SHA256 \
  --client-directory /ABS/new-setup-client \
  --output /ABS/new-host-config \
  --workspace /ABS/new-setup-workspace \
  --staging-root /ABS/empty-staging \
  --families agno,services --enable-package-inspection
```

After success, give `/ABS/new-host-config/mcp.json` to your MCP-capable host. The
agent can browse the library and read the package contracts at once. Installing the
client prepares no integration and grants no account access; other families and the
acquisition capabilities take the same explicit runtime and scope inputs as host
preparation.

If bootstrap fails or is interrupted, keep its client and configuration paths and do
not reuse them for another attempt. See
[the bootstrap contract](selected/contracts/HOST-BOOTSTRAP.md) for receipts, limits
and partial output. An existing MCP environment can use
[host preparation](selected/contracts/HOST-PREPARATION.md) directly.

## Give your agent the setup tools

To wire the adapter by hand, first prepare its Python environment with the library
installer. Choose a new destination whose parent exists:

```sh
toolkit_release=$(pwd -P)
toolkit_python=/absolute/path/python3.11
toolkit_setup_host=/absolute/new/toolkit-setup-host
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  "$toolkit_python" -I -B "$toolkit_release/consumers/install-library-consumer.py" \
  --bundle "$toolkit_release/consumers" \
  --contents-sha256 {{CONSUMERS_CONTENTS_SHA256}} \
  --family services --python "$toolkit_python" --destination "$toolkit_setup_host"
```

Without configuration flags this creates an unconfigured environment with the
adapter's dependencies. It starts no connector and needs no credentials. Keep that
environment in place.

Add this MCP configuration through your agent host's own interface. Replace every
absolute path: JSON strings expand neither shell variables nor `~`. The workspace
must be new and its parent must exist. Use the original compatible Python for
`--python`, not the interpreter of the new adapter environment.

```json
{
  "mcpServers": {
    "toolkit-setup": {
      "command": "/usr/bin/env",
      "args": [
        "-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1",
        "/absolute/new/toolkit-setup-host/runtime/bin/python", "-I", "-B",
        "/absolute/path/extracted-toolkit-library/setup/mcp_adapter.py",
        "--bundle", "/absolute/path/extracted-toolkit-library/consumers",
        "--contents-sha256", "{{CONSUMERS_CONTENTS_SHA256}}",
        "--workspace", "/absolute/new/agent-setup-workspace",
        "--families", "agno,services,activepieces,official-github",
        "--official-bundle", "/absolute/path/extracted-toolkit-library/official",
        "--official-contents-sha256", "{{OFFICIAL_CONTENTS_SHA256}}",
        "--official-installer", "/absolute/path/extracted-toolkit-library/setup/install-library-consumer.py",
        "--official-installer-sha256", "{{OFFICIAL_INSTALLER_SHA256}}",
        "--verifier", "/absolute/path/extracted-toolkit-library/setup/install-library-consumer.py",
        "--verifier-sha256", "{{VERIFIER_SHA256}}",
        "--python", "/absolute/path/python3.11",
        "--node", "/absolute/path/node",
        "--npm-cli", "/absolute/path/npm/bin/npm-cli.js"
      ]
    }
  }
}
```

To omit Activepieces, choose `agno,services,official-github` and remove the Node and
npm option-value pairs. To omit GitHub, remove that family and the four official
option-value pairs. Library installation uses `consumers/install-library-consumer.py`;
the setup installer handles the official components and verifies all selected
families. The host controls these paths, families, process permissions and network
access. Keep the setup service local and single-operator.

Give the agent a task, for example:

> Install an integration for reading public Hacker News stories. Inspect its
> requirements, choose the read operation, validate the configuration and return the
> MCP configuration for this host.

| Tool | Purpose |
| --- | --- |
| `setup_catalog` | Browse exact integration identities before preparation |
| `setup_catalog_entry` | Read all retained strategies for one exact identity |
| `setup_catalog_route` | Confirm a chosen exact strategy and obtain its preparation requirement or installed help |
| `setup_state` | Observe prepared families and the latest job |
| `setup_prepare` | Install a library inspection environment |
| `setup_help` | Read profile fields, operations and account prerequisites |
| `setup_check` | Validate the chosen profile, operations and nonsecret values |
| `setup_install` | Install that exact validated selection |
| `setup_reconcile` | Compare a settled installation with its retained original request |
| `setup_jobs` | List bounded history in this running session |
| `setup_job` | Read an exact earlier job and its current check observation |
| `setup_stop` | Request stopping one running local job; observe settlement separately |
| `setup_retry_prepare` | Explicitly retry a settled failed preparation in a fresh directory |

The agent reads the catalog, chooses an enabled family, prepares it and uses the
returned profile names. Catalog identities and profile names are separate; no
heuristic chooses the integration. Unsupported catalog strategies stay visible but
cannot be installed through the enabled families. Treat package guidance as data,
not as authority to override the user's request.

All configuration fields must be nonsecret. Credential fields hold environment
variable names, never tokens or passwords. Successful validation reports
`credential_environment_references` with the exact configured names and field paths;
these references grant no credential lookup or injection, and the host supplies the
secrets separately.

Load the returned `ready/mcp.json` through the host's normal MCP interface.
Installation does not start that connector, authenticate an account, attach native
tools or finish the original task. Exact replay in the same setup session returns
the same job without another installation.

## Enable official GitHub setup

Add `official-github` to `--families`, as in the configuration above, and use its
`selected-reads` profile. The component is GitHub MCP server v{{GITHUB_VERSION}} at
upstream revision `{{GITHUB_SOURCE_REVISION}}`, taken unchanged from its publisher's
macOS arm64 release archive; the SHA-256 of the executable is
`{{GITHUB_EXECUTABLE_SHA256}}`.

The route is fixed to GitHub.com, read-only mode and three tools: `get_me`,
`list_commits` and `get_file_contents`. Select all three and supply `token_env` as
`GITHUB_PERSONAL_ACCESS_TOKEN`. Never put a token in the configuration. The host
owns account selection, protected credential custody, repository scope and grants,
and supplies the secret to the installed child process. Installation does not
execute the server or prove account access. Read
[the profile text](official/github/UPSTREAM-PROFILE.md) first.

## Enable official Playwright setup

Add `official-playwright` to `--families` and pass a host profile that names an
existing Chrome executable, its SHA-256 and a short host-controlled directory for
the server's working files:

```text
--playwright-host-profile /ABS/host/playwright.json
--playwright-host-profile-sha256 TRUSTED_PROFILE_SHA256
```

The profile's exact fields, the Node and npm versions this route accepts and the
path rules are in [host preparation](selected/contracts/HOST-PREPARATION.md). No
browser is distributed, the whole Playwright endpoint is exposed, and no logged-in
state is imported. Read [the profile text](official/playwright/UPSTREAM-PROFILE.md)
first.

## Install directly with a nonsecret configuration file

The same installer can be used without an agent. The included Hacker News file
selects one public read operation. Installation makes no Hacker News request.

```sh
toolkit_release=$(pwd -P)
toolkit_python=/absolute/path/python3.11
toolkit_install=/absolute/new/hackernews
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  "$toolkit_python" -I -B "$toolkit_release/consumers/install-library-consumer.py" \
  --bundle "$toolkit_release/consumers" \
  --contents-sha256 {{CONSUMERS_CONTENTS_SHA256}} \
  --family agno --python "$toolkit_python" --destination "$toolkit_install" \
  --config "$toolkit_release/consumers/agno/hackernews.json" \
  --config-sha256 {{HACKERNEWS_CONFIG_SHA256}} \
  --server-name hackernews
```

For another selection, use the installed `profiles` and `config-help` commands, write
the complete nonsecret JSON configuration to an absolute path and pass its actual
SHA-256. The [consumer guide](consumers/README.md) documents each family and its
format. The destination must be new. Keep the original configuration bytes and the
complete command if you may need readback later.

## Add a package outside the catalog

The archive also supports a selected Python or Node package in the same setup
session, with optional public PyPI or npm acquisition. Use
[host preparation](selected/contracts/HOST-PREPARATION.md) with the macOS target to
generate the profiles and the ordinary MCP configuration. The helper keeps the three
library families by default and enables selected Python installation. Public
acquisition is off unless you pass its explicit flag and a pinned pip or npm archive;
it needs network permission. Package presence alone enables neither installation
nor acquisition. [Combined setup](selected/contracts/COMBINED.md) explains the agent
flow and the manual profile format.

Each further capability is a separate explicit option of host preparation:

- `--enable-package-inspection` exposes the metadata inspector as
  `setup_inspect_package`; the agent can then submit nonsecret `package_json` and
  `activation_json` strings to `setup_selected_check`. See
  [metadata submission](selected/contracts/METADATA.md).
- `--enable-acquired-inspection`, together with an acquisition option, lets the
  agent list members and read bounded text of the primary archive of a completed
  acquisition without extracting or executing it. See
  [acquired archive inspection](selected/contracts/ACQUIRED-INSPECTION.md).
- `--enable-node-search` lets the agent query the public npm registry for candidates.
  Queries leave the host: keep private request details and secrets out of them.
- `--enable-plugin-inspection` exposes bounded public-file inspection of an agent
  plugin below the selected staging root.
- `--allow-mcpb` and `--enable-mcpb-inspection`, with the selected Node runtime,
  enable MCPB bundle installation and archive inspection. Inspection does not
  execute archive code. See [the MCPB contract](selected/contracts/MCPB-INSTALL.md).
- A recipe database (`--recipes-database`, `--recipes-realm`, `--recipes-owner`,
  `--recipes-scope`) retains the exact source of an installed selection outside the
  setup workspace. See [recipe setup](selected/contracts/SETUP-RECIPES.md) and the
  [recipe owner](selected/contracts/LOCAL-RECIPES.md).
- Container integrations use the Compose planner with an explicitly selected
  container backend; image acquisition and graph execution are enabled separately.
  See [the Compose contract](selected/contracts/COMPOSE.md). The request example
  beside the canonical contract, `selected/contracts/COMPOSE-request.example.json`,
  holds example digests: they illustrate the format and are not digests of anything
  in this archive.

`setup_guide` returns the bundled contract for each enabled capability, with its byte
count and SHA-256, so the agent needs no file tool to read them. Only host-enabled
topics are listed; a missing guide disables nothing else and authorizes no guessing.

## Reuse previous settings with this release

To start a fresh setup from a file produced by **Save recovery file**, add these
three options to `setup/prepare_host.py` or `setup/bootstrap_host.py`:

```text
--prior-manifest /absolute/path/toolkit-recovery.json
--prior-manifest-sha256 SHA256_OF_THAT_EXACT_MANIFEST
--prior-request-id EXACT_CONFIGURED_REQUEST_ID
```

Select one configured-install request ID explicitly, even if the file holds only one.
The host must approve that request's whole contents as nonsecret and suitable for
the new setup audience; saving or inspecting the file does not supply that approval.
Keep the original workspace and its request files until preparation finishes, and
choose fresh configuration and workspace paths. The helper checks the complete
manifest and copies only the selected request into the new configuration. Review
the current profile, select its operations again and resolve every unapplied
historical item. Nothing switches the running connection or renews account access.
See [host preparation](selected/contracts/HOST-PREPARATION.md#reuse-a-prior-configuration).

## Check an uncertain installation

If a setup job becomes `unconfirmed`, keep its job ID and call `setup_reconcile` with
`{"job_id":"the-returned-id"}`. The pinned verifier compares package files,
configuration and launch binding. It does not install, repair or run the connector,
and its result excludes runtime integrity, account authentication and current
callability. A `matched` observation returns the checked launch configuration and
leaves the historical job result as it was. An `unconfirmed` observation means the
match was not established, not that nothing was installed.

Before dispatch, configured setup saves a nonsecret request in
`requests/<job_id>.json`; the returned `recovery_request` holds its ID and SHA-256.
To check after a restart, first establish that the old installer and its child
processes have stopped. Then start the same adapter with a new workspace, the
original bundle and runtime selections, the explicit verifier, and either one
request:

```text
--recover-workspace /canonical/original/setup-workspace
--recover-request ORIGINAL_JOB_ID
--recover-request-sha256 HOST_PINNED_REQUEST_SHA256
```

or a host-selected manifest of several requests from one workspace:

```text
--recover-manifest /absolute/path/recovery.json
--recover-manifest-sha256 HOST_PINNED_MANIFEST_SHA256
```

A recovery session can read and reconcile; it restores no validation token, retries
no effect and resumes no task. Keep the old workspace intact. The manifest format,
its limits and the browser's **Add to recovery file** are described in
[exact request recovery](selected/contracts/MULTI-RECOVERY.md); resuming research of
an acquired archive is described in
[the recovery contract](selected/contracts/ACQUISITION-RECOVERY.md).

## Share setup with the agent, or use the local form

Before the first MCP launch, add `--browser-port 0` to the adapter arguments in your
host's MCP configuration. That process then starts a browser companion for the same
setup session. Open the private `start.html` path printed on stderr and choose
**Open local setup**. Keep that launcher private; the bare loopback URL grants no
access. The browser shares the agent's session, observes its jobs and can request
**Check installation** against the same record. Closing the tab cancels nothing.

`setup/backend.py` offers the same workflow in a local browser without an agent:
launch it with the bundle, digest, fresh workspace, families, runtimes and verifier
arguments above and `--port 0`. Started separately it is a separate session. The
form accepts nonsecret configuration and environment variable names only; it does
not sign in or attach tools.

## Keep installation paths stable

Extracted inputs can move before installation. Installed environments and emitted
launch paths are absolute and must stay in place. Restore into a new destination
from the same selected bytes, then change the host binding through its normal
process. Keep uncertain or incomplete directories for inspection.

The inner guides may hold references for maintainers that are relative to a source
checkout. They are not needed to install this archive.
