import React from "react";
import type {
  WorkGraphWireBinding,
  WorkGraphWireEdge,
  WorkGraphWireEvent,
  WorkGraphWireItem,
} from "../types";
import { WORKGRAPH_GRAPH_NODE_CAP, workGraphItemOwnerLabel } from "../lib/workgraph-layout";
import { formatWorkGraphTimestamp } from "../lib/workgraph-time";
import { PanelStatusRow } from "./PanelStatusRow";
import { WorkGraphGraphView } from "./WorkGraphGraphView";

export type WorkGraphViewMode = "tree" | "graph";

// Module-level so the chosen view survives dock remounts (same trick as
// the conversation card's expansion registries - dock panels are torn
// down and rebuilt on focus changes).
let workGraphViewModeMemory: WorkGraphViewMode = "tree";

/// Snapshot-backed panel state assembled by ConsoleApp. `denied` is a -32030
/// access outcome (rendered as "no grant", never as an empty store);
/// `unavailable` means no WorkGraph service is configured on the runtime.
export interface WorkGraphPanelData {
  items: WorkGraphWireItem[];
  edges: WorkGraphWireEdge[];
  attention: WorkGraphWireBinding[];
  events: WorkGraphWireEvent[];
  capturedAt: string | null;
  unavailable: boolean;
  denied: boolean;
  error: string | null;
}

// Two CAS classes on the action payloads: goal actions (confirm /
// request-close) carry the goal WORK ITEM's revision; attention actions
// (pause / resume / reassign) carry the binding's machine revision.
interface WorkGraphPanelProps {
  data: WorkGraphPanelData;
  canManage: boolean;
  onRefresh: () => void;
  // An absent revision means the snapshot never carried one: handlers must
  // resolve the live revision before mutating (never substitute 0).
  onClaim?: (input: { itemId: string; revision?: number }) => void;
  onClose?: (input: { itemId: string; revision?: number }) => void;
  onGoalConfirm?: (input: { bindingId: string; revision?: number }) => void;
  onGoalRequestClose?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionPause?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionResume?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionReassign?: (input: { bindingId: string; revision?: number; identity: string }) => void;
}

export interface WorkGraphPanelTreeRow {
  item: WorkGraphWireItem;
  itemId: string;
  depth: number;
}

/// DFS order over parent edges (WorkEdge kind "parent" runs child→parent).
/// Roots first (by created_at, then id), children indented under parents;
/// items with a parent edge to an unknown item render as roots.
export function buildWorkGraphPanelTree(
  items: WorkGraphWireItem[],
  edges: WorkGraphWireEdge[],
): WorkGraphPanelTreeRow[] {
  const byId = new Map<string, WorkGraphWireItem>();
  for (const item of items) {
    if (typeof item.id === "string" && item.id) byId.set(item.id, item);
  }
  const parentOf = new Map<string, string>();
  for (const edge of edges) {
    if (edge.kind !== "parent") continue;
    if (
      typeof edge.from_id === "string" && edge.from_id
      && typeof edge.to_id === "string" && edge.to_id
      && edge.from_id !== edge.to_id
      // Multiple parents per child are allowed upstream; placement is
      // first-parent-wins, matching the inline card fold.
      && !parentOf.has(edge.from_id)
    ) {
      parentOf.set(edge.from_id, edge.to_id);
    }
  }
  const childrenOf = new Map<string, string[]>();
  const roots: string[] = [];
  for (const id of byId.keys()) {
    const parent = parentOf.get(id);
    if (parent && byId.has(parent)) {
      const children = childrenOf.get(parent) || [];
      children.push(id);
      childrenOf.set(parent, children);
    } else {
      roots.push(id);
    }
  }
  const sortIds = (ids: string[]): string[] => (
    [...ids].sort((left, right) => {
      const leftKey = byId.get(left)?.created_at || "";
      const rightKey = byId.get(right)?.created_at || "";
      if (leftKey !== rightKey) return leftKey < rightKey ? -1 : 1;
      return left < right ? -1 : left === right ? 0 : 1;
    })
  );
  const rows: WorkGraphPanelTreeRow[] = [];
  const visited = new Set<string>();
  const visit = (id: string, depth: number) => {
    if (visited.has(id)) return;
    visited.add(id);
    const item = byId.get(id);
    if (!item) return;
    rows.push({ item, itemId: id, depth });
    for (const child of sortIds(childrenOf.get(id) || [])) {
      visit(child, depth + 1);
    }
  };
  for (const root of sortIds(roots)) visit(root, 0);
  return rows;
}

export function workGraphBindingStatusLabel(binding: WorkGraphWireBinding): string {
  const state = binding.status?.state || "active";
  if (state === "paused") {
    const until = formatWorkGraphTimestamp(binding.status?.until, { date: true });
    return until ? `paused until ${until}` : "paused";
  }
  return state;
}

