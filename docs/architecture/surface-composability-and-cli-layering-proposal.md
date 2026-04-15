# Surface Composability and CLI Layering Proposal

Status: Implemented in stages; retained as the parent rationale and command-layering record
Scope: Surface composition, product layering, and CLI organization for Meerkat
Related:
- `docs/architecture/meerkat-runtime-dogma.md`
- `docs/architecture/RMAT.md`
- `docs/architecture/finite-ownership-ledger.md`

## Problem

Meerkat's base architecture is already strongly composable:

- `meerkat` exposes reusable factory, session-service, persistence, and
  runtime-backed surface seams
- `meerkat-comms` can be used without pulling in the full product stack
- `meerkat-rpc`, `meerkat-rest`, and `rkat` all run on the same underlying
  runtime-backed execution model

The current pain is not that Meerkat lacks composability. The pain is that the
product surfaces, especially `rkat`, have accumulated their own thick bootstrap,
executor, and UX layers on top of the shared substrate.

That creates three related problems:

1. smaller supported surfaces are harder to build than they should be
2. the batteries-included CLI risks baking in assumptions that should instead
   live in shared composition seams
3. the user-facing CLI surface is larger and more internally shaped than a
   human-first tool should be

We need a plan that improves composability first, then uses that improved
substrate to cleanly separate:

- batteries-included product UX
- smaller focused surfaces
- advanced/operator capabilities

## Investigation Summary

The current codebase already supports two important composition levels:

### 1. Embedded / minimal substrate

This is the direct `AgentFactory` + `SessionService` path for testing and
embedded use. It supports small compositions without the full runtime-backed
product shell.

### 2. Runtime-backed product substrate

This is the canonical product path:

- `PersistentSessionService`
- `MeerkatMachine`
- runtime bindings from `prepare_bindings(...)`
- shared surface helpers in `meerkat::surface`

The shared helper layer is real. The main issue is that product surfaces do not
all consume it consistently end-to-end.

In practice:

- the reusable architecture exists
- REST and RPC use parts of it
- `rkat` duplicates more bootstrap and wrapper logic than it should

This is good news. It means the main task is surface consolidation and clearer
layering, not a machine-architecture rewrite.

## Dogma Constraints

This proposal must preserve the current runtime and machine authority rules:

- machine semantics stay owned by the canonical machines and compositions
- surfaces remain skins, not authorities
- helper layers may remove duplication, but must not invent new semantic truth
- public UX may differ from runtime lowering, but the lowering must still route
  through canonical owners

In particular, this proposal is not permission to:

- move runtime semantics into CLI code
- create a parallel surface-owned routing or lifecycle model
- introduce a second app-facing identity system beside existing domain handles

## Recommendation

Treat this as a three-workstream effort:

1. Make surface composability first-class
2. Define product surface strategy on top of that composable base
3. Redesign `rkat` UX and command organization to reflect the new layering

This order matters. If `rkat` UX is redesigned first, it will likely freeze in
more product-specific assumptions that then become harder to remove from the
surface stack.

## Current shipped outcome

The shared-surface/runtime work from this proposal is now reflected in the codebase:

- shared embedded and runtime-backed surface helpers back the main product surfaces
- `rkat`, `rkat-rpc`, `rkat-rest`, and `rkat-mcp` consume those shared seams
- `e2e-build` proves minimal composed fixtures plus built product surfaces
- the canonical CLI execution family is centered on `run --resume[=<SESSION>]`

The product-layering work also shipped two official slim binaries:

- `rkat-mini`
- `rkat-rpc-mini`

These are static capability-gated builds of the existing CLI/RPC crates, not
new reimplementations and not runtime-dynamic command subsets. On the CLI side,
optional command surfaces now follow compiled capability features such as
`mob`, `mcp`, and `skills`, rather than a profile-specific hide/show switch.

## Workstream 1: Make Composability Great Again

### Goal

Make small and large Meerkat surfaces compose from the same shared building
blocks without requiring each surface to recreate its own bootstrap shell.

### Expected Scope

This workstream should mostly affect:

- shared surface bootstrap helpers
- common runtime-backed wiring helpers
- reusable surface composition utilities
- packaging and feature composition
- reference examples for supported surface sizes

It should not, by default, require changes to:

- machine semantics
- `MeerkatMachine` control-plane semantics
- mob semantics
- core runtime ownership rules

