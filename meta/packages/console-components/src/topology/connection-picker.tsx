import React from "react";

import {
  topologyAffordanceFor,
  topologyCapabilityAllowsRequest,
  topologyEdgeKey,
  topologyMutationIntent,
  topologyOperationFor,
  topologyOperationIsPending,
  type TopologyActionCapability,
  type TopologyCanonicalEdge,
  type TopologyConnectionState,
  type TopologyEdgeRef,
  type TopologyEndpoint,
  type TopologyManagementState,
  type TopologyMutationIntent,
  type TopologyMutationKind,
  type TopologyOperationReceipt,
} from "@console-core";

const DEFAULT_VISIBLE_LIMIT = 100;

export interface TopologyBoundedAction {
  id: string;
  label: string;
  description?: string | null;
  /** Number of concrete edge mutations the trusted host will request. */
  operationCount: number;
  /** A finite host-declared ceiling for this specific action. */
  maxOperations: number;
  capability: TopologyActionCapability;
  receipt?: TopologyOperationReceipt | null;
}

export interface ConnectionPickerProps {
  endpoints: readonly TopologyEndpoint[];
  edges: readonly (TopologyEdgeRef | TopologyCanonicalEdge)[];
  management: TopologyManagementState;
  sourceId?: string | null;
  defaultSourceId?: string | null;
  onSourceChange?: (sourceId: string | null) => void;
  onRequestMutation?: (intent: TopologyMutationIntent) => void | Promise<void>;
  /** Explicitly ask the host to prepare/query a pair that has no live affordance yet. */
  onRequestPairInspection?: (edge: TopologyEdgeRef) => void | Promise<void>;
  /**
   * Direct interaction is an opt-in host mode for smaller, user-facing
   * rosters. The host resolves pair state automatically; the picker never
   * exposes a preparatory Check action and keeps repair semantics behind the
   * simpler Connect / Disconnect controls.
   */
  interactionMode?: "explicit" | "direct";
  /** Stable pair keys currently being resolved by a direct-mode host. */
  resolvingPairKeys?: ReadonlySet<string>;
  onRetryOperation?: (receipt: TopologyOperationReceipt) => void | Promise<void>;
  bulkActions?: readonly TopologyBoundedAction[];
  onRequestBulkAction?: (action: TopologyBoundedAction) => void | Promise<void>;
  allowSourceChange?: boolean;
  visibleLimit?: number;
  title?: React.ReactNode;
  description?: React.ReactNode;
}

interface ActionState {
  action: TopologyMutationKind;
  label: string;
  ariaLabel: string;
  disabled: boolean;
  reason: string | null;
  approvalRequired: boolean;
  retryReceipt: TopologyOperationReceipt | null;
}

function endpointSearchText(endpoint: TopologyEndpoint): string {
  const presentation = endpoint.presentation;
  return [
    presentation.label,
    endpoint.ref.id,
    presentation.caption,
    presentation.section,
    presentation.scopeLabel,
    ...(presentation.searchTerms || []),
    ...Object.values(endpoint.tags || {}),
  ].filter(Boolean).join(" ").toLocaleLowerCase();
}

function endpointSection(endpoint: TopologyEndpoint): string {
  return endpoint.presentation.section?.trim()
    || endpoint.presentation.scopeLabel?.trim()
    || "Endpoints";
}

function endpointTone(endpoint: TopologyEndpoint): React.CSSProperties {
  return endpoint.presentation.accent
    ? { "--topo-node-accent": endpoint.presentation.accent } as React.CSSProperties
    : {};
}

function pairState(
  management: TopologyManagementState,
  edge: TopologyEdgeRef,
  connectedEdgeKeys: ReadonlySet<string>,
): TopologyConnectionState {
  return topologyAffordanceFor(management, edge)?.state
    || (connectedEdgeKeys.has(topologyEdgeKey(edge)) ? "connected" : "disconnected");
}

function preferredAction(
  state: TopologyConnectionState,
  receipt: TopologyOperationReceipt | null,
  hostPreference?: TopologyMutationKind | null,
): TopologyMutationKind {
  if (receipt?.retryable && ["failed", "partial", "conflict", "denied"].includes(receipt.status)) {
    return receipt.action;
  }
  if (hostPreference) return hostPreference;
  if (state === "degraded" || state === "conflict") return "reconnect";
  return state === "connected" ? "disconnect" : "connect";
}

