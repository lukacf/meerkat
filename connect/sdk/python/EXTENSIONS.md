# Installed custom extension artifacts

The optional `meerkat-toolkit[extensions]` SDK surface verifies a host-selected
Python extension and its selected dependency wheels before importing its
entrypoint. A standalone host can use it without the paired host. It does not install
software or select packages, and it does not change the MCP installer.

The canonical package inspector remains responsible for package meaning,
compatibility and required host-service contracts. The application supplies
the resulting exact wheel paths, SHA-256 digests, entrypoint and service names.
Dependency metadata is checked against that concrete selection and the current
interpreter; this code does not discover or resolve missing dependencies.

```python
from pathlib import Path
from meerkat_toolkit.extension_artifacts import (
    ExtensionArtifacts, SelectedWheel, load_extension_entrypoint,
)
from meerkat_toolkit.host_services import StagedHostServices

# Values below come from the host's validated package selection.
artifacts = ExtensionArtifacts(
    primary=SelectedWheel(Path(selected_wheel), selected_sha256),
    dependencies=tuple(SelectedWheel(Path(path), digest)
                       for path, digest in selected_dependencies),
)
services = StagedHostServices(declared=declared_services,
                              available=available_services)
services.bind(scoped_handles)
entry = load_extension_entrypoint(
    artifacts, site_packages=Path(selected_site_packages),
    entrypoint=selected_entrypoint, services=services,
    required_services=required_services,
)
# The host now decides whether and how to instantiate and activate entry.
```

`load_extension_entrypoint` returns the selected Python object without invoking
it. A missing required service, unbound service view, altered wheel, mismatched
installed payload/RECORD, incomplete dependency closure or conflicting import
origin raises `ExtensionArtifactError` before the selected entrypoint import.
`verify_extension_artifacts` performs the same software checks without importing
the extension and returns an `ExtensionArtifactObservation`. That observation is
software identity, not authorization, readiness or a durable attestation.
Its `primary_dist_info`, sorted `primary_files` (relative path/SHA-256 pairs,
excluding RECORD) and exact `primary_requirements` let a host compare its
release and contract records with the checked artifact without parsing the
wheel again. These immutable values describe the observed bytes, not a promise
that files cannot change after verification. Hosts with staged handles can use
this verification-only seam before binding services and retain their existing
construction, activation and lifecycle order.

## Supported profile and limits

This initial profile accepts up to 256 explicitly selected `py3-none-any` wheels
with regular Python modules/packages and a closed dependency graph. It checks
the current interpreter's version and dependency markers. Wheel filenames,
metadata, RECORD, retained digest, installed payload and import ownership must
agree. Files are bounded to 64 MiB; both retained wheel bytes and expanded
payload bytes are independently bounded to 256 MiB for the selected set.

The installed directory must be an existing canonical absolute path already
on the interpreter's import path. The loader does not modify `sys.path`.
Ordinary pip and uv bookkeeping is supported. Generated current-interpreter
bytecode is cache, not identity: only paths derived from verified source may
appear as unhashed pip cache rows, and physical caches are removed before import.
The immutable source and metadata, including RECORD, are not rewritten.

Native wheels, namespace-only packages, extras, direct-URL dependency edges,
startup hooks, `.pth`, `.data`, precompiled wheel bytecode, shared import roots
and extensions replacing host/runtime modules are not supported. Dependencies
must be selected wheels, not implicit imports from the host environment.
Host API imports remain an explicit host contract. The private paired host
candidate now verifies the actual robot extension, which declares no
third-party dependencies, while the paired host provides its API. Robotd's separate
native dependency graph stays outside this verifier. Neither the installed
extension checks nor synthetic facades establish a completed the robot or
The paired host migration.

## Trust and ownership

This is a before-import software check in a trusted interpreter. The host owns
its process, `sys.path`, `sys.meta_path`, path hooks, existing `sys.modules`
objects and import machinery. A malicious custom import hook or code already
running in that process can bypass normal Python import semantics. Origin
checks on already loaded modules do not attest their in-memory code or undo
prior execution. Concurrent filesystem mutation by the same OS account is
outside this guarantee. The verifier is not a sandbox or continuous monitor.

The host must serialize install/update/import as appropriate and supply scoped
service handles that enforce their own authority and lifetime. Required service
availability is checked, but the loader cannot authorize or revoke a handle.
Activation, admission, audiences, routing, cleanup, readiness and revocation
remain with the application and Meerkat/MobKit owners. Service connectors and
agent frontends can share these artifact checks without sharing their roles.

## The paired host migration boundary and provenance

The paired host can delegate artifact and import checks here after its existing
release/lock, selected activation, contract and policy validation. It must retain
those host decisions and its runtime lifecycle. An explicit candidate adapter
and source-only migration notes accompany the verification evidence; canonical
The paired host and the earlier frozen acceptance snapshots are unchanged.

The extracted checks originate in the private accepted the paired host candidate at
`docs/verification/20261003-host-service-dependencies/author/snapshot/src/paired_host/extensions/artifacts.py`,
SHA-256 `0d134552e9fc0cd4abf04a098a432c10f6a0c9ea46bacdbb7398c89924229841`.
The two extracted Toolkit modules are licensed under MIT OR Apache-2.0 as
stated in `LICENSE-EXTRACTED`. The original paired host project remains unchanged.
The separate `packaging>=25.0` dependency follows the paired host's existing declared
minimum; this candidate's tests run with packaging 26.3. No packaging source is
copied into these modules.
