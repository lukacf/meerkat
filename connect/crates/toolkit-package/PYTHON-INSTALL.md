# Selected Python installation contract

`PythonInstallPlan` projects one explicit Python MCP selection from the existing
`IntegrationPackage` with either `ValidatedManifest` or `ValidatedLocalSoftware`.
Both validated forms belong to the existing package owner. An agent can research and
choose the package, original wheel, complete dependency closure and entrypoint.
The trusted local installer validates that exact choice before realizing an
environment with ordinary pip. This is not a resolver or a native agent setup
workflow.

## Select the document form

Current source supports the plain service form described in the
[metadata guide](../../sdk/consumer-setup/METADATA.md). It admits one independent
local Python service with an exact pinned closure, connection-wide operations
and no known or unresolved selected-strategy requirements. Custom host,
lifecycle, enforcement or selected-resource obligations require the full
activation path; do not remove them from the researched package to fit this form.

| Input form | Fixed selected file | Planner flag | Installer and readback flag | Canonical identity |
| --- | --- | --- | --- | --- |
| Full activation | `activation.json` | `--selection` | Omit `--document-kind` | `manifest_digest` |
| Plain software | `declaration.json` | `--declaration` | `--document-kind local_software_declaration` | `document_kind` plus `declaration_digest` |

For both forms, `--selection-sha256` is the SHA-256 of the exact selected file,
separate from its canonical digest. The new form never emits `manifest_digest`.
Use the same form and raw hash for installation, reconciliation and explicit
restore. Both forms together, an unknown kind or mismatched receipt identity are
refused. Software declarations do not establish native schema observation,
authentication, grants or readiness.

The historical delivery and execution evidence below concern full activation.
The new form requires a coherently built current installer and planner; those
older delivery paths and passing runs do not qualify it. In the install and
readback examples, select those current executables, add the plain-form flag
above and use the raw declaration hash when choosing plain software.

## Install the selected bundle

The selected Python installer uses a new virtual environment and ordinary
offline pip. Supported native targets are CPython 3.11 on macOS arm64
(default) and 64-bit Linux x86_64 (`--python-target linux-x86_64`). The exact
`mcp-server-git==2026.8.18` selection passed a fresh installation and separate
restore under network denial using CPython 3.11.15. Both emitted configurations
were consumed by a generic MCP client, which initialized, listed 12 tools,
read the exact fixture commit with `git_show` and closed. This establishes that
selected package and platform, not arbitrary Python packages or other hosts.
The [execution receipt](../../docs/verification/20261003-selected-python/attempt2/acceptance.json)
retains both runs. [Independent readback](../../docs/verification/20261003-selected-python/independent-review/final-readback.json)
accepts the bounded component: both physical installations, exact inputs, wheel
payloads, generated-file hashes and distribution graphs match. Bootstrap
preservation was checked by the executing verifier; the independent readback
checks the remaining file/path sets but cannot reconstruct the unretained
preinstallation bootstrap byte map.

Use the installer and `toolkit-python-plan` in `dist/selected-python-20261003`.
The earlier library and selected Node deliveries do not contain this Python
mode. The complete input closure must accompany the executables.

After reviewing the complete input directory described below, the host selects
the raw activation digest and trusted executable paths. Use a fresh destination
whose parent already exists and is controlled by that host:

```sh
env -i PATH=/usr/bin:/bin \
  /ABS/python3.11 -I -B /ABS/selected-python-20261003/install-library-consumer.py \
  --selected-python /ABS/reviewed-python-bundle \
  --selection-sha256 HOST_SELECTED_RAW_ACTIVATION_SHA256 \
  --component python-server \
  --validator /ABS/selected-python-20261003/toolkit-python-plan \
  --python /ABS/python3.11 \
  --destination /ABS/new/selected-python-install
```

