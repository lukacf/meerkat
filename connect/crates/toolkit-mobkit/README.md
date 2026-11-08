# Optional MobKit tool adapters

The crate supplies separate read-only discovery tools and the retained
`find_and_install` admission protocol. Applications explicitly register the
appropriate dispatcher in a native bundle and select which profiles receive it.
Neither adapter changes an agent's capabilities merely by being linked.

## Public discovery

`discovery::DiscoveryTools::new(&catalog)` takes an explicitly public
`PackageCatalog`. Hosts must keep private connection records and restricted
catalogs outside this adapter. The constructor makes an immutable snapshot and
rejects oversized metadata.

- `list_integration_recipes` returns compact pages in exact package-identity
  order. An optional exact `name` chosen by the agent returns all matching
  publishers and versions; it does not rank them or infer intent. Summaries
  contain identity, digest, display name, description, roles and optional
  library digest. Pass the same name and returned snapshot/cursor for the next
  page. A changed inventory or name filter invalidates the old cursor.
- `read_integration_recipe` takes an exact identity and package content digest.
  It returns the complete recipe, provenance, assessments and unresolved setup
  requirements. This pins recipe content, not an earlier catalog origin.

Both tools are read-only. They perform no network fetch, shell execution,
installation, authentication or semantic matching. Recipe text and catalog
claims are untrusted investigation evidence. The agent decides what to
investigate and may search outside the inventory. Service connectors and agent
frontends remain independent roles.

The older metadata-only catalog, available through
`toolkit_package::bundled_discovery_catalog()`, retains ten experimental research
entries. The versioned library is available through
`toolkit_package::bundled_integration_library()` and
`DiscoveryTools::from_library(&library)`. It retains 14 package versions across
11 identities and 17 strategies: 6 remote service recipes and 11 guidance-only
recipes, all experimental. These are description counts, not functioning-service
counts. Gmail, Slack and Calendar keep `0.1.0` and `0.2.0` as separate choices;
strategy failure does not trigger fallback. Library
reads require the returned `library_digest` as well as the package digest;
snapshots include retained prerequisites and endpoint configuration.

The [historical Gmail six-case discovery run](../../docs/coordination/20260930-upstream-review/gmail-library-successor-verification/20261001T044817.044748Z/summary.json)
checks both constructors, exact bundle reads, complete paging, changed
prerequisites, stale digests, strict fields, empty inventories and metadata
limits. It also records 7 library cases, 2 bundled-metadata cases, and scoped
strict Clippy and formatting passes. This uses default features on the recorded
local Meerkat 0.8.44 graph, with optional MobKit 0.8.43 and `native-host` disabled.
It exercises this crate's actual discovery dispatcher, not a MobKit member or
setup-agent continuation.

From the repository root, explicitly select the
[native candidate workspace](../../native-candidate/README.md) and its checked-in
lock. The current manifest selects the development sources described in
[the build guide](../../BUILDING.md); do not overlay the historical discovery
run's patch configuration. These local development prerequisites do not establish
a released native integration:

```sh
./scripts/repo-cargo test --manifest-path native-candidate/Cargo.toml --locked -p toolkit-mobkit --test discovery
```

See the [library contract](../toolkit-package/integrations/README.md),
[current Gmail verification](../toolkit-package/GMAIL-VERIFICATION.md), and
[independent execution closure](../../docs/coordination/20260930-upstream-review/gmail-library-successor-independent-review/execution-closure/REVIEW.md)
for exact selection and evidence limits. Actual setup-agent composition and
usable-account verification remain required work.

## Acquisition admission

`FindAndInstallTool` advertises one mutating tool, `find_and_install`, for
explicitly selected agents. Direct construction with `FindAndInstallTool::new`
retains standalone protocol dependencies but cannot enroll an installed native
owner. Its context-aware production dispatch returns unavailable before host
admission, reference resolution or storage access. Context-free dispatch refuses
access.

On Unix with `native-host`, the installed path uses
[`InstalledAcquisitionTools`](src/installed_host/acquisition/enrollment.rs).
The host's installed acquisition must adopt those exact dispatcher objects and
retain their owner and bound native runtime. Constructing the tools does not
register a bundle or grant permission; an unbound, retired or dropped owner
cannot serve calls. The [dispatcher](src/lib.rs) enters the exact reviewed effect
once before host admission and carries the entered native operation through
acquisition and worker advancement. This implemented source path still requires
the native and installed qualification described in [the build guide](../../BUILDING.md).
The standalone protocol fixtures do not qualify that path.

The model supplies original request text, supplied context and labeled
observations. Strict deserialization rejects authority, routing, package,
backend and correlation fields, including nested authority claims. Integration
selection belongs to setup agents.

Host admission uses `SetupToolHost::admit_invocation`, including on replay. Its
application implementation must authenticate the actual
runtime invocation, original interaction/task custody, selected optional
capability and current requester ceiling. Public IDs, model call IDs and old
snapshots cannot establish those facts. Exact retry correlation survives a lost
reply while current permission is checked again.

A successful protocol admission commits the acquisition and work-admission
handoff before replying. The reply identifies the operation, generated
revision/phase and whether this invocation created the operation. Installed
dispatch can also return a worker observation and, when recorded by the
acquisition, an observation that the approved follow-up instruction was admitted.
The agent reports its business-task result separately. An admission receipt alone
does not prove setup completion, worker launch or task success. A timeout can
race a durable commit, so an exact retry recovers the same operation rather than
creating another. Expired missing requests cannot create work; historical
recovery does not restore effect permission.

The historical native fixtures use
`MobBootstrapSpec::register_tool_bundle("toolkit", dispatcher)` and profile
selection through `tools.rust_bundles`. The recorded
[twelve-test native fixture](../../docs/reviews/mobkit-child-native-fixture/verification/20260929T222850.900201Z/manifest.json)
checks selected-profile visibility, child inheritance, revival and explicit setup
refusal through real local runtime and SQLite sessions. The separate
[discovery fixture](../../docs/reviews/discovery-native-composition-fixture/verification/20260929T230023.193890Z/manifest.json)
checks actual recipe dispatch, hidden controls and retained history across revival
and cold bootstrap with a deterministic LLM. Both pass strict fixture and actual
MobKit library lint. Earlier successful setup admission/replay remains historical
protocol evidence. These fixtures predate the current installed-owner path and
do not qualify its authenticated invocation, setup-team launch or original-task
continuation. Current qualification remains pending; no source description or
historical fixture result certifies a connected account or frontend.
