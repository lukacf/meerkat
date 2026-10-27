import { topologyEdgeKey } from "@console-core";
import type {
  ConsoleTopologyControlCapabilities,
  TopologyActionCapability,
  TopologyConnectionState,
  TopologyEdgeAffordance,
  TopologyEdgeRef,
  TopologyManagementMode,
  TopologyManagementState,
  TopologyMutationIntent,
  TopologyMutationKind,
  TopologyOperationEdgeResult,
  TopologyOperationReceipt,
  TopologyOperationStatus,
} from "@console-core";

export type { ConsoleTopologyControlCapabilities } from "@console-core";

import type { ConsoleAgent, ConsoleTopologyNode } from "../types";

export interface RuntimeTopologyEndpoint {
  authority?: string | null;
  identity: string;
}

interface RuntimeTopologyNodeAffordances {
  can_connect?: boolean;
  can_disconnect?: boolean;
  can_reconnect?: boolean;
  can_bulk?: boolean;
  can_cross_authority?: boolean;
}

interface RuntimeTopologyNode {
  endpoint: RuntimeTopologyEndpoint;
  role?: string;
  labels?: Record<string, string>;
  affordances?: RuntimeTopologyNodeAffordances;
}

interface RuntimeTopologyEdge {
  edge: {
    a: RuntimeTopologyEndpoint;
    b: RuntimeTopologyEndpoint;
  };
  actual?: boolean;
  declared?: boolean;
  operator_added?: boolean;
  suppressed?: boolean;
  desired?: boolean;
}

interface RuntimeTopologyQuery {
  authority?: string;
  revision: number;
  policy?: {
    mode?: "disabled" | "read_only" | "editable";
    allow_bulk?: boolean;
    max_batch_size?: number;
    allow_cross_authority?: boolean;
  };
  nodes: RuntimeTopologyNode[];
  edges: RuntimeTopologyEdge[];
}

export interface NormalizedConsoleTopology {
  nodes: ConsoleTopologyNode[];
  management: TopologyManagementState;
}

export interface NormalizeConsoleTopologyOptions {
  agents?: readonly ConsoleAgent[];
  fallbackNodes?: readonly ConsoleTopologyNode[];
  capabilities?: ConsoleTopologyControlCapabilities | null;
  connectionSourceId?: string | null;
  operations?: readonly TopologyOperationReceipt[];
  consoleReadOnly?: boolean;
}

const MUTATION_KINDS: readonly TopologyMutationKind[] = [
  "connect",
  "disconnect",
  "reconnect",
];
const STOCK_ENDPOINT_KEY_PREFIX = "mk1";

/**
 * Stable, reversible stock-console id for a runtime endpoint. Both authority
 * and identity are encoded because identities are only authority-local.
 */
export function consoleTopologyEndpointKey(
  endpoint: RuntimeTopologyEndpoint,
  fallbackAuthority?: string | null,
): string {
  const authority = endpoint.authority || fallbackAuthority || "";
  return [
    STOCK_ENDPOINT_KEY_PREFIX,
    encodeURIComponent(authority),
    encodeURIComponent(endpoint.identity),
  ].join("|");
}

export function parseConsoleTopologyEndpointKey(
  id: string,
): RuntimeTopologyEndpoint | null {
  const parts = id.split("|");
  if (parts.length !== 3 || parts[0] !== STOCK_ENDPOINT_KEY_PREFIX) return null;
  try {
    const authority = decodeURIComponent(parts[1]);
    const identity = decodeURIComponent(parts[2]);
    if (!identity) return null;
    return { identity, authority: authority || undefined };
  } catch {
    return null;
  }
}

