# MobKit Console Progressive Customization Proposal

Status: execution-ready draft after two adversarial cycles
Date: 2026-06-02

## Goal

MobKit Console should be customizable at the depth each host needs. A host
should be able to:

- serve the stock embedded `/console` unchanged;
- configure the stock console through `config/console.toml`;
- wrap the stock console inside a host application shell;
- rearrange MobKit-provided components into a different layout;
- replace one region, such as navigation, while reusing MobKit transcript,
  composer, dock, activity, topology, roster, and lifecycle components;
- build a custom product shell on top of selected headless controllers and
  selected components;
- ignore the React packages and use the MobKit console protocol directly.

The stock console should remain the reference assembled application, but
console correctness should move into smaller protocol, model, controller, and
component boundaries. The goal is progressive mix-and-match customization, not
a giant options object and not a fork of `console/src/ConsoleApp.tsx` for every
consumer.

## Motivation

`console/src/ConsoleApp.tsx` currently owns too many responsibilities:

- fetching `/console/experience`;
- interpreting console configuration;
- maintaining live event, timeline, optimistic-send, and identity logs;
- managing dock state and panel targets;
- routing panel kinds to concrete React panels;
- composing the topbar, sidebar, dock, rail, resizers, and panels.

This supports a configurable stock app, but it does not yet give upstream
consumers a clean way to adopt only the parts that fit.

Three current consumer shapes drive the design:

- **Meerkat App / Studio:** project and thread are product concepts. Projects
  can contain long-lived project or domain agents, and user-created threads may
  be backed by thread agents. Studio may want its own project/thread navigator
  and Electron/Rust transport while reusing MobKit transcript, composer, dock,
  topology, activity, or lifecycle controls.
- **Operator validator app:** has an Ant Design application shell and admin routes. It
  uses MobKit console configuration for stock grouping, pins, buttons, hidden
  controls, and initial layout, and also wraps the stock console with host
  toolbar actions that remain host-owned.
- **Meerkat Fugue:** exposes MobKit's stock console behind an operator/status
  server. It wants MobKit-owned console behavior and host auth wrapping, but
  not a Fugue-projected browser facade for roster, timeline, history, identity
  routing, or send acknowledgement.

These are compatible only if MobKit is strict about what it owns and what hosts
own.

## Execution Scope

This plan must be executable end to end inside the MobKit repository by an
agentic implementation system. The real upstream consumers above are design
evidence, not execution dependencies. The implementation must not require
checking out, modifying, testing, or deploying Meerkat App / Studio, the operator deployment
Validator, or Meerkat Fugue.

Instead, MobKit will add self-contained proving fixtures that model the relevant
consumer shapes:

| Fixture | Models | Purpose |
| --- | --- | --- |
| `reference-wrapper` | Fugue-like stock console behind a host wrapper | Proves Level 0 stays canonical and host status/auth wrappers do not become console facades |
| `configured-host-shell` | production-like `config/console.toml` plus inert host links and toolbar | Proves Level 1/1.5 config and host-shell behavior without depending on the operator deployment |
| `custom-host-shell` | Studio-like custom navigation, host records, and injected non-HTTP transport | Proves Level 3/4 mix-and-match without importing Studio nouns or code |

These fixtures may live under `examples/`, `console/fixtures/`, or
`packages/*/test-fixtures/`, whichever gives the cleanest build and test
surface. Their names should be generic. They should not import external
consumer repositories.

## Agent Execution Contract

An implementation agent should treat this document as the work breakdown and
gate ledger. It should execute phases in order unless a later phase needs a
small enabling scaffold; if so, the scaffold must not claim the later gate as
complete until the full gate evidence exists.

Rules for execution:

- Work only in this MobKit repository.
- Do not check out, modify, or test real consumer repositories.
- Keep `console/src/ConsoleApp.tsx` behavior as the reference until an
  explicit no-behavior-change extraction gate proves otherwise.
- Prefer local fixtures and fake transports over integration with real upstream
  products.
- For each gate, add a durable evidence path: a test name, script, workflow
  job, generated schema, fixture file, or README/design section.
- Check a gate only after the evidence exists and the relevant command passes.
- Leave gates unchecked when evidence is design-only, implied, manually
  inspected, or blocked by missing implementation.
- Keep private workspace package imports private until the Phase 6 public
  surface decision is complete.

Minimum local command vocabulary:

| Command | Purpose |
| --- | --- |
| `git diff --check` | Whitespace and patch hygiene for touched files |
| `npm --prefix console run phase0:types --silent` | Current console type/unit gate |
| `npm --prefix console run phase1:targets --silent` | Current persisted-target migration gate |
| `npm --prefix console run e2e:browser --silent` | Stock browser behavior gate |
| `npm --prefix console run build --silent` | Embedded console bundle refresh |

New gates introduced by this plan should extend this command vocabulary with
named scripts rather than relying on prose. Good names are specific and local,
for example `contracts:console`, `core:navigation`, `headless:transport`,
`components:reuse`, and `fixtures:console-customization`.

## Non-goals

- Do not make MobKit own host product concepts such as projects,
  workspace threads, initiatives, issue lanes, automation runs, worktrees, or
  deployment status records.