export function workGraphBindingTargetLabel(binding: WorkGraphWireBinding): string {
  const target = binding.target;
  if (!target) return "";
  if (typeof target.session_id === "string" && target.session_id) return target.session_id;
  const ownerKey = target.owner_key;
  if (ownerKey) {
    return [ownerKey.kind, ownerKey.id].filter(Boolean).join(":");
  }
  return "";
}

export function workGraphEventLine(event: WorkGraphWireEvent): string {
  const kind = typeof event.kind === "string" ? event.kind.replace(/_/g, " ") : "event";
  const at = formatWorkGraphTimestamp(event.at, { date: true });
  const item = typeof event.item_id === "string" && event.item_id ? event.item_id : "";
  return [at, kind, item].filter(Boolean).join(" · ");
}

/// Kept under the panel's historical export name; the single definition of
/// the precedence chain lives in workgraph-layout.ts so the tree rows, the
/// graph nodes, and the graph selection footer cannot drift apart.
export function workGraphOwnerLabelOf(item: WorkGraphWireItem): string {
  return workGraphItemOwnerLabel(item);
}

/// Latest observed revision of the binding's bound goal work item. Goal
/// confirm / request-close CAS against this (the server checks the ITEM's
/// revision), never against the binding's machine revision.
export function workGraphGoalRevisionOf(
  binding: WorkGraphWireBinding,
  items: WorkGraphWireItem[],
): number | undefined {
  const itemId = binding.work_ref?.item_id;
  if (!itemId) return undefined;
  const item = items.find((candidate) => candidate.id === itemId);
  return typeof item?.revision === "number" ? item.revision : undefined;
}

/// `meta/workgraph/events` params for the panel's recent-events tail.
/// Upstream returns events ASCENDING truncated to `limit`, so a bare
/// `{limit}` query pins the OLDEST window forever once the ledger outgrows
/// it. Page from the snapshot's `event_high_water_mark` instead; a null/
/// absent mark (fresh store or older runtime) falls back to the bare query.
export function workGraphEventsParams(
  eventHighWaterMark: number | null | undefined,
  limit: number,
): { limit: number; after_seq?: number } {
  if (typeof eventHighWaterMark === "number" && Number.isFinite(eventHighWaterMark)) {
    return { limit, after_seq: Math.max(0, Math.floor(eventHighWaterMark) - limit) };
  }
  return { limit };
}

/// The panel renders the tail newest-first; upstream delivers ascending.
export function workGraphEventsNewestFirst(events: WorkGraphWireEvent[]): WorkGraphWireEvent[] {
  return [...events].reverse();
}

/// Sequences overlapping refreshes: `begin()` returns an `isCurrent` probe
/// that goes false the moment a newer refresh begins, so a stale resolution
/// never overwrites a fresher snapshot.
export function createWorkGraphRefreshSequencer(): { begin: () => () => boolean } {
  let latest = 0;
  return {
    begin() {
      latest += 1;
      const token = latest;
      return () => token === latest;
    },
  };
}

export const __workGraphPanelTest = {
  buildWorkGraphPanelTree,
  workGraphBindingStatusLabel,
  workGraphBindingTargetLabel,
  workGraphEventLine,
  workGraphOwnerLabelOf,
  workGraphGoalRevisionOf,
  workGraphEventsParams,
  workGraphEventsNewestFirst,
  createWorkGraphRefreshSequencer,
};

function statusDotClass(status: string | undefined): string {
  return `workgraph__dot is-${status || "open"}`;
}

function ItemRow({
  row,
  canManage,
  onClaim,
  onClose,
}: {
  row: WorkGraphPanelTreeRow;
  canManage: boolean;
  onClaim?: WorkGraphPanelProps["onClaim"];
  onClose?: WorkGraphPanelProps["onClose"];
}) {
  const { item, itemId, depth } = row;
  const status = item.status || "open";
  const revision = typeof item.revision === "number" ? item.revision : undefined;
  const terminal = status === "completed" || status === "cancelled" || status === "failed";
  const owner = workGraphOwnerLabelOf(item);
  return (
    <div
      className="workgraph__item"
      data-testid={`workgraph-panel-item:${itemId}`}
      style={{ paddingLeft: `${depth * 16}px` }}
    >
      <span className={statusDotClass(status)} aria-hidden="true" />
      <span className="workgraph__item-title" title={item.description || item.title}>
        {item.title || itemId}
      </span>
      {item.priority && item.priority !== "medium" ? (
        <span className={`workgraph__chip is-priority-${item.priority}`}>{item.priority}</span>
      ) : null}
      {owner ? <span className="workgraph__chip">{owner}</span> : null}
      <span className="workgraph__item-status">{status.replace(/_/g, " ")}</span>
      {canManage && onClaim && status === "open" && !owner ? (
        <button
          type="button"
          className="workgraph__action"
          data-testid={`workgraph-panel-action:${itemId}:claim`}
          onClick={() => onClaim({ itemId, revision })}
        >
          Claim
        </button>
      ) : null}
      {canManage && onClose && !terminal && status !== "blocked" ? (
        <button
          type="button"
          className="workgraph__action"
          data-testid={`workgraph-panel-action:${itemId}:close`}
          onClick={() => onClose({ itemId, revision })}
        >
          Done
        </button>
      ) : null}
    </div>
  );
}

