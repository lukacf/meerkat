# Forge native service host composition

This application binds Forge's compiled GHE programs to Connect's real
`CollectionProgram` and `CommittedCollectionResult` interfaces. The Linux
`forge-service-host` executable and catalog are implemented as source, but have
not been compiled or installed. They use Connect's reviewed protected entry,
closed mapping and original retirement owner. No installed path or artifact
identity is assigned here. Connect's ordinary `--stdio` and acceptance examples
do not replace this application entry.

## Protected entry contract

The source-defined command is:

```text
forge-service-host --connect-protected-service --parent-fd N --mapping-fd M
```

This is not a released command. The root launcher transfers original protocol
stdin at FD 0 and stdout at FD 1. FD 2 is reserved. N and M are distinct open
descriptors >= 3, written in canonical decimal. The exact option order is
required; unknown, duplicate, missing, non-UTF-8 or extra arguments refuse.
The private startup boundary verifies all four raw descriptors are open,
adds close-on-exec and transfers each into `OwnedFd` exactly once. It never
duplicates descriptors, initializes standard I/O wrappers, reopens a path or
creates replacement pipes. The boundary is main's first application action;
a process-wide guard prevents a second acquisition even after failure. Unsafe
code is limited to this private binary module; the library still forbids it.

Immediately afterward `ProtectedServiceInputs::adopt` verifies the actual
objects, parent, sealed mapping, identity and installation. Only then does main
create a Tokio runtime, start the catalog and join the retained owner. Errors
exit nonzero without reflecting private arguments or native error bodies.
Non-Linux entry refuses without fallback. Root still owns pre-exec and helper
isolation, controlled loader environment, namespaces, original reservation,
process/descendant custody and joined I/O. Descriptor privacy is not isolation.

The exact compiled mapping selection is:

| Field | Value |
| --- | --- |
| `application.application_id` | `meerkat-forge` |
| `source_selection` | `forge.ghe-collections.v1` |
| `catalog_id` | `forge.ghe-collections.v1` |
| collection `family` | `official-github` |
| collection `profile` | `enterprise-selected-reads` |
| collection `agent` | `forge-ingestion` |

These are behavior selectors, not authenticated principals or installed
receipts. Source/requirement IDs and original resources remain separate. The
catalog rejects a mismatched selection/profile/agent, an unknown program, extra
or duplicate operations, or program limits exceeding the original source's
shared limits. It performs no callback I/O or native admission. Actual native
commissioning, endpoint/account checks and original caller authority remain
required. Other service families need their own compiled adapters.

Two application integration gates remain explicit: the public Python SDK must
adopt root-launched original pipes without spawning a replacement or taking
root process custody; the original host must consume the retained historical
archive alongside the matching SDK-validated final collection report. Archive
writing is implemented below, but the native entry and host consumer have not
been qualified together. Exit status alone is not durable ingestion.

## Retain historical data after original close

The catalog retains the original validated `CatalogSelection::state_root` once,
without callback I/O, and refuses a different root. After the same native owner
joins, main archives completed parser rows even when the final native selection
recheck or close failed. It does not wait for a fabricated success report.

The private path is:

```text
<original state_root>/forge-ghe-history-v1/<sha256(native instance ID as UTF-8)>.json
```

The native instance ID is the original authenticated `HELLO` invocation from
Connect's protected parent handshake. The root SDK binding must use that exact
invocation, not the workspace ID or the provider-setup interaction's instance.
Reusing a workspace/state root therefore does not reuse an archive filename.

Schema `forge.ghe.historical-data.v1` includes `evidence_kind` equal to
`historical_data_only`, the full `native_instance_id`, and `rows`. Each row keeps
`source_id`, `requirement_id`, exact `resource`, original native `context`, the
structured typed `collection`, and `observations`. Observations retain each
operation, arguments, serialized native audit, exact payload string and optional
structured companion. Provider failures, missing fields and capped metadata
remain data. No native handle or Ready object is serialized or reconstructed.
Every row must have the same actual native instance. Empty history writes no
archive. The in-memory store clears rows only after a successful durable write;
its original replay keys remain.