For Linux, use native Linux validator binaries and add
`--python-target linux-x86_64` to both installation and `--verify-existing`
commands. Installation checks the actual runtime before creating the virtual
environment. Linux readback requires its explicit target receipt and permits
only the standard relative `runtime/lib64 -> lib` directory link. This remains
a retained-file check, not proof of current interpreter compatibility or
permission to execute. Existing Mac receipts retain their original shape.

Replace `python-server` with the selected component ID. Installation freezes and
validates the inputs, inspects every wheel before invoking pip, installs only
the exact local hash-locked requirements, and verifies the installed environment.
It does not start the server. On success, `ready/mcp.json` supplies an ordinary
stdio MCP entry and `ready/installed.json` records the software observations.
The host supplies the launch environment and independently checks the actual
MCP contract. `ready` does not mean account, role or business-task readiness.

Software restore repeats the same selected inputs into another new directory;
it does not overwrite an existing environment. The host switches clients and
joins the previous process. This does not restore credentials, grants or
provider state, or replay tool calls.

## Reconcile an existing selected installation

The current source installer supports `--verify-existing` with the same original
`--selected-python`, raw `--selection-sha256`, `--component`, trusted `--validator`
and `--python` paths, plus the existing `--destination`. Earlier deliveries may
not include this command. Invoke it using a trusted CPython 3.11 verifier, outside
the environment being inspected:

```sh
env -i PATH=/usr/bin:/bin \
  /ABS/python3.11 -I -B /ABS/current/install-library-consumer.py \
  --selected-python /ABS/reviewed-python-bundle \
  --selection-sha256 HOST_SELECTED_RAW_ACTIVATION_SHA256 \
  --component python-server \
  --validator /ABS/trusted/toolkit-python-plan \
  --python /ABS/python3.11 \
  --destination /ABS/existing/selected-python-install \
  --verify-existing
```

The command copies the bounded original metadata into private temporary storage
under the host's `TMPDIR`, or `/tmp` when unset. The supplied directory must be
absolute, existing and outside both the original bundle and destination after
resolving symlinks. Invalid locations are refused before temporary-file creation.
The host must permit writes there. The command invokes only
the host-selected canonical validator there, with bounded output, a single
deadline and process-group cleanup on interruption. It uses a minimal child
environment, closed stdin and discarded stderr. It never invokes the selected
runtime, installed environment, pip, donor code or MCP server. Unlike library
readback, selected readback therefore has one trusted metadata child and temporary
writes. It requests no network access and never writes into, repairs or promotes
the observed installation.

Original metadata remains limited to 1 MiB per document. Captured validator
output and the existing receipt use the installer's 64 MiB file budget. The MCP
file is bounded by the freshly derived encoded configuration length, including
Unicode escaping and literal arguments, rather than the library's 64 KiB
configuration limit. Malformed or excessively nested JSON is a fixed refusal.

The fresh plan binds the original artifact and closure. Readback compares every
copied input, wheelhouse file, retained non-RECORD wheel payload, selected
distribution and launch field. It checks the venv configuration and interpreter
links against the host-selected resolved runtime path. That path binding does
not establish runtime executable bytes, compatibility or prior probe results.
Archive shadowing checks use the verifier's own CPython 3.11 standard-library
metadata; they do not probe the selected runtime. Unexpected import/startup
files, links, special files, changed selections, altered launch bindings and
incomplete or crossed publication cause refusal. `ready` must contain exactly
`installed.json` and `mcp.json`, with no `pending-ready` sibling.

The result scope is `retained_payload_and_launch_binding_only`. Historical
bootstrap preservation and runtime claims are excluded. The fixed bootstrap
categories remain outside the payload proof. Generated-file hashes are compared
only for consistency with the mutable receipt: changing both those files and
their recorded hashes cannot prove provenance or safety. Receipt fields never
select file exemptions or establish authorization. The host must retain exclusive
custody of the originals, selected executables and destination while observing;
no-follow reads are not an atomic snapshot or a defense against a concurrent
malicious writer. A success does not establish protocol health, authentication,
operation permission or readiness. A failure leaves the existing destination
unchanged and uncertain; the command does not retry installation.