- Do not turn `config/console.toml` into a product-builder DSL.
- Do not move runtime authority, routing policy, auth, leases, agent existence,
  or lifecycle capability into browser state.
- Do not require React adoption for protocol-only consumers.
- Do not require hosts to adopt the full stock shell in order to reuse a
  transcript, composer, dock, or navigation model.
- Do not publish new public console packages until the private boundaries,
  release policy, and local proving fixtures are proven.

## Boundary Rule

MobKit runtime/protocol owns MobKit facts and authoritative state transitions.
Core/headless owns derived view state, labeled optimistic state, and user
preference transitions. Visuals own presentation and input mechanics. Host
adapters own host-specific nouns. Runtime authority stays server-side.

| Concern | Owner |
| --- | --- |
| Identity, member, lifecycle, routing, gating, event, send, auth, and capability contracts | MobKit runtime/protocol |
| Wire parsing, RPC/SSE/fetch adapters, multipart upload helpers | Protocol transport |
| Timeline replay, stream reconnect, event ordering, interaction correlation | Headless timeline/conversation controllers |
| Dock state, targets, split/focus/close, neutral navigation, pins/order/collapse reducers | Console core/headless |
| Sidebar, topbar, host tree, command palette, drag handles, drop indicators, CSS, icons | Components/host shell |
| Host project/thread/initiative/run/worktree mapping into MobKit identities and targets | Host adapter |
| Host admin routes, confirmations, preflight, product auth, and non-MobKit actions | Host application |

For drag-to-reorder, the model layer owns stable IDs, current order, valid move
metadata, `moveBefore`/`moveAfter`, persistence hooks, and whether the order is
user preference or runtime order. The visual layer owns pointer mechanics,
drop-zone rendering, animation, horizontal versus vertical layout, and styling.
Keyboard reorder, focus restoration, disabled move state, and announcement text
are semantic enough to be part of the model/component contract, not an
afterthought in one sidebar implementation.

## Customization Levels

| Level | Name | Consumer action | Examples |
| --- | --- | --- | --- |
| 0 | Stock console | Serve `/console` unchanged | `reference-wrapper` fixture |
| 1 | Configure | Use `config/console.toml` for title, theme, grouping, pins, layout, links, visible controls | `configured-host-shell` fixture |
| 1.5 | Stock in host shell | Host wraps `/console` in its own page or iframe without adopting headless; this is not an auth boundary | `configured-host-shell` fixture |
| 2 | Recompose | Use MobKit components in a different layout | Topbar controls, hidden rail, alternate workbench |
| 3 | Replace region | Custom navigation or one custom panel, reuse MobKit transcript/composer/dock | `custom-host-shell` fixture |
| 4 | Custom shell | Host owns product shell and adopts selected headless controllers/components | `custom-host-shell` fixture |
| 5 | Protocol only | Use MobKit console protocol or SDK without React | CLI, dashboard, low-level operator tools |

Level 0, Level 1, and Level 1.5 are not lesser modes. They are first-class and
must remain boring, stable, and cheap.

## Proposed Layers

The layer names below describe ownership. They do not require immediate public
package names.

| Layer | Owns | Must not own |
| --- | --- | --- |
| Protocol transport | Wire contracts, tolerant parsers, HTTP/RPC/SSE adapters, multipart/blob helpers | React state, host nouns, visual models |
| Core models | Pure reducers and view models: transcript, dock, navigation, activity, ordering, pins, rich content | Network, DOM, auth, runtime policy |
| Headless controllers | Timeline, conversation, workbench, navigation, command orchestration | Host product objects, server authority |
| Components | Model/callback renderers: transcript, composer, dock, nav renderers, activity, topology, roster | Hidden MobKit state, transport, host payload parsing |
| Stock console | Reference assembled `/console` app and `console_config` consumption | Shared correctness that custom shells need |
| Host adapters | Host projects/threads, admin routes, status shell, host persistence | MobKit facts, lifecycle truth, timeline truth |

### Protocol Transport

The protocol transport owns wire shapes and low-level access. It should start
as private internal modules. A future public package or SDK subpath is a
separate release decision.

Responsibilities:

- canonical or mechanically checked TypeScript representations of
  `/console/experience`, timeline pages, console frames, identity status,
  routing, gating, topology, config, RPC errors, send responses, multipart
  uploads, and blob references;
- tolerant parsers from wire snake_case into protocol objects;
- `createHttpConsoleTransport({ baseUrl })`;
- no React dependency and no host product nouns.

Contract source of truth:

| Surface | Canonical source before public graduation |
| --- | --- |
| `console_config` | Rust serde structs in `meerkat-mobkit/src/console_config.rs` |
| `/console/experience` | A versioned schema artifact under `docs/rct` added before extraction |
| timeline query/page/stream frames | Versioned `docs/rct/console-rest-sse-contract-v*.json` schema artifact |
| console send, multipart upload, blob references, RPC errors | Versioned `docs/rct` schema artifact |
| command availability | `mobkit/capabilities` response from the authenticated server |

