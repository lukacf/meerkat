# MobKit Console: Unified Platform Direction (v5)

> Status: historical direction memo. The current implemented console contract is `docs/rct/console-rest-sse-contract-v0.5.0.json`: `POST /console/rpc` with `mobkit/console/send`, `GET /console/timeline`, `GET /console/timeline/stream`, and `GET /console/identity/{identity}/stream`. Older references below to `POST /interactions/stream`, `POST /console/identity/stream`, or `GET /console/events/stream` describe pre-implementation design alternatives, not the current surface.

## Framing

One console type: **operator/debugging workbench for identity-based multi-agent systems**.

The downstream app and the operator deployment are instances of the same product category. The core console needs are shared: agent roster with health, conversation inspection, tool call visibility, activity timelines, lifecycle actions, and flow triggers. The downstream app pushes more edges (gating, routing, domain state) but the operator deployment is a subset of the same design space.

**Architecture:** experience endpoint describes available data → host reads data and decides what panels to render → shared components render normalized view states → host provides domain-specific rendering via callbacks.

**Notation:** Items marked **[EXISTS]** are current working surface. Items marked **[NEW]** require new runtime/protocol work.

---

## Tier 1 — Identity-native interaction model [NEW]

**Current state:** The console is member/session-wired end to end. Dock targets keyed by `member_id`, send via `mobkit/send_message(member_id, ...)`, SSE correlation by `session_id` on `POST /interactions/stream` (ephemeral — open, drain, close).

**Gap:** Neither `dispatch()` (returns `(FencingToken, bool)`) nor `send()` (blocks until completion) on `IdentityRuntime` provides what the console needs: a **non-blocking identity-addressed send that returns a correlation token, with events streamed on a persistent connection**.

### New runtime API surface required [NEW]

A new RPC method (e.g., `mobkit/interact`):

- **Request:** `{ identity: string, content: string, origin: string }` — `origin` identifies the caller for audit logging and dispatch routing (e.g., `"console"`, `"console:panel-3"`, `"connector:slack"`). Required — every interaction has a source.
- **Response:** `{ interaction_id: string, identity: string }` — the `interaction_id` is the per-turn UI correlation token
- **Semantics:** Non-blocking. Enqueues the turn and returns immediately with the correlation token. Does NOT block until completion.

This method does not exist today. It requires new gateway handler + runtime plumbing to bridge identity-addressed non-blocking send with interaction tracking.

### Per-identity SSE connection model [NEW]

The current `POST /interactions/stream` is ephemeral (one connection per send, drained and closed). The identity model requires a **persistent SSE connection per identity** that carries all events, with concurrent interactions multiplexed and filtered by `interaction_id`.

**Endpoint:** `POST /console/identity/stream` with `{ identity: string }` in the request body. POST-based because `AgentIdentity` is an opaque string that may contain characters not safe for URL paths (e.g., `identity:alice`, `family-group:main`). POST avoids encoding issues while remaining conventional for SSE subscription endpoints that need parameters.

**Multi-panel stream ownership:** shared refcounted connection per identity, managed by a connection pool in `lib/network.ts`. When the first dock panel targets an identity, the pool opens the SSE connection. Additional panels targeting the same identity share it (refcount increments). When the last panel releases, the connection closes. Panels subscribe/unsubscribe from the pool; they never manage SSE connections directly.

**Connection design:**
- Connection lifecycle: opened when first panel targets an identity, closed when last panel releases it
- Reconnection: host reconnects with `Last-Event-ID` for gap recovery
- Buffering: events between disconnect and reconnect are replayed on reconnect via `Last-Event-ID`

### Event envelope fields

Every streamed frame carries:

```typescript
{
  event_id: string;              // unique, monotonic for ordering
  interaction_id?: string;       // present on turn events, absent on lifecycle/system events
  identity: string;              // stable identity attribution (NOT member_id)
  event_type: string;            // "text_delta", "tool_call", "tool_result", "run_completed", etc.
  timestamp_ms: number;
  data: unknown;                 // event-type-specific payload
}
```