function endpointAuthority(
  endpoint: RuntimeTopologyEndpoint,
  fallbackAuthority?: string | null,
): string | null {
  return endpoint.authority || fallbackAuthority || null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function booleanValue(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}

function finiteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function parseEndpoint(value: unknown): RuntimeTopologyEndpoint | null {
  if (!isRecord(value)) return null;
  const identity = stringValue(value.identity);
  if (!identity) return null;
  return {
    identity,
    authority: stringValue(value.authority),
  };
}

function parseNodeAffordances(value: unknown): RuntimeTopologyNodeAffordances | undefined {
  if (!isRecord(value)) return undefined;
  return {
    can_connect: booleanValue(value.can_connect),
    can_disconnect: booleanValue(value.can_disconnect),
    can_reconnect: booleanValue(value.can_reconnect),
    can_bulk: booleanValue(value.can_bulk),
    can_cross_authority: booleanValue(value.can_cross_authority),
  };
}

function parseRuntimeTopologyQuery(value: unknown): RuntimeTopologyQuery | null {
  if (!isRecord(value)) return null;
  const revision = finiteNumber(value.revision);
  if (revision === null || !Array.isArray(value.nodes) || !Array.isArray(value.edges)) {
    return null;
  }
  const nodes = value.nodes.flatMap((candidate): RuntimeTopologyNode[] => {
    if (!isRecord(candidate)) return [];
    const endpoint = parseEndpoint(candidate.endpoint);
    if (!endpoint) return [];
    const labels = isRecord(candidate.labels)
      ? Object.fromEntries(
          Object.entries(candidate.labels)
            .filter((entry): entry is [string, string] => typeof entry[1] === "string"),
        )
      : undefined;
    return [{
      endpoint,
      role: stringValue(candidate.role) || undefined,
      labels,
      affordances: parseNodeAffordances(candidate.affordances),
    }];
  });
  const edges = value.edges.flatMap((candidate): RuntimeTopologyEdge[] => {
    if (!isRecord(candidate) || !isRecord(candidate.edge)) return [];
    const a = parseEndpoint(candidate.edge.a);
    const b = parseEndpoint(candidate.edge.b);
    if (
      !a
      || !b
      || (a.identity === b.identity && a.authority === b.authority)
    ) return [];
    return [{
      edge: { a, b },
      actual: booleanValue(candidate.actual),
      declared: booleanValue(candidate.declared),
      operator_added: booleanValue(candidate.operator_added),
      suppressed: booleanValue(candidate.suppressed),
      desired: booleanValue(candidate.desired),
    }];
  });
  const rawPolicy = isRecord(value.policy) ? value.policy : {};
  const mode = rawPolicy.mode === "read_only" || rawPolicy.mode === "editable"
    ? rawPolicy.mode
    : "disabled";
  return {
    authority: stringValue(value.authority) || undefined,
    revision,
    policy: {
      mode,
      allow_bulk: booleanValue(rawPolicy.allow_bulk),
      max_batch_size: finiteNumber(rawPolicy.max_batch_size) ?? undefined,
      allow_cross_authority: booleanValue(rawPolicy.allow_cross_authority),
    },
    nodes,
    edges,
  };
}

function edgeKey(edge: TopologyEdgeRef): string {
  return topologyEdgeKey(edge);
}

function canonicalEdge(from: string, to: string): TopologyEdgeRef {
  return from < to ? { from, to } : { from: to, to: from };
}

function mutationCapability(
  allowed: boolean,
  reason: string,
  permission: string,
): TopologyActionCapability {
  return allowed
    ? { state: "allowed", permission }
    : { state: "denied", reason, permission };
}

function endpointAllows(node: RuntimeTopologyNode, action: TopologyMutationKind): boolean {
  const affordances = node.affordances;
  if (!affordances) return false;
  if (action === "connect") return affordances.can_connect === true;
  if (action === "disconnect") return affordances.can_disconnect === true;
  return affordances.can_reconnect === true;
}

function pairAllowsCrossAuthority(
  fromNode: RuntimeTopologyNode,
  toNode: RuntimeTopologyNode,
  query: RuntimeTopologyQuery,
): boolean {
  const fromAuthority = endpointAuthority(fromNode.endpoint, query.authority);
  const toAuthority = endpointAuthority(toNode.endpoint, query.authority);
  // The embedded stock console has only the authority-local JSON-RPC
  // transport. Bilateral hosts build the shared management contract directly
  // and route its exact authority revision map through their coordinator.
  return fromAuthority === toAuthority;
}

function connectionState(edge: RuntimeTopologyEdge | undefined): TopologyConnectionState {
  if (!edge) return "disconnected";
  if (edge.actual && edge.suppressed) return "conflict";
  if (edge.actual) return "connected";
  // A suppressed declared edge is intentionally absent, but it must offer the
  // semantically distinct reconnect action rather than a fresh connect.
  if (edge.suppressed || edge.desired) return "degraded";
  return "disconnected";
}

function preferredAction(edge: RuntimeTopologyEdge | undefined): TopologyMutationKind {
  if (edge?.actual && edge.suppressed) return "disconnect";
  if (!edge?.actual && (edge?.suppressed || edge?.desired)) return "reconnect";
  if (edge?.actual) return "disconnect";
  return "connect";
}

function managementMode(
  query: RuntimeTopologyQuery,
  capabilities: ConsoleTopologyControlCapabilities | null | undefined,
  consoleReadOnly: boolean,
): TopologyManagementMode {
  if (query.policy?.mode === "disabled") return "disabled";
  if (
    query.policy?.mode !== "editable"
    || capabilities?.mode !== "editable"
    || capabilities?.can_plan !== true
    || capabilities?.can_apply !== true
    || consoleReadOnly
  ) {
    return "read_only";
  }
  return "editable";
}

function modeReason(mode: TopologyManagementMode): string | null {
  if (mode === "disabled") return "Connection management is disabled for this runtime.";
  if (mode === "read_only") return "You can inspect connections, but this runtime does not permit topology changes.";
  return null;
}

/**
 * Adapt the authoritative Meta query response into the transport-neutral
 * shared component contract. Endpoint grants are intersected for every pair;
 * missing affordances fail closed instead of inferring authority from labels.
 */
export function normalizeConsoleTopologyQuery(
  value: unknown,
  options: NormalizeConsoleTopologyOptions = {},
): NormalizedConsoleTopology | null {
  const query = parseRuntimeTopologyQuery(value);
  if (!query) return null;

  const fallbackById = new Map(
    (options.fallbackNodes || [])
      .map((node) => [node.identity || "", node] as const)
      .filter(([identity]) => Boolean(identity)),
  );
  const agentById = new Map(
    (options.agents || []).flatMap((agent) => {
      const identities = [agent.identity, agent.member_id, agent.agent_id].filter(
        (identity): identity is string => Boolean(identity),
      );
      return identities.map((identity) => [identity, agent] as const);
    }),
  );
  // Query producers should include every endpoint node. Still materialize a
  // fail-closed stub for an observed cross-authority edge when an older
  // producer omits its remote node: the connection remains visible, while
  // absent endpoint affordances correctly prevent mutation.
  const topologyNodes = [...query.nodes];
  const observedNodeIds = new Set(topologyNodes.map((node) =>
    consoleTopologyEndpointKey(node.endpoint, query.authority)
  ));
  for (const edge of query.edges) {
    for (const endpoint of [edge.edge.a, edge.edge.b]) {
      const id = consoleTopologyEndpointKey(endpoint, query.authority);
      if (observedNodeIds.has(id)) continue;
      observedNodeIds.add(id);
      topologyNodes.push({
        endpoint,
        role: "remote",
        labels: { topology_stub: "true" },
      });
    }
  }
  const nodeById = new Map(topologyNodes.map((node) => [
    consoleTopologyEndpointKey(node.endpoint, query.authority),
    node,
  ]));
  const actualPeers = new Map<string, Set<string>>();
  const edgeByKey = new Map<string, RuntimeTopologyEdge>();
  for (const edge of query.edges) {
    const pair = canonicalEdge(
      consoleTopologyEndpointKey(edge.edge.a, query.authority),
      consoleTopologyEndpointKey(edge.edge.b, query.authority),
    );
    if (pair.from === pair.to) continue;
    edgeByKey.set(edgeKey(pair), edge);
    if (!edge.actual) continue;
    const fromPeers = actualPeers.get(pair.from) || new Set<string>();
    fromPeers.add(pair.to);
    actualPeers.set(pair.from, fromPeers);
    const toPeers = actualPeers.get(pair.to) || new Set<string>();
    toPeers.add(pair.from);
    actualPeers.set(pair.to, toPeers);
  }

  const nodes = topologyNodes.map((node): ConsoleTopologyNode => {
    const runtimeIdentity = node.endpoint.identity;
    const identity = consoleTopologyEndpointKey(node.endpoint, query.authority);
    const authority = endpointAuthority(node.endpoint, query.authority);
    const fallback = fallbackById.get(identity) || fallbackById.get(runtimeIdentity);
    const agent = agentById.get(runtimeIdentity);
    return {
      identity,
      ref: { id: identity, authority, identity: runtimeIdentity },
      label: fallback?.label || agent?.label || runtimeIdentity,
      role: node.role || fallback?.role || agent?.role,
      state: fallback?.state || agent?.state,
      wired_to: Array.from(actualPeers.get(identity) || []).sort(),
      labels: { ...(fallback?.labels || {}), ...(agent?.labels || {}), ...(node.labels || {}) },
      group: fallback?.group || agent?.group,
      subgroup: fallback?.subgroup || agent?.subgroup,
      addressable: fallback?.addressable ?? agent?.addressable,
    };
  });

  const mode = managementMode(query, options.capabilities, options.consoleReadOnly === true);
  const globalAllowed = mode === "editable";
  const reason = modeReason(mode);
  const globalCapabilities = Object.fromEntries(MUTATION_KINDS.map((action) => [
    action,
    mutationCapability(
      globalAllowed,
      reason || `${action} is not permitted by the runtime.`,
      `topology.${action}`,
    ),
  ])) as TopologyManagementState["policy"]["capabilities"];

  // Keep the adapter O(nodes + edges): include every observed edge for graph
  // actions and every candidate pair for the currently selected picker source.
  const pairs = new Map<string, TopologyEdgeRef>();
  for (const key of edgeByKey.keys()) {
    const edge = edgeByKey.get(key);
    if (!edge) continue;
    const pair = canonicalEdge(
      consoleTopologyEndpointKey(edge.edge.a, query.authority),
      consoleTopologyEndpointKey(edge.edge.b, query.authority),
    );
    pairs.set(key, pair);
  }
  if (options.connectionSourceId && nodeById.has(options.connectionSourceId)) {
    for (const targetId of nodeById.keys()) {
      if (targetId === options.connectionSourceId) continue;
      const pair = canonicalEdge(options.connectionSourceId, targetId);
      pairs.set(edgeKey(pair), pair);
    }
  }
  const affordances: TopologyEdgeAffordance[] = Array.from(pairs).flatMap(([key, pair]) => {
    const { from, to } = pair;
    const fromNode = nodeById.get(from);
    const toNode = nodeById.get(to);
    if (!fromNode || !toNode) return [];
    const edge = edgeByKey.get(key);
    const crossAuthorityAllowed = pairAllowsCrossAuthority(
      fromNode,
      toNode,
      query,
    );
    const actions = Object.fromEntries(MUTATION_KINDS.map((action) => {
      const allowed = globalAllowed
        && crossAuthorityAllowed
        && endpointAllows(fromNode, action)
        && endpointAllows(toNode, action);
      return [action, mutationCapability(
        allowed,
        reason
          || (!crossAuthorityAllowed
            ? "Permission denied: cross-authority topology changes require explicit bilateral access."
            : `Permission denied: ${action} requires access to both endpoints.`),
        `topology.${action}`,
      )];
    })) as TopologyEdgeAffordance["actions"];
    return [{
      edge: { from, to },
      state: connectionState(edge),
      preferredAction: preferredAction(edge),
      actions,
      message: edge?.suppressed
        ? "Disconnected by operator. Reconnect restores the declared relationship."
        : edge?.desired && !edge.actual
          ? "The desired connection is not currently active."
          : null,
    }];
  });

  return {
    nodes,
    management: {
      revision: query.revision,
      policy: {
        mode,
        capabilities: globalCapabilities,
        // The stock console never supplies a bulk action. Preserve the finite
        // server bound for hosts that intentionally add one later.
        maxBatchSize: query.policy?.allow_bulk && options.capabilities?.can_bulk === true
          ? query.policy.max_batch_size
          : null,
        reason,
      },
      affordances,
      operations: [...(options.operations || [])],
      health: Array.from(edgeByKey.values()).some((edge) => edge.actual && edge.suppressed)
        ? "conflict"
        : Array.from(edgeByKey.values()).some((edge) => edge.desired && !edge.actual && !edge.suppressed)
          ? "degraded"
          : "ready",
    },
  };
}

function runtimeEndpoint(id: string): RuntimeTopologyEndpoint {
  return parseConsoleTopologyEndpointKey(id) || { identity: id };
}

function runtimeMutation(intent: TopologyMutationIntent) {
  const a = runtimeEndpoint(intent.edge.from);
  const b = runtimeEndpoint(intent.edge.to);
  const aAuthority = a.authority || null;
  const bAuthority = b.authority || null;
  if (aAuthority !== bAuthority) {
    throw new Error(
      "The stock topology RPC is authority-local; cross-authority or ambiguously qualified intents require a coordinator host adapter.",
    );
  }
  return {
    action: intent.action,
    edge: {
      a,
      b,
    },
  };
}

export function topologyPlanParams(intent: TopologyMutationIntent): Record<string, unknown> {
  if (intent.expectedAuthorityRevisions) {
    throw new Error(
      "The stock topology RPC is authority-local; bilateral intents require a coordinator host adapter.",
    );
  }
  return {
    expected_revision: intent.expectedRevision,
    operations: [runtimeMutation(intent)],
  };
}

export function topologyApplyParams(
  intent: TopologyMutationIntent,
  idempotencyKey: string,
): Record<string, unknown> {
  return {
    ...topologyPlanParams(intent),
    idempotency_key: idempotencyKey,
  };
}

function normalizeOperationStatus(value: unknown): TopologyOperationStatus {
  if (value === "pending") return "running";
  if (value === "applied" || value === "noop") return "succeeded";
  if (value === "partial_degraded") return "partial";
  if (value === "rolled_back") return "failed";
  return "failed";
}

function normalizeAuthorityRevisionTransitions(
  value: unknown,
): TopologyOperationReceipt["authorityRevisions"] {
  if (!isRecord(value)) return undefined;
  const entries = Object.entries(value).flatMap(([authority, transition]) => {
    if (!authority || !isRecord(transition)) return [];
    const before = finiteNumber(transition.base_revision);
    const after = finiteNumber(transition.revision);
    if (before === null || after === null) return [];
    return [[authority, { before, after }] as const];
  });
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

function normalizeEdgeResult(value: unknown): TopologyOperationEdgeResult | null {
  if (!isRecord(value) || !isRecord(value.edge)) return null;
  const a = parseEndpoint(value.edge.a);
  const b = parseEndpoint(value.edge.b);
  const action = value.action;
  if (!a || !b || !MUTATION_KINDS.includes(action as TopologyMutationKind)) return null;
  const status = value.status === "applied" || value.status === "noop"
    ? "succeeded"
    : "failed";
  return {
    edge: canonicalEdge(
      consoleTopologyEndpointKey(a),
      consoleTopologyEndpointKey(b),
    ),
    action: action as TopologyMutationKind,
    status,
    message: stringValue(value.error),
  };
}

function cloneTopologyMutationRequest(
  request: TopologyMutationIntent,
  idempotencyKey = request.idempotencyKey || null,
): TopologyMutationIntent {
  return {
    ...request,
    edge: canonicalEdge(request.edge.from, request.edge.to),
    ...(request.expectedAuthorityRevisions
      ? { expectedAuthorityRevisions: { ...request.expectedAuthorityRevisions } }
      : {}),
    ...(idempotencyKey ? { idempotencyKey } : {}),
  };
}

export function normalizeTopologyOperationReceipt(
  value: unknown,
  request?: TopologyMutationIntent | null,
): TopologyOperationReceipt | null {
  if (!isRecord(value)) return null;
  const operationId = stringValue(value.operation_id);
  if (!operationId || !Array.isArray(value.results)) return null;
  const results = value.results.flatMap((result): TopologyOperationEdgeResult[] => {
    const normalized = normalizeEdgeResult(result);
    return normalized ? [normalized] : [];
  });
  const first = results[0];
  const status = normalizeOperationStatus(value.status);
  const idempotencyKey = stringValue(value.idempotency_key) || request?.idempotencyKey || null;
  return {
    operationId,
    idempotencyKey,
    request: request ? cloneTopologyMutationRequest(request, idempotencyKey) : null,
    retryMode: null,
    action: first?.action || "connect",
    status,
    edge: results.length === 1 ? first.edge : null,
    requestedAt: stringValue(value.created_at),
    updatedAt: stringValue(value.updated_at) || stringValue(value.created_at),
    revision: finiteNumber(value.revision),
    authorityRevisions: normalizeAuthorityRevisionTransitions(value.authority_revisions),
    message: results.find((result) => result.message)?.message || stringValue(value.reason),
    // A server receipt is a known outcome. A fresh idempotency key is only
    // valid after an explicit revision rebase, never from this generic row.
    retryable: false,
    results,
  };
}

export function pendingTopologyReceipt(
  intent: TopologyMutationIntent,
  operationId: string,
): TopologyOperationReceipt {
  const request = cloneTopologyMutationRequest(intent, operationId);
  return {
    operationId,
    idempotencyKey: operationId,
    request,
    retryMode: null,
    action: intent.action,
    status: "running",
    edge: intent.edge,
    requestedAt: new Date().toISOString(),
    revision: intent.expectedRevision,
  };
}

function receiptsReferToSameRequest(
  current: TopologyOperationReceipt,
  receipt: TopologyOperationReceipt,
): boolean {
  if (current.operationId === receipt.operationId) return true;
  if (
    current.idempotencyKey
    && receipt.idempotencyKey
    && current.idempotencyKey === receipt.idempotencyKey
  ) return true;
  return Boolean(
    current.edge
    && receipt.edge
    && current.action === receipt.action
    && edgeKey(current.edge) === edgeKey(receipt.edge)
    && ["pending_approval", "queued", "running"].includes(current.status),
  );
}

/** Replace the optimistic receipt when the runtime returns its operation id. */
export function mergeTopologyOperationReceipt(
  current: readonly TopologyOperationReceipt[],
  receipt: TopologyOperationReceipt,
): TopologyOperationReceipt[] {
  const next = current.filter((entry) => !receiptsReferToSameRequest(entry, receipt));
  next.push(receipt);
  return next.slice(-32);
}

export type ConsoleTopologyRpcOperation = "plan" | "apply" | "operation_get";

export type ConsoleTopologyRpcExecutor = (
  operation: ConsoleTopologyRpcOperation,
  params: Record<string, unknown>,
) => Promise<unknown>;

export interface ConsoleTopologyMutationRequest {
  intent: TopologyMutationIntent;
  idempotencyKey: string;
}

export interface ConsoleTopologyMutationAttempt {
  receipt: TopologyOperationReceipt;
  error: string | null;
}

export function createConsoleTopologyMutationRequest(
  intent: TopologyMutationIntent,
  idempotencyKey: string,
): ConsoleTopologyMutationRequest {
  const key = idempotencyKey.trim();
  if (!key) throw new Error("topology mutation idempotency key is required");
  return {
    intent: cloneTopologyMutationRequest(intent, key),
    idempotencyKey: key,
  };
}

function topologyMutationErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error.trim()) return error.trim();
  return "Meta topology operation failed";
}

