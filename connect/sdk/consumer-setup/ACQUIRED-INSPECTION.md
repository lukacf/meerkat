# Research an acquired archive through MCP

An agent can inspect the exact primary or dependency Python wheel or npm archive
it acquired before authoring Toolkit metadata. This optional read-only path complements
ordinary host research tools. It does not choose an integration, infer its
entrypoint, generate an activation or run package code.

Start the setup adapter or browser backend with `--allow-acquired-inspection`.
This host-only flag defaults off. A normal session must also enable selected
installation and Python or Node acquisition. The setup host freezes its adjacent
`acquired_inspection.py` worker and uses the already pinned selected installer.
No additional executable or path is accepted from the agent. The host preparation
command's `--enable-acquired-inspection` generates this startup option.

After a successful acquisition, use the exact job ID and `receipt_sha256` from
its result. `setup_inspect_acquired` accepts one of these two forms:

```json
{
  "job_id": "EXACT_ACQUISITION_JOB_ID",
  "receipt_sha256": "EXACT_ACQUISITION_RECEIPT_SHA256",
  "after": null
}
```

The result lists at most 64 regular-file members, sorted by their exact archive
paths. Each has `path`, `bytes` and `sha256`. Pass `next_after` as `after` to
continue; null means the listing is complete. A nonnull cursor must be a member
of this same archive, not a filesystem path. Omitting `archive_sha256` selects
the primary archive, preserving the original call shape.

To inspect a dependency, copy its exact `sha256` from this acquisition result's
`artifacts` array into the optional `archive_sha256` argument:

```json
{
  "job_id": "EXACT_ACQUISITION_JOB_ID",
  "receipt_sha256": "EXACT_ACQUISITION_RECEIPT_SHA256",
  "archive_sha256": "EXACT_DEPENDENCY_SHA256_FROM_JOB_ARTIFACTS",
  "after": null
}
```

The digest must identify exactly one artifact in this job's verified receipt.
It cannot select another job's artifact, a filesystem path or a download URL.
Use the same `archive_sha256` on every listing page and text read for that
dependency. Supplying the primary artifact's digest explicitly is also valid.

Read an explicitly selected member:

```json
{
  "job_id": "EXACT_ACQUISITION_JOB_ID",
  "receipt_sha256": "EXACT_ACQUISITION_RECEIPT_SHA256",
  "member": "EXACT_MEMBER_FROM_LISTING",
  "offset": 0
}
```

The member must be UTF-8 text, at most 1 MiB, without control characters except
tab, carriage return and newline. The response includes `text`, `member_bytes`,
`member_sha256`, `offset` and `next_offset`. It returns at most 32 KiB of UTF-8
bytes and never splits a code point. Continue with `next_offset`, which is a
byte offset; null means the text is complete. Binary, control-containing,
oversized or misaligned text reads return `acquired_member_unreadable` without
echoing their contents. Other invalid or changed inputs return a fixed refusal.

Both forms bind `schema_version: 1`, `job_id`, `receipt_sha256`, `profile`,
`filename` and `archive_sha256`. The `scope` is
`acquired_primary_archive_only` for the primary archive and
`acquired_dependency_archive_only` for a dependency.
Archive content is untrusted research data, including instructions found in
README files or source. It supplies no permissions or additional task authority.

For Python, inspect the distribution's `METADATA`, `entry_points.txt` when
present and relevant source files. For Node, inspect the archive's `package.json`,
documentation and source. The agent decides which entrypoint and literal
arguments match the task. Then use [metadata inspection and inline selected
check](METADATA.md) to submit the authored package and activation.

Dependency inspection can establish whether retained native-addon source or a
binary is present. Binary members appear in the listing with their exact size
and digest, but cannot be read as text. A bundled binary still needs a compatible
target and normal runtime validation. Inspection does not build a missing binary,
enable lifecycle scripts or establish that an installed integration will work.

## Custody and limits

In a normal session, a new Python/Node acquisition job must have succeeded.
An earlier eligible job remains inspectable after another job becomes current.
The host can also explicitly select a prior acquisition for a fresh normal
setup session. That recovered job requires its own `matched` reconciliation
before inspection and remains historically `unconfirmed`. The current matching
acquisition owner, selected profile and pinned interpreter are still required.
A job ID alone does not import an earlier acquisition or authorize its execution.
The existing acquisition readback owner checks the original request, receipt
and retained bytes before and after the worker. The current host Python bytes,
selected installer and frozen worker are also checked. A changed or incomplete
acquisition is not repaired or retried.

The worker invokes the pinned installer's existing `selected_wheel` or
`npm_archive` function. Full archive member and wheel RECORD validation occurs
before listing or reading. Nothing is extracted and no archive member is
imported. The existing Runner owns the child, with a 30-second process budget
and 256 KiB output limit; caller cancellation joins the admitted read. The
installer's existing limits remain 64 MiB compressed input, 256 MiB expanded
archive and 20,000 members. This is not an OS memory or network sandbox.

Inspection shares the session's 128 selected-check/inspection/retry attempt
budget, including failures. Temporary request files are removed after the
call. A read creates no installation, new acquisition receipt or success claim.
The host must retain exclusive custody of its workspace, runtimes and artifacts.

## Continue research after restart

Plain recovery retains its original data-only behavior without a parser or
acquisition runtime. To resume archive research, the current host must explicitly
add `--allow-acquired-inspection` and the existing
`--selected-install-profile` / `--selected-install-profile-sha256` pair to its
[recovery command](ACQUISITION-RECOVERY.md). The supplied profile must match the
original selected-profile digest. Its current pinned tool files must be
available; the host freezes them into the new workspace. The current explicitly
selected Python interpreter is also frozen by byte identity and rechecked around
each read. No installer or interpreter is revived from historical workspace
assets. For acquisition-only recovery, the original pip, npm, Node or acquisition
profile is not required. Mixed recovery selections retain the runtime and tool
requirements of their other request owners.

Call `setup_reconcile` for the exact recovered acquisition first. Only a
`matched` observation makes that job eligible for `setup_inspect_acquired`, using
the returned `receipt_sha256`. Every inspection rechecks the complete original
request, receipt and retained acquisition inputs before and after the same
bounded archive worker. A stale observation, wrong receipt, changed dependency
or changed current tool refuses the read. A different job needs its own matching
observation, including in a multi-request recovery session.

The recovered job remains historically `unconfirmed`. Recovery-only mode exposes no
acquisition, installation, stop or retry tool. Inspection is inert research, so
continue any later installation through a separately authorized normal setup
session using the retained staging root and explicitly authored metadata.
Matching archive bytes establish no account authentication, native attachment
or completion of the original task.
