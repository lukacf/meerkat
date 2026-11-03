# Inspect several retained setup requests after restart

A recovery manifest opens selected requests from one original workspace in a
fresh readback-only session. For example, inspect separate Slack and Gmail
installations together without repeating either installation. The host selects
the requests; the setup service does not scan for work or infer success from
directories and receipts.

This is a current source capability. Use a packaged edition only if it explicitly
includes manifest recovery; older frozen archives are unchanged. The existing
[single-request options](README.md#recover-one-request-after-a-host-restart) keep
their original behavior.

## Select exact requests

In a current setup session, select each job you want to retain with **Add to
recovery file**, then choose **Save recovery file**. The browser downloads
`toolkit-recovery.json` using the exact bytes returned by the setup host. It
does not download installed software or credential values. Keep the original
workspace and its installed/acquired files at their existing paths.

An agent can obtain the same file without a shell:

```json
{"job_ids":["11111111111111111111111111111111","22222222222222222222222222222222"]}
```

Pass this object to `setup_recovery_manifest`. Replace the example IDs with
explicitly chosen job IDs from `setup_job` or `setup_jobs`. The response contains
`text`, `bytes`, `sha256`, the fixed filename and media type, and scope
`retained_setup_requests_only`. Save `text` as UTF-8 exactly, without formatting
it again; use the returned `sha256` as the manifest digest. A host may save it
on the agent's behalf. The tool itself writes no files.

Only jobs with a durably retained recovery request are eligible. This includes
running jobs whose request has already been committed, and recovered jobs whose
original workspace remains available. Export is a bookmark, not evidence that
work has settled or that any output exists. Unknown or unanchored jobs, duplicate
IDs and missing or changed request files refuse the entire export. No selected
job is silently omitted. The tool cannot accept another workspace or a new hash
to replace an original request. Readback still uses each request's existing
owner and current host bindings.

For an older host without this operation, retain each job's
`recovery_request.id` and `recovery_request.sha256`, plus its original workspace,
and create a UTF-8 JSON manifest with exactly these fields:

```json
{
  "schema_version": 1,
  "workspace": "/canonical/original/workspace",
  "requests": [
    {
      "id": "11111111111111111111111111111111",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    {
      "id": "22222222222222222222222222222222",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  ]
}
```

The example IDs and hashes are placeholders. Replace them with the exact retained
anchors. The manifest is limited to 64 KiB and 1-256 requests. IDs must be unique
32-character lowercase hexadecimal strings; hashes must be 64-character
lowercase hexadecimal SHA-256 values. Unknown fields are refused. `workspace`
must be one canonical absolute path without symlink aliases. A digest supplied
by the host pins its selection; a hash beside an untrusted file does not confer
trust.

Establish that previous workers are quiescent and keep their files under exclusive
host custody. This mode does not discover or terminate previous processes. Start
`backend.py` or `mcp_adapter.py` with the required existing host inputs and add:

```text
--workspace /absolute/new/readback-session
--recover-manifest /absolute/host-selected/recovery.json
--recover-manifest-sha256 HOST_PINNED_MANIFEST_SHA256
```

Both manifest options are required together. They cannot be combined with any
`--recover-workspace`, `--recover-request`, `--recover-request-sha256`,
`--prior-request` or `--prior-request-sha256` option. The new workspace must be
disjoint from the original workspace and all selected acquisition staging roots.

Each request uses its existing recovery owner and current host binding checks:
the enabled families, bundle, runtimes and selected profile or recipe pins must
be compatible. Library installations still require an explicitly pinned
verifier. Compatible library/official installations, selected Python/Node/MCPB
installations, hosted-recipe exports and Python/Node/Compose acquisitions can
share the selection. Repeated acquisitions of one kind must have matching frozen
host and staging identities. [Acquisition readback](ACQUISITION-RECOVERY.md)
continues to read frozen tools as data and needs no original pip, npm or engine
executable. This does not add recovery of Compose execution graphs.

The host validates every selected request envelope before creating the new
workspace, then retains the exact manifest as `recovery-manifest.json`. An
unsupported mix, missing request or changed binding refuses startup. Output
payloads are checked separately on explicit reconciliation; an unconfirmed
payload check for one job does not change another job's observation.

## Read each result independently

The session reports `mode: "recovery"` and
`job_history.recovery_selection: true`. Every imported job has
`status: "unconfirmed"`, `error: "historical_outcome_unknown"`, `recovered: true`
and its original `recovery_request` anchor. No old success result, validation ID,
prepared environment or observation is restored.

The MCP adapter exposes these base tools:

| Tool | Behavior |
| --- | --- |
| `setup_state` | Read session state and the last selected manifest entry. That entry is not necessarily the newest original job. |
| `setup_jobs` | List up to 50 summaries in reverse manifest order. Start with `{"before": null}` and pass `next_before` as `before` for the next page. |
| `setup_job` | Read one selected `job_id`, its retained anchor and its separate current observation. |
| `setup_reconcile` | Explicitly check one selected `job_id` through its existing recovery owner. |
| `setup_recovery_manifest` | Export explicitly selected retained requests as an exact recovery file, still referring to their original workspace. |

An explicitly opted-in [acquired-archive inspection](ACQUIRED-INSPECTION.md#continue-research-after-restart)
host also exposes `setup_inspect_acquired` for selected Python/Node acquisitions.
Each job needs its own matching reconciliation and receipt; an observation for
one job never admits another. This requires current pinned inspection tools and
does not restore any acquisition or installation authority. Selected installation
recovery may separately expose `setup_plugin_source` when its host profile
enables the existing retained-plugin-source read.

The browser offers the same selected-job history and per-job check. Opening or
selecting a job does not start reconciliation. Reconciliation rechecks the exact
request anchor and retains its existing scope, such as
`retained_payload_and_launch_binding_only` or `retained_acquisition_files_only`.
It does not rewrite historical status. After a cancelled wait, read that exact
job before requesting another check.

There is no aggregate readiness result and no setup, stop, retry or automatic
resumption. A matching file observation does not establish account access,
native attachment or original-task continuation. Continuing setup requires a
separate normal session and the existing host-owned admission path.