Fixtures are evidence, not canonical truth. Protocol TS types, parsers, and
golden fixtures must be generated from or mechanically checked against the
canonical source for their surface. Hand-maintained mirror types are
compatibility shims only. Breaking wire changes require a contract version bump
or advertised feature flag. Command availability must be refreshed or verified
through `mobkit/capabilities`, and clients must fail closed on mismatch.

### Core Models

`packages/console-core` should remain the home for pure model work:

- conversation grouping, transcript view state, rich content parsing, and
  tool-call accumulation;
- dock targets, panel state, split/focus/close/preset reducers;
- neutral navigation model and layout-independent pin/order/collapse reducers;
- activity/event buffer models;
- compatibility adapters for current `ConsoleSidebarViewState`.

Core models do not fetch, subscribe, access `window`, perform auth, or parse
host product payloads.

### Headless Controllers

The primary controller boundary is injected transport, not `baseUrl`:

```ts
const console = useMobKitConsoleController({
  transport,
  adapters,
});
```

`baseUrl` may remain a stock-console convenience:

```ts
const console = useMobKitConsoleController({
  transport: createHttpConsoleTransport({ baseUrl }),
});
```

The transport shape should cover at least:

```ts
type MobKitConsoleTransport = {
  loadExperience(): Promise<ConsoleExperience>;
  loadModules?(): Promise<ConsoleModulesResponse>;
  capabilities(): Promise<ConsoleCapabilities>;
  queryTimeline(input: ConsoleTimelineQuery): Promise<ConsoleTimelinePage>;
  subscribeTimeline(
    input: ConsoleTimelineSubscribeInput,
    onFrame: (frame: ConsoleFrame) => void,
  ): () => void;
  send(input: ConsoleSendInput): Promise<ConsoleSendAccepted>;
  executeCommand(input: ConsoleCommandRequest): Promise<ConsoleCommandResult>;
  upload?(input: ConsoleUploadInput): Promise<ConsoleUploadResult>;
  blobUrl?(blobId: string): string;
};
```

Generic `rpc(method, params)` and multipart RPC helpers may exist only as
private protocol plumbing underneath an adapter. They are not part of the
controller-facing, component-facing, or host-facing API. Public or experimental
headless APIs expose typed or allowlisted `ConsoleCommandSurface` operations
that validate target kind and server capability before dispatch.

Headless should decompose into smaller controllers:

- `timelineController`: query, subscribe, reconnect, backfill, cursors,
  ordering, replay errors;
- `conversationController`: identity logs, optimistic sends, response phases,
  transcript projection;
- `workbenchController`: panels, targets, split/focus/close, target
  persistence;
- `navigationController`: neutral navigation, pins, order, collapse, active
  and focused node state;
- `commandSurface`: capability-gated MobKit commands.

Hosts may adopt these pieces incrementally. A custom host shell should be able
to provide a non-HTTP transport and adopt conversation/dock controllers without
adopting stock navigation authority.

Electron hosts have an additional boundary: the renderer-facing transport must
adapt the host preload/IPC and desktop gateway APIs. It must not let renderer UI
call MobKit runtime RPC, `rkat` internals, or raw HTTP directly when the host
architecture requires gateway ownership. The `custom-host-shell` proof
transport must model this by going through a host adapter boundary rather than
direct browser HTTP/RPC, while preserving typed errors, capabilities,
cursors/events, blobs/uploads where supported, and the no-renderer-runtime-calls
rule.

### Components

`packages/console-components` should expose model/callback renderers:

- transcript and rich-content renderers;
- composer;
- dock;
- activity rail/log;
- navigation renderers such as stock sidebar, and optionally topbar/tree
  renderers when they are proven useful;
- topology, roster, and lifecycle controls as reusable components once their
  app-local dependencies are removed.

Components may offer convenience bindings, but the primitives must not import
the headless controller by default. Components receive targets and callbacks;
they do not interpret host payloads.

### Stock Console

The stock console remains the reference application:

```tsx
function ConsoleApp({ baseUrl }: { baseUrl: string }) {
  const console = useMobKitConsoleController({
    transport: createHttpConsoleTransport({ baseUrl }),
  });
  return <StockConsoleShell model={console.model} actions={console.commands} />;
}
```

This is an end state, not a first patch. During migration, stock visual output
and behavior are the acceptance bar.

### Host Adapter

Host adapters map host product objects into MobKit identities, targets, and
decorations. They do not synthesize canonical MobKit facts.

Allowed:

- add host navigation nodes;
- add host panels;
- decorate MobKit identities with host labels or placement;
- map a host project/thread record to an explicit MobKit backing target;
- hand a host action descriptor back to host-owned callbacks.

Forbidden:

- synthesize MobKit identities, timeline frames, session history, send
  acknowledgements, lifecycle state, routing truth, or lease facts;
- infer MobKit RPC calls from host payload fields;
- turn `config/console.toml` into a product DSL for any host.

Every fact crossing into reusable models from MobKit protocol, controller
derivation, optimistic state, or host adapters must carry provenance:

```ts
type ConsoleFactSource =
  | "mobkit-protocol"
  | "controller-derived"
  | "optimistic"
  | "host-adapter";
```