function topologyRpcError(error: unknown): Record<string, unknown> | null {
  if (!error || typeof error !== "object") return null;
  const rpcError = (error as { rpcError?: unknown }).rpcError;
  return isRecord(rpcError) ? rpcError : null;
}

function topologyRpcErrorKind(error: unknown): string | null {
  const rpcError = topologyRpcError(error);
  return rpcError && isRecord(rpcError.data) ? stringValue(rpcError.data.kind) : null;
}

function topologyRpcOutcomeRemainsAmbiguous(error: unknown): boolean {
  return topologyRpcErrorKind(error) === "topology_operation_in_progress";
}

function definitiveTopologyFailure(
  pending: TopologyOperationReceipt,
  request: ConsoleTopologyMutationRequest,
  error: unknown,
): ConsoleTopologyMutationAttempt {
  const message = topologyMutationErrorMessage(error);
  const rpcError = topologyRpcError(error);
  const rpcData = rpcError && isRecord(rpcError.data) ? rpcError.data : null;
  const embedded = normalizeTopologyOperationReceipt(rpcData?.receipt, request.intent);
  if (embedded) {
    return {
      receipt: {
        ...embedded,
        message: embedded.message || message,
        retryable: false,
        retryMode: null,
      },
      error: message,
    };
  }

  const code = finiteNumber(rpcError?.code);
  const kind = topologyRpcErrorKind(error) || "";
  const revisionConflict = kind === "topology_revision_conflict";
  const status: TopologyOperationStatus = code === -32001 || kind === "access_denied"
    ? "denied"
    : code === -32009 || kind.includes("conflict")
      ? "conflict"
      : "failed";
  return {
    receipt: {
      ...pending,
      status,
      message,
      updatedAt: new Date().toISOString(),
      // A stale CAS token can only move forward through the separately
      // labelled, user-invoked Rebase action. Other definitive failures have
      // no generic retry path.
      retryable: revisionConflict,
      retryMode: revisionConflict ? "revision_rebase" : null,
    },
    error: message,
  };
}