The independent `forge-history-archive` writer bounds encoding to 256 MiB, uses
exclusive pending files and a non-overwriting final link, and syncs file and
directory metadata. Files are `0400` in a `0700` directory. A failed write keeps
any pending bytes already created and makes the child exit nonzero; it does not erase an actual
native close acknowledgement or imply that close remains running. Root must
retain both native lifecycle facts and the separate retention failure.

Root must wait for actual child exit before collecting the final archive. Match
the full instance, application/login/viewer/origin/realm/session/job/server/agent
context, original source and requirement IDs, exact resources and actual call
observations against the matching original SDK-validated report. Retain failure
and partial reports too. Neither the file, its locator/hash, a request ID nor an
exit code grants source authority or proves ingestion/admission. Pending files
are never a final archive. Core/Meta still owns governed observation admission.
The root's lifetime filesystem isolation must cover this archival phase as well
as native helpers; this code does not create that isolation or modify provider
credential roots.

`launchkit.service_collection_history.match_history` now supplies the Forge
consumer's data-matching step. After the original SDK process join, the root
owner reads the original bounded archive and passes those bytes (or `None` if
absent), the original `ServiceSetupBinding`, a tuple of retained SDK
`event.report` objects, and the original `ServiceSetupClose`. It compares the
full binding/context, source/requirement/resource, operation, canonical argument
digest and serialized native audit. It refuses duplicate rows, missing verified
rows, changed native identities and unknown archive fields. Distinct native
final reads with equal operation/arguments remain distinct observations.

The result retains immutable parser data with the original reports and close
object, including failed native reports and nonzero process exits. It neither
opens a path nor reconstructs reports from JSON. It does not independently
recompute the full native result digest from an extracted text payload or redo
the Rust parser's interpretation. The original protected file and native report
remain required evidence. Root custody, actual native acceptance and governed
admission remain separate. The matching controls use synthetic SDK data only.

## Current source

`GheProgram` composes immutable selection intent before commissioning from a
compiled `ProgramSpec`, its repository and finite limits. It creates a native
requirement without an installed connection or account. Both `accepts_target`
and `start` validate the actual target's IDs, resource, operation ceiling and
endpoint/account/operation configuration. Each successful `start` captures the
actual `CollectionTarget::installed_connection()` in a fresh `BoundGheProgram`;
unsupported input produces only refusal. No installed configuration is
reconstructed from mapping text or cached across attempts.

`BoundGheProgram` preserves the caller's original source and requirement IDs.
It compares every actual target's source, requirement, resource,
raw operation ceiling and complete installed descriptor with that selection.
The descriptor must name an absolute executable, the exact enterprise host,
read-only mode, disabled default toolsets and an explicit matching native access
ceiling. Plain environment overrides and a false `account = none` declaration
are refused. A PAT slot is an expectation, never verified account evidence.
The protected owner must separately establish executable identity, native realm,
account/credential scope, environment policy and original caller authority.

Each `start` creates a fresh cursor. It accepts only actual committed native
results with the exact pending operation and arguments, maps content variants
directly through the pinned decoder, and keeps the original native context
constant throughout the sequence. A refusal ends that cursor. No arbitrary JSON
program, receipt, authenticated caller, native Ready object or replacement login
is constructed. The adapter does not dispatch, run a subprocess, log in, retry,
refresh a host or publish anything.

The catalog covers PR statuses, review threads, changed files/commits, ordinary
comments/reviews, bounded commit history, workflow runs/jobs, issue discovery,
issue comments, direct issue hierarchy and PR discovery. `PullRequestChanges`,
`WorkflowRunJobs`, `IssueComments` and `IssueHierarchy` request native read-only enforcement for their initial detail
call and retain the opaque read observation returned by the real committed,
joined result. After the intervening pages, the exact final detail read consumes
that handle through `ReobserveRead`. No replacement arguments or requirement
are supplied. A missing handle refuses; it never falls back to a normal duplicate
call. The final body passes through the same parser checks and historical audit
retention. Neither a parser result nor serialized history can provide a handle.
This source integration is uncompiled and has no native acceptance result.
`ProgramSpec::from_resource` accepts only these registered program IDs and the
exact resource shape emitted by the catalog, including selector and budget
fields. It runs the existing pure constructor checks and requires an exact
resource round trip. Unknown or ignored fields, executable commands and native
readiness flags are refused. This decodes selection intent only; the native
owner still supplies the actual target, original caller and final report.
Repository metadata and revision-bearing historical check support remain separate inventory
requirements.