Runtime-derived facts retain contract version, source route/RPC, cursor or
snapshot identifier, capability version when relevant, and timestamp when
available. Optimistic facts retain correlation or idempotency data and must
reconcile to MobKit facts or fail visibly. Host-adapter facts remain typed as
host facts and never satisfy MobKit lifecycle, timeline, routing, send-ack, or
capability contracts.

## Host Wrappers And Status APIs

Host wrappers may authenticate, terminate TLS, path-prefix or proxy the console,
set security headers, and perform named non-semantic compatibility shims. They
must not rewrite console JavaScript semantics, patch protocol responses,
synthesize or filter MobKit timeline frames, inject host status facts into
MobKit facts, widen CORS, or change CSRF posture.

A host shell is not an auth boundary. When MobKit app auth is disabled, direct
console routes such as `/console`, `/console/rpc`, `/console/send`,
`/console/timeline`, `/console/timeline/stream`, assets, SSE endpoints, and blob
routes still need explicit protection through the server, proxy, platform, or
infrastructure. Client-side admin routing is not sufficient evidence.

Host status APIs may feed host shell widgets only as host-provenance facts. The
stock console and MobKit headless controller must not consume host status APIs
for roster, timeline, history, identity routing, send acknowledgement,
lifecycle truth, routing truth, or capability discovery. The `reference-wrapper`
fixture must model this split: `/console/rpc` remains MobKit browser console
RPC, while any host operator bridge remains separate and is never a
browser/headless fallback.

## Neutral Navigation Model

The reusable model should not be named after a sidebar. It should be introduced
before the headless controller API hardens.

Sketch:

```ts
type ConsoleNavigationModel<TTarget = ConsoleWorkbenchTarget> = {
  orientation?: "vertical" | "horizontal" | "tree" | "palette";
  activeNodeId?: string;
  focusNodeId?: string;
  nodes: ConsoleNavigationNode<TTarget>[];
  order: ConsoleNavigationOrderState;
};

type ConsoleNavigationNode<TTarget> =
  | ConsoleNavigationGroup<TTarget>
  | ConsoleNavigationItem<TTarget>;

type ConsoleNavigationGroup<TTarget> = {
  type: "group";
  id: string;
  label: string;
  children: ConsoleNavigationNode<TTarget>[];
  expanded: boolean;
  disabled?: boolean;
  ariaLabel?: string;
};

type ConsoleNavigationItem<TTarget> = {
  type: "item";
  id: string;
  label: string;
  target?: TTarget;
  selected?: boolean;
  pinned?: boolean;
  disabled?: boolean;
  meta?: ConsoleNavigationMeta[];
  actions?: ConsoleNavigationAction[];
  ariaLabel?: string;
};
```

Required model operations:

- `selectNode`;
- `toggleGroup`;
- `pinNode`;
- `moveNode({ id, targetId, position, scope })`;
- `canMove`;
- focus intent after move or selection;
- live-region announcement text for reorder.

Stock `ConsoleSidebar` can remain as a renderer and compatibility surface, but
the neutral model must support vertical sidebars, horizontal bars, command
palettes, trees, and host-owned project/thread navigation.

## Workbench Target Extensions

Drop `{ kind: string; hostPayload: unknown }` as the primary escape hatch.
Use typed, namespaced extension targets with explicit serialization.

```ts
type ConsoleWorkbenchTarget<THost = never> =
  | MobKitWorkbenchTarget
  | HostWorkbenchTarget<THost>;

type MobKitWorkbenchTarget =
  | { kind: "mobkit/identity-chat"; id: string; identity: string; title?: string }
  | { kind: "mobkit/identity-inspect"; id: string; identity: string; title?: string }
  | { kind: "mobkit/topology"; id: string; title?: string }
  | { kind: "mobkit/activity"; id: string; title?: string }
  | { kind: "mobkit/roster"; id: string; title?: string }
  | { kind: "mobkit/routing"; id: string; title?: string }
  | { kind: "mobkit/gating"; id: string; title?: string }
  | { kind: "mobkit/logs"; id: string; title?: string };

type HostWorkbenchTarget<TPayload> = {
  kind: `${string}/${string}`;
  id: string;
  title: string;
  payloadVersion: number;
  payload?: TPayload;
  iconHint?: string;
  provenance: "host";
};
```

Rules:

- Host target kinds must be namespaced and must not use the `mobkit/` prefix.
- Unknown host targets may persist, focus, split, close, and render fallback UI.
- Unknown host targets cannot call MobKit send, lifecycle, routing, gating, or
  arbitrary RPC.
- Persistence requires a stable ID, namespace, payload version, JSON-serializable
  payload, size limit, and inert fallback when the host target type is unknown.
- Components receive host targets but never parse their payloads unless a host
  renderer provides that parser.

## Security And Command Boundary

The headless layer exposes a `ConsoleCommandSurface`, not generic action
facades.

Every MobKit command must:

- map to one named MobKit RPC, REST, send, or multipart endpoint;
- validate the target kind before dispatch;
- require an advertised server capability;
- fail closed when the capability is absent;
- include idempotency or correlation data where the server contract supports it;
- never infer commands from host payloads.

