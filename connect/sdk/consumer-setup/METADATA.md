# Prepare package and software metadata

Use this guide with [selected setup](SELECTED.md) when an agent has chosen an
integration outside the ready-made library. The package describes what
was investigated. A plain local service can select its software with a
`LocalSoftwareDeclaration`, before host configuration or access selection.
A full `ActivationManifest` remains the path for custom host composition.
Neither document grants execution or account access.

The examples below are complete inert format examples. Their URLs, IDs, hashes,
operations and any host references are placeholders, not an installable integration.
Replace them from source evidence and, only where the full activation requires
it, current host facts. Artifact and component rules remain in the corresponding
installer contracts.

With a host-enabled package inspector, the complete metadata path can use MCP:
author a nonsecret `package_json` string, call `setup_inspect_package`, use its
`summary.package_content_digest` when authoring `declaration_json` or
`activation_json`, then submit the package and exactly one selected document
to `setup_selected_check` with the acquired `staged_id`, selected
`profile` and `component`. MCPB also needs a nonsecret `configuration_json`
string. Each document is limited to 32 KiB UTF-8 and the complete encoded request
to 64 KiB. These are document strings, not filesystem paths or JSON objects.
The host writes them only to the private proposal and computes their raw hashes.
No setup-host shell or metadata-file write is needed for these steps. Research,
integration choice and source requirements remain the agent's responsibility;
only the actual host owners can supply current host facts when required.

If the agent needs evidence from an acquired primary or dependency wheel or npm archive,
[acquired archive inspection](ACQUIRED-INSPECTION.md) can expose bounded member
listings and exact text through MCP without extracting or executing the package.

## Operation selection and runtime enforcement

Bare `mcp.json` exposes the server to an ordinary MCP client. Declaring read
operations in package or selection metadata does not filter that endpoint or
grant permission to call it.

Connect's native protected attachment can instead restrict calls to an approved
subset of exact MCP tool names. The consuming host enforces the current grant
and MCP access configuration; installation and metadata do not create that
authority. When that host path is available, a server advertising separate
write tools is not, by itself, a reason to reject a read subset. Inspect the
actual tool schemas, preserve the exact operation-to-tool mapping and request
only the operations needed for the task through the host's protected review.

An operation-name allowlist does not constrain query semantics, prevent startup
effects or provide filesystem confinement. A permitted query tool could still
accept mutating SQL, and starting a server could write files before any tool
call. Verify those behaviors and the host's separately supplied resource
controls. Do not label a connection read-only from tool names or effect metadata
alone. If the required enforcement is unavailable, retain that concrete blocker.

## Describe the investigated package

Toolkit `Id` fields are stable identifiers, not display names: 1-192 ASCII
characters, starting with a letter or digit and containing only letters,
digits, `.`, `_`, `-`, `/`, `:` or `@`. Slash-separated segments cannot be empty,
`.` or `..`. Spaces, backslashes and non-ASCII characters are invalid.
For example, use an agent-chosen identifier such as `example-provider`, not the
display label `Example Provider`.

This applies to package `identity.publisher` and `identity.name`, strategy and
export IDs, activation component IDs and placement, operation IDs, and typed
host references: configuration owner/reference, policy owner, readiness
owner/checks, shutdown owner/operation, contract/service names, authority IDs,
diagnostics and cleanup. Dependency component IDs, state namespaces/owners,
reversible rollback owners/operations and portable MCP entry names/components
follow the same rule. References must match their declarations exactly; never
rewrite a host-supplied identifier to make it pass validation.

Package/component versions and configuration/source revisions are `Revision`
values instead: nonblank, at most 256 UTF-8 bytes, with no control characters.
`display_name`, descriptions, URLs and license text are not `Id` fields. Digests
have their separate 64-character lowercase hexadecimal SHA-256 format.

When available, `setup_inspect_schema` removes the need to calculate a digest
manually. Supply the exact nonsecret JSON text for the schema evidence you have:

```json
{"schema_json":"{\"type\":\"object\",\"properties\":{\"path\":{\"type\":\"string\"}}}"}
```

Use its returned `schema_digest` only with the recorded `exact_supplied_utf8`
convention and retain the supplied text and its evidence source. Whitespace and
JSON escape spelling affect this SHA-256. The tool accepts a bounded JSON object
or boolean; it does not validate JSON Schema, resolve references, inspect a
package, execute a tool or establish schema equality, readiness or authority.
Source/manual reconstruction remains declaration evidence. For hosted metadata,
the actual admitted schema observation and its host-recorded hashing convention
remain required; this helper does not replace that convention or observation.

