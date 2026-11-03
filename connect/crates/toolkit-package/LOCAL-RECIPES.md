# Retain an exact local recipe choice

`toolkit-local-recipes` lets a trusted local operator save an agent's selected
recipe documents and decision reference, then list or read that choice from a
new process. It uses `LocalRecipeStore` and the existing durable `SqliteStore`.
The saved origin is `local_investigation`. This does not publish the choice into
the maintained library or make it a supported integration.

The agent chooses what to retain. This command does not investigate, rank or
select integrations. It does not install software, execute recipe instructions,
contact endpoints, acquire credentials, grant access or establish readiness.

## Build the local operator command

From the Toolkit checkout:

```sh
./scripts/repo-cargo build -p toolkit-package --features local-recipes --bin toolkit-local-recipes
```

Use the resulting `toolkit-local-recipes` executable from that build's reported
target directory. The examples below assume it is on your operator command path.
No native Meerkat or MobKit runtime is needed for this local metadata workflow.

## Initialize a store before the first investigation

The trusted host can initialize a database without saving a placeholder recipe:

```sh
toolkit-local-recipes init \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice
```

Use an absolute path in an existing host-controlled directory. `init` requires
the same explicit realm and owner as the other commands, with no additional
flags. It uses the existing `SqliteStore` initializer and saves no recipe,
decision, verification, event or effect record. The store's own metadata is
created as needed. Success is exactly:

```json
{"schema_version":1,"scope":"retained_recipe_metadata","status":"initialized"}
```

`initialized` means the store was initialized now or was already initialized.
It does not assert that the database is empty or grant access to a namespace.
Repeating `init` preserves existing recipes, store identity and record claims.
The host still authenticates the caller and chooses the scope.

Initialization inherits the store owner's existing behavior: a SQLite database
with `user_version` 0 receives the Toolkit tables, even if it has unrelated
tables already. Choose a dedicated database path. SQLite-open and schema-version
failures are reported; initialization is not a full database integrity audit.
Opening can change journal mode or create WAL sidecars before failure.
The command does not promise an untouched file on error and
does not replace or delete a database. If output is lost, initialization may
have committed. Repeat `init` and observe the result instead of removing the
database. Directory custody and path-replacement limits below still apply.

`init` is a new source capability. Use an executable whose `--help` lists it;
previously frozen archives keep their original command set and accompanying
guide. `list` and `read` still require an initialized database and never create
one implicitly.

## Save the host-approved documents

The host must first select and sanitize the input documents and decision
reference. These JSON documents can contain arbitrary text. Validation does not
detect or guarantee the absence of secrets. Never put tokens, passwords, private
account data or credential values in them. Keep any required secrets with their
existing credential owner.

Use absolute file paths. The database parent directory must already exist and
be controlled by the host. `save` may create the named database. Each input must
be a regular file of at most 1 MiB; the store also limits its complete encoded
record to 2 MiB. No directories are created and no files named in the document
contents are opened.

```sh
toolkit-local-recipes save \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice \
  --plugin /absolute/reviewed-choice/plugin.json \
  --mcp /absolute/reviewed-choice/mcp.json \
  --recipes /absolute/reviewed-choice/recipes.json \
  --decision-owner setup-decisions \
  --decision-record selected-tracker \
  --decision-revision 1
```

`--mcp` is optional for a guidance-only bundle. This form uses the existing
`IntegrationBundle` parser: retained HTTPS remote MCP recipes or guidance-only
plans with explicit blockers. Remote recipes cannot carry inline headers or URL
queries. Local Python, Node and MCPB selections use the input form below, preserving
their canonical package bytes.

Success is JSON with `schema_version: 1`, `scope: retained_recipe_metadata`,
`status: retained`, and a `recipe` summary. The summary includes the package
identity, bundle digest, package content digest, local decision origin,
strategies and export roles. The package content digest is the value used by
an `ActivationManifest`; it is distinct from the bundle digest used for lookup.
Both first save and exact replay return `retained`; the command does not claim
which call originally created the record.

An incoming bundle rejected by the initial save parser returns `error:
invalid_bundle` with a fixed diagnostic, for example:

```json
{"schema_version":1,"scope":"retained_recipe_metadata","status":"error","error":"invalid_bundle","diagnostic":{"phase":"before_store_open","code":"mcp_schema"}}
```

