/**
 * Transport-neutral topology control models shared by Meta console hosts.
 *
 * These types intentionally describe observed state and server-issued
 * capabilities. They do not grant authority: a console may only offer a
 * mutation when both the global policy and the specific endpoint pair say it
 * is available.
 */

export type TopologyRevision = string | number;

/** Runtime-advertised policy gates for the optional topology control plane. */
export interface ConsoleTopologyControlCapabilities {
  mode?: "disabled" | "read_only" | "editable";
  can_query?: boolean;
  can_plan?: boolean;
  can_apply?: boolean;
  can_bulk?: boolean;
  max_batch_size?: number;
  can_cross_authority?: boolean;
}

export interface TopologyEndpointRef {
  /** Stable opaque host id. Labels must never be used as identity. */
  id: string;
  /** Optional runtime authority metadata. The opaque `id` remains canonical. */
  authority?: string | null;
  /** Optional runtime identity metadata. The opaque `id` remains canonical. */
  identity?: string | null;
}

/** Display-only metadata. None of these fields carry runtime authority. */
export interface TopologyEndpointPresentation {
  label: string;
  caption?: string | null;
  section?: string | null;
  scopeId?: string | null;
  scopeLabel?: string | null;
  crossScope?: boolean;
  accent?: string | null;
  searchTerms?: string[];
}

export interface TopologyEndpoint {
  ref: TopologyEndpointRef;
  presentation: TopologyEndpointPresentation;
  state?: string | null;
  tags?: Record<string, string>;
}

/**
 * An undirected endpoint pair. `from` and `to` are stable endpoint ids; use
 * `topologyEdgeKey` whenever the pair is used as a lookup key.
 */
export interface TopologyEdgeRef {
  from: string;
  to: string;
}

export type TopologyConnectionState =
  | "disconnected"
  | "connected"
  | "degraded"
  | "conflict";

export interface TopologyCanonicalEdge extends TopologyEdgeRef {
  key: string;
  state: Exclude<TopologyConnectionState, "disconnected">;
  revision?: TopologyRevision | null;
  message?: string | null;
}

export type TopologyMutationKind = "connect" | "disconnect" | "reconnect";
export type TopologyMutationOrigin = "picker" | "graph" | "host_action";

export type TopologyCapabilityState =
  | "allowed"
  | "approval_required"
  | "denied"
  | "unsupported";

export interface TopologyActionCapability {
  state: TopologyCapabilityState;
  /** Human-readable explanation suitable for a disabled-state tooltip. */
  reason?: string | null;
  /** Optional policy key for diagnostics; never interpreted as authority. */
  permission?: string | null;
}

export interface TopologyMutationCapabilities {
  connect: TopologyActionCapability;
  disconnect: TopologyActionCapability;
  reconnect: TopologyActionCapability;
  /** Absent means the server does not expose bulk topology mutation. */
  bulk?: TopologyActionCapability | null;
}

export type TopologyManagementMode = "disabled" | "read_only" | "editable";

export interface TopologyMutationPolicy {
  mode: TopologyManagementMode;
  capabilities: TopologyMutationCapabilities;
  /** Required finite ceiling whenever bulk mutation is exposed. */
  maxBatchSize?: number | null;
  reason?: string | null;
}

export interface TopologyMutationIntent {
  action: TopologyMutationKind;
  edge: TopologyEdgeRef;
  expectedRevision: TopologyRevision;
  /** Exact bilateral CAS map; hosts must not collapse it to one revision. */
  expectedAuthorityRevisions?: Record<string, number>;
  origin: TopologyMutationOrigin;
  reason?: string | null;
  idempotencyKey?: string | null;
}

export type TopologyOperationStatus =
  | "pending_approval"
  | "queued"
  | "running"
  | "succeeded"
  | "denied"
  | "conflict"
  | "partial"
  | "failed"
  | "cancelled";

export interface TopologyOperationEdgeResult {
  edge: TopologyEdgeRef;
  action: TopologyMutationKind;
  status: "succeeded" | "denied" | "conflict" | "failed";
  message?: string | null;
}

