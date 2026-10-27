# Forge source development

Forge uses the shared Meta Console components and transcript adapter from this
repository. Build those sources together. Installing a standalone Console release
or replacing its transcript with another renderer does not reproduce Forge.

From the repository root, with Node.js 20+ and npm available:

```bash
umask 077
npm --prefix console ci --ignore-scripts
npm --prefix installer/web ci
npm --prefix installer/web run build
node --test installer/web/build-config.test.mjs
```

The Console installation supplies development tools such as jsdom. Forge declares
its own browser dependencies and pins React and ReactDOM to one installation.
The private umask preserves the reviewed DOM closure's exact file permissions;
the checker authenticates modes as well as bytes. Install into a fresh checkout
if an earlier Console dependency installation used different permissions.
The build graph check bundles the actual application, including the shared chat,
and rejects browser dependencies resolved from a sibling installation.

The Guide runs one exact selected Meta release, `SELECTED_VERSION` in
`launchkit/guide_sdk.py`, with the Core release of the same version: the runtime
admission, the product verifier and the client pin all require that value
exactly. The Python project selects that release's published client,
`meerkat-meta==0.9.3`, for the Guide, and packaged products carry the same
wheel. Its hash-pinned bytes and provenance are in `vendor/guide/`. This pure
Python client includes the public helper transport-wait option.
This is separate from the repository's current Meta release version and from
each application's selected runtime. Updating repository source does not upgrade
an installed application, choose its gateway, or qualify a new SDK/native pair.
Tests for retained predecessor applications also require the historical
`meerkat-mobkit==0.8.42` SDK. That is a test dependency, not a runtime fallback.

## Run the regression checks

Use a source checkout for development, separate from a prepared installation and
its saved state. From the repository root, create a development environment if
one does not already exist. Python 3.12 matches hosted CI:

```bash
python3.12 -m venv installer/.venv
installer/.venv/bin/python -m pip install --no-index --no-deps --require-hashes --find-links installer/vendor/guide -r installer/requirements-guide-sdk.txt
installer/.venv/bin/python -m pip install -e './installer[test]'
```

