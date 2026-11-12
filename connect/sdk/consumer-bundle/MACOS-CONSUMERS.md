# macOS local library consumer guide

This guide accompanies the macOS consumer export for Apple silicon (arm64).
The bundle holds the Agno, Activepieces and services components, the npm lock
and all 115 dependency archives (111 Python wheels and 4 npm archives) for
offline installation. The three components are built from the Connect source
of the release that ships this bundle; their digests are recorded in
`CONTENTS.json`. Every dependency archive is the file its public index
publishes for the pinned name and version, checked against the index digest
before it is copied. Platform-specific wheels are the macOS arm64 or universal
builds for macOS 11 or later.

This consumer bundle supplies 36 Agno and 63 Activepieces profiles, plus the
alternative Slack/Gmail/Calendar/Drive service component, for one runtime
target: CPython 3.11 on macOS arm64. The catalog holds 101 adopted integration
records and the 9 discovery records of the Connect repository's discovery
catalog, keyed by full publisher/name/version identity. Names may repeat
across those identities. The discovery records are manual guidance, including
frontend and custom-extension recipes; they add no installed profiles and no
adopted integration count. GitHub and Playwright
have separate packages; their executables are not included here. The
alternative Slack/Gmail/Calendar/Drive component does not add adopted
identities.

Use the delivered installer with ordinary pip and npm. No Toolkit source build,
test harness or native Meerkat build is needed. The agent chooses a profile
and prepares its configuration. Installation does not choose a service, sign
into an account, grant access or attach tools to a native agent.

The base bundle excludes the optional Google verifier dependencies of the
services component; obtain that closure separately if needed.

## Choose an exact catalog option

The bundle's `routes.json` maps 99 adopted Agno/Activepieces entries and four
explicit Slack/Gmail/Calendar/Drive alternatives to their exact setup profiles.
It is derived from the routes of the Linux library of the same line: the
maintainer associations and profile names are the same, and the component
digests and the catalog identities that name them are those of this bundle.
`CONTENTS.json` pins this build output. The export never interprets a user's
intent and infers no mapping from names or prose.

In the accompanying setup interface, **Set up this option** prepares the chosen
library if needed and opens its profile. The agent tool `setup_catalog_route`
provides the same exact handoff. Operations and nonsecret configuration remain
explicit choices. Official GitHub and Playwright keep their separate manual
setup paths, as do all discovery-only strategies. Both source catalog hashes
and their separate counts are recorded in `CONTENTS.json`; composition
preserves record values and regenerates JSON bytes.

## Select the runtime

`CONTENTS.json` schema 1 is the macOS contract. The installer admits:

| Runtime | Required observation |
| --- | --- |
| Python | CPython 3.11, macOS arm64 |
| Node | v24.19.0 or v22.19.0, macOS arm64 |
| npm | the npm that belongs to the selected Node, invoked with that Node |

Only the relevant language runtime is needed for an installation. Python is
also needed to run the installer itself. Supply absolute executable paths.
The installer checks the selected runtime before creating a virtual environment
or invoking a package manager. A mismatch may leave a private directory of
frozen inputs and diagnostic logs; it publishes no installation receipt.

This is an exact consumer profile, not a claim that every macOS release,
Python patch release or Node version has been checked. Intel Macs are not a
target of this bundle. Other targets need their own dependency closure.

## Verify the selected delivery

Choose the `CONTENTS.json` digest from the trusted delivery record. A digest
computed from an untrusted replacement does not authenticate it. Check all
files before installation:

```sh
cd /absolute/path/toolkit-macos-consumers
env -i PATH=/usr/bin:/bin /usr/bin/shasum -a 256 -c SHA256SUMS
```

Each family has an exact hashed requirements file or npm lock and cache
inputs. Pip and npm own installation; Toolkit checks the resulting versions
and the archive payloads.

## Install a chosen configuration

Select a profile using `catalog.json` and the family's `PROFILE-GUIDE.md`.
The component guides describe each component; the installation steps and the
runtime in this file govern this delivery.

For a credential-free starting configuration, `agno/hackernews.json` selects
the Hacker News read operation. Use a new destination with an existing parent:

```sh
toolkit_bundle=/absolute/path/toolkit-macos-consumers
toolkit_python=/absolute/path/python3.11
toolkit_contents_sha256=EXPECTED_DIGEST_FROM_SELECTED_DELIVERY
toolkit_config_sha256=EXPECTED_DIGEST_OF_SELECTED_CONFIGURATION
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  "$toolkit_python" -I -B "$toolkit_bundle/install-library-consumer.py" \
  --bundle "$toolkit_bundle" --contents-sha256 "$toolkit_contents_sha256" \
  --family agno --python "$toolkit_python" \
  --destination /absolute/new/hackernews \
  --config "$toolkit_bundle/agno/hackernews.json" \
  --config-sha256 "$toolkit_config_sha256" --server-name hackernews
```