`mobkit/capabilities` is authoritative for mutation and command availability in
the authenticated runtime context. `/console/experience` is an initial UI and
configuration projection, not the final authorization source. Controllers must
refresh or verify capabilities before mutating commands and fail closed if the
capability is absent or stale.

Authentication, CSRF, CORS, and operator bridges are deployment boundaries, not
visual customization. Protocol/headless code must not:

- broaden CORS;
- introduce cookie-dependent mutation flows without explicit CSRF posture;
- collapse browser console RPC into host/operator bridges;
- treat `require_app_auth=false` as anything other than an explicit local or
  host-protected deployment choice.

Host actions are opaque descriptors returned to host callbacks. Review,
summary, dry-run, reset, status refresh, and deployment operations remain
host-owned when they are not MobKit console commands.

Configured `[[sidebar.buttons]]` are inert navigation descriptors only: label,
href, target, icon. They must not grow method, request body, preflight,
capability, confirmation, or mutation semantics. Host routes own any
confirmation and POST behavior reached through those links.

## Agentic Execution Plan

Each phase is designed as a self-contained implementation target for an
agentic system. A phase is complete only when every gate checklist item is
satisfied in the MobKit repository. External consumer repositories are not part
of the gate.

### Phase 0: Governance Before Extraction

Purpose: make package and API ownership explicit before moving code.

Implementation tasks:

- Decide whether future protocol/headless surfaces stay private, become SDK
  subpaths, or become separately published packages.
- Define package names, version coupling, stable/experimental export labels,
  changelog ownership, deprecation policy, and release path before any public
  package promise.
- Keep current `@console-core` and `@console-components` private while
  extracting.
- Reconcile current package README language such as "Stable public exports" and
  "Public entrypoint" with the private/experimental status of these workspace
  packages.
- Document that pre-public fixtures may use workspace-internal imports only.
  External hosts must not depend on private `@console-core` or
  `@console-components` aliases until a public package or SDK-subpath decision
  is made.

Gate checklist:

- [x] A short governance note exists in this design or package READMEs naming
      private, experimental, and future-public surfaces.
- [x] `packages/console-core/README.md` and
      `packages/console-components/README.md` no longer imply npm-public
      stability unless that is intentionally decided.
- [x] No new public package name, npm scope, or SDK export is introduced.
- [x] Existing stock console build still succeeds.
- [x] `git diff --check` passes for touched docs/files.

Evidence:

- Governance status is documented in this proposal and in
  `packages/console-core/README.md` and
  `packages/console-components/README.md`.
- Build command: `npm --prefix console run build --silent`.
- Hygiene command: `git diff --check`.

### Phase 1: Contract And Neutral Models

Purpose: define protocol drift gates and layout-neutral models before the
controller API hardens.

Implementation tasks:

- Add or identify canonical contract schemas for `/console/experience`,
  timeline pages, SSE frames, replay errors, RPC errors, `mobkit/console/send`,
  multipart upload, and blob references.
- Add Rust and TS tests that consume the same fixtures or mechanically compare
  against the same schema source.
- Introduce `ConsoleNavigationModel` and compatibility adapters to and from
  `ConsoleSidebarViewState`.
- Extract a stock navigation adapter/model from current `DesignSidebar`
  behavior, not only the smaller package `ConsoleSidebarViewState`.
- Add layout-independent order/pin/collapse reducer tests and keyboard/a11y
  requirements for reorder.
- Preserve configured selector inheritance, configured section order, synthetic
  `Pinned` section with family context, subgroup collapse/order, search
  expansion, virtualization, localStorage namespace/default precedence, and
  user-preference precedence.
- Add persisted dock target schema versioning and compatibility mapping from
  legacy unnamespaced kinds such as `agent-chat`, `identity-inspect`,
  `routing`, and `gating` to canonical namespaced targets. Old saved layouts
  must hydrate or fail inertly without dropping panels or rendering unsupported
  stock panels.

Gate checklist:

- [x] Canonical contract source is named for every REST/RPC/SSE surface touched.
- [x] A named contract sync test fails if TS parsers/types drift from the
      canonical schema or Rust structs.
- [x] `ConsoleNavigationModel` exists with tests for select, toggle, pin,
      move, focus intent, and live announcement text.
- [x] Compatibility adapters preserve existing `ConsoleSidebarViewState`.
- [x] Stock navigation adapter tests cover configured grouping, `Pinned`,
      subgroup collapse/order, search expansion, virtualization, and storage
      namespace precedence.
- [x] Dock target migration tests cover old persisted `agent-chat`,
      `identity-inspect`, `routing`, and `gating` layouts.
- [x] Existing `console` type/unit tests still pass.

Partial evidence:

- Expanded canonical contract:
  `docs/rct/console-rest-sse-contract-v0.5.0.json`.
- Contract constants and sync tests:
  `console/src/lib/contract.ts` and `console/src/lib/contract.test.ts`.
- Contract command:
  `npm --prefix console run contracts:console --silent`.
- Neutral model and adapters:
  `packages/console-core/src/navigation.ts`.
- Neutral model tests:
  `packages/console-core/src/navigation.test.ts`.
