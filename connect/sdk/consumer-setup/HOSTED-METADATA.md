# Author hosted MCP metadata

Use this complete format example with [the hosted recipe workflow](HOSTED-RECIPES.md).
It describes an existing HTTPS MCP service, not a package to install locally.
For local Python, Node or MCPB artifacts, use [selected package metadata](METADATA.md).

All `example.invalid` URLs, identities, timestamps, hashes, tool names and contract
references below are inert examples. Replace them with the agent's actual
investigation and the applicable contracts. Do not invent evidence,
approval records or readiness to make validation pass. The examples grant no
permission and make no claim about a real service.

The first three documents are retained together. The fourth is an independent
remote declaration used when exporting; it requires no host configuration or
policy snapshot. Send each document as its exact UTF-8 JSON string
in the corresponding setup-tool field, not as a filesystem path or nested object.
Each document is limited to 32 KiB; the complete encoded setup request is limited
to 64 KiB.

## Identifier syntax

Toolkit `Id` values are stable identifiers, not human display names. Each must
be 1-192 ASCII characters, start with a letter or digit, and contain only
letters, digits, `.`, `_`, `-`, `/`, `:` or `@`. Slash-separated segments must
not be empty, `.` or `..`; leading, trailing or doubled slashes are invalid.
Spaces, backslashes and non-ASCII characters are invalid. For example,
`example-provider` is an identifier; `Example Provider` is not.

These rules apply to the following fields in this example:

| Document | Fields typed as `Id` |
| --- | --- |
| Nested Toolkit package | `identity.publisher`, `identity.name`, `strategies[].id`, `known_exports[].id` |
| Recipe document | `maintainer`, `sources[].id`, `recipes[].strategy`, `recipes[].export`, `recipes[].plan.publisher`, unauthenticated `evidence`, `read_only_evidence` when present |
| Remote declaration | `package.publisher`, `package.name`, `strategy`, component `id` and `placement`, remote-source `publisher`, readiness `owner` and each `checks` entry, shutdown `owner` and `operation` |
| Declaration export and contract references | Export `id` and each `components` entry, operation `id` and each `required_authority` entry, enforcement-contract `name`, `diagnostics`, `cleanup`, portable MCP entry `name` and `component`; decision `owner` and `record` supplied to retention |

Other typed host references, including contract/service names, dependency
component IDs, state namespaces/owners and reversible rollback owners/operations,
use the same rules when those optional structures are selected. Identifiers
that refer to one another must match exactly; do not normalize a supplied host
reference or substitute a display label. The selected MCP server key must match
the portable entry's `name`, so choose an `Id`-compatible server name here.

The outer Agent Plugins `name` has a stricter rule: 1-64 lowercase ASCII letters,
digits, `-` or `.`, starting and ending with a letter or digit, with no `--` or
`..`. It must also equal the nested package name. Package/component versions,
legacy activation configuration revisions and a non-null `documented_api_version` are `Revision`
values instead: nonblank, at most 256 UTF-8 bytes and no control characters.
Do not apply `Id` spelling rules to those revisions, `display_name`, descriptions,
URLs or license text. Digests have their separate 64-character lowercase
hexadecimal SHA-256 format.

## 1. plugin.json