If a missing shared seam is discovered, we may need to lift a surface-local
mechanism into a shared helper. But that should still be a surface-layer change
unless the missing behavior is truly semantic.

### Current Findings

The codebase already has reusable pieces for:

- factory-backed embedded services
- runtime-backed service wiring
- session materialization
- persistent runtime executors
- peer ingress context updates
- schedule host composition

The missing part is not the raw architecture. The missing part is a sharper,
more discoverable surface-composition layer that product surfaces actually use
consistently.

### Deliverables

1. Inventory the current shared surface seams and the duplicate wrappers in:
   - `rkat`
   - `meerkat-rpc`
   - `meerkat-rest`
2. Define which bootstrap and executor patterns should be canonical shared
   helpers.
3. Extract or normalize reusable surface composition helpers where duplication
   is high.
4. Document supported composition tiers:
   - embedded/minimal
   - runtime-backed minimal
   - batteries-included product

### Success Criteria

- a small custom CLI can be built from supported shared helpers
- `rkat` stops being the only practical way to ship a Meerkat CLI
- surface-specific code is mostly transport and UX adaptation, not runtime
  reconstruction

## Workstream 2: Product Surface Strategy

### Goal

Define which user-facing products Meerkat should support and how they map onto
the composable substrate.

### Key Distinction

This workstream must separate:

- user-facing collaboration UX
- advanced/operator UX
- low-level substrate capabilities such as comms

The earlier investigation suggests:

- comms is a valid first-class primitive for minimal or embedded builds
- comms is usually not the right default abstraction for ordinary end users
- mobs and higher-level collaboration flows are the better default abstraction
  when the user wants agents to work together

### Main Questions

1. What product surfaces do we intend to ship?
   - one batteries-included `rkat`
   - smaller official CLIs
   - surface templates or examples
2. How should cross-machine collaboration feel in user-facing UX?
3. When should users see collaboration concepts versus direct comms concepts?
4. What minimal compositions should be explicitly supported and documented?

### Important Constraint

This workstream must not assume that plain `rkat run` is a mob member by
default. Ordinary session/turn UX and mob-member identity remain separate
concepts unless the runtime explicitly lowers a given feature into an implicit,
session-owned mob.

### Deliverables

1. Define the supported product tiers and what each includes.
2. Define the collaboration-versus-comms split for user-facing surfaces.
3. Decide whether a stable cross-machine host concept is needed and, if so,
   where it belongs:
   - runtime substrate
   - operator surface
   - product UX
4. Produce one or more concrete end-to-end stories:
   - tiny comms-only custom binary
   - small runtime-backed CLI
   - batteries-included collaborative CLI

## Workstream 3: `rkat` UX and Command Organization

### Goal

Redesign `rkat` so it is human-friendly first, pipeline-friendly second, and
reflects the actual layering of the shared surface stack.

### UX Principles

The new CLI should follow these principles:

- human-friendly first, pipeline-friendly second
- one canonical mental model for execution
- one name per concept across commands
- basic help should teach everyday usage, not expose internal wiring
- advanced/operator capabilities should remain available, but clearly layered
- user-facing syntax may be friendlier than the underlying lowering, as long as
  the lowering still routes through canonical owners

This implies a few practical rules:

- prefer a small set of memorable default commands and flags
- avoid exposing product-internal or source-identity plumbing in the primary UX
- keep per-run concerns distinct from realm-persistent concerns
- keep deployment/build concerns distinct from both
- avoid introducing surface syntax that only makes sense for the fully loaded
  product build if smaller compositions are meant to be supported

### Working Direction

The current direction already looks promising:

- one main execution verb
- clear distinction between runtime overrides, realm config, and build/deploy
  utilities
- smaller default help surface
- advanced/operator controls clearly separated from everyday task UX

### Constraints

The new CLI should play well with Workstream 1:

- only included capabilities should appear in the surfaced UX
- batteries-included `rkat` should be one supported composition, not the only
  composition
- CLI syntax should be a user-facing skin, not a second semantic authority

### Candidate Organization Model

The current working model is:

- runtime / ephemeral overrides
- realm / persistent config
- deployment / build utilities

This needs to be validated against the final surface strategy from Workstream 2.

### Proposed CLI Groupings

The current working groupings are:

#### 1. Runtime / ephemeral

This is "what should happen for this invocation?"

Examples:

- prompt execution
- resume/continue behavior as execution options
- model/provider overrides
- tool presets
- temporary skill inclusion
- temporary peer/collaboration inclusion
- stdin/streaming/keep-alive behavior