The codes are `mcp_schema`, `mcp_document`, `plugin_document`,
`recipe_document` and `bundle_contract`. They identify which contract to check
without returning input values or parser text. `mcp_schema` requires the exact
Agent Plugins MCP `$schema` value. This phase is attached only before this
invocation opens the recipe store; input files have already been read. An
`invalid_bundle` error without this diagnostic, or a missing response, makes
no such guarantee. Save/readback errors can follow a committed record.

The package identity is immutable within the selected realm and owner. Repeating
the same documents and decision is idempotent. Different document bytes or a
different decision under that identity fail with `immutable_conflict`. To retain
a revised choice, explicitly select a new package version or identity. There is
no replace, delete, promotion or automatic upgrade command.

## Reopen, list and read the exact choice

```sh
toolkit-local-recipes list \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice

toolkit-local-recipes read \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice \
  --publisher local-research --name selected-tracker --version 0.1.0 \
  --digest REPLACE_WITH_THE_64_LOWERCASE_HEX_DIGEST_RETURNED_BY_SAVE
```

`list` returns verified summaries under `recipes` and fixed error classifications
for rejected stored entries under `unverified`. A failed entry is never returned
as a reusable recipe. `read` requires the exact publisher, name, version and
bundle digest. A missing identity returns `status: not_found`; a stored recipe
with a different digest is refused. A matching read returns `status: found` and
the original `plugin_json`, optional `mcp_json`, and `recipes_json` strings under
`recipe.documents`, together with the decision reference in its origin.

The bundle digest identifies the parsed package, server and recipe content. It
does not identify the original JSON formatting or the selecting decision. The
store separately retains and checks the exact document strings and decision on
save replay. An authorized host should inspect both the returned content and
local decision evidence before reusing the choice. Retention does not verify
that the referenced decision owner exists or approved an action.

Both observation commands require an existing Toolkit database. They do not
create or initialize one. Opening an existing store can use SQLite WAL sidecars;
this is not a read-only filesystem handle. Store filesystem durability and
locking requirements still apply. Listing uses the existing store owner's
pagination and collects its verified results; command output is capped at 8 MiB
and is not a streaming or arbitrarily large catalog interface.

### Traverse a large collection in bounded pages

`list-page` uses the same canonical record verification and summary projection
without first reading the whole scope. Existing `list` output and behavior are
unchanged. Start with a limit from 1 to 50, then pass the returned cursor unchanged:

```sh
toolkit-local-recipes list-page \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice --limit 20

toolkit-local-recipes list-page \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice --limit 20 --cursor RETURNED_CURSOR
```

The schema-version-1 `retained_recipe_metadata` envelope contains:

```text
status: listed_page
request_cursor: null | STRING
limit: INTEGER
recipes: [ SAME_SUMMARIES_AS_LIST ]
unverified: [ { error: stored_recipe_unverified } ]
oversized: [ { identity: PACKAGE_IDENTITY, digest: HEX, error: summary_too_large } ]
next_cursor: null | STRING
scanned_records: INTEGER
```

The three arrays together contain at most `limit` rows. A summary over 64 KiB
becomes an `oversized` observation with its verified identity and digest. The
record is not deleted or treated as invalid; an exact `read` can still retrieve
it subject to that command's output limit. Invalid recipe contents or digests
produce fixed `unverified` observations with no copied key, error detail or body.
Malformed underlying storage envelopes still fail the read through the existing
store owner. The paginator does not repair or bypass a corrupt database.

Each call fetches at most 200 scoped records, one at a time, through the existing
keyset read. Unrelated records count toward this scan budget. `scanned_records`
also includes a final inspected row deferred because its summary would exceed
the page's byte budget; that row remains eligible on the next call. The complete
compact UTF-8 JSON envelope, including the CLI's terminating newline, is at most
256 KiB. No summary is truncated.

Always follow non-null `next_cursor`, even when all arrays are empty. Only an
observed empty store read terminates traversal; reaching a row, scan or byte
limit does not prove completion. A non-null cursor always advances past at least
one consumed record. Cursor validation binds the exact realm and owner and
rejects malformed, foreign-scope and unsupported-version values before CLI store
opening. `list-page` does not create an absent database.

