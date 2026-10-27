# Inspect public package metadata

`toolkit-package-inspect` wraps the existing IntegrationPackage and ActivationManifest validators, Agent Plugins importers and MCPB importer. It reads public JSON from standard input and emits one JSON report. Explicit file inspection reads a selected plugin below a trusted staging root. It does not choose an integration, run a package command, expand environment variables or activate anything.

```sh
toolkit-package-inspect integration-package < package.json
toolkit-package-inspect agent-plugin < plugin.json
toolkit-package-inspect mcp-servers < mcp.json
toolkit-package-inspect mcpb < manifest.json
toolkit-package-inspect mcpb-archive < archive-request.json
toolkit-package-inspect activation < selection.json
toolkit-package-inspect activation-update < update.json
toolkit-package-inspect activation-host < host-selection.json
```

The format must be explicit. Agent Plugins documents use the exact versioned schema declared by the canonical importer; an arbitrary MCP client's configuration is not necessarily that format. MCPB support is the existing 0.3 manifest subset.

For `integration-package`, supply the Toolkit `IntegrationPackage` document, not an npm package manifest. `summary` contains its `identity`, `package_content_digest`, strategy IDs and kinds, and export IDs and roles. The digest comes from `IntegrationPackage::content_digest`, including its validation and canonical serialization. Use this digest in an `ActivationManifest`'s `package_content_digest` when authoring a concrete selection. The existing plan validator still validates the complete selection before installation.

The top-level `source_digest` hashes the exact input bytes. It can differ between documents with the same `package_content_digest`: whitespace, object-key order and omitted fields with explicit schema defaults do not change the canonical package. Invalid JSON, unknown fields and typed parse failures return the fixed `package` error with the bounded diagnostic below; semantic validation errors retain the existing validator's diagnostic paths and messages.

For an `integration-package` deserialization failure, exit 2 returns exactly
`schema_version: 1`, `scope: "metadata_only"`, `status: "invalid"` and an `error`
with `kind: "invalid_metadata"`, `path: "package"`,
`message: "invalid integration package"` and this additional object:

```json
{
  "diagnostic": {
    "version": 1,
    "category": "data",
    "field_path": ["strategies", 0, "known_requirements", 0],
    "source_digest": "0000000000000000000000000000000000000000000000000000000000000000"
  }
}
```

The displayed digest is a placeholder; actual `source_digest` is the SHA-256 of
all exact input bytes as 64 lowercase hexadecimal characters. `category` is the
JSON deserializer's `data`, `syntax` or `eof` classification. It is not inferred
from error text. `field_path` contains at most 16 canonical field names or
nonnegative array indexes no greater than 1048576. A field name is allowed only
at its actual position in the `IntegrationPackage` model. The path stops at an
unknown key, enum segment or unsupported position, including a known field name
used in the wrong object. Missing fields and some enum errors identify their
nearest known container. An empty array identifies the root or an unavailable
safe location. No submitted value, unknown key or parser message is reflected.

The existing `IntegrationPackage` deserializer and semantic validator remain the
owners of acceptance. This diagnostic is a location for correction, not a second
schema, automatic repair, complete error inventory or readiness claim. Other
formats, semantic validation refusals, read failures and size-limit failures
retain their existing reports. Consumers may retain generic help for older
reports; unknown diagnostic versions or invalid envelopes must not be treated
as accepted metadata. See the [metadata guide](../../sdk/consumer-setup/METADATA.md)
for exact host service requirement shapes and evidence-preserving correction.

Input is limited to 1 MiB and the report to 256 KiB including its newline. Exit 0 means metadata was parsed, possibly with diagnostics; exit 2 means invalid arguments, input or output bounds, or rejected metadata. Exit 1 means output could not be written. Invalid MCP server entries are skipped by the canonical importer and reported individually. Check `accepted_server_count` and diagnostics; an empty or partial result is not a usable server configuration.

Every report has `scope: metadata_only`. Successful reports include the digest of the exact input bytes. Package role/strategy hints, transport names, field names and unresolved MCPB contracts are investigation evidence. They establish neither an installed artifact nor authenticated account or role readiness.

The document summaries omit command/argument values, environment/header values, endpoint URLs, user defaults and extension contents. The explicit file modes return selected text verbatim. Feed public metadata only: this is not a secret sanitizer or a trusted instruction channel.

## Inspect an MCPB before configuring it