`interaction_id` is **optional**. Turn events (text_delta, tool_call, tool_result, interaction_complete) carry it. Lifecycle/system events (agent_spawned, lease_expired, peer_wired, topology_changed) are identity-attributed but have no interaction — they flow on the same per-identity SSE stream with `interaction_id` absent. The host filters by `interaction_id` when isolating a turn, and shows all events when rendering the activity feed.

**Terminal event:** `event_type: "interaction_complete"` or `"interaction_failed"` with the matching `interaction_id`. Signals the host to stop waiting for more frames for that turn.

### Console codebase changes

- `lib/network.ts` — new `openIdentityStream(identity)` (persistent, refcounted connection pool) + `interact(identity, content)` alongside existing member-addressed functions. Both coexist during migration.
- `lib/adapters.ts` — dock targets keyed by identity, sidebar built from `IdentityStatus[]` when available
- `ConsoleApp.tsx` — send flow uses `mobkit/interact`, SSE subscription by identity with `interaction_id` filtering

### Migration coexistence

When `identity_status` is absent from the experience (pre-migration host or non-identity-first runtime), the sidebar adapter falls back to `agent_sidebar` member rows. Dock targets use `member_id`. Send uses `mobkit/send_message`. The adapter is a simple fallback: `identity_status ?? agent_sidebar`. The dock target type does not change — it gains an optional `addressingMode: "identity" | "member"` field so the send flow knows which path to use.

---

## Tier 1 — Experience contract grows data sections

**Current state [EXISTS]:** `/console/experience` returns `agent_sidebar`, `activity_feed`, `topology`, `health_overview`, `flows`, `chat_inspector`, `session_history`. One-shot GET, no refresh model.

### What changes

The experience endpoint adds new optional sections and per-section metadata. The panel contract stays as-is: `ConsoleDockTarget.kind` + host's `renderPanelBody` switch. No panel registry.

| Section | Status | What it describes |
|---|---|---|
| `agent_sidebar` | **[EXISTS]** | Agent roster, labels, affordances, groups |
| `activity_feed` | **[EXISTS]** | SSE subscription config, event contract |
| `topology` | **[EXISTS]**, grows **[NEW]** | Currently module nodes. Grows to include identity nodes + managed edges |
| `health_overview` | **[EXISTS]** | Runtime health, loaded modules |
| `flows` | **[EXISTS]** | Scheduling, dispatch methods |
| `gating` | **[NEW]** | Pending entries, audit trail, risk tiers, action capabilities. Optional |
| `identity_status` | **[NEW]** | Per-identity continuity generation, lease health, session mapping. Optional |
| `routing` | **[NEW]** section, **[EXISTS]** RPCs | Routes, delivery records, attempt history. RPCs exist; experience section + console panel are new |

### Per-section metadata [NEW]

```typescript
{
  schema_version: string;    // e.g., "1" — lightweight, for cross-host evolution
  refresh:
    | { mode: "poll", interval_ms: number }
    | { mode: "stream", topic: string, update_semantics: "full_snapshot" | "append" }
  capabilities?: string[]   // only for actionable sections: ["approve", "deny", "escalate"]
}
```