- Target migration seam and tests:
  `packages/console-core/src/targets.ts` and
  `packages/console-core/src/targets.test.ts`.
- Test command:
  `npm --prefix console run phase0:types --silent`.
- Target migration command:
  `npm --prefix console run phase1:targets --silent`.
- Stock navigation adapter coverage:
  `console/src/panels/Sidebar.test.ts`, including configured grouping,
  `Pinned`, subgroup collapse/order, search expansion, virtualization, and
  storage namespace/default precedence tests.

### Phase 2: Private Extraction, No Behavior Change

Purpose: move pure logic without changing the stock UI or embedded console.

Implementation tasks:

- Move protocol helpers and pure reducers behind private/internal modules.
- Extract sidebar storage/order/pin logic without changing the stock sidebar.
- Keep `ConsoleApp.tsx` visual output and behavior unchanged.
- Add an embedded freshness gate: `npm --prefix console run build --silent`
  must copy fresh assets into `meerkat-mobkit/console-dist`, and CI/release
  preflight must fail if that build leaves an unexpected diff before Rust
  binaries are built.

Gate checklist:

- [x] `npm --prefix console run phase0:types --silent` passes.
- [x] Existing package/core/component tests touched by the extraction pass.
- [x] Stock browser e2e passes.
- [x] `config/console.toml` regression fixture preserves grouping, pins,
      button links, hidden controls, action visibility, and user preference
      precedence.
- [x] `npm --prefix console run build --silent` refreshes
      `meerkat-mobkit/console-dist`.
- [x] A freshness check verifies the refreshed embedded bundle is intentional.
- [x] No visible stock console behavior changes unless explicitly documented.

Evidence:

- Private sidebar preference/order/pin extraction:
  `packages/console-core/src/sidebar-preferences.ts`.
- Compatibility re-export and unchanged stock renderer path:
  `console/src/panels/Sidebar.tsx`.
- Extraction tests:
  `packages/console-core/src/sidebar-preferences.test.ts` and
  `console/src/panels/Sidebar.test.ts`.
- Configured host fixture:
  `console/fixtures/configured-host-shell/config/console.toml`.
- Config fixture regression:
  `meerkat-mobkit/tests/console_customization_fixtures.rs`.
- Freshness check:
  `console/check-embedded-freshness.cjs`.
- Commands:
  `npm --prefix console run phase0:types --silent`;
  `npm --prefix console run e2e:browser --silent`;
  `cargo test -p meerkat-mobkit --test console_customization_fixtures -- --nocapture`;
  `npm --prefix console run build --silent`;
  `npm --prefix console run embedded:freshness --silent`;
  `git diff --check`.

### Phase 3: Headless Experimental

Purpose: introduce injected transport and headless controllers behind private
or experimental exports.

Implementation tasks:

- Add `createMobKitConsoleController` and `useMobKitConsoleController` behind
  private or experimental exports.
- Use injected transport; keep `baseUrl` only as HTTP convenience.
- Add a fake transport/server for replay, reconnect, stale cursor, optimistic
  send cleanup, event ordering, replay unavailable, multipart send, and
  capability rejection.
- Add host-target inertness tests.

Gate checklist:

- [x] Controller-facing transport has typed operations, not public raw RPC.
- [x] `createHttpConsoleTransport({ baseUrl })` backs the stock console path.
- [x] Fake transport tests cover reconnect, backfill, ordering, optimistic
      reconciliation, replay errors, multipart, blobs, and capabilities.
- [x] `mobkit/capabilities` is verified before mutating commands.
- [x] Unknown/host targets can persist, focus, split, close, and render
      fallback UI.
- [x] Unknown/host targets cannot send, retire, respawn, reset, route, gate, or
      call arbitrary RPC.
- [x] Provenance is present on MobKit, controller-derived, optimistic, and host
      adapter facts.

Evidence:

- Private headless transport/controller:
  `console/src/lib/headless.ts`.
- Private React hook wrapper:
  `console/src/lib/headless-react.ts`.
- Headless fake-transport tests:
  `console/src/lib/headless.test.ts`.
- HTTP adapter route/method proof:
  `createHttpConsoleTransport uses stock console routes and typed RPC methods`.
- Reconnect/replay behavior remains covered by current stock network tests:
  `console/src/lib/network.test.ts`.
- Host-target inertness and persistence:
  `packages/console-core/src/targets.test.ts` and
  `console/src/lib/headless.test.ts`.
- Command:
  `npm --prefix console run headless:transport --silent`.

### Phase 4: Component Decomposition

Purpose: make visual pieces reusable without importing hidden MobKit state.

Implementation tasks:

- Move reusable visual surfaces out of stock-only panels only when their props
  are model/callback driven.
- Make the stock sidebar renderer consume a stock-specific
  `ConsoleNavigationModel` while preserving existing grouping, pinning,
  collapse, order, and localStorage precedence.
- Add component tests for keyboard reorder, focus retention, accessible labels,
  reduced-motion drag preview behavior, and no app-only imports.
- Add an alternate shell fixture with non-sidebar navigation.

Gate checklist:

- [x] Reusable components do not import the headless controller by default.
- [x] Components do not import `console/src` private types.
- [x] Sidebar renderer preserves current stock behavior through adapter tests.
- [x] Keyboard reorder and pointer reorder both work through the same model
      operations.
- [x] Focus retention and live-region announcement behavior are tested.
- [x] A local alternate-shell fixture renders non-sidebar navigation using
      MobKit models/components.
- [x] Stock console browser e2e still passes.
- [x] Full stock sidebar renderer replacement over `ConsoleNavigationModel`.

Partial evidence:

- Component boundary gate:
  `console/src/lib/component-boundary.test.ts`.
- Component exports and private status:
  `packages/console-components/src/index.ts`.
- Existing sidebar renderer tests:
  `packages/console-components/src/sidebar/console-sidebar.test.tsx` and
  `console/src/panels/Sidebar.test.ts`.
- Stock sidebar renderer model proof:
  `stock sidebar renderer flattens its ConsoleNavigationModel`.
- Layout-neutral move, focus intent, and live announcement tests:
  `packages/console-core/src/navigation.test.ts`.
- Shared keyboard/pointer reorder operation:
  `applyConsoleNavigationReorderIntent`, used by the stock sidebar reorder
  commit path in `console/src/panels/Sidebar.tsx`.
- Alternate shell render proof:
  `console/src/lib/component-render.test.tsx`.
- Stock browser command:
  `npm --prefix console run e2e:browser --silent`.
- Component boundary command:
  `npm --prefix console run components:reuse --silent`.

### Phase 5: Self-Contained Proving Fixtures

Purpose: prove every customization level in this repo without depending on
external consumers.

Implementation tasks:

- Add `reference-wrapper`, a Fugue-like wrapper that serves the stock console
  behind a host wrapper and exposes separate host status facts.
- Add `configured-host-shell`, a production-like fixture with
  `config/console.toml`, inert sidebar links, host toolbar actions, and direct
  route auth evidence.
- Add `custom-host-shell`, a Studio-like fixture with host-owned records,
  custom navigation, explicit MobKit backing targets, injected non-HTTP
  transport, and selected MobKit components.

Gate checklist:

- [x] `reference-wrapper` proves `/console` and `/console/rpc` are served by
      an executable MobKit reference-console dispatch path, while host status
      APIs remain host-provenance only and are not merged into console
      protocol responses.
- [x] `reference-wrapper` proves host wrappers do not rewrite console JS,
      patch protocol responses, synthesize timeline frames, widen CORS, or
      change CSRF posture.
- [x] `configured-host-shell` proves `config/console.toml` grouping,
      subgroups, pins, inert sidebar button IDs/hrefs, hidden controls,
      `[actions]` visibility such as `show_respawn=false`, initial
      agent/control, and persisted localStorage-style preference precedence
      through the shared sidebar order/pin helpers.
- [x] `configured-host-shell` proves a host toolbar can own confirmations and
      POSTs without turning `console.toml` buttons into mutations.
- [x] `configured-host-shell` proves direct `/console/*`, send, SSE, asset, and
      blob routes have explicit server/proxy/platform protection when MobKit
      app auth is disabled.
- [x] `custom-host-shell` proves custom host records and navigation can bind
      transcript/composer/dock to explicit MobKit backing targets that pass the
      workbench target migration contract.
- [x] `custom-host-shell` proves injected non-HTTP transport works without
      raw browser RPC and without host nouns in MobKit exports by driving
      `createMobKitConsoleController` with a fixture-backed host adapter.
- [x] All fixtures run through named local test commands.

Evidence:

- Reference wrapper fixture:
  `console/fixtures/reference-wrapper/fixture.json`.
- Configured host shell fixtures:
  `console/fixtures/configured-host-shell/fixture.json` and
  `console/fixtures/configured-host-shell/config/console.toml`.
- Custom host shell fixture:
  `console/fixtures/custom-host-shell/fixture.json`.
- Fixture tests:
  `console/src/lib/customization-fixtures.test.ts` and
  `meerkat-mobkit/tests/console_customization_fixtures.rs`.
  The TS fixture test executes wrapper dispatch, sidebar preference precedence,
  target migration, headless timeline query, and headless send through a
  non-HTTP adapter; the Rust fixture test parses the fixture
  `config/console.toml` through MobKit's config loader.
- Command:
  `npm --prefix console run fixtures:console-customization --silent`.

### Phase 6: Public Surface Decision

Purpose: decide whether any extracted surface graduates beyond private use.

Decision for this implementation pass: keep the extracted console surfaces
private to the MobKit workspace. `@console-core`, `@console-components`, and
the headless transport/controller files remain private/internal. No new npm
package, SDK subpath, or public package name is introduced in this PR.

Stable labels in this pass mean "workspace-stable": covered by local gates and
safe for the stock console and local proving fixtures. Experimental labels mean
private implementation seams, especially the headless transport/controller,
that may change before any public-package or SDK-subpath decision.

Implementation tasks:

- Decide whether to keep console packages private, publish packages, or expose
  SDK subpaths.
- Add version parity, API surface tests, changelog entries, npm dry-run/publish
  lanes if public, and registry readback if published.