`mcpb-archive` requires a build with `local-artifacts` on macOS or Linux. Supply an absolute host-owned staging root, one direct child directory, the exact raw SHA-256 of its `artifact.mcpb` and explicitly selected public text members:

```json
{"root":"/absolute/staging","directory":"./chosen","artifact_sha256":"EXACT_64_LOWERCASE_HEX_DIGEST","files":["./README.md"]}
```

The reader always includes `manifest.json` first. `files: []` reads only that manifest; naming it once explicitly is also accepted. The canonical ZIP32 reader validates every archive member, including unselected entries and their CRCs. It captures selected bytes without extracting files or executing code. The raw archive is limited to 64 MiB; at most 16 selected files fit within 32 KiB each and 128 KiB in total. Existing archive expansion, entry and path limits remain in force.

The report binds `artifact_sha256`, raw byte length and exact selected file hashes. `source_digest` hashes the canonical `files` array. `archive_entry_count` counts explicit ZIP members; `inventory_entry_count` also counts implied parent directories. `inventory` contains the first 128 sorted paths with size and directory flag; `inventory_truncated` identifies a partial listing. Every selected member must be control-safe UTF-8.

The manifest's existing MCPB inspection adds `summary.declarations` with runtime, entry point, command and arguments. Configuration fields and unresolved contracts remain in the canonical summary, while exact public manifest text provides descriptions and choices. These are source declarations, not a generated launch plan. Sensitive fields remain visible as prerequisites, but secret defaults are rejected and the current installer still accepts only its self-contained, nonsecret Node subset. Successful inspection does not prove installability, authentication or readiness.

An agent uses these observations to author its explicit package, activation and configuration. Bind that activation to the observed archive hash, then use the existing selected check/install owner. No defaults, scopes, operations or executable choices are inferred by the reader.

## Read an ordinary plugin

`agent-plugin-files` takes `{ "root": "/ABS/host-staging", "directory": "./selected-plugin", "files": ["./plugin.json", "./mcp.json", "./skills/example/SKILL.md"] }` on stdin. The directory must be one direct child. The canonical reader anchors that child below the host root, then keeps every read below the child even if paths are replaced. Requested files must be regular, bounded, UTF-8 text. This exact-file form does not enumerate directories or execute imported code.

To discover unknown skill or supporting-file names, use the exclusive inventory form through the same command:

```json
{"root":"/ABS/host-staging","directory":"./selected-plugin","inventory":"./"}
```

The successful response has `status: "inventoried"`, the requested `directory`, `complete: true` and an `inventory` array sorted by plugin-relative path. Each row has `path` and `kind` (`file`, `directory`, `symlink` or `special`); only regular-file rows have a `byte_length` metadata observation. It contains no file text, symlink target, source digest or retained `plugin_source`. The fixed root and selected plugin remain unchanged when the agent makes another call with `inventory: "./skills"`, then a discovered skill directory or a supporting-assets directory. The agent chooses which public files to read using the existing `files` form, always including `./plugin.json`.

Each inventory call enumerates one directory without recursion or content reads. It accepts at most 256 entries, 1024 UTF-8 bytes per returned path, 64 KiB total returned path bytes and 15 components in the requested directory (16 in a returned child). Names must be valid UTF-8 with no control characters, backslash, colon, empty component, `.` or `..`; `./` alone denotes the selected plugin root. Both request forms remain below the fixed staging boundary. A deliberately selected directory symlink may resolve inside the plugin root; listing a symlink only classifies it and never follows it automatically. Selected outside-root directories and special files refuse.

An over-limit listing, unreadable or invalid name, duplicate entry or observed directory metadata change returns `invalid_metadata` with no partial inventory or `complete` field. There is no pagination or silently truncated success. `complete: true` means this single bounded enumeration finished, not that the entire package was inventoried or that file contents are immutable. File type and size are current observations only. Later exact reads recheck containment, type, text and byte limits and retain their own selected-source evidence; inventory alone grants no authority and proves no package compatibility or readiness.

Select 1-16 unique paths, including `./plugin.json`. Each file is at most 32 KiB and the total is at most 128 KiB; the existing 256 KiB encoded output limit still applies. Control characters other than newline, carriage return and tab are refused. Missing requested files refuse the complete request. A guidance-only plugin needs no `mcp.json`.

The report contains `files` with exact `path`, `text`, `byte_length`, `source_digest` and `inspection`. Manifest and MCP files receive the existing canonical inspection report; other text has a null inspection. Unknown namespaces remain opaque. Invalid recognized Toolkit metadata refuses import. Invalid MCP siblings retain the canonical diagnostics; accepted siblings do not make skipped entries usable.

