# toolkit-package

Inert integration descriptions and exact activation selection validation for the
Meerkat Toolkit. This crate never executes package code, chooses integrations,
grants access, or declares a service/frontend ready.

## Data flow

1. Read an `IntegrationPackage` or import portable metadata. Discovery strategies
   may leave provider configuration unresolved. Source identity, version and
   provenance stay explicit.
2. An agent selects the integration and completes an `ActivationManifest`.
3. `manifest.validate(&package)` validates the pinned package, selected strategy,
   role declarations, dependency closure, state/lifecycle contracts and event
   delivery semantics. It returns a `ValidatedManifest` with a payload digest.
4. `selection.check_host(&capabilities)` reports missing contracts and declared
   optional degradation before integration code is imported.
5. Policy, credential, deployment and runtime owners handle authorization and
   effects. Their evidence must refer to the exact selection digest. Successful
   validation or compatible metadata does not substitute for those receipts.

Configuration and policy are owner-bound digest references. Secret values do not
belong in the manifest. A remote service records endpoint identity and observed
contract; it does not pretend to have immutable implementation bytes.

Both roles use the same selected component graph. A connector has operations or
service events. A frontend has admission, conversation, output and optional live
contracts; it need not provide any tools. Components not reachable from selected
exports are rejected. Request-lifetime components cannot support a persistent
frontend or daemon dependency. `state_changes_from` reports required migrations
and retained state on removal without performing them.

## Portable metadata