function AttentionRow({
  binding,
  goalRevision,
  canManage,
  onGoalConfirm,
  onGoalRequestClose,
  onAttentionPause,
  onAttentionResume,
  onAttentionReassign,
}: {
  binding: WorkGraphWireBinding;
  // Bound goal work item's revision — the CAS token for confirm/request-close.
  goalRevision?: number;
  canManage: boolean;
} & Pick<WorkGraphPanelProps,
  "onGoalConfirm" | "onGoalRequestClose" | "onAttentionPause" | "onAttentionResume" | "onAttentionReassign"
>) {
  const [reassignOpen, setReassignOpen] = React.useState(false);
  const [reassignIdentity, setReassignIdentity] = React.useState("");
  const bindingId = binding.binding_id || "";
  const revision = binding.machine_state?.revision;
  const statusLabel = workGraphBindingStatusLabel(binding);
  const targetLabel = workGraphBindingTargetLabel(binding);
  const isActive = statusLabel === "active";
  const isPaused = statusLabel.startsWith("paused");
  const live = isActive || isPaused;
  // Reassign authority is machine-derived from the binding mode upstream:
  // only coordinate-mode bindings can reassign, so others get no affordance.
  const canReassign = live && binding.mode === "coordinate";
  const bindingInput = { bindingId, revision };
  const goalInput = { bindingId, revision: goalRevision };
  if (!bindingId) return null;
  return (
    <div className="workgraph__binding" data-testid={`workgraph-panel-binding:${bindingId}`}>
      <div className="workgraph__binding-line">
        <span className={`workgraph__mode is-${binding.mode || "pursue"}`}>{binding.mode || "pursue"}</span>
        <span className="workgraph__binding-status">{statusLabel}</span>
        {targetLabel ? <span className="workgraph__binding-target">{targetLabel}</span> : null}
        {binding.work_ref?.item_id ? (
          <span className="workgraph__chip" title="Bound work item">{binding.work_ref.item_id}</span>
        ) : null}
        <span className="workgraph__spacer" />
        {canManage && onAttentionPause && isActive ? (
          <button type="button" className="workgraph__action" onClick={() => onAttentionPause(bindingInput)}>Pause</button>
        ) : null}
        {canManage && onAttentionResume && isPaused ? (
          <button type="button" className="workgraph__action" onClick={() => onAttentionResume(bindingInput)}>Resume</button>
        ) : null}
        {canManage && onGoalConfirm && live ? (
          <button type="button" className="workgraph__action" onClick={() => onGoalConfirm(goalInput)}>Confirm</button>
        ) : null}
        {canManage && onGoalRequestClose && live ? (
          <button type="button" className="workgraph__action" onClick={() => onGoalRequestClose(goalInput)}>Request close</button>
        ) : null}
        {canManage && onAttentionReassign && canReassign ? (
          <button
            type="button"
            className="workgraph__action"
            aria-expanded={reassignOpen}
            onClick={() => setReassignOpen((value) => !value)}
          >
            Reassign
          </button>
        ) : null}
      </div>
      {reassignOpen && canManage && onAttentionReassign && canReassign ? (
        <div className="workgraph__reassign">
          <input
            placeholder="Target agent identity…"
            value={reassignIdentity}
            onChange={(event) => setReassignIdentity(event.target.value)}
            data-testid={`workgraph-panel-reassign-input:${bindingId}`}
          />
          <button
            type="button"
            className="workgraph__action"
            disabled={!reassignIdentity.trim()}
            data-testid={`workgraph-panel-reassign-submit:${bindingId}`}
            onClick={() => {
              onAttentionReassign({ ...bindingInput, identity: reassignIdentity.trim() });
              setReassignOpen(false);
              setReassignIdentity("");
            }}
          >
            Reassign to identity
          </button>
        </div>
      ) : null}
    </div>
  );
}