function ambiguousTopologyFailure(
  pending: TopologyOperationReceipt,
  error: unknown,
): ConsoleTopologyMutationAttempt {
  const detail = topologyMutationErrorMessage(error);
  const message = topologyRpcOutcomeRemainsAmbiguous(error)
    ? `Topology operation is still in progress: ${detail}. Resolve checks the original request without creating a new mutation.`
    : `Topology outcome is unknown because the response was lost: ${detail}. Resolve checks the original request without creating a new mutation.`;
  return {
    receipt: {
      ...pending,
      status: "failed",
      message,
      updatedAt: new Date().toISOString(),
      retryable: true,
      retryMode: "resolve_ambiguous",
    },
    error: message,
  };
}

async function resolveTopologyOperationReceipt(
  receipt: TopologyOperationReceipt,
  request: ConsoleTopologyMutationRequest,
  execute: ConsoleTopologyRpcExecutor,
): Promise<TopologyOperationReceipt> {
  try {
    const result = await execute("operation_get", { operation_id: receipt.operationId });
    return normalizeTopologyOperationReceipt(result, request.intent) || receipt;
  } catch {
    // The apply receipt is already authoritative. A failed follow-up status
    // read must not turn a known outcome into a new mutation or a false error.
    return receipt;
  }
}

/**
 * Admit and apply one stock-console topology mutation. The planning failure
 * path is definitively side-effect free. Once apply begins, only a structured
 * JSON-RPC error is treated as a known rejection; transport/protocol failures
 * retain the exact request for same-key recovery.
 */