export interface TopologyOperationReceipt {
  operationId: string;
  /** Correlates a host's optimistic receipt with the server operation id. */
  idempotencyKey?: string | null;
  /**
   * Exact mutation admitted for this operation. Hosts retain this only when a
   * user-visible recovery action may need to resolve an ambiguous transport
   * outcome. Recovery must replay this request and its idempotency key; it
   * must never silently replace the CAS token with a fresh revision.
   */
  request?: TopologyMutationIntent | null;
  /**
   * `resolve_ambiguous` means the server may already have committed the exact
   * request. A retry is resolution, not a new mutation. `revision_rebase` is
   * reserved for an explicit user-approved rebase that intentionally mints a
   * new idempotency key.
   */
  retryMode?: "resolve_ambiguous" | "revision_rebase" | null;
  action: TopologyMutationKind;
  status: TopologyOperationStatus;
  edge?: TopologyEdgeRef | null;
  requestedAt?: string | null;
  updatedAt?: string | null;
  revision?: TopologyRevision | null;
  authorityRevisions?: Record<string, TopologyAuthorityRevisionTransition>;
  message?: string | null;
  retryable?: boolean;
  results?: TopologyOperationEdgeResult[];
}

export interface TopologyEdgeAffordance {
  edge: TopologyEdgeRef;
  state: TopologyConnectionState;
  /** Pair-specific optimistic revision. Preferred over the aggregate view token. */
  expectedRevision?: TopologyRevision | null;
  /** Exact bilateral CAS map for this pair. Never union maps from other pairs. */
  expectedAuthorityRevisions?: Record<string, number> | null;
  /**
   * Server/host-selected repair action for ambiguous degraded or conflict
   * states. For example, an edge that is suppressed but still physically
   * present must retry `disconnect`, while an absent suppressed edge should
   * offer `reconnect`.
   */
  preferredAction?: TopologyMutationKind | null;
  actions: Partial<Record<TopologyMutationKind, TopologyActionCapability>>;
  message?: string | null;
}

export type TopologyManagementHealth = "ready" | "degraded" | "conflict";
export type TopologyCasScope = "homogeneous" | "mixed";

/** Fully controlled topology mutation state supplied by a trusted host. */
export interface TopologyManagementState {
  revision: TopologyRevision;
  /**
   * Whether every affordance shares one bilateral CAS scope or the view
   * aggregates distinct authority pairs. Mixed views must supply CAS on every
   * edge and never fall back to this legacy global map.
   */
  casScope?: TopologyCasScope;
  /** Exact per-authority CAS snapshot for a homogeneous bilateral view. */
  authorityRevisions?: Record<string, number>;
  policy: TopologyMutationPolicy;
  affordances: TopologyEdgeAffordance[];
  operations?: TopologyOperationReceipt[];
  health?: TopologyManagementHealth;
  message?: string | null;
}

export interface TopologyActorRef {
  id: string;
  label?: string | null;
}

export interface TopologyAuthorityRevisionTransition {
  before: number;
  after: number;
}

export interface TopologyAuditEvent {
  id: string;
  occurredAt: string;
  action: TopologyMutationKind;
  edge: TopologyEdgeRef;
  outcome: TopologyOperationStatus;
  actor?: TopologyActorRef | null;
  operationId?: string | null;
  revision?: TopologyRevision | null;
  message?: string | null;
}

export function topologyEdgeKey(edge: TopologyEdgeRef): string;
export function topologyEdgeKey(from: string, to: string): string;
export function topologyEdgeKey(
  edgeOrFrom: TopologyEdgeRef | string,
  maybeTo?: string,
): string {
  const from = typeof edgeOrFrom === "string" ? edgeOrFrom : edgeOrFrom.from;
  const to = typeof edgeOrFrom === "string" ? maybeTo || "" : edgeOrFrom.to;
  return JSON.stringify(from < to ? [from, to] : [to, from]);
}

/** Decode keys produced by `topologyEdgeKey` without assuming opaque-id syntax. */
export function topologyEdgeFromKey(key: string): TopologyEdgeRef | null {
  try {
    const pair = JSON.parse(key) as unknown;
    if (
      !Array.isArray(pair)
      || pair.length !== 2
      || typeof pair[0] !== "string"
      || typeof pair[1] !== "string"
      || !pair[0]
      || !pair[1]
      || pair[0] === pair[1]
    ) return null;
    return canonicalTopologyEdge({ from: pair[0], to: pair[1] });
  } catch {
    return null;
  }
}

