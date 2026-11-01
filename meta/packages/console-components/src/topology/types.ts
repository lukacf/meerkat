import type { TopologyEndpointPresentation, TopologyEndpointRef } from "@console-core";
import type { ConsoleAgent } from "@console-core/runtime-types";

export type { ConsoleAgent, ConsoleFrame } from "@console-core/runtime-types";
export type {
  TopologyActionCapability,
  TopologyAuthorityRevisionTransition,
  TopologyCanonicalEdge,
  TopologyConnectionState,
  TopologyEdgeAffordance,
  TopologyEdgeRef,
  TopologyEndpoint,
  TopologyEndpointPresentation,
  TopologyEndpointRef,
  TopologyManagementState,
  TopologyMutationIntent,
  TopologyMutationKind,
  TopologyMutationOrigin,
  TopologyOperationReceipt,
} from "@console-core";

/**
 * The panel's views. `graph` is the Lanes map (its test id stays
 * `topology-view:graph`), `clusters` the grouped map, `roles` the role tree,
 * `list` the table of agents and their connections, and `connections` the
 * full connection editor (reached from the overflow menu).
 */
export type TopologyPanelView = "graph" | "clusters" | "roles" | "list" | "connections";

/**
 * Optional host-supplied display metadata. These fields never carry runtime
 * authority; they only let a shared topology surface explain host groupings.
 */
export interface ConsoleTopologyNodePresentation
  extends Omit<TopologyEndpointPresentation, "label"> {
  /** Optional display override; `ConsoleTopologyNode.label` remains primary. */
  label?: string;
}

export interface ConsoleTopologyNode {
  identity?: string;
  /** Optional canonical endpoint metadata for topology-aware hosts. */
  ref?: TopologyEndpointRef;
  label?: string;
  role?: string;
  state?: string;
  wired_to?: string[];
  labels?: Record<string, string>;
  group?: string;
  subgroup?: string;
  addressable?: boolean;
  presentation?: ConsoleTopologyNodePresentation;
}

/**
 * One pair the topology control plane reports, with its flags. `from` and
 * `to` name the pair's endpoints the way the host's nodes do (an endpoint
 * key, an identity or an alias). Only an `actual` pair is wired; a declared
 * or desired pair that is not actual is a wiring the runtime does not have.
 */
export interface ConsoleTopologyLink {
  from: string;
  to: string;
  /** Both members list each other: the pair is wired now. */
  actual: boolean;
  /** The team's definition declares the pair. */
  declared?: boolean;
  /** The runtime wants the pair wired (declared and not suppressed, or operator added). */
  desired?: boolean;
  /** An operator disconnected a declared pair on purpose. */
  suppressed?: boolean;
  /** An operator added the pair outside the definition. */
  operatorAdded?: boolean;
  /** The endpoints sit in different authorities (another team or runtime). */
  cross?: boolean;
}

/**
 * A route that feeds a recipient from a channel. Only a route whose
 * recipient is a node on the map is drawn; the rest stay in Routing.
 */
export interface ConsoleTopologyRoute {
  /** The route's key, unique per runtime. */
  key: string;
  /** The channel the route listens on ("email", "notification"); the source node. */
  channel: string;
  /** Who the route delivers to: an agent identity or an outside address. */
  recipient: string;
  sink?: string | null;
  module?: string | null;
  /**
   * The runtime keeps this route for every member on its own (a roster
   * route), as opposed to one an operator configured.
   */
  managed?: boolean;
  /** Deliveries on this route, when the host could match them to it. */
  deliveries?: { sent: number; failed: number; lastAtMs?: number | null } | null;
}

/** How a helper came from its parent. */
export type ConsoleTopologyLineageKind = "side_chat" | "fork" | "spawn" | "delegate";

/** A parent that started a helper, fork or side chat. */
export interface ConsoleTopologyLineage {
  parent: string;
  child: string;
  kind: ConsoleTopologyLineageKind;
  atMs?: number | null;
}

/** Messages one agent sent another inside the host's traffic window. */
export interface ConsoleTopologyTraffic {
  from: string;
  to: string;
  count: number;
  /** When the latest of them was sent. */
  lastAtMs?: number | null;
}

/**
 * Peer messages one agent sent another: one message at `atMs`, or `count`
 * of them in a bucket that starts at `atMs` (the activity ledger keeps one
 * per minute). The panel sums them for its window, draws the trend and plays
 * the newest single messages as particles.
 */
export interface ConsoleTopologyMessage {
  id?: string;
  from: string;
  to: string;
  atMs: number;
  /** Messages this entry stands for; 1 when omitted. */
  count?: number;
}

/**
 * The host's activity history (the console's activity ledger): messages per
 * ordered pair in a time range, per-minute series and open runs. The panel
 * listens to it and reads it again when its version changes.
 */
export interface ConsoleTopologyActivitySource {
  subscribe(listener: () => void): () => void;
  getVersion(): number;
  /** Messages per ordered pair sent in the range. */
  traffic(window?: { sinceMs?: number; untilMs?: number }): ReadonlyArray<{ from: string; to: string; count: number; lastMs: number }>;
  /** Counts per bin, oldest first; a null identity sums every agent. */
  series(
    identity: string | null,
    metric: "messagesSent" | "turnsStarted",
    window: { bins: number; binMs?: number; endMs?: number },
  ): number[];
  /** A run of the agent started and has not ended. */
  hasOpenRun(identity: string): boolean;
}

/** A memory store drawn in the SHARED lane. */
export interface ConsoleTopologyStore {
  id: string;
  label: string;
  /** Records the store holds, when the host knows. */
  records?: number | null;
}

/** An agent's own memory, from the memory overview (identity scopes). */
export interface ConsoleTopologyMemoryScope {
  identity: string;
  /** Active records in the agent's scope. */
  records: number;
  quarantined?: number;
}

/**
 * Whether the routes read worked. `denied`: the caller lacks routing access
 * (the legend says so and CHANNELS stays hidden). `unavailable`: the runtime
 * serves no routes or the read failed.
 */
export type ConsoleTopologyRoutesAccess = "loaded" | "denied" | "unavailable";

/**
 * The host's agent sections (the sidebar's Category view), so the map's lanes
 * read the same words as the sidebar.
 */
export interface ConsoleTopologyGrouping {
  /** The section an agent belongs to. */
  sectionOf: (agent: ConsoleAgent) => string;
  /** Section order, top to bottom. Sections not listed follow in name order. */
  order?: readonly string[];
}

/** The host's one rule for an agent's state (the sidebar's and the roster's). */
export interface ConsoleTopologyAgentStatus {
  /** A turn or a tool is running now. */
  working: boolean;
  /**
   * Health that needs attention, drawn as the node's badge and counted as a
   * problem: warn (degraded, needs repair) or crit (stalled, broken).
   */
  health?: "warn" | "crit" | null;
  /** The health in words when it needs attention ("Degraded", "Stalled"). */
  healthWord?: string | null;
  /** The agent left the team (retired, retiring, stopped). */
  retired?: boolean;
}