These should be optimized for:

- interactive use
- shell pipelines
- one-off automation

#### 2. Realm / persistent config

This is "what should this realm know or remember by default?"

Examples:

- saved skills
- saved MCP server configuration
- default model/tool settings
- stored peer aliases or collaboration defaults, if supported

These should be explicit, boring, and clearly persistent.

#### 3. Deployment / build

This is "package, validate, transform, or publish artifacts."

Examples:

- mobpack packaging
- validation
- inspect/build/deploy flows
- future WASM or bundle utilities

These commands can be more operational and do not need to share the same UX
shape as `run`.

### Default Versus Advanced Views

The CLI should have an intentional "basic view" and "advanced/operator view."

The basic view should surface:

- starting a new run
- continuing an existing run
- choosing a model
- choosing an output mode
- selecting a tool preset
- attaching obvious per-run collaboration inputs

The advanced/operator view should surface:

- low-level comms or transport controls
- raw policy overrides
- advanced tool filtering
- direct mob/operator controls
- packaging/build/deploy commands

This split is important both for usability and for composability. Smaller
surface compositions should be able to expose only the portions they actually
include without inventing a second documentation or command taxonomy.

### Working Syntax Direction

The current design discussion suggests:

- one main execution verb, with resume/continue semantics folded into execution
  options rather than separate top-level verbs
- a user-facing distinction between:
  - per-run overrides
  - realm config
  - deployment/build utilities
- collaboration-oriented syntax for ordinary users
- lower-level comms/operator syntax only where the surface is explicitly
  advanced or minimal

The exact command tree remains open, but the proposal direction is:

- execution should feel like one cohesive family
- realm configuration should use explicit resource-management command groups
- build/deploy tooling should be clearly separate from day-to-day execution

### Tentative Command Map

The following map is intentionally tentative. It captures the current working
direction from the design discussions and is meant to preserve good ideas and
migration targets, not to freeze final syntax prematurely.

Some parts of this map are more settled than others:

- the runtime / realm / deployment grouping looks strong
- collapsing `run` / `resume` / `continue` into one execution family looks
  strong
- `skill` as a singular resource command looks strong
- peer-related runtime syntax remains provisional and depends on Workstream 2

### Resolved Versus Open

The current CLI direction is a mix of strong recommendations and still-open
questions. This section is meant to make that explicit so later work can move
faster without re-debating already solid ground.

#### Strong current recommendations

- group the CLI into:
  - runtime / ephemeral
  - realm / persistent config
  - deployment / build
  - utility / introspection
- keep bare `rkat "prompt"` as friendly execution shorthand
- make `rkat run` the canonical explicit execution verb
- collapse `run` / `resume` / `continue` into one execution family with
  `--resume`
- treat `--resume` without a value as `--resume last`
- rename `skills` to singular `skill`
- replace `--preload-skill` with a user-facing `--skill <PATH_OR_ID>`
- deprecate `--skill-reference`
- de-emphasize internal/source-shaped flags from the main help surface
- treat `mob` primarily as deployment/build plus advanced operator surface
- separate default/basic help from advanced/operator help

#### Directionally strong, but still needs concrete design

- peer should be runtime-first rather than primarily realm-configured
- collaboration-oriented UX should be the ordinary user-facing path
- lower-level comms/operator UX should remain available for advanced and
  minimal surfaces
- smaller official CLIs or supported surface compositions should be possible
  from the same substrate as the full `rkat`
- only included capabilities should appear in a given surface's help and UX

#### Still open

- the exact peer syntax and transport/bootstrap model
- whether `peer` should also have a first-class realm-persistent command family
- whether `sessions` and `realms` should be renamed to singular forms
- whether there should be a user-facing noun such as `team` in addition to or
  instead of some current `mob` concepts
- whether runtime one-off collaboration should have a dedicated `--team` or
  `--mob-definition` style flag
- whether `models` should remain a namespace or be flattened to a single
  command
- how much of the final CLI tree should be visible in smaller official
  compositions versus only in the batteries-included `rkat`

#### Buckets

- `run`: ephemeral/runtime
- config/resource commands: realm-persistent
- mob pack-style commands: deployment/build
- doctor/models/capabilities: utility/introspection

#### Top-Level Mapping