export function canonicalTopologyEdge(edge: TopologyEdgeRef): TopologyEdgeRef {
  return edge.from < edge.to
    ? { from: edge.from, to: edge.to }
    : { from: edge.to, to: edge.from };
}

/** Stable scalar view token for a bilateral revision map; never use it as CAS. */
export function topologyAuthorityRevisionToken(
  revisions: Readonly<Record<string, number>>,
): string {
  return JSON.stringify(
    Object.entries(revisions).sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0),
  );
}

export function topologyCapabilityAllowsRequest(
  capability: TopologyActionCapability | null | undefined,
): boolean {
  return capability?.state === "allowed" || capability?.state === "approval_required";
}

export function topologyOperationIsPending(
  receipt: TopologyOperationReceipt | null | undefined,
): boolean {
  return receipt?.status === "pending_approval"
    || receipt?.status === "queued"
    || receipt?.status === "running";
}

/**
 * Coordinator CAS is bilateral: exactly two named authorities, each at a
 * finite non-negative integer revision. Do not normalize, union, or truncate
 * maps in the presentation layer.
 */
export function topologyIsBilateralAuthorityRevisionMap(
  revisions: Readonly<Record<string, number>> | null | undefined,
): revisions is Readonly<Record<string, number>> {
  if (!revisions || Object.keys(revisions).length !== 2) return false;
  return Object.entries(revisions).every(([authority, revision]) => (
    Boolean(authority)
    && Number.isFinite(revision)
    && Number.isInteger(revision)
    && revision >= 0
  ));
}

export function topologyAffordanceFor(
  management: Pick<TopologyManagementState, "affordances">,
  edge: TopologyEdgeRef,
): TopologyEdgeAffordance | null {
  const key = topologyEdgeKey(edge);
  return management.affordances.find((candidate) => topologyEdgeKey(candidate.edge) === key) || null;
}

export function topologyOperationFor(
  management: Pick<TopologyManagementState, "operations">,
  edge: TopologyEdgeRef,
): TopologyOperationReceipt | null {
  const key = topologyEdgeKey(edge);
  const operations = management.operations || [];
  for (let index = operations.length - 1; index >= 0; index -= 1) {
    const operation = operations[index];
    if (operation.edge && topologyEdgeKey(operation.edge) === key) return operation;
  }
  return null;
}

/**
 * Resolve a UI request against both global and pair-specific capabilities.
 * Returning null is intentional fail-closed behavior.
 */
export function topologyMutationIntent(
  management: TopologyManagementState,
  action: TopologyMutationKind,
  edge: TopologyEdgeRef,
  origin: TopologyMutationOrigin,
): TopologyMutationIntent | null {
  if (management.policy.mode !== "editable") return null;
  const globalCapability = management.policy.capabilities[action];
  const pairCapability = topologyAffordanceFor(management, edge)?.actions[action];
  if (!topologyCapabilityAllowsRequest(globalCapability)) return null;
  if (!topologyCapabilityAllowsRequest(pairCapability)) return null;
  if (topologyOperationIsPending(topologyOperationFor(management, edge))) return null;
  const affordance = topologyAffordanceFor(management, edge);
  const edgeAuthorityRevisions = affordance?.expectedAuthorityRevisions;
  if (edgeAuthorityRevisions && !topologyIsBilateralAuthorityRevisionMap(edgeAuthorityRevisions)) {
    return null;
  }
  if (management.casScope === "mixed" && !edgeAuthorityRevisions) {
    return null;
  }
  const fallbackAuthorityRevisions = management.casScope === "mixed"
    ? null
    : management.authorityRevisions;
  if (fallbackAuthorityRevisions && !topologyIsBilateralAuthorityRevisionMap(fallbackAuthorityRevisions)) {
    return null;
  }
  const expectedAuthorityRevisions = edgeAuthorityRevisions || fallbackAuthorityRevisions;
  const intent: TopologyMutationIntent = {
    action,
    edge: canonicalTopologyEdge(edge),
    expectedRevision: affordance?.expectedRevision ?? management.revision,
    origin,
  };
  if (expectedAuthorityRevisions) {
    intent.expectedAuthorityRevisions = { ...expectedAuthorityRevisions };
  }
  return intent;
}
