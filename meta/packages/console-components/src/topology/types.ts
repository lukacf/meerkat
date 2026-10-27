import type { TopologyEndpointPresentation, TopologyEndpointRef } from "@console-core";

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

export type TopologyPanelView = "graph" | "roles" | "connections";

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