## Validate the canonical selection

The validator reads the package and exactly one selected document. Full activation:

```sh
/ABS/toolkit-python-plan \
  --package /ABS/selected/package.json \
  --selection /ABS/selected/activation.json \
  --component python-server
```

Plain software uses the same executable and component with the exclusive flag:

```sh
/ABS/toolkit-python-plan \
  --package /ABS/selected/package.json \
  --declaration /ABS/selected/declaration.json \
  --component python-server
```

The caller must select and trust the executable, freeze the input documents and
pin their raw bytes independently. It must invoke the executable on those frozen
inputs on every installation. Saved plan JSON is a projection, not authorization
or a reusable validation receipt. The CLI emits one JSON object and reads no
credentials, environment configuration, wheel contents or network resources.
Errors contain fixed codes rather than document values or paths.

Each document is limited to 1 MiB. Paths must be absolute and name regular files;
the Unix read refuses a final-component symlink. The operator must control the
parent directories. This is not a race-free filesystem sandbox.

## Selected input layout

```text
selected/
  package.json           # Existing IntegrationPackage
  activation.json        # Existing ActivationManifest
  artifact.whl           # Exact original primary wheel bytes
  closure/
    requirements         # Exact pinned requirements bytes
    dependency-1.0-py3-none-any.whl
    ...                  # Exact original dependency wheel bytes
```

For plain software, replace `activation.json` in this layout with fixed
`declaration.json`; do not supply both forms. Artifact and closure paths are
unchanged. `package.json` describes the Toolkit package, not Python packaging metadata.
The selected component's `Artifact` source owns all installation pins:

- `content_digest` binds `artifact.whl`.
- The final URL path segment supplies the original primary wheel filename. It
  must be a safe flat ASCII filename ending in `.whl`, at most 192 bytes long.
  The installer restores that name when supplying the unchanged wheel to pip.
- `PinnedClosure.artifacts` binds `requirements` and every dependency wheel by
  original filename and digest. At most 256 entries are accepted, including
  `requirements`. Case aliases and a dependency alias of the primary wheel
  filename are refused.
- `Entrypoint.executable` supplies the relative `.py` path inside the primary
  wheel. Each path segment remains an ASCII Python identifier. Literal arguments
  remain unchanged.
- Without `Entrypoint.python_console_script`, the existing module mode uses
  `python -I -B -m`. For example, `package/server.py` projects to `package.server`.
  In this mode `__init__.py` cannot be selected and no console alias is inferred.
- With `python_console_script`, supply the exact case-sensitive name observed in
  the primary wheel's `dist-info/entry_points.txt` `console_scripts` section.
  The safe script-name profile uses ASCII letters, digits, `_`, `.`, and `-`,
  beginning with a letter, digit, or `_`. The metadata module must exactly match
  the module derived from `executable`; `package/__init__.py` means `package` in
  this mode. The source and every parent package must belong to the primary
  wheel, with no dependency namespace or extension/module shadowing.

Console mode uses canonical fixed `python -I -B -c` code with the module, dotted
callable, script name and literal arguments passed as separate argv values.
It imports the selected module, resolves the declared attributes, calls the
result with no explicit arguments and passes its return value to `sys.exit`.
The callable receives the declared arguments through `sys.argv`. Installation
and readback parse metadata without importing it, so callable existence and
runtime compatibility still require the actual consuming journey.

Generated venv wrappers are not launch authority. Readback independently derives
this exact launch from pinned wheel metadata and the selected document, even
when a generated wrapper and its recorded hash change together. `gui_scripts`
cannot satisfy a console selection. Selected entry-point extras are unsupported
and refused, even if a similarly named dependency is present; requirements are
never silently ignored or inferred. Legacy module mode does not select or
promise usability of generated scripts. Other runtime profiles and generic raw
artifact preparation refuse this Python-only selector.