export async function executeConsoleTopologyMutation(
  request: ConsoleTopologyMutationRequest,
  execute: ConsoleTopologyRpcExecutor,
): Promise<ConsoleTopologyMutationAttempt> {
  const pending = pendingTopologyReceipt(request.intent, request.idempotencyKey);
  try {
    await execute("plan", topologyPlanParams(request.intent));
  } catch (error) {
    return definitiveTopologyFailure(pending, request, error);
  }

  let result: unknown;
  try {
    result = await execute("apply", topologyApplyParams(request.intent, request.idempotencyKey));
  } catch (error) {
    return !topologyRpcError(error) || topologyRpcOutcomeRemainsAmbiguous(error)
      ? ambiguousTopologyFailure(pending, error)
      : definitiveTopologyFailure(pending, request, error);
  }
  const receipt = normalizeTopologyOperationReceipt(result, request.intent);
  if (!receipt) {
    return ambiguousTopologyFailure(
      pending,
      new Error("Meta returned an invalid topology operation receipt"),
    );
  }
  return {
    receipt: await resolveTopologyOperationReceipt(receipt, request, execute),
    error: null,
  };
}

/**
 * Resolve an apply whose transport outcome is ambiguous. This deliberately
 * skips plan and replays the byte-equivalent semantic request with the same
 * idempotency key. The runtime either returns the original receipt or a
 * definitive idempotency error; it can never create a second logical change.
 */