Status note: the shipped CLI has now taken the canonical singular route for
`session`, `realm`, and `skill`. Historical references to `sessions`, `realms`,
and `skills` below should be read as pre-migration discussion, not current CLI
syntax.

| Current | Proposed | Bucket | Action |
| --- | --- | --- | --- |
| bare `rkat "prompt"` | keep | runtime | keep as friendly shorthand for `run` |
| `rkat run` | keep | runtime | canonical execution verb |
| `rkat resume` | fold into `rkat run --resume[=<SESSION>]` | runtime | deprecate command |
| `rkat continue` | fold into `rkat run --resume[=last]` | runtime | remove/deprecate |
| `rkat sessions ...` | `rkat session ...` | utility | canonical singular form implemented |
| `rkat blob ...` | keep | utility | keep, advanced |
| `rkat realms ...` | `rkat realm ...` | realm/config | canonical singular form implemented |
| `rkat mcp ...` | keep | realm/config | keep |
| `rkat skills ...` | `rkat skill ...` | realm/config | canonical singular form implemented |
| `rkat mob ...` | split mentally, keep namespace | mostly deployment/build | keep, but narrow primary purpose |
| `rkat config ...` | keep | realm/config | keep |
| `rkat capabilities` | keep | utility | keep |
| `rkat models ...` | simplify to `rkat models` | utility | flatten if possible |
| `rkat doctor` | keep | utility | keep |
| `rkat init` | keep | realm/config | keep |

#### Runtime Surface

This should become the main human-facing interface.

Current runtime commands:

- `rkat [prompt]`
- `rkat run`
- `rkat resume`
- `rkat continue`

Proposed runtime commands:

- `rkat [prompt]`
- `rkat run [OPTIONS] <PROMPT>`

Tentative runtime flags:

- `--resume[=<SESSION>]`
- `--skill <PATH_OR_ID>` repeatable
- `--peer <...>` repeatable
- `--listen[=<ADDR>]`

Peer-specific syntax is still open and should be treated as provisional pending
Workstream 2.

Runtime flags to keep in basic help:

- `--resume[=<SESSION>]`
- `--model`
- `--output`, `--json`
- `--stream`, `--no-stream`
- `--tools`
- `--skill`
- `--peer`
- `--listen`
- `--stdin`
- `--keep-alive`
- `--max-duration`
- `--verbose`

Runtime flags to demote to advanced:

- `--param`
- `--params-json`
- `--schema`
- `--max-tool-calls`
- `--allow-tool`
- `--block-tool`
- `--instructions`
- `--system`
- `--wait-for-mcp`
- `--label`

Runtime flags to replace or deprecate:

- `--preload-skill` -> replace with `--skill`
- `--skill-ref` -> deprecate
- `--skill-reference` -> deprecate
- `--app-context` -> hide or keep advanced-only

Resume/continue migration targets:

- `rkat resume last "foo"` -> `rkat run --resume "foo"`
- `rkat resume ~2 "foo"` -> `rkat run --resume ~2 "foo"`
- `rkat continue "foo"` -> `rkat run --resume "foo"`

#### Realm / Config Surface

This is persistent `.rkat` state.

Keep:

- `rkat config get/set/patch`
- `rkat mcp add/remove/list/get`
- `rkat realm current/list/show/create/delete/prune`
- `rkat init`

Change:

- `rkat skills list/inspect` should become `rkat skill ...`

Tentative `skill` surface:

- `rkat skill add <PATH_OR_ID>`
- `rkat skill remove <NAME_OR_ID>`
- `rkat skill list`
- `rkat skill get <NAME_OR_ID>`
- `rkat skill inspect <NAME_OR_ID>`
- maybe `rkat skill enable <NAME_OR_ID>`
- maybe `rkat skill disable <NAME_OR_ID>`

This preserves the desired distinction:

- ephemeral: `rkat run --skill ./foo`
- persistent: `rkat skill add ./foo`

Potential peer config surface:

- `rkat peer save <...>`
- `rkat peer remove <NAME>`
- `rkat peer list`

This is explicitly optional. Peer should remain runtime-first unless Workstream
2 decides otherwise.

#### Deployment / Build Surface

This should be the operational/artifact layer.

Keep under `mob`:

- `rkat mob pack`
- `rkat mob inspect`
- `rkat mob validate`
- `rkat mob deploy`
- `rkat mob web build`

These already feel naturally grouped.

Move out of the primary mob story:

- `mob run-flow`
- `mob flow-status`
- `mob spawn-helper`
- `mob fork-helper`
- `mob member-status`
- `mob force-cancel`
- `mob respawn`
- `mob wait-kickoff`

These should be treated as advanced/operator commands, not basic CLI story.

Possible directions:

- keep them under `mob`, but demote them in help
- or split them into a clearer operator grouping such as `mob ops ...` or
  `mob runtime ...`

Working mob mental model:

- runtime delegation for normal users comes from tool presets such as
  `--tools full`
- realm persistence for reusable teams may later get a separate noun if needed
- `mob` remains primarily the artifact/operator namespace

#### Utility / Introspection Surface

Keep as-is or simplify slightly:

- `rkat doctor`
- `rkat capabilities`
- `rkat models`

Tentative simplification:

- flatten `rkat models catalog` into `rkat models`, unless future subcommands
  justify keeping the extra level

#### Session / Blob / Realm Admin

These are useful, but should not dominate the basic view.

Keep:

- `rkat sessions list/show/delete/interrupt`
- `rkat blob get`
- `rkat realms ...`

Naming cleanup candidates:

- `sessions` -> `session`
- `realms` -> `realm`
- `skills` -> `skill`

`skills` -> `skill` is the strongest current recommendation. The others are
optional and can be deferred.

#### New Commands and Flags Needed

Runtime:

- `--resume[=<SESSION>]`
- `--skill <PATH_OR_ID>` repeatable
- `--peer <...>` repeatable
- `--listen[=<ADDR>]`

Realm/config:

- `rkat skill add`
- `rkat skill remove`
- `rkat skill get`
- maybe `rkat peer save/list/remove`

Possible future runtime escape hatch:

- `rkat run --team <PATH>`
- or `rkat run --mob-definition <PATH>`

This would support ephemeral one-off team topology without going through realm
config or mobpack, if such a need remains after Workstream 2.

#### Commands to Deprecate

- `rkat resume`
- `rkat continue`
- `rkat skills` in favor of `rkat skill`
- `--preload-skill`
- `--skill-ref`
- `--skill-reference`

#### Suggested Final Shape

- `rkat [PROMPT]`
- `rkat run [--resume[=SESSION]] [runtime options] <PROMPT>`
- `rkat skill ...`
- `rkat mcp ...`
- `rkat config ...`
- `rkat realm ...`
- `rkat mob ...`
- `rkat session ...`
- `rkat blob ...`
- `rkat models`
- `rkat capabilities`
- `rkat doctor`

#### Strong Current Recommendations

- collapse `run` / `resume` / `continue` into `run + --resume`
- introduce `--skill <path-or-id>`
- keep peer runtime-first
- treat `mob` primarily as deployment/build plus advanced operator surface
- rename `skills` to `skill`

### Deliverables

1. Define the canonical top-level command tree.
2. Separate default help from advanced/operator help.
3. Map current commands and flags to the new organization.
4. Define how collaborative multi-agent UX lowers onto the underlying runtime
   without leaking internal implementation details into basic CLI usage.

## Proposed Order

### Phase 1: Surface composability inventory and consolidation

- identify current reusable seams
- identify duplication in product surfaces
- decide what becomes shared helper surface

### Phase 2: Supported surface/product matrix

- define the official compositions we want to support
- decide what belongs in user-facing collaboration UX versus operator UX

### Phase 3: `rkat` redesign

- redesign CLI tree and help
- map old syntax to new syntax
- implement behind the composable surface layer from Phase 1

## Non-Goals

This proposal does not assume:

- a rewrite of the core runtime
- a rewrite of the machine catalog
- a rewrite of mob ownership or identity semantics
- immediate introduction of a new universal identity model above plain sessions
  and mob members

Those may become separate proposals later if required, but they are not the
default outcome of this plan.

## Open Questions

1. Do we want to ship official smaller CLIs, or only make them easy for others
   to build?
2. Should there be a dedicated shared surface-composition crate, or is
   extending `meerkat::surface` enough?
3. Which parts of `rkat` are legitimately product-specific and should stay
   local even after MCGA?
4. For cross-machine collaboration, what is the minimum user-facing concept
   that is honest, composable, and consistent with the architecture?

## Immediate Next Step

Use this proposal as the parent planning document for Workstream 1.

The next concrete task is to produce a detailed inventory of:

- reusable surface helpers that already exist
- duplicated bootstrap/executor/materialization logic across surfaces
- recommended consolidation targets