function capabilityReason(
  management: TopologyManagementState,
  action: TopologyMutationKind,
  edge: TopologyEdgeRef,
): string | null {
  if (management.policy.mode === "disabled") {
    return management.policy.reason || "Connection management is disabled.";
  }
  if (management.policy.mode === "read_only") {
    return management.policy.reason || "Connection management is read-only.";
  }
  const globalCapability = management.policy.capabilities[action];
  if (!topologyCapabilityAllowsRequest(globalCapability)) {
    return globalCapability.reason || `${action} is not permitted.`;
  }
  const pairCapability = topologyAffordanceFor(management, edge)?.actions[action];
  if (!pairCapability) return `${action} is not available for this endpoint pair.`;
  if (!topologyCapabilityAllowsRequest(pairCapability)) {
    return pairCapability.reason || `${action} is not permitted for this endpoint pair.`;
  }
  return null;
}

function operationStatus(receipt: TopologyOperationReceipt | null): string | null {
  if (!receipt) return null;
  switch (receipt.status) {
    case "pending_approval": return "Pending approval";
    case "queued": return "Queued";
    case "running": return `${receipt.action === "disconnect" ? "Disconnecting" : "Connecting"}…`;
    case "partial": return "Partial";
    case "conflict": return "Conflict";
    case "denied": return "Denied";
    case "failed": return "Failed";
    case "cancelled": return "Cancelled";
    case "succeeded": return null;
  }
}

function stateStatus(state: TopologyConnectionState): string {
  switch (state) {
    case "connected": return "Connected";
    case "disconnected": return "Not connected";
    case "degraded": return "Degraded";
    case "conflict": return "Conflict";
  }
}

function directConnectionDetail(
  state: TopologyConnectionState,
  receipt: TopologyOperationReceipt | null,
  actionState: ActionState | null,
): string | null {
  if (receipt) {
    switch (receipt.status) {
      case "pending_approval": return "Waiting for approval to change this connection.";
      case "queued":
      case "running": return "Connection change in progress.";
      case "partial":
      case "failed":
      case "cancelled": return "The last connection change did not complete.";
      case "conflict": return "The connection changed while this action was running.";
      case "denied": return "You do not have permission to change this connection.";
      case "succeeded": break;
    }
  }
  if (actionState?.reason) {
    return actionState.approvalRequired
      ? "Approval is required to change this connection."
      : "This connection cannot be changed right now.";
  }
  if (state === "degraded" || state === "conflict") {
    return "This connection needs repair.";
  }
  return null;
}

function buildActionState(
  management: TopologyManagementState,
  source: TopologyEndpoint,
  target: TopologyEndpoint,
  edge: TopologyEdgeRef,
  state: TopologyConnectionState,
  receipt: TopologyOperationReceipt | null,
  hasMutationHandler: boolean,
  hasRetryHandler: boolean,
  binaryLabels: boolean,
): ActionState {
  const affordance = topologyAffordanceFor(management, edge);
  const action = preferredAction(state, receipt, affordance?.preferredAction);
  const pairCapability = affordance?.actions[action];
  const globalCapability = management.policy.capabilities[action];
  const approvalRequired = pairCapability?.state === "approval_required"
    || globalCapability.state === "approval_required";
  const retryReceipt = receipt?.retryable
    && ["failed", "partial", "conflict", "denied"].includes(receipt.status)
    ? receipt
    : null;
  const pending = topologyOperationIsPending(receipt);
  let reason = capabilityReason(management, action, edge);
  if (!reason && retryReceipt && !hasRetryHandler) reason = "The host did not provide a retry handler.";
  if (!reason && !retryReceipt && !hasMutationHandler) reason = "The host did not provide a mutation handler.";
  if (pending) reason = receipt?.message || operationStatus(receipt) || "Operation pending.";

  let label = action === "disconnect" ? "Disconnect" : action === "reconnect" ? "Reconnect" : "Connect";
  if (retryReceipt?.retryMode === "resolve_ambiguous") label = "Resolve";
  else if (retryReceipt?.retryMode === "revision_rebase") label = "Rebase";
  else if (retryReceipt) label = "Retry";
  else if (pending) label = receipt?.status === "pending_approval" ? "Awaiting approval" : "Pending";
  else if (approvalRequired) label = "Request approval";
  else if (reason) label = reason.toLocaleLowerCase().includes("denied") ? "Denied" : label;

  if (binaryLabels) {
    label = action === "disconnect" ? "Disconnect" : "Connect";
  }

  return {
    action,
    label,
    ariaLabel: `${label} ${target.presentation.label} ${action === "disconnect" ? "from" : "to"} ${source.presentation.label}`,
    disabled: Boolean(reason),
    reason,
    approvalRequired,
    retryReceipt,
  };
}