export function WorkGraphPanel({
  data,
  canManage,
  onRefresh,
  onClaim,
  onClose,
  onGoalConfirm,
  onGoalRequestClose,
  onAttentionPause,
  onAttentionResume,
  onAttentionReassign,
}: WorkGraphPanelProps): React.JSX.Element {
  const capturedAt = formatWorkGraphTimestamp(data.capturedAt, { date: true, seconds: true });
  const rows = React.useMemo(
    () => buildWorkGraphPanelTree(data.items, data.edges),
    [data.items, data.edges],
  );
  // The snapshot includes terminal rows (the panel is the operator
  // inspection surface), so its size tracks the store's full history. The
  // tree gets the same rendered-row cap and overflow honesty as the graph.
  const visibleRows = rows.length > WORKGRAPH_GRAPH_NODE_CAP
    ? rows.slice(0, WORKGRAPH_GRAPH_NODE_CAP)
    : rows;
  const treeOverflowCount = rows.length - visibleRows.length;
  const [viewMode, setViewModeState] = React.useState<WorkGraphViewMode>(workGraphViewModeMemory);
  const [selectedGraphItem, setSelectedGraphItem] = React.useState<string | null>(null);
  const setViewMode = (mode: WorkGraphViewMode) => {
    workGraphViewModeMemory = mode;
    setViewModeState(mode);
  };

  if (data.unavailable) {
    return (
      <div className="console-panel workgraph" data-testid="workgraph-panel">
        <div className="workgraph__empty">WorkGraph is not configured on this runtime.</div>
      </div>
    );
  }

  return (
    <div className="console-panel workgraph" data-testid="workgraph-panel">
      <div className="workgraph__head">
        <h3>WorkGraph</h3>
        {capturedAt ? (
          <span className="workgraph__captured">as of {capturedAt}</span>
        ) : null}
        <span className="workgraph__spacer" />
        <div className="workgraph__view-toggle" role="group" aria-label="WorkGraph view mode">
          <button
            type="button"
            className={`workgraph__action${viewMode === "tree" ? " is-active" : ""}`}
            aria-pressed={viewMode === "tree"}
            data-testid="workgraph-view-toggle:tree"
            onClick={() => setViewMode("tree")}
          >
            Tree
          </button>
          <button
            type="button"
            className={`workgraph__action${viewMode === "graph" ? " is-active" : ""}`}
            aria-pressed={viewMode === "graph"}
            data-testid="workgraph-view-toggle:graph"
            onClick={() => setViewMode("graph")}
          >
            Graph
          </button>
        </div>
        <button
          type="button"
          className="workgraph__action"
          onClick={onRefresh}
          data-testid="workgraph-panel-refresh"
        >
          Refresh
        </button>
      </div>
      {/* A failed refresh keeps the last items on screen. Its error sits in a
          reserved row below the header, so it never moves the header controls
          or the items under a click in progress. */}
      <PanelStatusRow error={data.error} />
      {data.denied ? (
        <div className="workgraph__empty" data-testid="workgraph-panel-denied">
          You do not have a grant to view WorkGraph state.
        </div>
      ) : (
        <>
          <div className="workgraph__section">
            <div className="workgraph__sec-label">Work items</div>
            {viewMode === "graph" ? (
              <WorkGraphGraphView
                items={data.items}
                edges={data.edges}
                attention={data.attention}
                selectedId={selectedGraphItem ?? undefined}
                onSelect={setSelectedGraphItem}
              />
            ) : rows.length === 0 ? (
              <div className="workgraph__empty">No work items.</div>
            ) : (
              <>
                {visibleRows.map((row) => (
                  <ItemRow
                    key={row.itemId}
                    row={row}
                    canManage={canManage}
                    onClaim={onClaim}
                    onClose={onClose}
                  />
                ))}
                {treeOverflowCount > 0 ? (
                  <div className="workgraph__empty" data-testid="workgraph-panel-overflow">
                    +{treeOverflowCount} more items not shown
                  </div>
                ) : null}
              </>
            )}
          </div>
          <div className="workgraph__section">
            <div className="workgraph__sec-label">Attention</div>
            {data.attention.length === 0 ? (
              <div className="workgraph__empty">No attention bindings.</div>
            ) : (
              data.attention.map((binding, index) => (
                <AttentionRow
                  key={binding.binding_id || `binding-${index}`}
                  binding={binding}
                  goalRevision={workGraphGoalRevisionOf(binding, data.items)}
                  canManage={canManage}
                  onGoalConfirm={onGoalConfirm}
                  onGoalRequestClose={onGoalRequestClose}
                  onAttentionPause={onAttentionPause}
                  onAttentionResume={onAttentionResume}
                  onAttentionReassign={onAttentionReassign}
                />
              ))
            )}
          </div>
          <div className="workgraph__section">
            <div className="workgraph__sec-label">Recent events</div>
            {data.events.length === 0 ? (
              <div className="workgraph__empty">No events.</div>
            ) : (
              <div className="workgraph__events">
                {data.events.map((event, index) => (
                  <div className="workgraph__event" key={`${event.seq ?? index}`}>
                    {workGraphEventLine(event)}
                  </div>
                ))}
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