- `schema_version` is required. Lightweight — stays "1" until a breaking change. Cheap insurance for a multi-host platform contract.
- `refresh` is required. Defines how the host gets updates. `update_semantics` specifies whether stream events are full replacement snapshots or append-only entries. (`"patch"` omitted — no section currently needs diff-based updates. Add when there's a concrete need with a pinned format like JSON Patch RFC 6902.)
- `capabilities` is optional — only meaningful for sections with write actions (gating). Omitted for read-only sections (topology, health).

### Client-side data management

The experience endpoint returns sections with refresh metadata. The host's `ConsoleApp` is responsible for:
- Fetching initial experience
- Starting polling or SSE subscriptions per section based on declared refresh
- Storing section data and notifying consuming components
- Handling shared consumption (e.g., topology data used by both the graph panel and the inspection panel)

This is a **host responsibility**, not a shared utility. Hosts use their own state management (Zustand, React Query, plain React state). A recommended pattern will be documented, but the shared layer does not prescribe a data manager — it prescribes the data shapes and refresh contracts.

---

## Tier 1 — Identity inspection panel [NEW]

A shared inspect view per identity. Shows what the data model can actually provide; grows as the data model grows. Does NOT fake fields that lack backing data.

### Data sources

| Field | Source | Status |
|---|---|---|
| Lifecycle state (active, retiring, suspended) | `mobkit/status_identity` | **[EXISTS]** |
| Continuity generation + lease info | `mobkit/status_identity` | **[EXISTS]** |
| Output preview + is_final | `mobkit/inspect_identity` | **[EXISTS]** |
| Peer reachable count | `mobkit/inspect_identity` | **[EXISTS]** |
| Current session ID + member mapping | `mobkit/status_identity` | **[EXISTS]** |
| Topology peers (wired-to) | experience `topology` section | **[EXISTS]** for modules, **[NEW]** for identity graph |
| Recent tool calls | **[NEW]** — requires: (1) identity-attributed events in the event model, (2) tool_call_id on tool events, (3) `mobkit/query_events` supporting identity-based filtering (currently does not). All three are new work. |
| Last activity timestamp | **[NEW]** — same dependency as above: identity-attributed events + identity-based event query. Currently `query_events` does not filter by identity and drops agent events that lack displayable payloads. |

The inspection panel renders fields backed by [EXISTS] sources on day one. "Recent tool calls" and "last activity" appear only when the event model, event persistence, and query API all support identity-attributed, tool-call-id-tagged events. The panel grows incrementally — it does not block on the full event model.

Dock panel kind: `"identity-inspect"`. Shared view state type in `@console-core`.

---

## Tier 2 — Virtualized activity log [NEW]

**Current state [EXISTS]:** `ConsoleActivityPulsePanel` — handles ~50 items, no virtualization, no search/filter.

### Shared primitive in `@console-components`

- Virtualized scrolling (1000+ items without DOM bloat)
- Text search filter
- Category/agent filter chips (by identity when identity-attributed events are available, falls back to member_id/agent_id)
- Pause/resume toggle
- Click-to-navigate (focuses relevant identity in dock)
- Auto-scroll with "N new events" indicator when scrolled up

### Retention contract

- `maxBufferSize: number` — configurable by host, default 1000
- Eviction: circular buffer (drop oldest when full)
- Backward pagination: host-provided `onLoadMore?: () => Promise<items[]>` for scrolling into history beyond the buffer
- Shared component manages DOM virtualization; host manages the data buffer

Host provides `renderItem`. Pulse/Roster/Feed stay for summary use cases.

### Activity log data source

The activity log shows all events from all identities — architecturally separate from per-identity conversation streams. Data sources:

- **All-events SSE stream [EXISTS as `mobkit/subscribe`, NEW for identity attribution]:** The current `mobkit/subscribe` RPC supports `scope: "mob"` which streams all mob events. Under the identity-native model, this stream must carry the same envelope format as per-identity streams (with `identity` attribution and optional `interaction_id`) **[NEW — current events are member/session-shaped and lack identity attribution; adding identity to the all-events envelope is runtime work]**, so the shared virtualized list component works consistently across both feeds.
- **New endpoint [NEW]:** `GET /console/events/stream` — a dedicated all-events SSE endpoint for the activity log, carrying identity-attributed events in the standard envelope format. Alternatively, the existing `mobkit/subscribe` can be enhanced to emit identity-attributed envelopes.
- **The activity log does NOT merge per-identity streams client-side.** It uses a single all-events stream. Per-identity streams are for conversation panels only.

---

## Tier 2 — Tool call/result blocks [NEW]

New block type in `@console-core`:

```typescript
interface ConversationRichToolCallBlock {
  type: "tool-call";
  toolCallId: string;
  name: string;
  arguments: string;
  result?: string;
  status: "pending" | "success" | "error";
}
```

Tool call blocks are always collapsible (args and result expand/collapse independently). No `collapsible` field — it's an invariant of the block type.

### Pairing by `tool_call_id`, not adjacency

Agents issue parallel tool calls; results return in arbitrary order.

### Shared `ToolCallAccumulator` in `@console-core`

A stateful helper that the host feeds frames into:

- Tracks open calls by `tool_call_id`
- Matches out-of-order results to their calls
- Handles never-arrived results: after a configurable timeout (default 60s), status transitions from `"pending"` to `"error"` with a timeout indicator
- Produces `ConversationRichToolCallBlock` entries that update from pending → complete/error
- Handles the common sequences: `tool_call A` → `tool_call B` → `tool_result B` → `tool_result A`

Each host should NOT reimplement this. The accumulator is shared; the host just feeds frames and reads blocks.

### Host rendering callback

On `ConversationPane`: `renderToolBlock?: (block: ToolCallBlock) => ReactNode | null`

- Return `null` → default shared renderer
- Return ReactNode → domain-specific rendering (MobKit knows nothing about what the tools do)

---

## Tier 2 — Actor topology graph [NEW for identity graph, EXISTS for module graph]

### Shared base panel

- Identity nodes positioned in a graph layout (from `topology` section of experience)
- Managed edges between them
- Node state indicators (active, idle, error)
- Click to inspect (opens identity inspection panel)

Host overlays via callback. Base graph and layout is shared; hosts annotate nodes with domain-specific decorations.

---

## Tier 2 — Generic gating/approvals surface [NEW]

MobKit owns gating data types **[EXISTS]**: `GatingEvaluateResult`, `GatingDecisionResult`, `GatingAuditEntry`, `GatingPendingEntry`.

The runtime currently exposes **[EXISTS]** a single decision path centered on `mobkit/gating/decide` plus pending query and audit query.

Console surface **[NEW]** — requires new work at multiple layers:

### Read side

- Pending entries with identity, action, risk tier, timestamp
- Approved/denied/escalated history
- Audit trail per decision

### Write/action side [NEW — requires new gateway RPCs + gating module extension]

The current `mobkit/gating/decide` is a programmatic API called by code, not an RPC endpoint shaped for human operators. Console-friendly action methods need to be designed. Two approaches:

**Option A — Extend `mobkit/gating/decide` for console use:**

The existing `decide` method gains an optional `rationale` field and the console calls it with outcome values (approve/deny/escalate). No new RPC methods needed.

**Option B — New console-specific action methods:**

```
mobkit/gating/approve    → { pending_entry_id, rationale? }
mobkit/gating/deny       → { pending_entry_id, rationale? }
mobkit/gating/escalate   → { pending_entry_id, rationale? }
```

These are **[NEW]** gateway handlers that wrap the underlying gating module. They require: gateway RPC registration, auth/permission checks (who can approve?), and gating module support for accepting external human decisions.

**Whichever approach is chosen:**
- Actions are **idempotent**: approving an already-approved entry is a no-op, not an error
- Response: `{ entry_id, outcome: "approved" | "denied" | "escalated", resolved_at }`
- `rationale` support declared in capabilities: `capabilities: ["approve", "deny", "escalate", "rationale"]`
- Action methods are declared in the experience `gating` section capabilities, so the shared panel knows what buttons to render

The shared panel renders action buttons based on declared capabilities. Hosts specialize the **entry context rendering** (what policy information is shown alongside the action), not the action mechanics.

Optional — only rendered when `gating` is present in the experience contract.

---

## Tier 2 — Routing/delivery surface [NEW section, EXISTS RPCs]

Separate from `flows` (scheduling). The routing and delivery RPCs already exist in MobKit:

| RPC | Status | What it does |
|---|---|---|
| `mobkit/routing/resolve` | **[EXISTS]** | Resolve a recipient to a route (route_id, channel, sink, target_module) |
| `mobkit/routing/routes/list` | **[EXISTS]** | List configured routes |
| `mobkit/routing/routes/add` | **[EXISTS]** | Add a route |
| `mobkit/routing/routes/delete` | **[EXISTS]** | Remove a route |
| `mobkit/delivery/send` | **[EXISTS]** | Send a payload via a resolved route |
| `mobkit/delivery/history` | **[EXISTS]** | Query delivery records (status, attempts, retry info) |

The data types also exist: `RoutingResolution` (route_id, recipient, channel, sink, target_module, retry_max, backoff_ms, rate_limit_per_minute), `DeliveryRecord` (delivery_id, route_id, recipient, status, attempts[], idempotency_key), `DeliveryAttempt` (attempt number, status, backoff_ms).

### What's new

**Experience `routing` section [NEW]:**

```typescript
routing: {
  schema_version: "1",
  capabilities: ["list_routes", "resolve", "delivery_history"],
  refresh: { mode: "poll", interval_ms: 10000 }
}
```

**Console panel [NEW]:** A shared routing/delivery panel that shows:

- Configured routes (from `mobkit/routing/routes/list`)
- Recent deliveries with status (from `mobkit/delivery/history`) — delivered, failed, retrying
- Per-delivery attempt history (attempt count, backoff, final status)

Host-specific overlays (NOT shared): app-specific notification semantics, Slack channel rendering, business-specific routing reasons. The shared component provides the route list + delivery timeline chrome; hosts provide domain-specific context rendering where needed.

---

## Tier 3 — UX polish

### Response phase indicator [NEW]

```typescript
type ResponsePhase = "waiting" | "tool-executing" | "generating" | null;
```

- `"waiting"` — sent, no events received yet (the most common and longest phase)
- `"tool-executing"` — `tool_call` event received, waiting for result
- `"generating"` — `text_delta` events arriving
- `null` — idle

Host derives the phase from the event stream. Shared component renders the appropriate indicator. `"thinking"` omitted until the event model supports a thinking event from providers.

### Watch/monitoring semantics [EXISTS partial, NEW full spec]

**Current state [EXISTS]:** `ConsoleSidebarItem` has `pinned: boolean` (sorts to top, shows pin icon) and `unread: boolean` (bold title). These cover basic "I selected this" and "something happened" states.

**Full monitoring model [NEW]:**

#### Watchlist

Operators need to explicitly monitor a subset of identities. Watching is distinct from pinning (layout preference) and selection (current focus).

```typescript
interface WatchState {
  watched: boolean;          // operator opted in to monitoring this identity
  alertLevel: "elevated" | "critical" | null;  // threshold-based
}
```

- `watched` on `ConsoleSidebarItem` — when true, the identity's events are highlighted in the activity log, and state changes trigger visual alerts
- `alertLevel` drives visual treatment: `null` = standard watched indicator (no alert), `"elevated"` = amber badge, `"critical"` = red badge + optional notification
- Host sets `alertLevel` based on domain-specific rules (e.g., the operator deployment: agent processing for >60s = elevated; the downstream app: gate decision pending >5m = critical)

**Sidebar rendering:**
- Watched items show a persistent indicator (eye icon or status dot), not just on hover
- Alert level maps to badge color: `"elevated"` → amber, `"critical"` → red
- Unwatched items with `unread: true` still show a subtle unread indicator

#### Degraded-state badges

Independent of chat activity. An identity can be degraded (lease expired, error state, peer unreachable) without any unread messages.

```typescript
interface DegradedState {
  degraded: boolean;
  reason?: string;           // "lease_expired" | "error" | "peer_unreachable" | "stalled"
}
```

- Rendered as a warning badge on the sidebar item, independent of `unread`
- Host derives degraded state from `mobkit/status_identity` (lifecycle state) and `mobkit/inspect_identity` (peer reachable count, is_final)
- The shared sidebar component renders the badge; the host provides the data

#### Sticky activity filters

The activity log should support persistent filter presets tied to the watchlist.

```typescript
interface ActivityFilterPreset {
  id: string;
  label: string;                    // "Watched only", "Critical", "All"
  identityFilter: "all" | "watched" | "watched_critical";
  eventTypeFilter?: string[];       // optional: only show specific event types
}
```

- The virtualized activity log accepts `filterPresets: ActivityFilterPreset[]` and `activePresetId: string`
- Preset chips render above the event list (shared component)
- "Watched only" filters the all-events stream to identities in the operator's watchlist
- "Critical" filters to identities with `alertLevel: "critical"`
- Presets are host-configured (different operators may want different defaults)
- The active preset persists across page refreshes (host stores in localStorage or similar)

#### Data flow

```
Host derives watch/alert/degraded state from:
  - operator preference (watched: true/false, stored client-side)
  - mobkit/status_identity (lifecycle state → degraded)
  - mobkit/inspect_identity (peer health → degraded)
  - domain rules (processing time → alertLevel)
    ↓
Host builds ConsoleSidebarItem with:
  - pinned, unread (existing)
  - watched, alertLevel (new fields on view state)
  - degraded, degradedReason (new fields on view state)
    ↓
Shared sidebar component renders:
  - pin icon (existing)
  - watch indicator + alert badge (new)
  - degraded warning badge (new)
    ↓
Activity log filters by watchlist when preset is active
```

**New fields on `ConsoleSidebarItem` [NEW in @console-core]:**

```typescript
// Added to existing ConsoleSidebarItem
{
  watched?: boolean;
  alertLevel?: "elevated" | "critical" | null;
  degraded?: boolean;
  degradedReason?: string;
}
```

**New fields on `ConsoleSidebarMeta` [EXISTS — alertLevel maps to tone]:**

Alert levels map to `ConsoleSidebarMetaTone`: `"elevated"` → new `"warning"` tone (amber), `"critical"` → `"negative"` (red). `null` (no alert) shows no badge beyond the watched indicator.

### Per-agent lifecycle actions [EXISTS for identity-first RPCs on IdentityRuntime, NEW for console-facing HTTP/RPC surface]

The identity-first lifecycle methods (retire, respawn, reset, inspect) exist on `IdentityRuntime` in the Rust core **[EXISTS]**. However, the console-facing HTTP/RPC surface currently exposes only member-oriented retire/respawn **[EXISTS]** and does not expose the full identity-first lifecycle set **[NEW]**. Wiring identity-first lifecycle actions through the console RPC layer requires new gateway handlers.

Console UI: context menu on sidebar items (retire, respawn, reset, inspect). Action strip for global flow triggers (host-configurable via action strip items).

---

## Host-specific surfaces

| Surface | Who | Extension point |
|---|---|---|
| Domain state projections | The downstream app | Custom dock panel `kind` + `renderPanelBody` |
| Connector-specific routing context | The downstream app | Host overlay on shared routing panel (business routing reasons, channel semantics) |
| Domain-specific tool renderer | Both | `renderToolBlock` callback |
| Flow trigger actions | Both | Action strip items + `onBlockAction` |
| Domain-specific context menu | Both | `onItemContextMenu` callback |
| Domain-specific gating entry context | Both | Gating entry render callback |

---

## Architectural principle

The experience endpoint describes what data is available, with protocol metadata per section (schema version, refresh behavior, capabilities for actionable sections). The host decides what panels to show. Shared components render normalized view states via callbacks and slots. MobKit owns generic operator primitives (identity, gating, topology, lifecycle, activity). Hosts own domain-specific rendering and policy semantics. MobKit knows nothing about Slack, reviews, app domains, or domain-specific gate policies — but it provides typed extension points for hosts that do.