`ImportedPluginManifest` and `ImportedMcpServers` implement inert parsing for
[Agent Plugins 1.0.0](https://agent-plugins.org/specification). Unknown top-level
manifest fields are ignored with diagnostics; invalid known fields reject the
manifest. Unknown extension namespaces stay opaque. A bad MCP entry is isolated
from valid sibling entries. `ai.rkat.toolkit` is the toolkit namespace.
Duplicate JSON object keys at any depth reject the entire input document before
typed validation, including conflicting transport, credential or toolkit-contract
fields. Diagnostics do not include those keys or values. This ambiguity check
does not change ordinary invalid-entry isolation or case-duplicate header checks.

`PluginRoot` anchors reads to a capability directory handle, checks containment
through `cap-std`, rejects non-regular files and bounds metadata reads. It is not
an execution sandbox or artifact verification service. A backend must verify the
selected artifact bytes at realization. Placeholder expansion is literal and
single-pass; imported environment/header data is not a portable secret store.

`ImportedMcpbManifest` parses the [MCPB 0.3 manifest](https://github.com/modelcontextprotocol/mcpb/blob/main/MANIFEST.md)
and exposes unresolved contracts and sensitive configuration requirements. It
retains platform metadata without claiming it has been implemented. Bundle
archive extraction, executable verification and launch are outside this parser.
Import success does not mean executable compatibility or toolkit support.

Portable MCP projections may only include independent components. A component
that needs toolkit contracts, host services, package state or dependencies cannot
also be exported as a generic entry that ignores those requirements.

`PortablePluginExport::new` produces Agent Plugins files from validated values.
Absent optional fields stay absent, unknown namespaces remain opaque and
placeholders remain literal. Toolkit MCP entries require a `ValidatedManifest`
that declares them portable, with matching names, endpoints and transports.
Artifact realization is explicitly unsupported. `with_file` copies inert skill
files through `PluginRoot`; `write_new` writes into a new directory without
replacing existing files. It is a file export, not an atomic or durable install.

`IntegrationBundle::export_portable` additionally requires the exact bundle
digest and a concrete `ActivationManifest`. It validates the selected recipe
through the same owner as `validate_selection`, then exports only that recipe's
retained endpoint. Host compatibility is not claimed by an inert export.
The [local operator CLI](LOCAL-RECIPES.md#export-an-independent-mcp-endpoint)
can export an exact saved choice while returning its local decision provenance.

A generic client receives the whole MCP endpoint, not an operation-filtered
proxy. Toolkit operation declarations, resource restrictions, result limits and
grants are not transferred or enforced by `mcp.json`. The client owns its own
authentication and access policy. Export does not execute instructions, launch
servers, expand placeholders or resolve credentials. Derived `Serialize`
encodings remain unchanged because retained bundle digests depend on them.

Connect's native protected attachment can separately enforce a host-approved
subset of exact tool names. Separate write tools do not by themselves rule out
that read subset; allowed query semantics, startup effects and filesystem
confinement still need independent assessment and controls. See
[operation selection and runtime enforcement](../../sdk/consumer-setup/METADATA.md#operation-selection-and-runtime-enforcement).

## Validation

`bundled_adopted_catalog()` separately exposes the adopted local MCP profiles as
scoped compatibility metadata. It is generated from the canonical OSS inventory;
see [catalog inputs](catalog/README.md). The current isolated default-feature
verification is retained under `docs/verification/20261003-adopted-catalog`.
It does not resolve or build native Meerkat/MobKit dependencies.

The [versioned library](integrations/README.md) retains 14 package versions across
11 identities and 17 strategies: 6 remote service recipes and 11 guidance-only
recipes. Gmail retains both `0.1.0` and `0.2.0`; versions and strategies are
explicit agent choices. See [current Gmail verification](GMAIL-VERIFICATION.md)
for the local SDK artifacts and the later Node 22.0.0 receipt.

The accepted catalog checks use the existing
[Cargo patch configuration](../../docs/reviews/native-mobkit-propagation-verification/upstream-patches.toml)
and unchanged lock on the historical local Meerkat 0.8.44 development graph.
The optional MobKit package is 0.8.43; these checks use default features and do
not enable `native-host`. From the repository root:

```sh
./scripts/repo-cargo test --config docs/reviews/native-mobkit-propagation-verification/upstream-patches.toml --locked -p toolkit-package --test integration_library
./scripts/repo-cargo test --config docs/reviews/native-mobkit-propagation-verification/upstream-patches.toml --locked -p toolkit-package --test bundled_discovery
./scripts/repo-cargo clippy --config docs/reviews/native-mobkit-propagation-verification/upstream-patches.toml --locked --no-deps -p toolkit-package -p toolkit-mobkit --all-targets -- -D warnings
```

The [recorded successor run](../../docs/coordination/20260930-upstream-review/gmail-library-successor-verification/20261001T044817.044748Z/summary.json)
passed 7 integration-library cases, 2 bundled-discovery cases, and the separate
6-case MobKit discovery-adapter suite, plus scoped Clippy and formatting checks.
The [independent evidence closure](../../docs/coordination/20260930-upstream-review/gmail-library-successor-independent-review/execution-closure/REVIEW.md)
records the exact graph and retained earlier failures. This is catalog and
adapter evidence, not current-main native or live-provider verification.

The crate's broader tests cover partial recipes, immutable selections, separate roles, dependency
cycles/version mismatches, migration obligations, metadata namespace isolation,
unsupported contracts, durable versus best-effort events, containment attacks,
exact payload changes, and imported configuration failure boundaries. These are
contract tests, not live account, deployment or hardware validation.

## Optional local byte preparation

On Linux and macOS, the `local-artifacts` feature exposes
`local_artifacts::LocalArtifactPreparer`. A trusted host supplies a capability
root dedicated to one host-recorded preparation attempt, a `ValidatedManifest`,
the selected component id, an explicit layout and already-open local input files.
The host records that root with its effect intent before calling preparation.
The root has fixed `incomplete` and `prepared` child locations, so recovery does
not depend on a returned random locator. Distinct attempts use distinct roots. Preparation rewinds each input, freezes and
checks its selected SHA-256, materializes a private tree, records complete file
and directory membership, and publishes without overwriting another result.
Nothing is imported, executed, downloaded or authenticated.

The two layouts are a single file at the selected entrypoint, with explicit
execute intent, and a strict ZIP32 profile. ZIP input supports Stored and
Deflate, ASCII relative paths and ordinary files/directories. Version 1 rejects
ZIP64, extra fields, encryption, data descriptors, split archives, archive
prefixes/gaps/trailing data, non-ASCII names, case aliases, links, special files
and platform-ambiguous paths. Unsupported input fails explicitly. ZIP metadata
is bounded before decoding; the library streaming decoder verifies CRC and
actual expanded-byte limits. This is not a general ZIP or wheel installer.

`SelfContained` supplies no dependency files. `PinnedClosure` supplies exactly
the selected dependency ids and verifies their opaque bytes; it does not build
an interpreter environment. `Locked` requires a separate ecosystem-realization
effect. Remote and host-provided sources do not become fictional local files.

A `PreparedLocalArtifact` provides bounded, freshly checked reads and its inert
`PreparedArtifactEvidence`. Persist the evidence through the existing setup
store. `reopen_verified` requires the original expected component and layout from the
host's persisted intent, independently of disk evidence. It compares the stored
receipt, verifies pins again, re-derives the expected inventory from raw bytes,
and checks exact tree
membership, types, modes and contents. A self-consistent edited inventory cannot
certify edited output. Locator strings and deserialized evidence confer no
filesystem or execution authority.

Final directories and executable files are owner-read/execute (`0500`), other
files are owner-read (`0400`). These API-level immutable artifacts are not a
sandbox against the trusted filesystem owner. A future launcher must preserve
verified bytes through its own execution boundary. File/directory sync and a
no-replace rename must succeed before returning success; a post-publication
sync failure returns `PublishedButUnconfirmed` with a locator for reconciliation.
No durability claim exceeds the host filesystem's sync guarantees.

The fixed incomplete directory remains available to the existing effect owner's
cleanup/reconciliation logic. Dropping a handle does not remove published bytes.
Callers must persist their decision/effect intent before preparing, retain worker
ownership through cancellation, and obtain current approval for later effects.
Preparation evidence attaches to an exact activation proposal; it does not grant
execution or establish service/frontend readiness.

The source tests for this optional path run with:

```sh
./scripts/repo-cargo test --config docs/reviews/native-mobkit-propagation-verification/upstream-patches.toml --locked -p toolkit-package --features local-artifacts local_artifacts::tests
```

They use synthetic bytes and temporary directories, with no plugin execution.
This optional feature suite is separate from the catalog successor receipt above.

## Optional local recipe reuse

The `local-recipes` feature exposes `LocalRecipeStore`, which saves an
agent-selected bundle outside the catalog into a host-injected durable
`toolkit_store::ToolkitStore` namespace. The authenticated host supplies the
`StoreScope`; tool input never selects it. A save parses the exact plugin, MCP
and recipe documents through `IntegrationBundle::parse` and admits them, with
the agent's decision evidence, under the package identity. The record id is
`local-recipe/v1/` followed by the publisher, name and version, each written as
`<byte length>:<value>`, so identities whose components contain `/` cannot
collide. Replaying the same
documents and decision is idempotent; other content under that identity is an
immutable conflict. A store that does not declare durable unique admission is
refused, and an unacknowledged save is reported as unconfirmed, never as saved.

Every load re-verifies the store record digest, the saved bundle digest, the
identity key and the parse of the exact bytes; `load_exact` also requires the
selected bundle digest. A record naming any identity other than the requested
one is refused with `IdentityMismatch`. Failed records are listed as unverified, never returned
as reusable. Loaded bundles carry `CatalogOrigin::LocalInvestigation` with their
decision, and a curated or imported `IntegrationLibrary` refuses them instead of
relabeling them. Only the documents and decision are stored. The caller must
sanitize them before saving; validation cannot detect every secret in free text.
The store does not collect environment values, grants, readiness, events or a
setup journal. Reuse does not install,
activate or claim service access, and guidance-only recipes stay guidance-only.
The public discovery tools do not read local recipes; a host that wants them
listed builds its own local-investigation library.

The [local operator guide](LOCAL-RECIPES.md) covers the feature-gated
`toolkit-local-recipes` consumer and its explicit host scope. Save replay checks
the exact returned documents and decision before reporting success. Repeated
library insertion also refuses conflicting local provenance instead of silently
retaining the first decision.
