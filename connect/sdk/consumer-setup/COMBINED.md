# Catalog and selected packages in one setup session

The combined library archive keeps the catalog and all its existing library
families, and adds selected Python/Node installation and optional public
PyPI/npm acquisition to the same setup Controller. The host enables these paths explicitly.
Installing the archive alone enables no network access or additional tools.

Start with the extracted archive's root `README.md`. Its offline, unconfigured
services installation supplies the MCP client dependencies for
`setup/mcp_adapter.py`. Reuse that environment and the original host-selected
CPython 3.11 executable. There is no second bootstrap environment or setup
process for selected packages.

Use [host preparation](HOST-PREPARATION.md) to generate the existing profiles
and ordinary MCP configuration from an exact release and explicit runtime
inputs. It avoids manual JSON and hash copying. The profile formats below
remain available for hosts composing their own configuration.

## Enable Node alongside the library

The [host preparation command](HOST-PREPARATION.md) can add the Node validator
and public npm profile while preserving the library's Node/npm pair. Pass a
complete `--selected-node` and `--selected-npm-cli` pair with their hashes.
Add the independent `--allow-public-npm` flag and pinned full npm archive only
when public acquisition is intended. Python and npm network scopes are separate.

Linux Activepieces uses Node 22.23.2/npm 10.9.8. The selected npm acquisition
path requires Node 24.15 or later within major 24 and npm 12.2.0. Both pairs can
now coexist in one setup process. The effective selected pair is retained in
selected-package request bindings and checked again during recovery. Omitting
the new overrides preserves the older shared-runtime behavior; it does not
silently select another version or disable a library family.

Compose and MCPB retain their separate standalone delivery profiles. The setup
Controller's explicit selected Node pair also governs MCPB when a host composes
that optional profile separately; this archive does not add its executable.

## Enable the existing host profiles

Before starting the MCP command in the root guide, create two nonsecret files
outside the new setup workspace. Use canonical absolute paths. The staging
root must be a host-owned directory disjoint from the setup workspace.

For macOS arm64, `selected-install.json` is:

```json
{
  "schema_version": 1,
  "scope": "standing_local_software_installation_only",
  "staging_root": "/ABS/host/staging",
  "installer": {
    "path": "/ABS/extracted-library/selected/bin/install-library-consumer.py",
    "sha256": "c04a86a9bbd9b6384be1c03bea29b1f816fe8965c070da34237e46ad31dc2481"
  },
  "validators": {
    "python": {
      "path": "/ABS/extracted-library/selected/bin/toolkit-python-plan",
      "sha256": "0436d6682899f33fc6eefb9246d49397a8f63c401017db261a68e4618b4e9f55"
    }
  }
}
```

For Linux x86_64, use the same shape, add
`"python_target": "linux-x86_64"` at the top level, and replace the installer
hash with `4b4c468d97f73d6ce60117ad6a48deb0ee5f291953fd7143744124f1819b89a0`
and the planner hash with
`de4895a3f231e52c71256b412fbd265c03425ceb01e165f3fd5875244f33c9a0`.
These are the exact tools in this platform's archive. The selected installer
is distinct from both `consumers/install-library-consumer.py` and
`setup/install-library-consumer.py`; retain the paths shown above.

`python-acquisition.json` uses the same host Python executable as the library
and selected installation paths:

```json
{
  "schema_version": 1,
  "scope": "trusted_pypi_wheel_acquisition",
  "python_sha256": "SHA256_OF_THE_HOST_PYTHON_EXECUTABLE",
  "pip": {
    "path": "/ABS/host/trusted/pip.pyz",
    "sha256": "SHA256_OF_THE_TRUSTED_PIP_ZIPAPP"
  }
}
```

For Linux, also add `"target": "linux-x86_64"` to this second profile. The two
targets must agree. Omitted targets retain the macOS arm64 default. The worker
checks the native interpreter before invoking pip. Obtain and verify the pip
zipapp through the host's trusted distribution process; it is not bundled.

Compute each file's SHA-256 after filling in its final values. For example,
using the already selected Python executable:

```sh
"$toolkit_python" -I -c 'import hashlib,sys; print(hashlib.file_digest(open(sys.argv[1], "rb"), "sha256").hexdigest())' /ABS/host/selected-install.json
```