`package.json` is Toolkit metadata, separate from the upstream Python package.
Record the actual immutable identity, source revision, license, changes and
setup strategy. Leave assessments empty unless evidence supports a claim.

```json
{
  "schema_version": 1,
  "identity": {
    "publisher": "fixture",
    "name": "python-server",
    "version": "0.0.0-fixture"
  },
  "display_name": "Inert Python installation fixture",
  "description": "Typed example only; placeholder artifacts and declarations are not an installation or runtime claim.",
  "provenance": {
    "source": "https://example.invalid/python-server/source",
    "source_revision": "fixture-only",
    "license": "fixture-only",
    "notices": [],
    "local_patches": []
  },
  "strategies": [
    {
      "id": "python-local",
      "kind": "local_mcp",
      "instructions": "Fixture only. Select, retain and approve exact package and dependency artifacts before installation.",
      "sources": [
        "https://example.invalid/python-server/source"
      ],
      "known_requirements": [],
      "unresolved_details": []
    }
  ],
  "known_exports": [
    {
      "id": "service",
      "role": "service_connector",
      "description": "Whole MCP server, not an operation-filtered proxy."
    }
  ],
  "assessments": []
}
```

### Host service requirements and setup prose

`strategies[].known_requirements` is an array of typed `HostServiceRequirement`
objects. It is not an array of instructions or dependency descriptions. Each
object requires all four fields in this illustrative example:

```json
{
  "name": "fixture/service",
  "version": {"major": 1, "minimum_minor": 0},
  "authority": ["read"],
  "availability": {"kind": "required"}
}
```

`name` and each `authority` entry are Toolkit IDs. `version` requires unsigned
16-bit integers `major` and `minimum_minor` (0-65535). The major version must
match a host offer and its minor version must meet this minimum. `authority` is
an array of the exact service's authority IDs; use `[]` only when none are
required. This is a declaration, not a grant. Required availability has exactly
`{"kind":"required"}`. Optional availability requires a nonblank degradation
description:

```json
{"kind":"optional","degraded_behavior":"Report this service as unavailable and continue without it."}
```

Use these objects only for an evidenced host service contract, with its actual
name, version and authority vocabulary. Do not invent a service from a package's
Node/Python prerequisite, dependency list, directory restriction or launch
instructions. Put established setup facts and restrictions in `instructions`,
supporting URLs in `sources`, open questions or unverified conditions in
`unresolved_details` (an array of nonblank strings), and attribution or source
notices in `provenance.notices` (an array of strings). Keep license findings in
`provenance.license` and preserve relevant unresolved license questions. An
empty `known_requirements` is appropriate when no typed host service requirement
is known; it does not establish that all runtime prerequisites are satisfied.

### Correct a rejected package without losing evidence

When supported by the pinned inspector, a parse refusal from
`setup_inspect_package` adds `version: 1`, `category`, `field_path` and
`source_digest` to its fixed `diagnostic`. Categories are `data` (a JSON value
does not match the typed model), `syntax`, and `eof` (incomplete JSON). The digest
is 64 lowercase hexadecimal characters for the exact submitted UTF-8 bytes.
For example, `["strategies",0,"known_requirements",0]` identifies the first
requirement in the first strategy; the object shape above explains its type.

The path contains only canonical field names and array indexes, at most 16
segments. Unknown keys, missing fields, enum decoding and other parse boundaries
can leave only the nearest known container or an empty root path. The diagnostic
does not quote submitted keys, values or parser prose, and is not a complete
schema or list of every error. Older inspectors and semantic validation refusals
continue to return generic correction guidance.

Repair the indicated field and inspect the complete document again. Preserve
valid `unresolved_details`, `provenance.notices`, sources and restrictions. If a
requirement was written as prose, deliberately move that text to the appropriate
text field instead of clearing evidence arrays or fabricating a host contract.
Only a later accepted inspection supplies a canonical package content digest;
a diagnostic is not an accepted package or an installation/readiness result.

Obtain `package_content_digest` from `setup_inspect_package` when the host
exposes it. `source_digest` is the exact submitted UTF-8 hash, not this canonical
digest. Otherwise use the existing metadata inspector command. In a
standalone agent kit it is `bin/toolkit-package-inspect`:

```sh
"$TOOLKIT_INSPECTOR" integration-package < "$PACKAGE_JSON"
```

Require a successful exit and `status: "accepted"`, then use
`summary.package_content_digest`. The inspector reads inert metadata only; it
neither installs software nor starts the selected server. A host may expose it
through its recorded preparation tool, with the exact package JSON on stdin.