function boundedActionReason(
  management: TopologyManagementState,
  action: TopologyBoundedAction,
  hasHandler: boolean,
): string | null {
  if (management.policy.mode !== "editable") {
    return management.policy.reason || "Connection management is not editable.";
  }
  const globalBulk = management.policy.capabilities.bulk;
  if (!topologyCapabilityAllowsRequest(globalBulk)) {
    return globalBulk?.reason || "Bulk topology changes are not enabled.";
  }
  if (!topologyCapabilityAllowsRequest(action.capability)) {
    return action.capability.reason || "This bulk action is not permitted.";
  }
  if (!Number.isFinite(action.maxOperations) || action.maxOperations < 1) {
    return "This bulk action has no finite operation limit.";
  }
  const policyLimit = management.policy.maxBatchSize;
  if (!Number.isFinite(policyLimit) || Number(policyLimit) < 1) {
    return "The topology policy has no finite batch limit.";
  }
  if (action.operationCount < 1) return "This bulk action has no operations.";
  if (action.operationCount > action.maxOperations || action.operationCount > Number(policyLimit)) {
    return `This action exceeds the ${Math.min(action.maxOperations, Number(policyLimit))}-operation limit.`;
  }
  if (topologyOperationIsPending(action.receipt)) {
    return action.receipt?.message || operationStatus(action.receipt || null) || "Operation pending.";
  }
  if (!hasHandler) return "The host did not provide a bulk-action handler.";
  return null;
}