- Keep embedded `console-dist` freshness in release/tag workflows.

Gate checklist:

- [x] Every Phase 0-5 gate claimed by this private implementation pass has a
      named local command or source-level assertion.
- [x] Every Phase 0-5 target-state gate is complete.
- [x] Public package or SDK-subpath names are chosen, or the surfaces are
      explicitly kept private.
- [x] Stable and experimental exports are labeled and tested.
- [x] Semver, peer dependencies, changelog, and deprecation policy are written.
- [x] Release workflow includes parity, dry-run/publish/readback gates for any
      public package.
- [x] Release workflow rebuilds and verifies embedded `console-dist` before
      binaries are built.

Evidence:

- Private/public decision:
  this Phase 6 section plus `packages/console-core/README.md` and
  `packages/console-components/README.md`.
- Export and boundary tests:
  `npm --prefix console run components:reuse --silent` and
  `npm --prefix console run headless:transport --silent`.
- Existing package metadata:
  `packages/console-core/package.json`,
  `packages/console-components/package.json`, and `console/package.json` all
  remain private/internal for these surfaces.
- Release workflow embedded freshness:
  `.github/workflows/release.yml` runs
  `npm --prefix console run embedded:freshness --silent` before
  `build_binaries`.
- Because no new public console package or SDK subpath is introduced, no new
  npm publish, dry-run, or registry readback lane is required for the extracted
  console surfaces in this PR. Existing release parity/publish gates remain
  responsible for the repository's already-public artifacts.

## Acceptance Gates

| Gate | Coverage |
| --- | --- |
| Protocol | Experience, timeline query/page/stream, replay errors, RPC errors, send, multipart, blobs, capabilities, plus a named contract sync gate that proves TS parsers/types and Rust/schema artifacts match the canonical source |
| Headless | Reconnect/backfill, optimistic cleanup, ordering, target persistence, closed command names, target-kind command allowlists, capability rejection |
| Core reducers | Navigation groups, pin/order/collapse, dock split/focus/close, host target preservation |
| Components | Model/callback rendering, callback interaction tests, no controller imports by default, a11y, no app-only imports |
| Stock console | No behavior regression, existing `config/console.toml`, localStorage precedence, browser e2e |
| Embedded console | `npm --prefix console run build --silent`, failing diff check for `meerkat-mobkit/console-dist`, gateway serves fresh assets |
| Security | Host targets inert, command capability checks, no broad raw RPC escape hatch in headless |
| Proving fixtures | Executable `reference-wrapper` Level 0, `configured-host-shell` Level 1/1.5, `custom-host-shell` Level 3/4 |
| Release | Public package decision, semver, changelog, parity, dry-run/publish/readback if applicable |

Executable gates should be added as named scripts or workflow jobs, not only
categories. At minimum, the proposal requires a package-wide console test target
covering current `console` tests plus `packages/console-core` and
`packages/console-components` suites such as consumer smoke, portability, dock,
transcript, and rich-content tests. PR CI should run the private extraction,
protocol, core, component, stock browser, and target-migration gates relevant to
changed files. Release/tag workflows must run embedded freshness and any public
package parity/publish dry-run gates before binaries are built or packages are
published.

## Critical Adoption Notes

After the first adversarial cycle, these changes were adopted:

- private-first extraction before public package promises;
- `transport` as the controller boundary, with `baseUrl` only as shorthand;
- `Stock in host shell` as a first-class customization level;
- neutral navigation and reorder accessibility moved before controller API
  hardening;
- typed, namespaced, inert host targets instead of `hostPayload: unknown`;
- `ConsoleCommandSurface` with capability checks and target validation;
- provenance for distinguishing MobKit facts, controller-derived facts,
  optimistic facts, and host-adapter facts;
- local proving fixtures for the three upstream-consumer shapes.

These suggestions were intentionally constrained:

- Do not require the custom-shell Level 4 proof before Level 0/1/1.5 stabilizes.
- Do not promise public `@mobkit/*` package names yet.
- Do not require MobKit to provide every possible renderer; host-owned
  project/tree/topbar renderers are acceptable when MobKit exposes neutral
  models and selected reusable primitives.

After the second adversarial cycle, these targeted amendments were adopted:

- generic raw RPC was demoted to private protocol plumbing;
- `mobkit/capabilities` became the command authority;
- host shells were explicitly declared not to be auth boundaries;
- host wrappers and status APIs received allowed/forbidden rules;
- provenance became mandatory at reusable-model boundaries;
- `console.toml` sidebar buttons were constrained to inert navigation;
- stock sidebar behavior and persisted dock target migration became explicit
  migration gates;
- custom host renderer transport was constrained to host adapter/gateway
  boundaries;
- embedded console freshness moved into early migration and release gates;
- acceptance gates now require named scripts or workflow jobs, not only broad
  categories.

After the execution rewrite, these constraints were added:

- the plan must be executable entirely inside the MobKit repository;
- real upstream consumer repositories are design evidence, not gate
  dependencies;
- each phase has a gate checklist;
- external consumer proofs were replaced by local self-contained fixtures:
  `reference-wrapper`, `configured-host-shell`, and `custom-host-shell`.