**The operator cursor is not confidential.** It is canonical lowercase-hex JSON
of at most 4096 characters, encoding a scope digest and the last consumed store
key. That key can belong to an unrelated or invalid recipe record. Only a trusted
operator authorized to inspect scope keys should use this raw interface.
Encoding is neither encryption nor authentication. Audience-facing adapters
must keep the raw cursor private and expose their own bounded correlation handle;
they must not parse or reproduce the canonical row-selection logic.

Raw cursors work after reopening the same scope and do not bind the page size.
They are query positions, not a snapshot, access grant or proof of earlier reads.
Concurrent insertions before the cursor are not included. Restart from no cursor
to rescan; removals or additions can change later observations. No installation,
provider call or current authority is established by listing retained metadata.

## Retain an installed local selection

After an agent investigates and tests a local Python, Node or MCPB package, the same
store can retain its original `IntegrationPackage` and `ActivationManifest`
documents. This does not require converting the package into remote plugin
metadata or rewriting commit-valued provenance. Use the following alternative
`save` form; do not mix its flags with `--plugin`, `--mcp` or `--recipes`:

```sh
toolkit-local-recipes save \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice \
  --package /absolute/reviewed-choice/package.json \
  --selection /absolute/reviewed-choice/activation.json \
  --profile python --component time-server \
  --decision-owner setup-decisions \
  --decision-record selected-time --decision-revision 1 \
  --verification /absolute/reviewed-choice/verification.json
```

`--profile` is `python`, `node` or `mcpb`. MCPB also requires
`--configuration /absolute/reviewed-choice/configuration.json`; other profiles
refuse this flag. It must contain the exact nonsecret configuration bytes bound
by the selection. The verification file is a nonempty JSON
array of distinct references, for example:

```json
[
  {"owner": "setup-observations", "record": "time-install-and-calls", "revision": "1"}
]
```

The host supplies these references to its real historical observations,
including partial or failed results where relevant. They are not credentials
or renewed permission. The store validates the documents using the existing
manifest and selected installation-plan validators; it does not contact the
evidence owner, read artifact files, or verify that an installation succeeded.
For MCPB, it checks the configuration digest and duplicate-free string map.
Archive-dependent checks, including declared field sensitivity, completeness
and argument expansion, still require the MCPB installer and actual archive.
A retained record can therefore refer to an archive the installer will refuse.
Custom host-service selections are not supported by this retention form.

For a selection derived from an ordinary plugin, add
`--plugin-source /absolute/reviewed-choice/plugin-source.json` to preserve its
exact public source observation. This is the setup inspector's retained wrapper,
including original file text, per-file hashes and metadata diagnostics. The
owner validates its structure, limits and hashes and checks plugin/MCP metadata
with the existing importers. Historical inspection summaries remain untrusted
observations, not newly certified parser results or proof of build provenance.
No referenced file is opened while saving or reading this observation.

The optional source participates in the immutable recipe digest. Its raw hash
is exposed as `plugin_source_digest`; exact read returns its original UTF-8
string under `documents.plugin_source_json`. Source and setup directories can
be removed without losing the saved instructions. Old records without source
keep their existing digest. Adding source to an existing immutable identity,
or changing its bytes, is a conflict. This option requires a CLI whose `--help`
lists `--plugin-source`; earlier frozen binaries do not support it.

`list` and `read` use the same commands above. A local selection has
`kind: local_selection`, its profile and component, decision and verification
references, and a retention `digest`. Its full read returns the exact original
`package_json` and `selection_json` strings, plus `configuration_json` for MCPB
and `plugin_source_json` when retained.
Unlike a remote bundle digest,
this versioned retention digest binds all exact strings, profile, component,
decision and all verification references. Formatting changes therefore produce
a different digest. Save conflicts also preserve immutability across all
record formats; an old remote recipe cannot silently become a local selection.
Existing version-1 remote and version-2 Python/Node records remain readable
with unchanged bytes and digests. MCPB uses version 3 in the same store.

For reuse, the agent selects the saved identity and retention digest. The host
checks the historical evidence and supplies current policy, configuration and
other context through a fresh concrete selection. It may reuse unchanged
document bytes only after that check. The original saved selection remains
historical; reading it does not renew its policy revision or account authority.
Write the returned documents to host-controlled files and pass the current
selection, exact retained artifact files and host-approved destination to the
existing [Python](PYTHON-INSTALL.md), [Node](NODE-INSTALL.md) or
[MCPB](MCPB-INSTALL.md) installer. MCPB also takes the current configuration
bytes and their digest. Those
installers revalidate the documents and actual artifact bytes on every install.
They do not obtain current policy by looking up the recipe store.

