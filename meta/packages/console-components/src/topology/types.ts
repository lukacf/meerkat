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

/** One message to play along its edge (the live particle). */
export interface ConsoleTopologyMessage {
  id?: string;
  from: string;
  to: string;
  atMs: number;
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
  /** Health that needs attention: degraded (warn) or wedged, stalled, broken (crit). */
  health?: "warn" | "crit" | null;
  /** The state in words ("Working", "Idle", "Degraded"). */
  word?: string | null;
}
