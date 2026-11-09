# Acquire an agent-selected Python package

When a setup agent chooses an unfamiliar Python MCP server, the optional
`setup_acquire_python` tool downloads its wheel dependency closure and prepares
the existing selected-install layout. The agent decides what integration fits
the request. Standard pip resolves Python dependencies; Toolkit does not resolve
user intent or infer the package entrypoint.

## Host configuration

Enable the existing [selected Python installation](SELECTED.md) profile and
supply two additional setup-host arguments:

```text
--python-acquisition-profile /ABS/host/acquisition.json
--python-acquisition-profile-sha256 EXACT_PROFILE_SHA256
```

The profile is host input, never an argument supplied by the setup agent:

```json
{
  "schema_version": 1,
  "scope": "trusted_pypi_wheel_acquisition",
  "python_sha256": "SHA256_OF_THE_SELECTED_PYTHON_EXECUTABLE",
  "pip": {
    "path": "/ABS/trusted/pip.pyz",
    "sha256": "SHA256_OF_THE_TRUSTED_PIP_ZIPAPP"
  }
}
```

Paths must be absolute and canonical. Python is the same host-selected runtime
used by the installer. This profile supports native 64-bit CPython 3.11 on
macOS arm64 and Linux x86_64. To select Linux, add `"target": "linux-x86_64"`
to this profile and `"python_target": "linux-x86_64"` to the selected-install
profile. Both must agree before the workspace is created. Omission keeps the
macOS arm64 default; agents cannot override the host target. The actual worker
checks the interpreter before pip runs. Pip's standalone zipapp is host-supplied; obtain and verify it through the
host's trusted distribution process. The implementation has been exercised with
pip 26.2.1. The setup host freezes pip, the existing installer and the worker
source, and checks the pinned inputs before publication.

This scope explicitly permits trusted pip to contact public PyPI. It supplies no
credentials, proxy settings, custom indexes or arbitrary pip options. Pip runs
with a private home and temporary directory, disabled configuration files,
keyring, cache and version checks, and wheel-only dry-run resolution. It does
not install anything or execute a source build. Existing distributions in the
host environment do not participate in resolution.

The registry and pip remain trusted network participants. `--index-url` is not
an egress firewall. The setup code validates reported artifact URLs and fetches
only exact HTTPS `files.pythonhosted.org/packages/` URLs, with no redirects or
ambient proxy configuration. The trusted pip resolver contacts the index before
this validation. Hosts requiring enforced network isolation must provide it at
the process or network boundary.

## Agent flow

1. Research the user's request and choose an exact public PyPI distribution and
   normalized version. Inspect its supported operations and provenance.
2. Call `setup_acquire_python` with `request_id` (32 lowercase hexadecimal
   characters), a normalized distribution name such as `mcp-server-fetch`, and
   an exact version such as `2026.8.18`.
3. Observe the returned job through `setup_job` or `setup_state`. Success reports
   `scope: retained_python_wheel_inputs`, the `staged_id`, original wheel names,
   source URLs, versions, hashes and the acquisition receipt. The files are in
   that immediate child of the selected-install profile's staging root.
4. Inspect the actual primary wheel without importing it, using ordinary research
   tools or host-enabled [acquired archive inspection](ACQUIRED-INSPECTION.md).
   Author `package.json` and `activation.json` for inline selected-check submission
   or beside the retained `artifact.whl` and `closure/` files.
   Use the reported hashes and the canonical package inspector. The acquisition
   tool does not invent entrypoints, operations, policy or lifecycle owners.
5. Continue through `setup_selected_check`, review its concrete proposal, then
   call `setup_selected_install`. The existing validator and offline installer
   independently check the selection and wheel closure.

The same request ID and identical arguments return the original live-session
job, including a failed or interrupted job. Changed arguments with that ID are
refused, including reuse of that ID for Node acquisition. A deliberate new attempt uses a new request ID and destination; existing
staging is never overwritten. Replay does not recheck a previously returned
receipt, and retained bytes alone do not establish current installation readiness.

`setup_stop` requests cancellation through the existing job owner. Both pip
and each artifact download run in its supervised child-process path, so stop
and the total deadline also interrupt slow HTTP headers or chunk framing. Cancellation, process loss and
partial staging never become a successful job by inspecting directory existence.
A receipt is written last. An interrupted publication may retain partial files;
the next attempt uses a new directory. Recovery-only sessions cannot acquire.

## Bounds and meaning of success

The operation uses a 120-second deadline, enforced during network work by the
same supervised process owner, and at most 256 wheels, 64 MiB per wheel and 512 MiB total retained wheel
bytes. The existing wheel inspector checks archive contents, metadata, hashes,
startup hazards and duplicate paths without importing package code. The installer
later checks the complete selected module and dependency graph.

The pip worker has process CPU and file-size limits plus best-effort temporary
storage and memory monitoring. These are resource guards, not a hard memory or
filesystem sandbox; host isolation remains separate. No process-global security
policy is changed in the setup host.

An acquisition receipt proves which bytes were retained in this run. It grants
no account access, native capability, MCP attachment or original-task continuation.
The output remains useful independently of those upstream features.

## Recover retained inputs

Keep the returned `recovery_request` and original workspace. A new readback-only
host can check the final receipt and acquired files without downloading or
pulling again. Follow [acquisition recovery](ACQUISITION-RECOVERY.md). A matching
file check leaves the historical job outcome unchanged and supplies no execution
or account authority.