There is no implicit download, artifact cache or installer invocation during
save/read. Missing or changed artifacts must be resolved explicitly. This
executable `export` form remains for remote endpoints; local selections use their
existing installer. Selected retained guidance can be exported separately as
described below. Preserve the old observations and record the new outcome
with its owner instead of overwriting the retained recipe.

## Export selected retained guidance

A retained local selection or plain software declaration with
`documents.plugin_source_json` can export explicitly selected public skill text
after the original staging directory has been removed. First read the exact
saved identity and retention digest. Select literal paths from that observation
and bind its `plugin_source_digest` in a guidance selection file:

```json
{
  "plugin_source_digest": "REPLACE_WITH_THE_64_LOWERCASE_HEX_SOURCE_DIGEST",
  "files": [
    "./skills/report-guide/SKILL.md",
    "./skills/report-guide/data/example.csv"
  ]
}
```

The source digest is the raw hash returned by retained read, not the inspector
report's inner digest. The CLI accepts no caller-supplied file contents. Use an
executable whose `--help` lists `--guidance`:

```sh
toolkit-local-recipes export \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice \
  --publisher local-research --name selected-report --version 0.1.0 \
  --digest REPLACE_WITH_THE_64_LOWERCASE_HEX_RETENTION_DIGEST \
  --guidance /absolute/reviewed-choice/guidance.json \
  --destination /absolute/exports/report-guide
```

Supply exactly one of `--guidance`, `--selection` or `--declaration`. Guidance
requires 1-15 distinct retained files. Every selected file must be beneath
`./skills/<skill>/`, with that skill's `SKILL.md` explicitly included. Supporting
text can use any filename or extension, including JSON, YAML and CSV; it need
not be under `references/`. No directory traversal, globs, recursive selection,
root assets or MCP configuration selection is supported. Paths are at most 1024
UTF-8 bytes and 16 components after `./`; empty, dot, dot-dot, colon, backslash
and control-character components are refused. A selected file cannot also be
another selected file's parent. Missing retained paths are refused before any
destination is created.

The retained source owner revalidates its existing limits and hashes: at most
16 source files including `plugin.json`, at most 32 KiB per file and 128 KiB in
total, UTF-8 text without binary control characters, and exact per-file and
report digests. This validation does not identify secrets. The host must have
selected public text when retaining it and must authorize its export. Old
records without retained source remain readable but cannot use this form.

The output contains canonical standard `plugin.json` and exactly the selected
skill files. Recognized manifest metadata and every extension namespace are
preserved, including a validated `ai.rkat.toolkit` package and its requirements.
Unknown namespaces remain opaque. The exporter uses the existing manifest
constructor and refuses an invalid Toolkit namespace; it does not erase
requirements to make an executable export portable. No `mcp.json`, entrypoint,
activation selection, configuration or grant is created. Files are written
without execute bits. Text, including a code example, can still be interpreted
by a receiving client; export grants no authority to execute it.

A generic client can discover the skill through the standard `skills/`
directory and follow its relative references without understanding Toolkit
metadata. Only the explicitly selected bytes are copied; the host is responsible
for choosing every reference the recipient needs. Source files are read from
the exact retained record, never reopened in the former staging directory.
The saved selection, policy references and decision history are unchanged.

Success returns `selection_kind: guidance_only`, `plugin_source_digest`, the
exact selection digest and hashes and sizes for every output file. Run
`check-export` with the same `--guidance` file, identity, retention digest and
destination to compare the complete expected file set and detect changed or
extra entries. The destination custody, existing-directory refusal, uncertain
write and readback limits described below apply to this form too. Export and
readback do not establish installation, activation, host compatibility or
completion of the original task.

## Export an independent MCP endpoint

For a retained remote MCP recipe, an agent or trusted host can supply a concrete
`RemoteExportDeclaration` and export standard `plugin.json` and `mcp.json` files.
It preserves the complete component/export independence contract without host
configuration or policy snapshots. Use `selection_kind: remote_declaration` and
omit those snapshots and the remote source's `configuration_revision`:

```sh
toolkit-local-recipes export \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice \
  --publisher local-research --name selected-tracker --version 0.1.0 \
  --digest REPLACE_WITH_THE_64_LOWERCASE_HEX_BUNDLE_DIGEST \
  --declaration /absolute/reviewed-choice/declaration.json \
  --destination /absolute/exports/selected-tracker
```

The selection must name the exact package content digest, strategy, retained
endpoint and publisher, and service export. It must declare exactly that recipe's
server name in `portable_mcp_entries`. The shared contract validator refuses
components that need Toolkit contracts, host services,
package state or dependencies. Parsing the saved recipe is not sufficient proof
of portability. A guidance-only recipe has no executable export through this
command. Selected retained skill text uses the separate `--guidance` form above.

Existing `--selection` callers may still provide the full `ActivationManifest`,
including its original configuration/policy references and source revision.
For an MCP endpoint, supply exactly one of `--selection` or `--declaration`. Legacy serialization,
manifest digests, typed refusals and saved requests retain their meaning. Actual
Toolkit activation and `check_host` still require the full manifest; the new
inert export neither constructs a substitute manifest nor authorizes activation.
Schema hashes and lifecycle fields in either form are declarations, not verified
native observations or grants.

A remote recipe without `plan.authentication` keeps the original binding-required
contract. Its selection must declare `authentication: {"kind": "binding", ...}`;
a declaration cannot silently downgrade an existing authenticated recipe. Omitted
authentication fields retain their original serialized form and bundle digest.

For a provider that explicitly documents anonymous endpoint access, the recipe
can instead declare:

```json
"authentication": {"kind": "unauthenticated", "evidence": "provider-public-access"}
```

The evidence ID must name an existing retained source whose URL belongs to the
selected strategy's `sources`. Such a recipe cannot also declare a
`client_authentication` prerequisite. Its selected service export must explicitly
use `authentication: {"kind": "none"}`; a credential binding is also a mismatch.
The source reference is an agent-supplied documentary claim, not a verified
statement that the live service currently accepts anonymous access. Existing
policy, operation, host compatibility and readiness checks remain separate.
Adding this mode changes the recipe digest, so it must not overwrite an existing
immutable saved identity. Older executables reject the new field rather than
assuming anonymous access; use a CLI edition that supports this source contract.

All selection checks happen before creating the destination. Its parent must
exist and be host-controlled; the destination itself must not already exist.
The command does not overwrite files. Success returns `status: exported`, the
local recipe origin, the selection digest, and hashes and sizes for the written
files. The new form also returns `selection_kind: remote_declaration`; its optional
`declared_remote_service` has the same kind and a distinct `declaration_digest`.
Legacy service metadata keeps `manifest_digest` and no kind field. No format is
inferred from omitted fields or reinterpreted on recovery. The command does not
modify the saved documents or selecting decision. The exported
plugin metadata retains its Toolkit namespace, which generic clients may ignore.

The exported MCP entry exposes the whole selected endpoint. It does not create
a proxy enforcing selected-operation lists, resource restrictions, result limits
or Toolkit grants. The receiving client performs its own authentication and
authorization. Export does not contact the service or prove host compatibility,
installation, readiness or that the original task can continue.

This is a local file export, not an atomic or durable installation transaction.
An I/O failure returns `export_write_unconfirmed` and can leave an incomplete
directory. An interrupted command may have finished writing even if no success
response arrived. Inspect that directory before retrying with a new destination;
never interpret missing output as proof that no files were written. The generic
MCP client needs only `mcp.json`; the returned recipe and decision evidence remain
host metadata rather than executable authority.

### Check a retained export without repeating it

Use `check-export` with the exact retained recipe identity, bundle digest and
original selection form to compare an existing destination with the canonical
exporter output (`--selection` remains valid for a legacy manifest):

```sh
toolkit-local-recipes check-export \
  --database /absolute/host-state/toolkit.sqlite \
  --realm household --owner alice \
  --publisher local-research --name selected-tracker --version 0.1.0 \
  --digest REPLACE_WITH_THE_64_LOWERCASE_HEX_BUNDLE_DIGEST \
  --declaration /absolute/reviewed-choice/declaration.json \
  --destination /absolute/exports/selected-tracker
```