On Linux, protected acceptance execution also requires `bubblewrap`,
`libseccomp2` and working user, PID, mount and network namespace isolation. The
Forge workflow installs those packages and runs the actual isolation probe
before the suite. A missing or failed sandbox cannot qualify generated-code
execution; keep that failure visible instead of using an unsandboxed fallback.
On Ubuntu, the workflow also loads the distribution's `bwrap-userns-restrict`
AppArmor profile so bubblewrap can prepare its namespaces while its children
remain restricted. See Ubuntu's [user-namespace release notes](https://documentation.ubuntu.com/release-notes/24.04/#unprivileged-user-namespace-restrictions).
A local Ubuntu host needs the same profile; the steps are in the
[standalone bundle prerequisites](README.md#standalone-bundle).

Build the frontend with the commands above before running the complete backend
suite. This includes `npm --prefix console ci --ignore-scripts`, which supplies
the exact jsdom dependency closure for protected DOM checks. Some backend cases
also inspect or package the built browser files. Explicitly prepare the private,
hash-pinned Node checker before the suite:

```bash
PYTHONPATH=installer installer/.venv/bin/python -m launchkit.checker_runtime
```

This downloads and verifies the selected checker during preparation. Checks
themselves do not download it, replace the frontend's Node.js or run without
isolation. Use the same test commands as the Forge workflow:

```bash
NODE_OPTIONS=--max-old-space-size=3072 node --test --test-concurrency=2 installer/web/*.test.mjs
installer/.venv/bin/python -m pytest -vv installer/tests --tb=short --durations=20 --junitxml=installer/.artifacts/pytest.xml
```

Allow substantial time for the full backend suite: in one process it took about
two hours on a busy development host. The Forge workflow splits it into twelve
shards balanced on the test time a hosted run recorded
(`installer/tests/forge_shard_seconds.json`), each with a 120-minute limit and
its own 20 slowest tests. The `backend` job passes only once every shard
has, and it checks that the shards together ran every test file once
(`forge_shards.py verify`). Refresh the record from a complete run's junit
files with `python installer/tests/forge_shards.py record forge-pytest.xml...`;
a stale record only affects the balance, never which files run.
No failure limit stops a run early, so every failure is reported. A run
stopped by a job timeout is incomplete even when its earlier cases passed.
Preserve the failure output and XML before making a correction. To run one
shard's files as the workflow does:

```bash
installer/.venv/bin/python installer/tests/forge_shards.py 3 12 shard-files.txt shard-startup.txt
installer/.venv/bin/python -m pytest -vv --tb=short $(cat shard-files.txt)
```

The shards also rerun each file listed in
`installer/tests/checkout_sdk_startup_tests.txt` once, with
`PYTHONPATH=sdk/python:installer`, so the startup code imports this checkout's
Meta SDK rather than the Guide SDK: `$(cat shard-startup.txt)` holds a shard's
share.

Generated-host wiring checks use the installed Guide SDK, without a private
source intake. Toolkit-specific doctor checks additionally require the selected
product's genuine Toolkit wheel and skip explicitly when it is absent. Passing
these checks establishes SDK wiring with inert runtime fixtures, not a live
native application or compatibility of its persisted state.

For a focused change, run the affected files first, for example:

```bash
installer/.venv/bin/python -m pytest -q installer/tests/test_meta_guide_runtime.py installer/tests/test_visualizer_author_context.py
```

The animation-author tests use the selected Meta dispatcher's typed image
content and native member/session contract with inert runtime fixtures. They do
not establish rendered visual quality or a useful installed result. SDK
byte-installation tests forbid SDK imports during installation/readback, while
preserving genuine SDK modules already imported by the host. Do not replace the
selected SDK with the current repository source merely to make these cases pass.

The reviewed Guide SDK accepts the public `spawn_helper(transport_timeout=...)`
option. Forge passes the time remaining in the existing total 180-second visual
review window, including provisioning and retirement, rather than extending the
native execution budget. Other calls keep the SDK's default 60-second wait. The
dedicated
`test_same_helper_can_correct_invalid_publication_after_sixty_second_transport_default`
case exercises the installed SDK against a controlled transport that models a
late correction. It does not establish a real native helper's execution past
60 seconds. An unknown reply does not prove that native work stopped or
authorize another helper.

## Prepare a runnable product

`start.sh` requires a complete prepared product for Meta startup. A source
checkout and a built frontend are insufficient. The product owner supplies the
selected Guide gateway, SDKs, application runtime and private host configuration.
The host installs the Guide client wheel and Toolkit wheel; the application's
separately selected Meta wheel stays in the product closure for its own managed
environment. Native gateway versions and state directories keep their original
owners. A client SDK change does not migrate native state or replace a gateway.
Keep private settings and saved state outside source archives. Existing prepared
installations retain their own launcher and selected artifacts.

### Use a source-built development candidate

A private trial can use the producer's `source_built_development_gateway`
descriptor before an installed-gateway verification report exists. Forge reads
this through the same `MetaToolkitRuntimeTarget` and
`read_meta_toolkit_artifacts` path as other selected products. It returns
`origin="source-built-development"`; it does not fabricate a qualification
report, treat staging as successful installation, or select a released version
on the caller's behalf.

The runtime owners supply the actual candidate and its exact source bindings.
The Forge source commit and any applied changes also need to be recorded for the
trial. Matching version labels alone do not identify these inputs:

| Selected input | Required relationship |
| --- | --- |
| Development descriptor and `staging.json` | The target, staged candidate, build record, resource build and resource component agree. |
| `build-record.json` | Records the successful `toolkit_gateway` build and its resource digest; the binary, source manifest and resource build match the candidate. |
| Source manifest | The explicitly selected local file has the original build's digest. A relocated copy keeps the original producer records unchanged. |
| Native archive | Contains the complete selected product prefix, including the exact gateway and resource manifest with their required modes. |
| Application Meta and Toolkit SDK wheels | Match the owner-supplied byte expectations and the descriptor's wheel digests; wheel metadata, source payloads and RECORD entries are checked. |
| Model catalog and offline dependencies | Match the selected emitted catalog and the complete pinned dependency batch. |

These are existing producer records and selections, not values to invent in a
guide brief. `MetaToolkitRuntimeTarget.source_manifest` supplies the explicit
`ArtifactFile` for a relocated source manifest under the owned artifact root.
Do not rewrite paths inside the producer's hashed records to make a transfer
work. The descriptor, its neighboring records and archive, SDKs, catalog and
dependency files must remain available for the selected input checks.

For an owner-supplied `selected_target`, the read-only check is:

```python
from launchkit.runtime_candidates import read_meta_toolkit_artifacts

artifacts = read_meta_toolkit_artifacts(selected_target)
print(artifacts.origin)
```

This reads and verifies selected artifacts without starting a gateway, importing
the selected SDKs, or granting runtime admission. A changed or incomplete input
raises `CandidateArtifactError`. It is not an installation or host-refresh
command. The product and host owners use their existing preparation and startup
path with that exact selection; updating this checkout does not change a retained
installation or its in-progress build.

The source-only counterexamples in
[`test_development_product_selection.py`](tests/test_development_product_selection.py)
cover absent or changed source evidence, failed builds, changed SDK or catalog
bindings, modified product contents and false qualification claims. They use
inert artifacts. Actual hosted login, declared-class startup, useful application
work and recovery need observations from the selected development installation.

Frontend build checks and an empty browser opening establish source integration
only. Release acceptance still requires a useful installed job, correction,
restart and retained history through the supported product path.