The content digest hashes the parsed, validated typed package after defaults
and URL normalization. Canonical JSON recursively sorts object keys, preserves
array order and uses compact UTF-8 encoding, then SHA-256. Hashing the original
pretty-printed file does not compute this value. Use the owner's output instead
of maintaining a second canonicalization implementation.

## Select plain local service software

Use `declaration.json` for one independent Python or Node service connector.
This form selects the artifact closure and literal entrypoint without requiring
configuration, policy, readiness, shutdown, rollback or enforcement owner
references before software installation. The package and its provenance remain
complete and unchanged. It requires one local MCP strategy, one component, one
service export and one portable MCP entry. Selected `known_requirements` and
`unresolved_details` must both be empty, and every operation must declare
`resources.kind: "connection_wide"`.

Do not remove requirements, reinterpret uncertainty as resolved, or broaden a
selected-resource contract to fit this form. Custom host, lifecycle, enforcement,
state, frontend and event obligations stay on the full activation/custom path.
Those fields and unsupported roles are rejected here. Validation cannot discover
obligations omitted from prose; preserve all actual package requirements.
Authentication and required-authority fields are source declarations, not actual
credential ownership or grants. The existing protected public-service path
requires explicit authentication `none` and an exact raw operation map.

This complete inert example uses the package above and placeholder artifact and
schema digests. Its operation map illustrates spelling, not an observed tools/list
result. Use actual investigation evidence for every real value.

```json
{
  "schema_version": 1,
  "document_kind": "local_software_declaration",
  "package": {
    "publisher": "fixture",
    "name": "python-server",
    "version": "0.0.0-fixture"
  },
  "package_content_digest": "26818a8963232a3389bf997d9a0ff3cd59952434c64cf646f9d81c71fab23569",
  "strategy": "python-local",
  "components": [
    {
      "id": "python-server",
      "version": "0.0.0-fixture",
      "runtime": "python",
      "source": {
        "kind": "artifact",
        "location": "https://example.invalid/fixture_server-1.0-py3-none-any.whl",
        "content_digest": "1111111111111111111111111111111111111111111111111111111111111111",
        "dependencies": {
          "kind": "pinned_closure",
          "artifacts": {
            "requirements": "2222222222222222222222222222222222222222222222222222222222222222",
            "dependency_one-1.0-py3-none-any.whl": "3333333333333333333333333333333333333333333333333333333333333333"
          }
        },
        "entrypoint": {
          "executable": "fixture_server/__main__.py",
          "arguments": []
        }
      }
    }
  ],
  "exports": [
    {
      "role": "service_connector",
      "id": "service",
      "components": [
        "python-server"
      ],
      "authentication": {
        "kind": "none"
      },
      "operations": [
        {
          "id": "fixture-read",
          "schema_digest": "5555555555555555555555555555555555555555555555555555555555555555",
          "declared_effect": "read",
          "required_authority": [
            "read"
          ],
          "resources": {
            "kind": "connection_wide"
          },
          "result_limits": {
            "maximum_bytes": 4096,
            "maximum_items": 10
          }
        }
      ]
    }
  ],
  "portable_mcp_entries": [
    {
      "name": "fixture-python",
      "component": "python-server",
      "operation_tools": {
        "fixture-read": "fixture_read"
      }
    }
  ]
}
```

For Node, select the actual Node local-MCP strategy, set `runtime` to `node`, and
use its exact npm artifact, full closure and relative JavaScript entrypoint.
The existing Node planner still validates the ecosystem-specific rules.
Omitting `operation_tools` permits software installation only; it cannot establish
protected public-service metadata. When supplied, the map must cover every
selected operation exactly, with distinct bounded raw MCP names.

Submit `package_json` and `declaration_json` inline to `setup_selected_check`.
Do not also send `activation_json`, raw hashes or a `document_kind` argument.
The normalized retained request derives `document_kind` from this explicit field.
For staged files instead, send the two raw hashes plus
`document_kind: "local_software_declaration"`; `selection_sha256` hashes exact
`declaration.json` bytes. The plan and subsequent receipt/readback carry
`document_kind` and canonical `declaration_digest`, never `manifest_digest`.
The canonical digest is computed by the owner and differs from the raw-file hash.

After separately approving installation, use the exact job's reconciliation and
the existing protected service review. Software installation does not establish
schema observation, authentication, native grants, attachment or usability.

## Select artifacts and current host references

Create `activation.json` with the package identity, chosen strategy and content
digest above. The following neutral example matches the example package. Its
remaining digest values are placeholders.