Append these four option-value pairs to the **same** MCP or browser command
from the root guide before starting it. In MCP JSON, each option and value is
a separate string in `args`:

```text
--selected-install-profile /ABS/host/selected-install.json
--selected-install-profile-sha256 SHA256_OF_SELECTED_INSTALL_JSON
--python-acquisition-profile /ABS/host/python-acquisition.json
--python-acquisition-profile-sha256 SHA256_OF_PYTHON_ACQUISITION_JSON
```

Keep the existing `--bundle`, `--contents-sha256`, `--verifier`, runtime paths
and `--families agno,services,activepieces` arguments. This retains all enabled
catalog families. Existing optional official-server flags still work. To
permit only offline selected installation, omit the two acquisition options;
to retain the original library-only tool surface, omit all four options.

The acquisition scope permits trusted pip to resolve public PyPI wheels and
the setup worker to download their exact reported artifacts. It is not a
network firewall. The host owns network isolation and executable trust.
No source build, custom registry, credential lookup or selected-server
execution is authorized by this profile. See
[Python acquisition](PYTHON-ACQUISITION.md) for bounds and cancellation behavior.

## Give the agent both paths

In one connected session, the agent can browse `setup_catalog`, choose an exact
`setup_catalog_route`, and use `setup_prepare`, `setup_check` and `setup_install`
for library configurations. If the user needs a different integration, the
agent researches and chooses it, then calls `setup_acquire_python` with an exact
public distribution/version and a new request ID. Toolkit validates the
selection; it does not infer which package fits the user's intent.

The shared `setup_job`, `setup_jobs`, `setup_state` and `setup_stop` tools cover
both paths. Acquisition success means retained wheel inputs, not an installed
or usable integration. The agent inspects wheel metadata and source with
host-permitted bounded tools, without importing the package, then authors
`package.json`. With `--enable-package-inspection` selected during host
preparation, call `setup_inspect_package` with its exact `package_json` text.
Use the returned `summary.package_content_digest` in the agent-authored
activation, then pass both texts as `package_json` and `activation_json` to
`setup_selected_check`, together with the acquired `staged_id`, profile and
component. This submits metadata through MCP without writing files into
staging. Artifacts still come from the acquisition or host-staged inputs.

If that optional inspection tool is not enabled, obtain the same typed content
digest with the included inspector and follow the existing staged-file path:

```sh
"$toolkit_release/selected/bin/toolkit-package-inspect" integration-package < /ABS/host/staging/STAGED_ID/package.json
```

Require `status: "accepted"`, use `summary.package_content_digest` in the
activation, and call `setup_selected_check` followed by
`setup_selected_install`. Follow the complete
[metadata examples](METADATA.md) and [selected-install contract](SELECTED.md).
The supplemental `selected/agent/instructions.md` describes the selected
installer workflow; its tool paths are relative to `selected/`, while the
shared MCP entry point remains `setup/mcp_adapter.py`.

Keep every returned recovery anchor and the original workspace. The existing
[multi-request recovery](MULTI-RECOVERY.md) can read back selected library,
installation and acquisition requests together, subject to its host bindings.
Recovery makes no historical success claim and performs no automatic retry.

The host separately decides when to load an installed MCP configuration,
provide credentials, authenticate accounts and resume the original task.
Neither installation path supplies native admission, attachment or continuation.

## Distribution scope

The selected tools are retained bytes from accepted platform kits. Their
applicable crate, Rust and Toolkit licenses and retained source/build records
are under `selected/licenses/` and `selected/provenance/`. The selection record
identifies the exact donor archive and selected members. `donor-kit.json` and
dependency records describe the broader historical donor; they do not declare
additional shipped capabilities. Document records retain source and transformed
hashes. Unshipped references are labeled source repository paths, not local files.

This edition combines Python and Node with the library. The standalone Compose
and MCPB delivery profiles remain separate. Each selected Node/npm pair is
explicit, and the existing request binding records it for recovery. No library
family is removed or runtime version silently changed to add selected packages.

The normal host-preparation command also accepts explicit `official-github` and
`official-playwright` family selections when shipped for the chosen target.
The helper binds the separate official payload and existing installer from the
verified release; Playwright additionally requires the pinned host profile and
library Node/npm pair. These choices use the same catalog route, preparation,
check, install and readback tools. See [host preparation](HOST-PREPARATION.md).