In this mode only, the top-level `source_digest` hashes canonical JSON of the returned file rows, independent of the host path. It is an observation of the selected files, not a pin of the entire directory. Setup can retain this evidence with an agent-authored selection. Artifact pins, required host contracts and installation permission are still checked by their existing owners.

## Selected activation metadata

`activation` accepts exactly `{ "package": <IntegrationPackage>, "manifest": <ActivationManifest> }`. `activation-update` accepts exactly `{ "previous": <pair>, "next": <pair> }`, where each pair has that same shape. Unknown fields are refused in the wrappers and the canonical documents. Neither mode accepts a serialized validation receipt in place of these inputs.

Each pair is validated by `ActivationManifest::validate` against its own package. An update validates both pairs before calling the existing `next.state_changes_from(previous)` method. Failure returns no partial selection summary. Typed parse errors use fixed diagnostic text; semantic validation errors retain the canonical owner's diagnostic paths and messages.

An activation summary contains:

- `package`: the canonical package identity (publisher, name, version).
- `package_content_digest` and `manifest_digest`: the canonical package and validated activation bindings.
- `components`: selected component IDs, versions, source kind and declared `state` contracts. An artifact source also includes its declared `content_digest`; remote and host-provided sources contain only `kind`.
- `initialization_order` and `shutdown_order`: the validator's dependency order and its reverse.

An update summary contains `previous` and `next` activation summaries plus the canonical `state_changes`: `initialize`, `migration` or `retained_on_removal`. Migration entries retain the declared operation, artifact digest and reversibility. The canonical owner rejects a changed publisher/name, namespace owner transfer or format change without the required migration declaration. Removed state is retained. A change to configuration, policy or other manifest fields can change `manifest_digest` while `state_changes` remains empty.

The report's `source_digest` hashes the entire raw input wrapper, including both pairs for an update. It is not an individual document's raw hash. Whitespace and object ordering can change that digest without changing the canonical bindings. Input and output limits apply to the complete request and report, not separately to each pair.

These are declarations and obligations, not migration receipts or observed database state. The host still has to verify actual component and migration artifacts, quiesce writers, invoke the state owner, reconcile uncertain outcomes and apply its activation rules. The activation and update modes do not check host capability satisfaction, execute lifecycle order, perform a migration, authorize rollback or prove readiness. A reversible declaration does not authorize restoring an old database or rewinding external effects. Initial inspection does not assert that any state namespace is empty.

## Host compatibility before importing code

`activation-host` accepts exactly `{ "selection": <pair>, "host": <HostCapabilities> }`.
It validates the same package/manifest pair, then calls the canonical
`ValidatedManifest::check_host`. The report remains `metadata_only`; it does not
load code or hand out service handles.

The summary contains `activation` (the unchanged activation summary),
`host_digest` (canonical host capabilities), `compatible`, `compatibility`
(all required gaps and optional degradation), and `services`. Each service row
contains its `scope`, complete `requirement`, and `available` result from the same
version/authority comparison. Manifest, component and host-provided source
requirements retain their scopes; they are not merged into a package-wide grant.

Exit 0 and `status: accepted` mean valid metadata, even when `compatible` is false.
A host must reject every required gap before import, including export contracts,
runtime, placement and activation contracts. An available service alone is
insufficient. Optional absence stays unavailable and retains its declared degraded
behavior; the host must support that behavior or refuse the selection.

The host must derive offers from its actual implementations and bind the input
to its current selected package, artifacts, configuration and policy. Publisher
metadata cannot declare the host's capabilities or grant itself authority.
Compatibility and authority vocabulary matching do not establish authorization,
service activation, lifecycle completion or readiness. Existing policy owners
must scope and revoke the actual handles they provide.

[Activation-mode verification](../../docs/verification/20261003-activation-inspector/REVIEW.md) covers 20 unit tests, strict Clippy and 12 actual process cases using a retained local executable. Previously accepted installation kits and library archives retain their original inspector bytes; this source change does not update those distributions.

The binary is built by the `toolkit-package` crate. The [original verification](../../docs/verification/20261002-metadata-inspect/REVIEW.md) used an isolated workspace containing the unchanged crate, avoiding this repository's historical private native dependency graph. That executable passed seven process checks and predates `integration-package`. The `integration-package` mode and its separately delivered local executable have [their own evidence](../../docs/verification/20261003-agent-prepared-install/inspector/REVIEW.md).