```json
{
  "schema_version": 1,
  "package": {
    "publisher": "fixture",
    "name": "python-server",
    "version": "0.0.0-fixture"
  },
  "package_content_digest": "26818a8963232a3389bf997d9a0ff3cd59952434c64cf646f9d81c71fab23569",
  "strategy": "python-local",
  "configuration": {
    "owner": "fixture/config",
    "reference": "selected",
    "revision": "1",
    "content_digest": "0000000000000000000000000000000000000000000000000000000000000000"
  },
  "policy": {
    "owner": "fixture/policy",
    "revision": "1",
    "content_digest": "0000000000000000000000000000000000000000000000000000000000000000"
  },
  "components": [
    {
      "id": "python-server",
      "version": "0.0.0-fixture",
      "source": {
        "kind": "artifact",
        "location": "https://example.invalid/fixture_server-1.0-py3-none-any.whl",
        "content_digest": "1111111111111111111111111111111111111111111111111111111111111111",
        "dependencies": {
          "kind": "pinned_closure",
          "artifacts": {
            "requirements": "2222222222222222222222222222222222222222222222222222222222222222",
            "dependency_one-1.0-py3-none-any.whl": "3333333333333333333333333333333333333333333333333333333333333333"
          }
        },
        "entrypoint": {
          "executable": "fixture_server/__main__.py",
          "arguments": []
        }
      },
      "runtime": "python",
      "placement": "local",
      "lifetime": "request",
      "activation_scope": {
        "kind": "independent"
      },
      "dependencies": [],
      "required_contracts": [],
      "host_services": [],
      "state": [],
      "readiness": {
        "owner": "fixture/mcp-client",
        "checks": [
          "initialize-and-list"
        ]
      },
      "shutdown": {
        "owner": "fixture/mcp-client",
        "operation": "close-and-join",
        "maximum_wait_ms": 1000,
        "timeout": "preserve_uncertainty"
      },
      "rollback": {
        "kind": "owner_reconciliation",
        "owner": "fixture/operator",
        "operation": "reconcile"
      }
    }
  ],
  "exports": [
    {
      "role": "service_connector",
      "id": "service",
      "components": [
        "python-server"
      ],
      "authentication": {
        "kind": "none"
      },
      "operations": [
        {
          "id": "fixture-read",
          "schema_digest": "5555555555555555555555555555555555555555555555555555555555555555",
          "declared_effect": "read",
          "required_authority": [
            "read"
          ],
          "resources": {
            "kind": "connection_wide"
          },
          "result_limits": {
            "maximum_bytes": 4096,
            "maximum_items": 10
          }
        }
      ],
      "events": [],
      "enforcement_contract": {
        "name": "fixture/mcp-client-policy",
        "version": {
          "major": 1,
          "minimum_minor": 0
        }
      },
      "diagnostics": "fixture-observe",
      "cleanup": "fixture-close"
    }
  ],
  "required_contracts": [],
  "host_services": [],
  "portable_mcp_entries": [
    {
      "name": "fixture-python",
      "component": "python-server"
    }
  ]
}
```

Replace these fields deliberately:

- `configuration` and `policy` select current host facts in the full activation
  form. Lifecycle and enforcement contracts must describe the actual selected
  composition and its owners; declaration is not proof that an owner ran or
  enforced them. Do not invent references or reuse old permission as authority.
- The artifact URL names the original wheel. `content_digest` hashes its exact
  bytes. `entrypoint.executable` is a real relative Python path inside that
  wheel, used as a module by the installer; it is not a shell command.
- `dependencies.artifacts` binds the exact requirements file and every retained
  dependency wheel. Stage them under `closure/`, preserving original filenames.