export async function resolveAmbiguousConsoleTopologyMutation(
  ambiguous: TopologyOperationReceipt,
  execute: ConsoleTopologyRpcExecutor,
): Promise<ConsoleTopologyMutationAttempt> {
  const requestIntent = ambiguous.request;
  const idempotencyKey = ambiguous.idempotencyKey || requestIntent?.idempotencyKey || null;
  if (
    ambiguous.retryMode !== "resolve_ambiguous"
    || !ambiguous.retryable
    || !requestIntent
    || !idempotencyKey
  ) {
    return {
      receipt: ambiguous,
      error: "This topology operation has no ambiguous outcome to resolve.",
    };
  }

  const request = createConsoleTopologyMutationRequest(requestIntent, idempotencyKey);
  const pending = pendingTopologyReceipt(request.intent, request.idempotencyKey);
  let result: unknown;
  try {
    result = await execute("apply", topologyApplyParams(request.intent, request.idempotencyKey));
  } catch (error) {
    return !topologyRpcError(error) || topologyRpcOutcomeRemainsAmbiguous(error)
      ? ambiguousTopologyFailure(pending, error)
      : definitiveTopologyFailure(pending, request, error);
  }
  const receipt = normalizeTopologyOperationReceipt(result, request.intent);
  if (!receipt) {
    return ambiguousTopologyFailure(
      pending,
      new Error("Meta returned an invalid topology operation receipt"),
    );
  }
  return {
    receipt: await resolveTopologyOperationReceipt(receipt, request, execute),
    error: null,
  };
}