## Validation and installation responsibilities

For full activation, the canonical manifest validator first checks the package
binding and activation selection. The Python profile then requires one independent local Python
component, one service connector export and one portable MCP entry naming that
component. It requires `LocalMcp`, an artifact with `PinnedClosure`, and a Request
or Deployment lifetime. It refuses agent frontends, event routes, externally
managed components and compositions that need another component or Toolkit
activation contracts.

The JSON has the same fields as `NodeInstallPlan`, plus `artifact_filename` and
`module`: schema version, package identity, canonical package digest and the
selected form's canonical identity from the table above, strategy, component,
export, artifact digest, complete closure,
entrypoint and portable MCP name. These fields are derived from validated input;
`PythonInstallPlan` intentionally has no deserializer.

The installer checks wheel archive safety, original filename/metadata agreement,
wheel tags, exact requirement hashes, entrypoint ownership and import shadowing
before pip installation. The current profile refuses `.data` relocations,
startup hooks such as `.pth` and `sitecustomize`, retained bytecode, ambiguous
package/module/binary siblings, and replacement of interpreter or pip bootstrap
modules. Dependencies must be original wheels; source builds and network
resolution are unsupported.

After ordinary offline pip, it verifies every retained non-RECORD wheel payload,
the unchanged fresh-venv bootstrap, the exact installed file graph and the
distribution versions. Pip's rewritten `RECORD` and generated entrypoint scripts
are recorded by hash, not claimed as retained wheel bodies. Module mode uses
the explicit derived module; console mode uses the selected metadata callable
through fixed code and refuses selected extras. Neither mode launches a generated
wrapper or establishes runtime usability. A valid module-shaped path alone does not prove
that the wheel contains it or that importing it is safe. The plan owner neither
opens wheels nor executes package code.

The concrete Git proof retains 33 selected wheels and observes two bootstrap
distributions (`pip==26.0.1`, `setuptools==82.0.0`), for 35 installed distributions.
Each run verifies 996 retained payload files and preserves 1,646 bootstrap files;
107 pip-generated file hashes are recorded separately. The first attempt stopped
before pip on a legitimate setuptools template filename; the retained correction
allows literal parentheses in the existing safe relative-path checks.

Installation and a client configuration do not prove account authentication,
operation admission, readiness or host lifecycle integration. Request and
Deployment on the full activation path remain declarations until their owners
fulfill them. A generic MCP client receives the whole endpoint, including tools
not selected in either document form, and owns the child environment, launch and joined shutdown. The host
must explicitly admit those effects and any external executable prerequisites.

Connect's native protected attachment can separately enforce an approved subset
of exact MCP tool names through the host's current grant and MCP access
configuration. Separate write tools do not, by themselves, rule out that
protected read subset. Metadata and installation do not create this authority.
An operation-name allowlist does not constrain query semantics, prevent startup
effects or provide filesystem confinement. Inspect the allowed tools' arguments
and implementation, server startup behavior and separately supplied resource
controls. See [operation selection and runtime enforcement](../../sdk/consumer-setup/METADATA.md#operation-selection-and-runtime-enforcement).

For the Git server, Git itself is an external host-owned executable prerequisite,
with deliberate Git configuration and access to the selected local repository.
`--repository` supplies a repository choice; it is not an OS sandbox or a grant.
A declared `git_show` read does not remove the server's write tools. The example
uses no provider account or credentials, and neither native `find_and_install`
nor original-task continuation is supplied by this command. Native phase-2/C1
contracts and host-protected authentication presentation remain separate work.

The source fixtures under `tests/fixtures/python-install` are inert schema
examples with placeholder digests. They are not installable distributions or
evidence of platform support. Runtime and installed-package support must be
established separately for the selected original wheel and closure.