- Export operations describe the actual selected API and its schema evidence.
  A declared operation alone does not filter a whole MCP endpoint; see
  [operation selection and runtime enforcement](#operation-selection-and-runtime-enforcement).
  Installation, schema discovery and authorization remain separate observations.
- Keep package, strategy, component and portable MCP entry references consistent.
  The validator rejects unsupported combinations; do not remove an unresolved
  requirement merely to make a document pass.

The Python profile requires an independent local Python component with a pinned
closure, a service connector and a matching portable MCP entry. Read the
[Python installer contract](../../crates/toolkit-package/PYTHON-INSTALL.md) for
its precise limits. In an extracted kit, use `contracts/PYTHON-INSTALL.md`.

For the file-based path, stage these two files with the original `artifact.whl` and `closure/`. Pass
SHA-256 of each exact JSON file as `package_sha256` and `selection_sha256` to
`setup_selected_check`. For inline submission, pass the complete `package_json`
and `activation_json` strings instead of either hash; leave the acquired
artifact and closure in staging. These raw-file hashes differ from the typed package
content digest. Install only the resulting validated selection through
`setup_selected_install`. A valid selection is still not a running server or
authenticated account.

### Reuse an acquired artifact and closure

After this live setup session successfully acquires a Python or Node package,
`setup_selected_check` can take its exact retained reference alongside inline
metadata:

```json
"acquired_source": {
  "job_id": "EXACT_ACQUISITION_JOB_ID",
  "receipt_sha256": "EXACT_ACQUISITION_RECEIPT_SHA256"
}
```

Set `staged_id` to that same job ID and `profile` to the acquisition's Python
or Node profile. In the selected `declaration_json` or `activation_json`, the component named by `component` must
have a source containing exactly `kind: "artifact"` and the researched
`entrypoint`. Omit its `location`, `content_digest` and `dependencies` entirely.
The host reads the retained acquisition and fills only those three fields,
including every dependency pin. Supplying any of them as an override is refused.
The target component must occur exactly once. Other components and every other
metadata field are preserved for the canonical validator.

Continue authoring the package, entrypoint and operations from evidence.
Preserve any custom requirements and use the full activation path for their
host and lifecycle composition. This reference does not select an integration
or supply missing semantics. The same pinned validator checks the complete
expanded document of the explicit selected form. Its bytes, hash and exact acquisition reference
are retained in the ordinary proposal and bound by `effect_sha256`.

The host verifies the successful local job, exact receipt, current host bindings
and complete retained payload before and after checking and freezing. A changed,
unrelated, unsuccessful or recovered acquisition is refused. This form requires
inline documents; it cannot be combined with document hashes or MCPB. Inline
inputs retain their 32 KiB/document and 64 KiB/request limits. The expanded
selected document may exceed 32 KiB but must fit the existing 1 MiB document limit and
canonical validator limits. Installation, retry and recovery use the frozen
selection and do not need the old acquisition files. No acquisition, installation
or permission is created by materializing the reference.

For a [remembered integration](RECIPES.md), preserve the returned historical
package and selected document, and obtain the pinned artifact bytes separately.
A saved software declaration stays a software declaration; a full activation
requires current host references before reuse. Revalidate through the same setup
tools. Retention never renews permission, stores wheels or creates install/retry
authority. The new local-software recipe form carries `declaration_json` and
`declaration_digest`; legacy activation documents and digests remain unchanged.

## Declare protected environment credentials

For an investigated local Python/Node stdio service, the optional portable entry
field `environment_secret_slots` names every secret environment variable that the
selected launch needs. This works with either full `activation_json` or plain
`declaration_json`. Keep source evidence and the service's binding authentication
requirement; do not invent a native account, publisher configuration field or
host credential identity. For example, after verifying these literal names in
the selected package's documentation:

```json
{
  "name": "investigated-service",
  "component": "service-process",
  "operation_tools": {"read-items": "actual_read_items"},
  "environment_secret_slots": ["SERVICE_API_KEY", "SERVICE_TOKEN"]
}
```

The matching service export's `exports[].authentication` must use the complete
`binding` shape below. All four fields are required; `owner`, `requirement` and
`credential_audience` are Toolkit `Id` strings, not nested objects. The three
example IDs are placeholders for the investigated package's declared requirement:

```json
{
  "kind": "binding",
  "owner": "service-process",
  "requirement": "api-key",
  "credential_audience": "service"
}
```

Put this object in the service export, not the portable MCP entry. These IDs
describe the package's authentication requirement; they neither identify an
existing native account nor establish a credential binding or grant. Keep the
secret environment names in `environment_secret_slots`, and collect their values
only through the later protected input flow. `{"kind":"none"}` contradicts a
declared secret slot and is rejected. Unknown authentication fields are rejected
as well; do not guess additional token, account or binding fields.

The names are not values. The list contains 1-256 distinct names of at most 128
ASCII characters, starting with a letter or underscore and otherwise containing
letters, digits or underscores. The canonical validator sorts the names; it
rejects duplicates, empty lists, null, no-auth contradictions, missing operation
maps and remote/non-service use. Omission preserves the existing metadata and
software installation behavior. Other local process runtimes can describe these
requirements, but the current selected credential setup supports Python/Node.

Declare slots before `setup_selected_check`. The existing immutable plan,
installation approval and reconciliation retain them. The generic `mcp.json`
launch contains no values and does not gain native `secret_env` from this field.
After matched readback, an available standalone `toolkit_enable_secret_service`
can review the selected job and collect values through hidden terminal input.
Never add slots or credential values to that later tool call. Native realm
configuration, vault storage, access grants and actual service use remain
separate outcomes. Installed Console/Profile support and live qualification for
this route remain pending.