For Slack or Gmail, select `--family services`, the corresponding nonsecret
configuration, and a new destination. For Activepieces, select
`--family activepieces --node /absolute/path/node --npm-cli /absolute/path/npm-cli.js`
instead of the family-specific `--python` option. The Python interpreter still
runs the installer. These are explicit choices, not automatic fallbacks.

The optional configuration flags are all-or-none. The installer freezes their
exact bytes, runs the installed component's `check-config`, and publishes
`ready/mcp.json` with the actual absolute serving command. Load that standard
MCP configuration through your host. The host supplies the child environment
and referenced credentials; the file does not clear ambient environment or
attest account access. Validation output is discarded so that the installer
does not copy account details into diagnostics.

Without the configuration flags, `installed.json` records the component's base
command. Use that command with `profiles` and `config-help --profile PROFILE`
to inspect the setup guidance. Authentication and unresolved provider
prerequisites remain visible; those commands do not acquire credentials.

## Restore and diagnose

To restore, use the same trusted bundle, digest, runtime and selected
configuration in a different empty destination. Existing installations are
never overwritten. The installed graph and the archive payloads must still
match.

If the installer is interrupted, inspect `ready/installed.json` or the
unconfigured `installed.json` and step logs. A response can be interrupted
after publication. Keep incomplete installations unused and choose a new
directory after investigating. Publication gives atomic visibility of the
configured receipt and launch file, not power-loss durability or protection
from a malicious owner changing the directory.

All initial and restored environments remain owned by the invoking host.
Provider-side effects, live authentication, account readiness, native setup and
original-task continuation need their own evidence.

Successful `check-config` output includes
`credential_environment_references`, with literal JSON `field_path` keys and
the exact configured `environment_variable` name. Agno and services paths start
with `config`; Activepieces fields are at the root. Public profiles return `[]`.
Use installed `config-help` for credential meaning and prerequisites. The host
freezes and binds configuration to the intended request, performs protected
lookup and supplies the child environment. These names confer no account or
injection authority.

For Zendesk article search, set `auth_mode=oauth_access_token`, the selected
`subdomain`, timeout and `api_key_env` reference; omit `account_email`.
The host supplies and renews the existing access token. Omitted mode or
`auth_mode=api_token` keeps the account-email/API-token route. No OAuth
exchange, automatic account selection or Basic fallback is performed.

## Reproduce the export

The export is produced in the release run on macOS arm64. From a Connect
checkout, with the pinned public inputs already fetched into a directory,
first build the three components from `sdk/agno`, `sdk/services` and
`sdk/activepieces`:

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  /absolute/path/python3.11 -I -B scripts/build-library-components.py \
  --pins release/library-pins-macos-arm64.json \
  --pinned-inputs /absolute/path/pins \
  --python /absolute/path/python3.11 \
  --node /absolute/path/node --npm-cli /absolute/path/npm-cli.js \
  --source-date-epoch COMMIT_TIME_IN_SECONDS \
  --output /absolute/new/components
```

It writes the three files and `components.json` with their digests. Then
export:

```sh
env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  /absolute/path/python3.11 -I -B scripts/export-library-consumers.py \
  --target darwin-arm64-cpython311 \
  --pins release/library-pins-macos-arm64.json \
  --pinned-inputs /absolute/path/pins \
  --agno-wheel /absolute/new/components/meerkat_toolkit_agno-0.1.5-py3-none-any.whl \
  --agno-sha256 DIGEST_FROM_COMPONENTS_JSON \
  --services-wheel /absolute/new/components/meerkat_toolkit_services-0.1.23-py3-none-any.whl \
  --services-sha256 DIGEST_FROM_COMPONENTS_JSON \
  --activepieces-archive /absolute/new/components/rkat-toolkit-activepieces-0.1.0.tgz \
  --activepieces-sha256 DIGEST_FROM_COMPONENTS_JSON \
  --closure-record /absolute/new/dependency-closure-macos-arm64.json \
  --output /absolute/new/toolkit-macos-consumers
```

The exporter never infers the target from its own host. It verifies every
input against its pin or its stated digest, refuses existing outputs and
writes checksums last. It performs no downloads and runs no package code. The
component build runs the two staging scripts, the pinned build backend and
npm; the Activepieces build reads its locked development dependencies from
the npm registry. The installer and this guide come from the exporter
checkout, with their origin recorded in `CONTENTS.json`.