`HistoricalStore` retains completed parser output together with each exact call,
original payload, structured companion and serialized native audit observation.
The store bounds retained call/result/audit bytes to 32 MiB and retains up to 256 original
context keys, including after rows are taken. It never silently overwrites an
observation. These are historical data: Connect still rechecks the selection
after the completion callback and can refuse the overall source. Taking rows
cannot substitute for that final native report, original cleanup ownership or
credential-continuity evidence. The application must retain both the final
native source report and these correlated historical data when composing actual
ingestion. Retain the report from the matching protocol7 SDK-validated collection
event, including partial/failure reports. The request ID alone is not completion;
`ProtectedServiceRun::join` proves owner close completion, not a source report or
durable storage receipt. A native `ServiceCollectionFailure` also retains its
actual reports and original error without proving delivery over a broken pipe.
No data-export protocol or successful ingestion claim is added here.

## Dependency and verification limits

The manifest pins Connect `2eef4a3684a45cd6c51dcbec4515400ad6a86d3f` and uses the
same Core development patch graph as that commit's native workspace. It stays
separate from the pure serde-only GHE crate and the released Forge runtime.
This revision includes the reviewed protected entry, bounded diagnostics and
public original-pipe SDK adoption. Its Python and TypeScript report parsers
accept repeated reads with distinct native observations while rejecting replay.
The original SDK regression and SDK boundary suites pass; those results do not
qualify the native adapter. The existing native validation lane remains the
owner of compilation and platform-specific acceptance.
The application selects Connect's default library feature set. The first
portable consumer check failed in Connect because unconditional setup code
referenced implementation types gated behind its `console` feature. Connect's
owner is correcting those feature boundaries; the corrected dependency source
and compiler validation remain pending.
The owner's private candidate has a reviewed lockfile; this source crate has no
committed lockfile or successful native build result. Formatting and manifest
parsing do not typecheck the APIs or execute controls.

Fourteen native-crate controls cover invalid target selectors, configuration
and account declaration refusal, operation ceilings, the compiled catalog and
closed resource decoder, factory composition without a configuration, separate
attempt configuration retention, repeated `begin`, compiled catalog selectors
and ceilings, and closed descriptor argument parsing. They remain unrun.
Linux ownership controls must also prove exact descriptor numbers, no duplicate
FD 0/1 owners, CLOEXEC before helpers, closed/aliased/wrong-mode object refusal,
and joined original pipe closure. Parser controls do not establish any of these
kernel or lifecycle facts. The original process-entry unsafe boundary still
needs actual Linux qualification on the existing lane.
The original owner must also exercise real factory target rejection in both
`accepts_target` and `start`, distinct starts with different installed descriptors,
and refusal of changed targets within an existing cursor, as well as real
committed-result delivery, target/context changes, parser errors and storage
limits through the actual native engine. Tests must not fabricate
`CommittedCollectionResult` or replace native authority with fixture structs.

For each of the four programs with a final detail read, the existing owner
must also prove an actual first read, intervening page and final reobservation:
distinct native input/run/call identities with equal operation and arguments,
both committed and joined, and a changed final body causing the parser's
existing refusal. Missing handles, native classification refusal, cancellation,
changed target/configuration, budget exhaustion and a consumed handle must not
dispatch a replacement or leave a retryable cursor. These controls are pending
on the existing native qualification lane; no native objects are fabricated here.

The pure program crate's retained 105 controls, 11 additional hierarchy controls
and three serialization controls remain independent evidence for parser/data
behavior. Five archive filesystem controls and strict clippy pass independently
on the Mac. They do not prove this adapter, parent authentication,
original pipe ownership, final retirement/cleanup or installed GHE ingestion.
