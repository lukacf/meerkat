# Acquire an agent-selected MCPB

The agent researches a public bundle, chooses its exact HTTPS URL and expected
SHA-256, and calls `setup_acquire_mcpb`. The existing setup job retains matching
bytes. `setup_inspect_mcpb` then uses the canonical archive reader; ordinary
selected check and installation remain separate operations. No bundle code runs
during acquisition or inspection.

## Enable the host capability

Use a selected library containing the target-specific MCPB reader and installer
and declaring `mcpb-archive-inspection`. The maintained library packaging supports
macOS arm64 and Linux x86_64; older Linux editions omit this capability. Create a
nonsecret host profile with this exact shape, replacing the trusted Python pin:

```json
{
  "schema_version": 1,
  "scope": "trusted_public_mcpb_acquisition",
  "python": {"path": "/ABS/trusted/python3.11", "sha256": "TRUSTED_PYTHON_SHA256"},
  "allowed_origins": ["https://github.com"],
  "allowed_redirect_origins": ["https://release-assets.githubusercontent.com"]
}
```

The profile must select the same canonical Python executable as host preparation.
Origins are exact canonical HTTPS origins; no wildcards, path rules or implicit
parent-domain matches are accepted. An explicit nondefault port belongs to the
origin. These example origins are not a built-in catalog limit. The host may
select other sources after reviewing their trust and network requirements.

Add the following to the normal [host preparation](HOST-PREPARATION.md) or
[offline bootstrap](HOST-BOOTSTRAP.md) command, alongside the existing selected
Node path/hash and `--allow-mcpb --enable-mcpb-inspection`:

```text
--mcpb-acquisition-profile /ABS/host/mcpb-network.json
--mcpb-acquisition-profile-sha256 TRUSTED_PROFILE_SHA256
--enable-acquired-inspection
```

Preparation validates and retains the profile before emitting ordinary MCP
configuration. It does not download anything. Without explicit acquisition
configuration, the new download tool is absent. Linux archives carrying only
the Python compatibility code do not thereby gain MCPB inspection/installation.

## Acquire, inspect, install

Call `setup_acquire_mcpb` with an exact request:

```json
{
  "request_id": "0123456789abcdef0123456789abcdef",
  "url": "https://github.com/PUBLISHER/PROJECT/releases/download/VERSION/tool.mcpb",
  "artifact_sha256": "EXPECTED_ARTIFACT_SHA256"
}
```

The initial URL is public nonsecret metadata with no query, fragment or userinfo.
No agent-supplied headers, credentials, runtime options or output paths are
accepted. Poll the returned job using `setup_job`. On success its result contains
`staged_id`, `primary.sha256`, `receipt_sha256` and source observations. The scope
`retained_mcpb_artifact_bytes` proves matching retained bytes, not a valid package,
publisher identity, license clearance, installation or readiness.

Inspect that exact acquisition with `setup_inspect_mcpb`:

```json
{"job_id": "RETURNED_JOB_ID", "receipt_sha256": "RETURNED_RECEIPT_SHA256", "files": []}
```

`manifest.json` is always included. Additional explicit text files use the
existing reader bounds. The agent evaluates this untrusted evidence and authors
package, activation and configuration documents. Supply the acquired `staged_id`
to `setup_selected_check`, then use its exact proposal for the separately
admitted `setup_selected_install`. [MCPB installation](../../crates/toolkit-package/MCPB-INSTALL.md)
defines the supported runtime and configuration contract.

## Stops, recovery and transport limits

The download has a 120-second total deadline, a 64 MiB body limit and at most
five explicitly checked redirects. Socket waits are capped at ten seconds.
TLS verification is enabled; inherited proxy settings, cookies and caller
headers are not used. Signed redirect queries stay inside worker memory and
are never included in results, retained settings, logs or diagnostics. Failure
uses fixed error codes. Origins do not establish public resolved addresses;
the host's network controls own DNS and actual egress enforcement.

`setup_stop` uses the existing job cancellation owner. Partial evidence remains
after cancellation or failure. Exact request replay within the same live
controller returns its original job; a changed URL/hash or cross-kind reuse
refuses. There is no automatic retry or alternate source selection.

Retain the job's `recovery_request` and workspace under exclusive host custody.
The existing [acquisition recovery](ACQUISITION-RECOVERY.md) startup can reconcile
the original request, frozen inputs, receipt and staged bytes without network or
code execution. To inspect after restart, the host must additionally select the
same current selected-profile hash and enable `--allow-acquired-inspection`.
First call `setup_reconcile`; only a current matched observation permits the
job-bound MCPB inspection form. Every read checks the evidence before and after
the current pinned canonical reader. Recovery never exposes arbitrary staged
inspection, download or installation, and historical job outcome remains
unconfirmed. These point-in-time checks require quiescent host custody; they are
not an atomic snapshot against concurrent writers.