export function ConnectionPicker({
  endpoints,
  edges,
  management,
  sourceId: controlledSourceId,
  defaultSourceId = null,
  onSourceChange,
  onRequestMutation,
  onRequestPairInspection,
  interactionMode = "explicit",
  resolvingPairKeys = new Set<string>(),
  onRetryOperation,
  bulkActions = [],
  onRequestBulkAction,
  allowSourceChange = true,
  visibleLimit = DEFAULT_VISIBLE_LIMIT,
  title = "Connections",
  description,
}: ConnectionPickerProps): React.JSX.Element {
  const [uncontrolledSourceId, setUncontrolledSourceId] = React.useState<string | null>(defaultSourceId);
  const [query, setQuery] = React.useState("");
  const deferredQuery = React.useDeferredValue(query);
  const sourceId = controlledSourceId === undefined ? uncontrolledSourceId : controlledSourceId;
  const endpointById = React.useMemo(
    () => new Map(endpoints.map((endpoint) => [endpoint.ref.id, endpoint])),
    [endpoints],
  );
  const source = sourceId ? endpointById.get(sourceId) || null : null;
  const connectedEdgeKeys = React.useMemo(
    () => new Set(edges.map((edge) => topologyEdgeKey(edge))),
    [edges],
  );

  const selectSource = React.useCallback((nextSourceId: string | null) => {
    if (controlledSourceId === undefined) setUncontrolledSourceId(nextSourceId);
    onSourceChange?.(nextSourceId);
  }, [controlledSourceId, onSourceChange]);

  const normalizedQuery = deferredQuery.trim().toLocaleLowerCase();
  const candidates = React.useMemo(() => endpoints.filter((endpoint) => (
    endpoint.ref.id !== source?.ref.id
    && (!normalizedQuery || endpointSearchText(endpoint).includes(normalizedQuery))
  )), [endpoints, normalizedQuery, source?.ref.id]);
  const cappedCandidates = candidates.slice(0, Math.max(1, visibleLimit));
  const hiddenCount = Math.max(0, candidates.length - cappedCandidates.length);
  const sections = new Map<string, TopologyEndpoint[]>();
  for (const endpoint of cappedCandidates) {
    const section = endpointSection(endpoint);
    const entries = sections.get(section) || [];
    entries.push(endpoint);
    sections.set(section, entries);
  }

  const health = management.health || "ready";
  const featureDisabled = management.policy.mode === "disabled";
  const resolvedDescription = description ?? (interactionMode === "direct"
    ? "Choose an endpoint to see or change its peer connections."
    : "Choose an endpoint, then inspect or change its peer connections.");

  return (
    <div className="topo-edit" data-testid="connection-picker" data-management-mode={management.policy.mode}>
      <div className="topo-edit__column">
        <div className="topo-edit__intro">
          <strong>{title}</strong>
          <span>{resolvedDescription}</span>
        </div>

        {featureDisabled ? (
          <div className="topo-edit__notice is-disabled" role="status">
            {management.policy.reason || "Connection management is disabled for this runtime."}
          </div>
        ) : null}
        {health !== "ready" ? (
          <div className={`topo-edit__notice is-${health}`} role="status">
            <strong>{health === "conflict" ? "Topology conflict" : "Topology degraded"}</strong>
            <span>{management.message || "The displayed topology may need reconciliation."}</span>
          </div>
        ) : null}

        {source ? (
          <section className="topo-edit__focus" data-testid={`connection-picker-source:${source.ref.id}`}>
            <div className="topo-edit__focus-identity" style={endpointTone(source)}>
              <span className={`topo-edit__dot${source.presentation.crossScope ? " is-cross-scope" : ""}`} />
              <span>
                <strong>{source.presentation.label}</strong>
                <small>{source.presentation.caption || source.ref.id}</small>
              </span>
            </div>
            {allowSourceChange ? (
              <button className="topo-edit__quiet-btn" type="button" onClick={() => selectSource(null)}>
                Change
              </button>
            ) : null}
            {bulkActions.length > 0 ? (
              <div className="topo-edit__bulk-actions" data-testid="connection-picker-bulk-actions">
                {bulkActions.map((action) => {
                  const reason = boundedActionReason(management, action, Boolean(onRequestBulkAction));
                  return (
                    <button
                      key={action.id}
                      type="button"
                      disabled={Boolean(reason)}
                      title={reason || action.description || undefined}
                      onClick={() => void onRequestBulkAction?.(action)}
                    >
                      {action.label} <span>{action.operationCount}</span>
                    </button>
                  );
                })}
              </div>
            ) : null}
          </section>
        ) : (
          <div className="topo-edit__notice" role="status">
            {interactionMode === "direct"
              ? "Pick an endpoint below to see its connections."
              : "Pick an endpoint below to inspect its connections."}
          </div>
        )}

        <label className="topo-edit__search">
          <span aria-hidden="true">⌕</span>
          <input
            aria-label="Search endpoints"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder="Search endpoints, scopes, and labels"
          />
          {query ? (
            <button type="button" aria-label="Clear connection search" onClick={() => setQuery("")}>×</button>
          ) : null}
        </label>

        <div className="topo-edit__roster">
          {cappedCandidates.length === 0 ? (
            <div className="topo-edit__empty">No endpoints match “{query.trim()}”</div>
          ) : Array.from(sections, ([section, sectionEndpoints]) => (
            <section className="topo-edit__section" key={section}>
              <h3>{section}</h3>
              {sectionEndpoints.map((endpoint) => {
                const isSource = endpoint.ref.id === source?.ref.id;
                const edge = source ? { from: source.ref.id, to: endpoint.ref.id } : null;
                const state = edge ? pairState(management, edge, connectedEdgeKeys) : "disconnected";
                const receipt = edge ? topologyOperationFor(management, edge) : null;
                const actionState = source && edge
                  ? buildActionState(
                      management,
                      source,
                      endpoint,
                      edge,
                      state,
                      receipt,
                      Boolean(onRequestMutation),
                      Boolean(onRetryOperation),
                      interactionMode === "direct",
                    )
                  : null;
                const affordance = edge ? topologyAffordanceFor(management, edge) : null;
                const inspectionAvailable = Boolean(
                  interactionMode === "explicit"
                  && source
                  && edge
                  && !affordance
                  && onRequestPairInspection,
                );
                const unresolvedDirectPair = Boolean(
                  interactionMode === "direct" && source && edge && !affordance,
                );
                const pairIsResolving = Boolean(
                  edge && resolvingPairKeys.has(topologyEdgeKey(edge)),
                );
                const status = inspectionAvailable
                  ? "Not inspected"
                  : unresolvedDirectPair
                    ? pairIsResolving ? "Loading…" : "Unavailable"
                    : operationStatus(receipt) || stateStatus(state);
                const rawDetail = unresolvedDirectPair
                  ? pairIsResolving
                    ? "Loading current connection status from Meta."
                    : "Current connection status is unavailable."
                  : receipt?.message
                    || affordance?.message
                    || actionState?.reason;
                const detail = interactionMode === "direct"
                  ? unresolvedDirectPair
                    ? rawDetail
                    : directConnectionDetail(state, receipt, actionState)
                  : rawDetail;
                return (
                  <div
                    className={`topo-edit__row is-${state}${endpoint.presentation.crossScope ? " is-cross-scope" : ""}`}
                    data-testid={`connection-picker-row:${endpoint.ref.id}`}
                    data-connection-state={state}
                    key={endpoint.ref.id}
                    style={endpointTone(endpoint)}
                  >
                    <button
                      className="topo-edit__identity"
                      type="button"
                      onClick={() => selectSource(endpoint.ref.id)}
                      disabled={!allowSourceChange && Boolean(source)}
                      aria-pressed={isSource}
                    >
                      <span className={`topo-edit__dot${endpoint.presentation.crossScope ? " is-cross-scope" : ""}`} />
                      <span className="topo-edit__identity-copy">
                        <strong>{endpoint.presentation.label}</strong>
                        <small>{endpoint.presentation.caption || endpoint.ref.id}</small>
                        {detail ? <small className="topo-edit__reason">{detail}</small> : null}
                      </span>
                    </button>
                    {source && edge && inspectionAvailable ? (
                      <div className="topo-edit__action">
                        <span className="topo-edit__status">{status}</span>
                        <button
                          aria-label={`Check ${endpoint.presentation.label} connection availability with ${source.presentation.label}`}
                          className="topo-edit__toggle"
                          type="button"
                          onClick={() => void onRequestPairInspection?.(edge)}
                        >
                          Check
                        </button>
                      </div>
                    ) : source && edge && unresolvedDirectPair ? (
                      <div className="topo-edit__action">
                        <span className={`topo-edit__status${pairIsResolving ? " is-running" : " is-unavailable"}`}>
                          {status}
                        </span>
                      </div>
                    ) : source && actionState && !unresolvedDirectPair ? (
                      <div className="topo-edit__action">
                        <span className={`topo-edit__status is-${receipt?.status || state}`}>{status}</span>
                        <button
                          className={`topo-edit__toggle is-${actionState.action}${actionState.approvalRequired ? " requires-approval" : ""}`}
                          type="button"
                          disabled={actionState.disabled}
                          aria-label={actionState.ariaLabel}
                          title={interactionMode === "direct"
                            ? directConnectionDetail(state, receipt, actionState) || undefined
                            : actionState.reason || undefined}
                          onClick={() => {
                            if (actionState.retryReceipt) {
                              void onRetryOperation?.(actionState.retryReceipt);
                              return;
                            }
                            const intent = topologyMutationIntent(
                              management,
                              actionState.action,
                              edge!,
                              "picker",
                            );
                            if (intent) void onRequestMutation?.(intent);
                          }}
                        >
                          {actionState.label}
                        </button>
                      </div>
                    ) : null}
                  </div>
                );
              })}
            </section>
          ))}
          {hiddenCount > 0 ? (
            <div className="topo-edit__overflow" role="status">
              {hiddenCount} more endpoints — search to narrow the roster.
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