The command uses the same exact recipe lookup and selection/portability
validation as `export`. It derives expected bytes through the matching export
constructor and calls `PortablePluginExport::inspect_existing`; it never calls `write_new`,
repairs files, creates the destination, executes package code or contacts the
endpoint. An absent recipe database is refused rather than created. Opening an
existing SQLite store can still create or update WAL sidecars; the read-only
claim concerns the export tree and retained recipe records, not an OS-level
read-only database connection. File reads can also update access times.

A completed check returns the existing schema-version-1
`retained_recipe_metadata` envelope, `status: inspected`, the same `recipe`,
`selection_digest` and `destination` fields, and this `export_check` shape:

```text
{
  state: matches | missing | mismatch | unavailable,
  files: {
    "plugin.json": {
      state: matches | missing | mismatch | unavailable,
      expected: { sha256: HEX, bytes: INTEGER },
      observed: { sha256: HEX, bytes: INTEGER } | null
    },
    "mcp.json": { ... }
  },
  inventory: {
    state: matches | missing | mismatch | unavailable,
    complete: BOOLEAN,
    unexpected_entries: INTEGER
  }
}
```

`matches` means the current regular-file bytes and exact expected inventory
matched. `missing` means the destination or an expected entry was absent.
`mismatch` means stable observed regular bytes differed or an unexpected entry
was present. `unavailable` covers unsafe file types, read failures, observed
changes during the check, unsupported platforms and inspection limits. The
aggregate precedence is unavailable, mismatch, missing, then matches. Every
expected file retains its canonical hash and size even when observation is
missing or unavailable. `observed` is null when no bounded complete file read
was available; this includes oversized mismatched files.

Unexpected names and file contents are not returned. `inventory.complete`
means all expected directories were enumerated within the limits.
`unexpected_entries` counts unexpected entries in those directories. An
unexpected directory counts as one entry; its unknown subtree is not traversed.
For a matching result the inventory is complete, its state is matches, the
unexpected count is zero, and every expected file has equal expected and
observed fingerprints.

The current implementation inspects Unix filesystems with no-follow,
nonblocking opens. It refuses symlinks and special files, including the
destination directory. The destination must use a direct path spelling without
repeated or trailing separators, `.` or `..` components; ambiguous spellings
return unavailable before inspection. It checks file identities, sizes, modes
and modification metadata before and after reads. A second bounded inventory scan and destination
identity check detect ordinary replacement during inspection. The generic Rust
API also supports nested expected inert files. Limits are 128 expected files,
512 entries per inventory, 32 path components, 1 MiB per file and 8 MiB of total
file reads. Unsupported platforms return unavailable without opening the tree.

The host must hold exclusive, quiescent custody of the destination and control
its ancestor directories. These checks are not an atomic snapshot against
external writers. Matching current bytes do not prove that an earlier export
succeeded, completed durably or returned a response. They also do not renew the
selection's policy or authentication references, prove endpoint readiness, or
authorize a business call. Keep a historical uncertain outcome separate from
this current-file observation.

## Host and operator boundary

The database, realm and owner are explicit trusted operator inputs. Choosing an
owner flag does not authenticate a caller. Do not expose this binary directly as
an untrusted agent or remote tool. A host offering private recipe discovery must
derive scope from its own authenticated caller and enforce its existing access
policy before using `LocalRecipeStore`. Public `DiscoveryTools` is unchanged and
does not read these private records. This command performs no library insertion.

Use host-controlled directories and normal local filesystem permissions. Input
and database final-component symlinks and non-regular files are rejected during
preflight; Unix document opens also refuse final-component symlinks. Ancestors
and database path changes are not a filesystem sandbox. The host must prevent
untrusted concurrent replacement of those paths. No scope, path, credential or
configuration value is taken from environment variables or default files.

The current length-prefixed recipe record ID must fit the store's 512-byte key
limit. Some otherwise valid long package identities therefore cannot be retained.
This command does not change that key format or the store limit.

Successful lookup results intentionally expose the retained operator-selected
metadata. They are not a redacted projection for arbitrary model contexts. Error
responses use fixed classifications rather than echoing file contents, paths or
backend messages. Errors exit with status 1; `not_found` is a successful lookup
result. `--help` is the only flag accepted without an operation.

If a save is interrupted or its output is lost, the database may already contain
the choice. Reopen and read it, or replay the same exact documents and decision.
Do not interpret an absent response as proof that nothing was retained. This
command's persistence result says nothing about installation, authentication,
activation, provider access or the original user's task completing.