The Agent Plugins envelope uses the exact `ai.rkat.toolkit` extension name. Its
value is the complete Toolkit `IntegrationPackage`. `plugin.name` and `version`
match the nested package identity; the identity's publisher identifies the
metadata author. It need not be the operator of the remote service.

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
  "name": "hosted-example",
  "version": "0.0.0-example",
  "description": "Inert hosted MCP metadata example; not a ready connection.",
  "extensions": {
    "ai.rkat.toolkit": {
      "schema_version": 1,
      "identity": {
        "publisher": "example-author",
        "name": "hosted-example",
        "version": "0.0.0-example"
      },
      "display_name": "Inert hosted MCP example",
      "description": "Replace every example choice and reference from actual investigation and host context.",
      "provenance": {
        "source": "https://example.invalid/service/docs",
        "source_revision": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "license": "Example only; record the actual documentation and redistribution scope.",
        "notices": [],
        "local_patches": []
      },
      "strategies": [
        {
          "id": "hosted-https",
          "kind": "remote_mcp",
          "instructions": "Use the documented HTTPS endpoint; observe its actual schemas and obtain current host permission before calling tools.",
          "sources": ["https://example.invalid/service/docs"],
          "known_requirements": [],
          "unresolved_details": []
        }
      ],
      "known_exports": [
        {
          "id": "example.service",
          "role": "service_connector",
          "description": "Whole remote MCP endpoint, not a filtered proxy."
        }
      ],
      "assessments": []
    }
  }
}
```

`provenance.source_revision` must be `sha256:` followed by the exact retained
source observation's SHA-256, matching a `recipes.sources` entry for the same URL.
Hash the observed source bytes, not the URL string or a rewritten summary.
Documentation observations do not pin the hosted implementation. Every strategy
source URL must occur in the recipe's source list. Leave `assessments` empty
unless there is evidence for a specific support claim.

## 2. mcp.json

Retained Agent Plugins documents require `$schema`, even if a generic MCP client
would accept a bootstrap file without it. The portable recipe path supports an
HTTPS `streamable-http` entry without inline headers or query credentials.

```json
{
  "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
  "mcpServers": {
    "example-service": {
      "type": "streamable-http",
      "url": "https://example.invalid/service/mcp"
    }
  }
}
```

Keep credentials outside these documents. A private test CA is a host-client
connection input, not a reason to disable TLS validation or add an unsupported
field to this portable entry.

## 3. recipes.json

The recipe associates the package strategy, export and MCP server name. It also
retains the source observations supporting its claims. At least one concrete
prerequisite is required. An already hosted, anonymous service can still require
the host to attach the configuration and admit the requested operations.

```json
{
  "schema_version": 1,
  "maintainer": "example-author",
  "reviewed_at_unix_ms": 1,
  "sources": [
    {
      "id": "example-docs",
      "url": "https://example.invalid/service/docs",
      "observed_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "upstream": {"kind": "documentation_observation"},
      "license": "Example only; record the actual documentation and redistribution scope."
    }
  ],
  "recipes": [
    {
      "strategy": "hosted-https",
      "export": "example.service",
      "role": "service_connector",
      "prerequisites": [
        {
          "kind": "application_binding",
          "instruction": "The host must attach this endpoint and admit actual schema-checked calls; this example is not that approval."
        }
      ],
      "plan": {
        "kind": "remote_mcp",
        "server": "example-service",
        "publisher": "example-provider",
        "documented_api_version": null,
        "authentication": {"kind": "unauthenticated", "evidence": "example-docs"},
        "read_only": true,
        "read_only_evidence": "example-docs"
      }
    }
  ]
}
```

Replace the timestamp with the actual review time in Unix milliseconds.
`documented_api_version: null` means the documentation supplies no such version;
do not confuse it with the MCP protocol version or the metadata package version.
The remote publisher is repeated in the declaration's remote source below.

Use `authentication.kind: unauthenticated` only when a retained source used by
this strategy documents anonymous access. Its `evidence` is that source's ID.
Do not add a `client_authentication` prerequisite to this anonymous mode.
Omitting `authentication` preserves the legacy binding-required mode; its
service declaration then needs the real authentication owner's binding reference,
not `{"kind":"none"}`.

`read_only` is a documented property, not the user's requested task. Set it to
`true` only with its retained source ID in `read_only_evidence`. If that claim is
not supported, use `false` and `null`; the host must still classify and admit
each chosen operation. Neither value filters the remote server's tool list.

## Retain and obtain the two distinct digests

Call `setup_remember_remote` with `plugin_json`, `mcp_json` and `recipes_json`.
For the agent's current investigation, omit `decision`. The current recipe
owner retains the exact submitted documents and creates a stable reference to
that submission in the same durable recipe record. It remains valid after the
setup workspace is removed, and identical submissions keep the same reference.
No earlier decision record or human approval is needed to establish this
research provenance. Saving still requires the host's explicit retention scope.

If an actual historical decision reference already exists, supply it unchanged
as `decision: {"owner":"ACTUAL_OWNER","record":"ACTUAL_RECORD","revision":"ACTUAL_REVISION"}`.
The recipe owner preserves that external claim without verifying the referenced
owner. Neither form is approval, source authenticity, service access or readiness.
Use the updated pinned recipe-owner executable for omitted-decision submissions;
older frozen kits require an external reference and cannot read the new marked
submission records. Do not invent an external reference to work around that.

Read back the result with `setup_recipe` when needed. Keep these owner-returned
fields separate:

| Returned field | Use |
| --- | --- |
| Recipe `digest` | Exact retained bundle lookup and `setup_export_recipe.digest`. It covers the bundle, not just the nested package. |
| `package_content_digest` | `declaration.json.package_content_digest`. It identifies the validated nested Toolkit package. |
| A source's `observed_sha256` | Exact observed documentation bytes; it is neither of the above. |

Do not hash the pretty-printed plugin file and substitute that for either owner
digest. The owner's typed package digest includes its canonical/defaulted form.
The inspector described in [METADATA.md](METADATA.md) takes the nested Toolkit
package object, not this entire Agent Plugins envelope.

## 4. declaration.json

This is a complete one-component `RemoteExportDeclaration`. Its package digest
is the owner-computed digest of the exact inert package above. For your selected
package, use the owner's returned `package_content_digest` instead. The explicit
`selection_kind` keeps it distinct from legacy activation manifests. Repeated-digit
hashes and `example-host` contract references remain placeholders.

```json
{
  "schema_version": 1,
  "selection_kind": "remote_declaration",
  "package": {"publisher": "example-author", "name": "hosted-example", "version": "0.0.0-example"},
  "package_content_digest": "d135697e413ead09f79bedad9d511010955c6f7b4b1dbb3ec34ae460bdaa84f7",
  "strategy": "hosted-https",
  "components": [
    {
      "id": "remote",
      "version": "example-observation-1",
      "source": {
        "kind": "remote_service",
        "endpoint": "https://example.invalid/service/mcp",
        "publisher": "example-provider",
        "observed_contract": "4444444444444444444444444444444444444444444444444444444444444444",
        "transport": "mcp_streamable_http"
      },
      "runtime": "remote",
      "placement": "remote-https",
      "lifetime": "externally_managed",
      "activation_scope": {"kind": "independent"},
      "dependencies": [],
      "required_contracts": [],
      "host_services": [],
      "state": [],
      "readiness": {"owner": "example-host/mcp-client", "checks": ["observe-complete-schemas"]},
      "shutdown": {
        "owner": "example-host/mcp-client",
        "operation": "close-and-join-local-client",
        "maximum_wait_ms": 30000,
        "timeout": "preserve_uncertainty"
      },
      "rollback": {
        "kind": "irreversible",
        "explanation": "The client does not own the hosted service or its rollback; local context cleanup remains the host client's responsibility."
      }
    }
  ],
  "exports": [
    {
      "role": "service_connector",
      "id": "example.service",
      "components": ["remote"],
      "authentication": {"kind": "none"},
      "operations": [
        {
          "id": "example_read",
          "schema_digest": "5555555555555555555555555555555555555555555555555555555555555555",
          "declared_effect": "read",
          "required_authority": ["example-host/explicit-read-permission"],
          "resources": {"kind": "connection_wide"},
          "result_limits": {"maximum_bytes": 16384, "maximum_items": 50}
        }
      ],
      "events": [],
      "enforcement_contract": {"name": "example-host/operation-policy", "version": {"major": 1, "minimum_minor": 0}},
      "diagnostics": "example-host/checker-report",
      "cleanup": "example-host/close-client"
    }
  ],
  "required_contracts": [],
  "host_services": [],
  "portable_mcp_entries": [{
    "name": "example-service",
    "component": "remote",
    "operation_tools": {"example_read": "provider_read"}
  }]
}
```

Do not add `configuration`, `policy` or `source.configuration_revision` to this
form. Copying an independent endpoint has no host activation effect and cannot
attest to those snapshots. The endpoint must match `mcp.json`, the remote publisher
must match the recipe, and strategy/export/component names must join exactly.
Actual native review owns current configuration, consumer and lifetime selection.
Full Toolkit activation still requires its existing `ActivationManifest` and host
compatibility checks; this export does not bypass them.

`observed_contract` identifies the actual retained MCP schema observation for
this endpoint. Observe it through the host-admitted checker or MCP client before
authoring this declaration; documentation alone does not establish its current
schemas. Record which observation bytes were hashed. Each `schema_digest`
identifies the corresponding actual tool input schema using the host's recorded
hashing convention. Copy neither placeholder hashes nor unobserved tool names.
The declaration validator checks consistency, not the truth or freshness of that
observation. Native admitted schema observation and HTTP egress ownership remain
separate prerequisites before protected remote publication; this format does not
supply a checker, network permission or a native observation receipt.

Readiness, shutdown, rollback, required authority and enforcement fields are
package contract declarations referring to applicable client/host contracts. The exporter validates their
shape and consistency; it does not execute checks, create grants or certify
those owners. `authentication: {"kind":"none"}` is permitted here because the
retained recipe explicitly documented anonymous access, not because a fake
authentication owner was invented.

The empty dependency, state and host-service lists describe this independent
remote example. Do not erase real dependencies to force portability. A hosted
recipe selects exactly one remote component and one service export; the
`portable_mcp_entries` name is the retained `mcpServers` key, not the tool name.

For public-service connection, include `operation_tools` on the selected
portable entry. Its keys are every selected service operation ID, exactly once;
its values are the distinct actual raw MCP tool names from the retained source
evidence. In this inert example, the declared operation `example_read` maps to
the server tool `provider_read`; the names need not be identical. The exporter
validates the mapping and returns it with the explicit authentication declaration
as `declared_remote_service`. Omitting the map still permits configuration
export, but supplies no public-service activation evidence. A mapping remains a
declaration; it does not prove a connection, current schemas or permission to
invoke those tools.

Call `setup_export_recipe` with the retained publisher/name/version and bundle
`digest`, plus the exact declaration string as `declaration_json`. Observe its job
and exported files using [HOSTED-RECIPES.md](HOSTED-RECIPES.md). Success produces
ordinary whole-endpoint MCP configuration, not an operation-filtered proxy.
Connection, TLS trust, account access, per-call permission and assessment of the
task result remain separate host/client steps.

Where the consuming host exposes Connect's native protected attachment, it can
separately enforce an approved subset of exact MCP tool names. A server's
separate write tools do not by themselves exclude a protected read subset.
An operation-name allowlist does not constrain query semantics, prevent startup
effects or provide filesystem confinement. Inspect the allowed tools and
server behavior, and require the host's separate resource controls. Metadata
and export do not provide that authority. See
[operation selection and runtime enforcement](METADATA.md#operation-selection-and-runtime-enforcement).

Existing callers may instead send a complete legacy `ActivationManifest` as
`activation_json`. Supply exactly one of these fields. Legacy canonical bytes,
manifest digests and saved requests keep their original meaning. The new form
returns `selection_kind: remote_declaration` and a distinct `declaration_digest`
in its service declaration; neither digest is an activation or readiness receipt.
