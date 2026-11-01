import type {
  WorkGraphWireBinding,
  WorkGraphWireEdge,
  WorkGraphWireEvent,
  WorkGraphWireItem,
} from "../types";
import { agentDisplayName, agentInitial, type AgentDirectory } from "../../../packages/console-components/src/panel-kit/agents";
import {
  formatClockTime,
  formatShortDate,
  humanizeEnum,
  joinMeta,
  plural,
  toEpochMs,
} from "../../../packages/console-components/src/panel-kit/format";
import { statusTone } from "../../../packages/console-components/src/panel-kit/tone";
import type { VizEdge, VizFrame, VizNode } from "../../../packages/console-components/src/viz/graph/types";
import { formatWorkGraphTimestamp } from "./workgraph-time";

/// The WorkGraph model (console v2 spec B2): one snapshot read once into
/// goal frames, item cards, the links between them, readiness and the
/// critical path. Pure functions with no React, so the panel, the graph,
/// the outline and the tests all derive the same facts. Geometry belongs
/// to the graph kit's layered layout; this file only says what to draw.

/// Snapshot items requested per refresh (the server's maximum). The
/// default of 100 silently kept the 100 least recently updated items.
export const WORKGRAPH_SNAPSHOT_LIMIT = 1000;
/// Activity events read per refresh.
export const WORKGRAPH_EVENTS_LIMIT = 200;
/// From this many items on, the panel opens in the Outline and the Graph
/// draws only the work that decides the finish (level of detail).
export const WORKGRAPH_LARGE_ITEM_COUNT = 300;
/// Most nodes the Graph draws (the kit's SVG budget); the rest is
/// reported as "+N more items not drawn", never silently cut.
export const WORKGRAPH_GRAPH_NODE_CAP = 300;
/// Rows the Outline renders before "+N more items not shown".
export const WORKGRAPH_OUTLINE_ROW_CAP = 200;
/// Edge words stay on every labelled edge up to this many, then on hover.
export const WORKGRAPH_EDGE_LABEL_LIMIT = 40;
/// The frame for items that belong to no goal.
export const WORKGRAPH_OTHER_FRAME_ID = "other-work";
export const WORKGRAPH_OTHER_FRAME_LABEL = "Other work";

const TERMINAL_STATUSES = new Set(["completed", "cancelled", "failed"]);
const PRIORITY_RANK: Record<string, number> = { low: 0, medium: 1, high: 2 };

// ── Words and names ──────────────────────────────────────────────────────

/// Owner-label precedence shared by the outline rows, the graph cards and
/// the drawer: the owner's stored name, else its key named by the shared
/// naming function, then the claim's (workGraphItemOwner without a roster).
export function workGraphItemOwnerLabel(item: WorkGraphWireItem): string {
  return workGraphItemOwner(item)?.name ?? "";
}

/// An item's owner as people read it, with the raw key for a tooltip.
export interface WorkGraphOwnerView {
  name: string;
  /// "agent:planner"; empty when the owner carries no key.
  key: string;
}

/// The member an owner key names: a lowered key carries the member under
/// its mob ("mob/customer-ops/agent/dispatch"), which the roster knows by
/// its last segment ("dispatch").
export function workGraphOwnerMember(id: string): string {
  const trimmed = id.trim();
  const slash = trimmed.lastIndexOf("/");
  return slash >= 0 && slash < trimmed.length - 1 ? trimmed.slice(slash + 1) : trimmed;
}

/// The owner for display, in the same owner-then-claim order: the roster
/// name when the agent directory knows the key (or the member a lowered key
/// names), else the name the work graph stored, else the member in words. A
/// session the roster does not know reads "Agent session", never its ID.
export function workGraphItemOwner(
  item: WorkGraphWireItem,
  directory?: AgentDirectory,
): WorkGraphOwnerView | null {
  for (const owner of [item.owner, item.claim?.owner]) {
    if (!owner) continue;
    const id = owner.key?.id?.trim() || "";
    const kind = owner.key?.kind?.trim() || "";
    const key = id && kind ? `${kind}:${id}` : id;
    const member = workGraphOwnerMember(id);
    const known = key
      ? directory?.lookup(key)
        ?? (kind === "session" ? directory?.lookup(id) : undefined)
        ?? (member !== id ? directory?.lookup(member) : undefined)
      : undefined;
    const name = known?.label.trim()
      || owner.display_name?.trim()
      || (kind === "session" && id ? "Agent session" : agentDisplayName(member));
    if (name) return { name, key };
  }
  return null;
}

const WORK_STATUS_WORDS: Record<string, string> = {
  open: "Open",
  in_progress: "In progress",
  blocked: "Blocked",
  completed: "Completed",
  cancelled: "Cancelled",
  failed: "Failed",
};

/// A work item status in words ("In progress"); unknown values are
/// humanized, never shown raw. A missing status reads "Open".
export function workGraphStatusWord(status: string | null | undefined): string {
  const key = status || "open";
  return WORK_STATUS_WORDS[key] || humanizeEnum(key);
}

const EDGE_WORDS: Record<string, string> = {
  blocks: "blocks",
  related: "related",
  supersedes: "replaces",
  derived_from: "derived from",
  parent: "also under",
};

/// A link between two work items in words, for the graph's edge labels:
/// "blocks", "replaces" (supersedes), "derived from", "also under" (a
/// second parent). Unknown kinds are humanized, never shown raw.
export function workGraphEdgeWord(kind: string): string {
  return EDGE_WORDS[kind] || humanizeEnum(kind).toLowerCase();
}

/// The item's status, "open" when the wire leaves it out.
export function workGraphItemStatus(item: WorkGraphWireItem | undefined): string {
  return item?.status || "open";
}

export function isWorkGraphTerminal(status: string): boolean {
  return TERMINAL_STATUSES.has(status);
}

/// How long ago, compactly, for a card's meta: "now", "12m", "5h", "3d",
/// "6w". Empty for a missing or invalid time.
export function workGraphAge(value: string | null | undefined, now: number): string {
  if (!value) return "";
  const at = Date.parse(value);
  if (!Number.isFinite(at)) return "";
  const minutes = Math.max(0, Math.floor((now - at) / 60_000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 14) return `${days}d`;
  return `${Math.floor(days / 7)}w`;
}

/// "Blocked by Run the regression suite" (and "and 2 others" past one).
export function workGraphBlockedByText(titles: readonly string[]): string {
  if (titles.length === 0) return "";
  return titles.length > 1
    ? `Blocked by ${titles[0]} and ${plural(titles.length - 1, "other")}`
    : `Blocked by ${titles[0]}`;
}

// ── The model ────────────────────────────────────────────────────────────

export type WorkGraphFrameState = "in_progress" | "blocked" | "open" | "done";

export interface WorkGraphProgress {
  /// Items that count toward the goal: everything but cancelled ones.
  total: number;
  completed: number;
  inProgress: number;
  blocked: number;
  failed: number;
  cancelled: number;
  open: number;
}

export interface WorkGraphModelItem {
  id: string;
  item: WorkGraphWireItem;
  status: string;
  title: string;
  /// Depth in the parent tree (roots are 0).
  depth: number;
  /// The parent that places the item (the first parent edge to a known
  /// item); null for roots.
  parentId: string | null;
  /// Parents beyond the placing one, known items only.
  extraParentIds: string[];
  childIds: string[];
  /// The frame the item is drawn in: its own id for a goal (the frame's
  /// header), the nearest goal frame above it, the Other work frame, or null
  /// when the graph has no goal frames at all.
  frameId: string | null;
  /// The top-level goal it belongs to (its own id for a top-level goal).
  goalId: string | null;
  /// A top-level goal: a root with descendants or an attention binding.
  isGoal: boolean;
  /// A goal inside a goal: drawn as a nested frame (depth 2).
  isNestedGoal: boolean;
  /// The placing parent's title when that parent is not the frame's own
  /// goal (items nested deeper than a frame shows).
  breadcrumb: string | null;
  /// Every blocker id (the "blocks" edges into the item), known or not.
  blockerIds: string[];
  /// Blockers that still block: absent, or present and not completed.
  unresolvedBlockerIds: string[];
  /// Known items this one blocks.
  dependentIds: string[];
  ready: boolean;
  critical: boolean;
  bindings: WorkGraphWireBinding[];
}

export interface WorkGraphModelFrame {
  id: string;
  /// The goal item heading the frame; null for Other work.
  goalId: string | null;
  parentFrameId: string | null;
  depth: 1 | 2;
  label: string;
  /// Items placed directly in the frame (not its header, not nested
  /// frames' headers or items).
  itemIds: string[];
  /// Every item under the frame at any depth (header excluded).
  allItemIds: string[];
  progress: WorkGraphProgress;
  state: WorkGraphFrameState;
}

export interface WorkGraphModelLink {
  id: string;
  /// The wire kind ("blocks", "supersedes", "parent", ...).
  kind: string;
  relation: "blocks" | "related" | "parent";
  from: string;
  to: string;
}

export interface WorkGraphCounts {
  total: number;
  open: number;
  inProgress: number;
  blocked: number;
  completed: number;
  cancelled: number;
  failed: number;
  ready: number;
  /// Distinct owners of the items in progress.
  inProgressOwners: number;
}

export interface WorkGraphModel {
  items: Map<string, WorkGraphModelItem>;
  /// Item ids, oldest first (created_at, then id).
  order: string[];
  /// Frames in drawing order: top-level frames by state (in progress,
  /// blocked, open, done), each followed by its nested frames.
  frames: WorkGraphModelFrame[];
  links: WorkGraphModelLink[];
  criticalPath: { itemIds: string[]; edgeIds: string[] };
  counts: WorkGraphCounts;
  /// "server" when the snapshot's ready_item_ids decided readiness.
  readySource: "server" | "client";
  /// Input rows that cannot be drawn: no id, or a repeated id.
  invalidCount: number;
}

export interface WorkGraphModelInput {
  items: readonly WorkGraphWireItem[];
  edges: readonly WorkGraphWireEdge[];
  attention?: readonly WorkGraphWireBinding[];
  /// The snapshot's ready_item_ids; null or absent falls back to the
  /// client rule (older runtimes).
  readyItemIds?: readonly string[] | null;
  now?: number;
}

function linkId(kind: string, from: string, to: string): string {
  return `${kind}:${from}:${to}`;
}

function textId(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function storedBlockerCount(item: WorkGraphWireItem): number {
  const count = item.machine_state?.unresolved_blocker_count;
  return typeof count === "number" && Number.isFinite(count) ? count : 0;
}

/// A blocker resolves only when it is present and completed: an absent
/// blocker (unknown or outside the snapshot) and a cancelled or failed one
/// still block, as the server's own readiness rule says.
export function workGraphBlockerResolved(blocker: WorkGraphWireItem | undefined): boolean {
  return Boolean(blocker) && workGraphItemStatus(blocker) === "completed";
}

/// Open, nothing unresolved before it, every child completed, and its time
/// window open (due_at, not_before and snoozed_until all past): the
/// client's copy of the server's readiness, for snapshots without
/// ready_item_ids.
function clientReady(
  item: WorkGraphWireItem,
  unresolved: number,
  children: readonly WorkGraphWireItem[],
  now: number,
): boolean {
  if (workGraphItemStatus(item) !== "open" || unresolved > 0) return false;
  if (children.some((child) => workGraphItemStatus(child) !== "completed")) return false;
  for (const value of [item.due_at, item.not_before, item.snoozed_until]) {
    if (!value) continue;
    const at = Date.parse(value);
    if (Number.isFinite(at) && at > now) return false;
  }
  return true;
}

function emptyProgress(): WorkGraphProgress {
  return { total: 0, completed: 0, inProgress: 0, blocked: 0, failed: 0, cancelled: 0, open: 0 };
}

function addProgress(progress: WorkGraphProgress, status: string): void {
  if (status === "cancelled") {
    progress.cancelled += 1;
    return;
  }
  progress.total += 1;
  if (status === "completed") progress.completed += 1;
  else if (status === "in_progress") progress.inProgress += 1;
  else if (status === "blocked") progress.blocked += 1;
  else if (status === "failed") progress.failed += 1;
  else progress.open += 1;
}

const FRAME_STATE_RANK: Record<WorkGraphFrameState, number> = { in_progress: 0, blocked: 1, open: 2, done: 3 };

function frameState(statuses: readonly string[]): WorkGraphFrameState {
  if (statuses.some((status) => status === "in_progress")) return "in_progress";
  if (statuses.some((status) => status === "blocked")) return "blocked";
  if (statuses.some((status) => !isWorkGraphTerminal(status))) return "open";
  return "done";
}

/// The longest chain of unfinished items over "blocks" edges, counted in
/// items. Ties go to the chain with more priority (high 2, medium 1, low 0,
/// summed), then the one that starts with the older item, then the lower
/// id. Completed and cancelled items are not steps to finish; failed ones
/// are (they must be redone). A blocks cycle is cut where the walk meets
/// it, deterministically.
export function workGraphCriticalPath(
  order: readonly string[],
  byId: ReadonlyMap<string, WorkGraphWireItem>,
  edges: readonly WorkGraphWireEdge[],
): { itemIds: string[]; edgeIds: string[] } {
  const eligible = (id: string) => {
    const item = byId.get(id);
    if (!item) return false;
    const status = workGraphItemStatus(item);
    return status !== "completed" && status !== "cancelled";
  };
  const rankOf = new Map(order.map((id, index) => [id, index]));
  const next = new Map<string, string[]>();
  for (const edge of edges) {
    if (edge.kind !== "blocks") continue;
    const from = textId(edge.from_id);
    const to = textId(edge.to_id);
    if (!from || !to || from === to || !eligible(from) || !eligible(to)) continue;
    const list = next.get(from) ?? [];
    if (!list.includes(to)) list.push(to);
    next.set(from, list);
  }
  for (const list of next.values()) list.sort((a, b) => (rankOf.get(a) ?? 0) - (rankOf.get(b) ?? 0));
  interface Chain { length: number; priority: number; next: string | null }
  const best = new Map<string, Chain>();
  const onPath = new Set<string>();
  const priorityOf = (id: string) => PRIORITY_RANK[byId.get(id)?.priority ?? "medium"] ?? 1;
  // Iterative depth-first walk, so a long chain cannot overflow the stack.
  const solve = (start: string) => {
    const stack: Array<{ id: string; index: number }> = [{ id: start, index: 0 }];
    onPath.add(start);
    while (stack.length > 0) {
      const frame = stack[stack.length - 1];
      const successors = next.get(frame.id) ?? [];
      if (frame.index < successors.length) {
        const successor = successors[frame.index];
        frame.index += 1;
        if (best.has(successor) || onPath.has(successor)) continue;
        onPath.add(successor);
        stack.push({ id: successor, index: 0 });
        continue;
      }
      let chosen: string | null = null;
      for (const successor of successors) {
        const candidate = best.get(successor);
        if (!candidate) continue;
        const current = chosen ? best.get(chosen)! : null;
        if (
          !current
          || candidate.length > current.length
          || (candidate.length === current.length && candidate.priority > current.priority)
        ) {
          chosen = successor;
        }
      }
      const tail = chosen ? best.get(chosen)! : null;
      best.set(frame.id, {
        length: 1 + (tail?.length ?? 0),
        priority: priorityOf(frame.id) + (tail?.priority ?? 0),
        next: chosen,
      });
      onPath.delete(frame.id);
      stack.pop();
    }
  };
  let start: string | null = null;
  for (const id of order) {
    if (!eligible(id)) continue;
    if (!best.has(id)) solve(id);
    const candidate = best.get(id)!;
    const current = start ? best.get(start)! : null;
    // `order` is oldest first, so the first chain of a tied length and
    // priority is the one that starts with the older item.
    if (
      !current
      || candidate.length > current.length
      || (candidate.length === current.length && candidate.priority > current.priority)
    ) {
      start = id;
    }
  }
  const itemIds: string[] = [];
  const edgeIds: string[] = [];
  for (let id = start; id; id = best.get(id)?.next ?? null) {
    if (itemIds.length > 0) edgeIds.push(linkId("blocks", itemIds[itemIds.length - 1], id));
    itemIds.push(id);
  }
  return { itemIds, edgeIds };
}

/// Reads one snapshot into the model every WorkGraph view draws from.
export function buildWorkGraphModel(input: WorkGraphModelInput): WorkGraphModel {
  const now = input.now ?? Date.now();
  const byId = new Map<string, WorkGraphWireItem>();
  for (const item of input.items) {
    if (typeof item.id === "string" && item.id) byId.set(item.id, item);
  }
  const invalidCount = input.items.length - byId.size;
  const order = [...byId.keys()].sort((left, right) => {
    const leftKey = byId.get(left)?.created_at || "";
    const rightKey = byId.get(right)?.created_at || "";
    if (leftKey !== rightKey) return leftKey < rightKey ? -1 : 1;
    return left < right ? -1 : left === right ? 0 : 1;
  });
  const rank = new Map(order.map((id, index) => [id, index]));
  const byOrder = (a: string, b: string) => (rank.get(a) ?? 0) - (rank.get(b) ?? 0);

  // Parents: the first parent edge places the child (the outline's
  // first-parent-wins rule); later ones are extra parents.
  const firstParent = new Map<string, string>();
  const extraParents = new Map<string, string[]>();
  for (const edge of input.edges) {
    if (edge.kind !== "parent") continue;
    const from = textId(edge.from_id);
    const to = textId(edge.to_id);
    if (!from || !to || from === to) continue;
    const placed = firstParent.get(from);
    if (placed === undefined) firstParent.set(from, to);
    else if (placed !== to && byId.has(to)) {
      const extras = extraParents.get(from) ?? [];
      if (!extras.includes(to)) extras.push(to);
      extraParents.set(from, extras);
    }
  }
  // Effective parents and depths; a parent cycle is cut where the walk
  // closes it, so every item is placed exactly once.
  const parentOf = new Map<string, string | null>();
  const depthOf = new Map<string, number>();
  const resolve = (start: string) => {
    const path: string[] = [];
    const onPath = new Set<string>();
    let current: string | null = start;
    while (current && !depthOf.has(current)) {
      path.push(current);
      onPath.add(current);
      const parent = firstParent.get(current);
      if (!parent || !byId.has(parent) || onPath.has(parent)) {
        parentOf.set(current, null);
        depthOf.set(current, 0);
        path.pop();
        break;
      }
      current = parent;
    }
    for (let index = path.length - 1; index >= 0; index -= 1) {
      const id = path[index];
      const parent = firstParent.get(id)!;
      parentOf.set(id, parent);
      depthOf.set(id, (depthOf.get(parent) ?? 0) + 1);
    }
  };
  for (const id of order) resolve(id);
  const childrenOf = new Map<string, string[]>();
  for (const id of order) {
    const parent = parentOf.get(id);
    if (!parent) continue;
    const list = childrenOf.get(parent) ?? [];
    list.push(id);
    childrenOf.set(parent, list);
  }

  const bindingsOf = new Map<string, WorkGraphWireBinding[]>();
  for (const binding of input.attention ?? []) {
    const itemId = binding.work_ref?.item_id;
    if (!itemId) continue;
    bindingsOf.set(itemId, [...(bindingsOf.get(itemId) ?? []), binding]);
  }

  // Blocks edges, deduplicated, self edges dropped.
  const blockersOf = new Map<string, string[]>();
  const dependentsOf = new Map<string, string[]>();
  for (const edge of input.edges) {
    if (edge.kind !== "blocks") continue;
    const from = textId(edge.from_id);
    const to = textId(edge.to_id);
    if (!from || !to || from === to) continue;
    const blockers = blockersOf.get(to) ?? [];
    if (blockers.includes(from)) continue;
    blockers.push(from);
    blockersOf.set(to, blockers);
    if (byId.has(to)) dependentsOf.set(from, [...(dependentsOf.get(from) ?? []), to]);
  }

  // Goals and frames.
  const isGoal = (id: string) =>
    (depthOf.get(id) ?? 0) === 0 && ((childrenOf.get(id)?.length ?? 0) > 0 || bindingsOf.has(id));
  const isNestedGoal = (id: string) => {
    const parent = parentOf.get(id);
    return Boolean(parent) && isGoal(parent!) && (childrenOf.get(id)?.length ?? 0) > 0;
  };
  const goalIds = order.filter(isGoal);
  const hasFrames = goalIds.length > 0;
  const frameOf = (id: string): string | null => {
    if (isGoal(id) || isNestedGoal(id)) return id;
    for (let ancestor = parentOf.get(id) ?? null; ancestor; ancestor = parentOf.get(ancestor) ?? null) {
      if (isGoal(ancestor) || isNestedGoal(ancestor)) return ancestor;
    }
    return hasFrames ? WORKGRAPH_OTHER_FRAME_ID : null;
  };
  const topGoalOf = (id: string): string | null => {
    let top: string | null = null;
    for (let current: string | null = id; current; current = parentOf.get(current) ?? null) {
      if (isGoal(current)) top = current;
    }
    return top;
  };

  const readySet = Array.isArray(input.readyItemIds) ? new Set(input.readyItemIds) : null;
  const critical = workGraphCriticalPath(order, byId, input.edges);
  const criticalSet = new Set(critical.itemIds);

  const items = new Map<string, WorkGraphModelItem>();
  for (const id of order) {
    const item = byId.get(id)!;
    const status = workGraphItemStatus(item);
    const blockerIds = blockersOf.get(id) ?? [];
    const unresolvedBlockerIds = blockerIds.filter((blocker) => !workGraphBlockerResolved(byId.get(blocker)));
    const childIds = childrenOf.get(id) ?? [];
    const frameId = frameOf(id);
    const parentId = parentOf.get(id) ?? null;
    const ownFrame = frameId === id;
    const breadcrumb = !ownFrame && parentId && parentId !== frameId
      ? byId.get(parentId)?.title || "Untitled work item"
      : null;
    // Ready: the server's set, cross-checked against the snapshot's own
    // blockers and the item's stored blocker count; without the set, the
    // client rule.
    const ready = readySet
      ? readySet.has(id) && unresolvedBlockerIds.length === 0 && storedBlockerCount(item) === 0
      : clientReady(item, unresolvedBlockerIds.length, childIds.map((child) => byId.get(child)!), now);
    items.set(id, {
      id,
      item,
      status,
      title: item.title || "Untitled work item",
      depth: depthOf.get(id) ?? 0,
      parentId,
      extraParentIds: extraParents.get(id) ?? [],
      childIds,
      frameId,
      goalId: topGoalOf(id),
      isGoal: isGoal(id),
      isNestedGoal: isNestedGoal(id),
      breadcrumb,
      blockerIds,
      unresolvedBlockerIds,
      dependentIds: dependentsOf.get(id) ?? [],
      ready,
      critical: criticalSet.has(id),
      bindings: bindingsOf.get(id) ?? [],
    });
  }

  // Frames: one per goal, nested goals inside their parent, then Other work.
  const frames: WorkGraphModelFrame[] = [];
  if (hasFrames) {
    const descendantsOf = (id: string): string[] => {
      const result: string[] = [];
      const stack = [...(childrenOf.get(id) ?? [])].reverse();
      while (stack.length > 0) {
        const next = stack.pop()!;
        result.push(next);
        stack.push(...[...(childrenOf.get(next) ?? [])].reverse());
      }
      return result;
    };
    const build = (goalId: string, parentFrameId: string | null): WorkGraphModelFrame => {
      const allItemIds = descendantsOf(goalId);
      const progress = emptyProgress();
      for (const child of allItemIds) addProgress(progress, items.get(child)!.status);
      const statuses = [items.get(goalId)!.status, ...allItemIds.map((child) => items.get(child)!.status)];
      return {
        id: goalId,
        goalId,
        parentFrameId,
        depth: parentFrameId ? 2 : 1,
        label: items.get(goalId)!.title,
        itemIds: allItemIds.filter((child) => items.get(child)!.frameId === goalId && child !== goalId),
        allItemIds,
        progress,
        state: frameState(statuses),
      };
    };
    const byState = (a: WorkGraphModelFrame, b: WorkGraphModelFrame) =>
      FRAME_STATE_RANK[a.state] - FRAME_STATE_RANK[b.state] || byOrder(a.id, b.id);
    const topFrames = goalIds.map((goalId) => build(goalId, null));
    const otherIds = order.filter((id) => items.get(id)!.frameId === WORKGRAPH_OTHER_FRAME_ID);
    let other: WorkGraphModelFrame | null = null;
    if (otherIds.length > 0) {
      const progress = emptyProgress();
      for (const id of otherIds) addProgress(progress, items.get(id)!.status);
      other = {
        id: WORKGRAPH_OTHER_FRAME_ID,
        goalId: null,
        parentFrameId: null,
        depth: 1,
        label: WORKGRAPH_OTHER_FRAME_LABEL,
        itemIds: otherIds,
        allItemIds: otherIds,
        progress,
        state: frameState(otherIds.map((id) => items.get(id)!.status)),
      };
    }
    const ordered = [...topFrames, ...(other ? [other] : [])].sort((a, b) => {
      const byFrameState = FRAME_STATE_RANK[a.state] - FRAME_STATE_RANK[b.state];
      if (byFrameState !== 0) return byFrameState;
      // Other work goes after the goals in its state.
      if ((a.goalId === null) !== (b.goalId === null)) return a.goalId === null ? 1 : -1;
      return byOrder(a.id, b.id);
    });
    for (const frame of ordered) {
      frames.push(frame);
      if (!frame.goalId) continue;
      const nested = (childrenOf.get(frame.goalId) ?? [])
        .filter((child) => items.get(child)!.isNestedGoal)
        .map((child) => build(child, frame.id))
        .sort(byState);
      frames.push(...nested);
    }
  }

  // Links: blocks, the related family, and second parents outside the
  // child's frames (containment already shows the first parent).
  const links: WorkGraphModelLink[] = [];
  const seen = new Set<string>();
  for (const edge of input.edges) {
    const kind = textId(edge.kind);
    const from = textId(edge.from_id);
    const to = textId(edge.to_id);
    if (!kind || kind === "parent" || !from || !to || from === to) continue;
    if (!byId.has(from) || !byId.has(to)) continue;
    const id = linkId(kind, from, to);
    if (seen.has(id)) continue;
    seen.add(id);
    links.push({ id, kind, relation: kind === "blocks" ? "blocks" : "related", from, to });
  }
  for (const id of order) {
    const entry = items.get(id)!;
    const frameChain = new Set<string>();
    for (let frame: string | null = entry.frameId; frame; frame = frame === WORKGRAPH_OTHER_FRAME_ID ? null : (items.get(frame)?.isNestedGoal ? parentOf.get(frame) ?? null : null)) {
      frameChain.add(frame);
    }
    for (const parent of entry.extraParentIds) {
      if (frameChain.has(parent)) continue;
      const linkKey = linkId("parent", id, parent);
      if (seen.has(linkKey)) continue;
      seen.add(linkKey);
      links.push({ id: linkKey, kind: "parent", relation: "parent", from: id, to: parent });
    }
  }

  const counts: WorkGraphCounts = {
    total: items.size,
    open: 0,
    inProgress: 0,
    blocked: 0,
    completed: 0,
    cancelled: 0,
    failed: 0,
    ready: 0,
    inProgressOwners: 0,
  };
  const owners = new Set<string>();
  for (const entry of items.values()) {
    if (entry.status === "in_progress") {
      counts.inProgress += 1;
      const owner = workGraphItemOwner(entry.item);
      if (owner) owners.add(owner.key || owner.name);
    } else if (entry.status === "blocked") counts.blocked += 1;
    else if (entry.status === "completed") counts.completed += 1;
    else if (entry.status === "cancelled") counts.cancelled += 1;
    else if (entry.status === "failed") counts.failed += 1;
    else counts.open += 1;
    if (entry.ready) counts.ready += 1;
  }
  counts.inProgressOwners = owners.size;

  return {
    items,
    order,
    frames,
    links,
    criticalPath: critical,
    counts,
    readySource: readySet ? "server" : "client",
    invalidCount,
  };
}

/// What the blocked items wait on, for the BLOCKED figure: the one blocker
/// they share by title, else how many distinct items hold them.
export function workGraphBlockedFoot(model: WorkGraphModel): string {
  const blockers = new Set<string>();
  for (const entry of model.items.values()) {
    if (entry.status !== "blocked") continue;
    for (const blocker of entry.unresolvedBlockerIds) blockers.add(blocker);
  }
  if (blockers.size === 0) return model.counts.blocked > 0 ? "marked blocked" : "nothing waiting";
  if (blockers.size === 1) {
    const [only] = blockers;
    return `waiting on ${model.items.get(only)?.title ?? "an item outside this view"}`;
  }
  return `waiting on ${plural(blockers.size, "item")}`;
}

// ── The graph the kit draws ──────────────────────────────────────────────

/// The node, edge and frame records the kit's GraphView takes, with the
/// WorkGraph payload the panel reads back in its node decorations.
export interface WorkGraphNodeData {
  /// An attention binding rests on the item (the goal ring).
  bound: boolean;
  critical: boolean;
}

export type WorkGraphVizNode = VizNode & { kind: "item" | "goal"; data?: WorkGraphNodeData };
export type WorkGraphVizEdge = VizEdge;
export type WorkGraphVizFrame = VizFrame;

export interface WorkGraphGraphOptions {
  selectedId?: string | null;
  /// Expand goals whose every item is finished (collapsed by default).
  showDone?: boolean;
  /// Draw cancelled items (hidden by default).
  showCancelled?: boolean;
  /// Draw the critical path in the accent.
  highlightCriticalPath?: boolean;
  directory?: AgentDirectory;
  now?: number;
  /// Who finished a completed item, from the activity (workGraphDoneBy).
  doneBy?: ReadonlyMap<string, string>;
  cap?: number;
}

export interface WorkGraphGraph {
  nodes: WorkGraphVizNode[];
  edges: WorkGraphVizEdge[];
  frames: WorkGraphVizFrame[];
  accentPath: { nodes: string[]; edges: string[] } | null;
  /// Level of detail: WORKGRAPH_LARGE_ITEM_COUNT items or more.
  lod: boolean;
  /// Items neither drawn nor folded into a collapsed goal nor hidden by a
  /// view option: the "+N more items not drawn" count.
  notDrawn: number;
  hiddenCancelled: number;
  edgeLabels: "always" | "hover";
}

/// The second line of an item card: priority when it matters (high, low),
/// what holds it or that it can start, who has it, where it sits when
/// deeper than its frame, and its age.
export function workGraphCardMeta(
  entry: WorkGraphModelItem,
  model: WorkGraphModel,
  options: { directory?: AgentDirectory; now: number; doneBy?: ReadonlyMap<string, string> },
): string {
  const priority = entry.item.priority === "high" || entry.item.priority === "low"
    ? humanizeEnum(entry.item.priority)
    : "";
  const blockedBy = entry.unresolvedBlockerIds.length > 0 && !isWorkGraphTerminal(entry.status)
    ? workGraphBlockedByText(entry.unresolvedBlockerIds.map((id) => model.items.get(id)?.title ?? "an item outside this view"))
    : "";
  const owner = workGraphItemOwner(entry.item, options.directory);
  let who: string;
  if (entry.status === "completed") {
    const by = options.doneBy?.get(entry.id);
    who = by ? `Done by ${by}` : "Done";
  } else if (owner) who = owner.name;
  else who = isWorkGraphTerminal(entry.status) ? workGraphStatusWord(entry.status) : "Unassigned";
  return joinMeta(
    priority,
    blockedBy,
    who,
    !blockedBy && entry.ready ? "Ready" : "",
    entry.breadcrumb ? `Under ${entry.breadcrumb}` : "",
    workGraphAge(entry.item.created_at, options.now),
  );
}

function hoverText(entry: WorkGraphModelItem, ownerName: string): string {
  const lines = [entry.title, `Status: ${workGraphStatusWord(entry.status)}`];
  if (ownerName) lines.push(`Owner: ${ownerName}`);
  if (entry.item.description) lines.push(entry.item.description);
  return lines.join("\n");
}

/// "3 of 5 done"; a goal with no items reads its own status.
export function workGraphProgressText(progress: WorkGraphProgress): string {
  return progress.total > 0 ? `${progress.completed} of ${progress.total} done` : "No items yet";
}

/// Who works a goal, for its frame header: the first live binding's agent
/// and what it does ("Dispatch · Leading the work"), or its pause
/// ("Insights · Paused until Sun 18:51"). Empty without a live binding.
export function workGraphBindingSummary(
  bindings: readonly WorkGraphWireBinding[],
  directory: AgentDirectory | undefined,
  now: number,
): string {
  return workGraphBindingTrailer(bindings, directory, now)?.text ?? "";
}

/// The goal header's trailer: the summary and the agent's initial.
export function workGraphBindingTrailer(
  bindings: readonly WorkGraphWireBinding[],
  directory: AgentDirectory | undefined,
  now: number,
): { text: string; avatar?: string } | null {
  const live = bindings.find((binding) => {
    const state = binding.status?.state || "active";
    return state === "active" || state === "paused";
  });
  if (!live) return null;
  const target = workGraphBindingTarget(live, directory);
  const paused = (live.status?.state || "active") === "paused";
  const text = joinMeta(target?.name ?? "", paused ? workGraphBindingStateText(live, now) : workGraphModeWord(live.mode));
  return text ? { text, ...(target?.name ? { avatar: agentInitial(target.name) } : {}) } : null;
}

/// What the Graph draws for a model and the view options: cards inside goal
/// frames, the links between them, the critical path in the accent, and
/// the level of detail for very large graphs (done goals folded, and only
/// the critical path, the work in progress and the blocked items drawn).
export function buildWorkGraphGraph(model: WorkGraphModel, options: WorkGraphGraphOptions = {}): WorkGraphGraph {
  const now = options.now ?? Date.now();
  const cap = options.cap ?? WORKGRAPH_GRAPH_NODE_CAP;
  const lod = model.items.size >= WORKGRAPH_LARGE_ITEM_COUNT;
  const selectedId = options.selectedId ?? null;
  const frameById = new Map(model.frames.map((frame) => [frame.id, frame]));
  const collapsedFrames = new Set<string>();
  for (const frame of model.frames) {
    if (frame.state === "done" && (lod || !options.showDone)) collapsedFrames.add(frame.id);
  }
  // A nested frame inside a collapsed frame is folded with it.
  const folded = (frameId: string | null): boolean => {
    for (let current = frameId; current; current = frameById.get(current)?.parentFrameId ?? null) {
      if (collapsedFrames.has(current) && current !== frameId) return true;
    }
    return false;
  };
  const inCollapsed = (entry: WorkGraphModelItem): boolean => {
    if (!entry.frameId) return false;
    if (entry.frameId === entry.id) return folded(entry.id);
    return collapsedFrames.has(entry.frameId) || folded(entry.frameId);
  };

  let hiddenCancelled = 0;
  const lodHidden = new Map<string, number>();
  const candidates: WorkGraphModelItem[] = [];
  for (const id of model.order) {
    const entry = model.items.get(id)!;
    const header = entry.frameId === entry.id;
    if (!header && entry.status === "cancelled" && !options.showCancelled && id !== selectedId) {
      hiddenCancelled += 1;
      continue;
    }
    if (lod && !header && !inCollapsed(entry) && id !== selectedId
      && !entry.critical && entry.status !== "in_progress" && entry.status !== "blocked") {
      const frame = entry.frameId ?? "";
      lodHidden.set(frame, (lodHidden.get(frame) ?? 0) + 1);
      continue;
    }
    candidates.push(entry);
  }
  // Frame headers first (they hold the picture together), then items in
  // frame order; the cap cuts from the end.
  const frameRank = new Map(model.frames.map((frame, index) => [frame.id, index]));
  candidates.sort((a, b) => {
    const aHeader = a.frameId === a.id ? 0 : 1;
    const bHeader = b.frameId === b.id ? 0 : 1;
    if (aHeader !== bHeader) return aHeader - bHeader;
    const aFrame = frameRank.get(a.frameId ?? "") ?? Number.MAX_SAFE_INTEGER;
    const bFrame = frameRank.get(b.frameId ?? "") ?? Number.MAX_SAFE_INTEGER;
    return aFrame - bFrame;
  });
  let drawn = candidates;
  let capped = 0;
  // Items folded into a collapsed frame are passed to the kit (it routes
  // their edges to the frame's header) but are not drawn; only drawn cards
  // count toward the cap.
  const visibleCount = (list: readonly WorkGraphModelItem[]) => list.filter((entry) => !inCollapsed(entry)).length;
  if (visibleCount(candidates) > cap) {
    const kept: WorkGraphModelItem[] = [];
    let visible = 0;
    for (const entry of candidates) {
      const hidden = inCollapsed(entry);
      if (!hidden && visible >= cap && entry.id !== selectedId) {
        capped += 1;
        continue;
      }
      if (!hidden) visible += 1;
      kept.push(entry);
    }
    drawn = kept;
  }
  const drawnIds = new Set(drawn.map((entry) => entry.id));

  const bound = (entry: WorkGraphModelItem) => entry.bindings.length > 0;
  const nodes: WorkGraphVizNode[] = drawn.map((entry, index) => {
    const header = entry.frameId === entry.id;
    const owner = workGraphItemOwner(entry.item, options.directory);
    const statusWord = workGraphStatusWord(entry.status);
    const frameGoal = header ? frameById.get(entry.id)?.parentFrameId ?? null : entry.frameId;
    const progress = header ? frameById.get(entry.id)?.progress : undefined;
    const trailing = header ? workGraphBindingTrailer(entry.bindings, options.directory, now) : null;
    return {
      ...(progress && progress.total > 0
        ? { progress: { done: progress.completed, inProgress: progress.inProgress, blocked: progress.blocked, total: progress.total } }
        : {}),
      ...(trailing ? { trailing } : {}),
      id: entry.id,
      kind: header ? "goal" : "item",
      label: entry.title,
      sublabel: header
        ? undefined
        : workGraphCardMeta(entry, model, { directory: options.directory, now, doneBy: options.doneBy }),
      frame: header ? (frameById.get(entry.id)?.parentFrameId ?? undefined) : entry.frameId ?? undefined,
      order: index,
      tone: statusTone(entry.status),
      status: entry.status,
      statusLabel: statusWord,
      avatar: !header && owner && !isWorkGraphTerminal(entry.status) ? agentInitial(owner.name) : undefined,
      className: `is-${entry.status}${entry.id === selectedId ? " is-selected" : ""}${entry.critical ? " is-critical" : ""}`,
      attributes: {
        "data-item-id": entry.id,
        "data-status": entry.status,
        "data-goal-id": frameGoal && frameGoal !== WORKGRAPH_OTHER_FRAME_ID ? frameGoal : undefined,
      },
      ariaLabel: `${entry.title}, ${statusWord}`,
      title: hoverText(entry, owner?.name ?? ""),
      data: { bound: bound(entry), critical: entry.critical },
    };
  });

  const frames: WorkGraphVizFrame[] = model.frames
    .filter((frame) => frame.goalId === null ? frame.itemIds.some((id) => drawnIds.has(id)) : drawnIds.has(frame.id))
    .map((frame, index) => {
      const hidden = lodHidden.get(frame.id) ?? 0;
      return {
        id: frame.id,
        label: frame.label,
        meta: joinMeta(
          workGraphProgressText(frame.progress),
          hidden > 0 ? `+${hidden} ${hidden === 1 ? "item" : "items"}` : "",
          // A folded goal says so: "Show done goals" in the menu opens it.
          collapsedFrames.has(frame.id) ? "collapsed" : "",
        ),
        parent: frame.parentFrameId ?? undefined,
        order: index,
        collapsed: collapsedFrames.has(frame.id),
        className: `workgraph-graph__frame is-${frame.state}`,
        attributes: { "data-goal-id": frame.goalId ?? undefined, "data-frame-state": frame.state },
      };
    });

  const critical = new Set(model.criticalPath.edgeIds);
  const edges: WorkGraphVizEdge[] = [];
  for (const link of model.links) {
    if (!drawnIds.has(link.from) || !drawnIds.has(link.to)) continue;
    const fromTitle = model.items.get(link.from)?.title ?? "";
    const toTitle = model.items.get(link.to)?.title ?? "";
    const word = workGraphEdgeWord(link.kind);
    edges.push({
      id: link.id,
      from: link.from,
      to: link.to,
      relation: link.relation,
      directed: link.relation === "blocks" || link.kind === "supersedes" || link.kind === "derived_from",
      label: word,
      title: link.relation === "parent" ? `${fromTitle} is also under ${toTitle}` : `${fromTitle} ${word} ${toTitle}`,
      className: `workgraph-graph__edge is-${link.kind}${critical.has(link.id) ? " is-critical" : ""}`,
      attributes: { "data-kind": link.kind },
    });
  }

  const pathNodes = model.criticalPath.itemIds.filter((id) => drawnIds.has(id));
  const pathEdges = model.criticalPath.edgeIds.filter((id) => edges.some((edge) => edge.id === id));
  const accentPath = options.highlightCriticalPath && pathNodes.length > 0
    ? { nodes: pathNodes, edges: pathEdges }
    : null;
  let notDrawn = capped + model.invalidCount;
  for (const count of lodHidden.values()) notDrawn += count;

  return {
    nodes,
    edges,
    frames,
    accentPath,
    lod,
    notDrawn,
    hiddenCancelled,
    edgeLabels: edges.length > WORKGRAPH_EDGE_LABEL_LIMIT ? "hover" : "always",
  };
}

/// The view a WorkGraph panel opens in: the Graph when the pane is at
/// least 720 px wide and holds 3 or more items (and fewer than the large
/// threshold), else the Outline. Without a measured width (server render,
/// jsdom) it is the Outline.
export function workGraphDefaultView(width: number | null | undefined, itemCount: number): "graph" | "tree" {
  if (!width || width <= 0) return "tree";
  if (itemCount >= WORKGRAPH_LARGE_ITEM_COUNT) return "tree";
  return width >= 720 && itemCount >= 3 ? "graph" : "tree";
}

// ── Activity ─────────────────────────────────────────────────────────────

export interface WorkGraphActivityDay {
  /// Local midnight, epoch ms.
  start: number;
  created: number;
  closed: number;
}

/// Items created and closed per local day, oldest day first, ending today.
export function workGraphActivityDays(
  events: readonly WorkGraphWireEvent[],
  now: number,
  days = 7,
): WorkGraphActivityDay[] {
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const result: WorkGraphActivityDay[] = [];
  for (let offset = days - 1; offset >= 0; offset -= 1) {
    const start = new Date(today);
    start.setDate(today.getDate() - offset);
    result.push({ start: start.getTime(), created: 0, closed: 0 });
  }
  const first = result[0]?.start ?? now;
  for (const event of events) {
    const kind = textId(event.kind).replace(/^item_/, "");
    if (kind !== "created" && kind !== "closed") continue;
    const at = Date.parse(textId(event.at));
    if (!Number.isFinite(at) || at < first || at > now + 60_000) continue;
    let index = result.length - 1;
    while (index > 0 && result[index].start > at) index -= 1;
    if (kind === "created") result[index].created += 1;
    else result[index].closed += 1;
  }
  return result;
}

/// Who finished each completed item, as far as the events window shows it.
/// Events carry no actor, so "Done by" names the owner who held the item
/// when its `closed` event landed: the last claim before the close with no
/// release in between. An item whose claim or close is outside the window
/// reads plain "Done".
export function workGraphDoneBy(
  events: readonly WorkGraphWireEvent[],
  directory?: AgentDirectory,
): Map<string, string> {
  const ordered = events
    .map((event, index) => ({ event, index, seq: typeof event.seq === "number" ? event.seq : Number.NaN }))
    .sort((left, right) => {
      if (Number.isFinite(left.seq) && Number.isFinite(right.seq) && left.seq !== right.seq) return left.seq - right.seq;
      return left.index - right.index;
    });
  const holder = new Map<string, string>();
  const doneBy = new Map<string, string>();
  for (const { event } of ordered) {
    const payload = event.payload && typeof event.payload === "object" ? event.payload as { item?: WorkGraphWireItem } : null;
    const item = payload?.item;
    const itemId = textId(event.item_id) || textId(item?.id);
    if (!itemId) continue;
    const kind = textId(event.kind).replace(/^item_/, "");
    if (kind === "claimed") {
      const owner = item ? workGraphItemOwner(item, directory) : null;
      if (owner) holder.set(itemId, owner.name);
      else holder.delete(itemId);
    } else if (kind === "released") {
      holder.delete(itemId);
    } else if (kind === "closed") {
      const name = holder.get(itemId);
      const status = textId(item?.status);
      if (name && (!status || status === "completed")) doneBy.set(itemId, name);
      else doneBy.delete(itemId);
      holder.delete(itemId);
    }
  }
  return doneBy;
}

// ── Attention bindings and activity in words ─────────────────────────────

/// What the agent on a goal does with it, in plain words; the attention
/// mode's own name ("coordinate", "falsify") stays the tooltip.
const MODE_WORDS: Record<string, string> = {
  pursue: "Doing the work",
  coordinate: "Leading the work",
  review: "Reviewing the work",
  falsify: "Testing the work",
  judge: "Judging the work",
  observe: "Watching the work",
};

/// What an attention binding does with its goal, in words. A missing mode
/// is "pursue", as upstream defaults it.
export function workGraphModeWord(mode: string | null | undefined): string {
  const key = mode || "pursue";
  return MODE_WORDS[key] || humanizeEnum(key);
}

/// The binding's raw state ("active", "paused until 2026-07-09 10:30"),
/// the form older surfaces and tests read.
export function workGraphBindingStatusLabel(binding: WorkGraphWireBinding): string {
  const state = binding.status?.state || "active";
  if (state === "paused") {
    const until = formatWorkGraphTimestamp(binding.status?.until, { date: true });
    return until ? `paused until ${until}` : "paused";
  }
  return state;
}

/// The binding's raw target: the session id or "kind:id" of the owner key.
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

const DAY_MS = 24 * 60 * 60 * 1000;

/// A nearby moment inside a sentence: the clock today, the weekday and clock
/// within the week, the date and clock beyond it.
export function workGraphNearMoment(ms: number, now: number): string {
  const clock = formatClockTime(ms);
  const date = new Date(ms);
  if (date.toDateString() === new Date(now).toDateString()) return clock;
  if (Math.abs(ms - now) < 6 * DAY_MS) {
    return `${date.toLocaleDateString(undefined, { weekday: "short" })} ${clock}`;
  }
  return `${formatShortDate(ms, now)}, ${clock}`;
}

/// A binding's state in words: "Active", "Paused until Fri 11:16",
/// "Replaced" (superseded by a newer binding). workGraphBindingStatusLabel
/// keeps the raw form.
export function workGraphBindingStateText(
  binding: WorkGraphWireBinding,
  now: number = Date.now(),
): string {
  const state = binding.status?.state || "active";
  if (state === "paused") {
    const until = toEpochMs(binding.status?.until);
    return until === null ? "Paused" : `Paused until ${workGraphNearMoment(until, now)}`;
  }
  if (state === "superseded") return "Replaced";
  return humanizeEnum(state);
}

/// Who a binding's attention rests on, by name: the roster name for a known
/// session or owner key, the bare key for an owner the roster does not know,
/// and "Agent session" for an unknown session (its ID stays in the
/// tooltip and the technical details). workGraphBindingTargetLabel keeps the
/// raw target.
export function workGraphBindingTarget(
  binding: WorkGraphWireBinding,
  directory?: AgentDirectory,
): WorkGraphOwnerView | null {
  const target = binding.target;
  if (!target) return null;
  const sessionId = typeof target.session_id === "string" ? target.session_id.trim() : "";
  if (sessionId) {
    return { name: directory?.lookup(sessionId)?.label.trim() || "Agent session", key: sessionId };
  }
  const id = target.owner_key?.id?.trim() || "";
  if (!id) return null;
  const kind = target.owner_key?.kind?.trim() || "";
  const key = kind ? `${kind}:${id}` : id;
  const member = workGraphOwnerMember(id);
  return {
    name: directory?.lookup(key)?.label.trim()
      || (member !== id ? directory?.lookup(member)?.label.trim() : undefined)
      || agentDisplayName(member),
    key,
  };
}

/// The agent a binding rests on, named for a sentence; null for a session
/// the roster does not know (the row says "Agent session").
function bindingAgentName(binding: WorkGraphWireBinding | null, directory?: AgentDirectory): string | null {
  if (!binding) return null;
  const target = workGraphBindingTarget(binding, directory);
  if (!target) return null;
  const sessionId = typeof binding.target?.session_id === "string" ? binding.target.session_id.trim() : "";
  if (sessionId && !directory?.lookup(sessionId)) return null;
  return target.name || null;
}

/// Latest observed revision of the binding's bound goal work item. Goal
/// confirm / request-close CAS against this (the server checks the ITEM's
/// revision), never against the binding's machine revision.
export function workGraphGoalRevisionOf(
  binding: WorkGraphWireBinding,
  items: readonly WorkGraphWireItem[],
): number | undefined {
  const itemId = binding.work_ref?.item_id;
  if (!itemId) return undefined;
  const item = items.find((candidate) => candidate.id === itemId);
  return typeof item?.revision === "number" ? item.revision : undefined;
}

/// How the goal's completion is accepted, in words.
export function workGraphCompletionPolicyText(
  policy: WorkGraphWireItem["completion_policy"],
  directory?: AgentDirectory,
): string {
  switch (policy?.kind || "self_attest") {
    case "self_attest":
      return "the agent doing it confirms it";
    case "host_confirmed":
      return "the host confirms it";
    case "principal_confirmed":
      return "a person confirms it";
    case "supervisor": {
      const id = policy?.owner_key?.id?.trim() || "";
      const key = policy?.owner_key?.kind && id ? `${policy.owner_key.kind}:${id}` : id;
      const name = (key && directory?.lookup(key)?.label.trim()) || (id ? agentDisplayName(id) : "");
      return name ? `${name} confirms it` : "its supervisor confirms it";
    }
    case "reviewer_quorum": {
      const threshold = typeof policy?.threshold === "number" ? policy.threshold : 1;
      return `${plural(threshold, "reviewer")} confirm it`;
    }
    default:
      return humanizeEnum(policy?.kind).toLowerCase();
  }
}

/// The compact "time · kind · id" line of an event.
export function workGraphEventLine(event: WorkGraphWireEvent): string {
  const kind = typeof event.kind === "string" ? event.kind.replace(/_/g, " ") : "event";
  const at = formatWorkGraphTimestamp(event.at, { date: true });
  const item = typeof event.item_id === "string" && event.item_id ? event.item_id : "";
  return [at, kind, item].filter(Boolean).join(" · ");
}

function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

/// The item an event is about: its own id, or the item, binding or edge in
/// its payload.
export function workGraphEventItemId(event: WorkGraphWireEvent): string {
  const payload = recordOf(event.payload);
  const item = recordOf(payload?.item) as WorkGraphWireItem | null;
  const attention = recordOf(payload?.attention) as WorkGraphWireBinding | null;
  const edge = recordOf(payload?.edge) as WorkGraphWireEdge | null;
  const execution = recordOf(recordOf(payload?.execution_binding)?.work_ref);
  return textId(event.item_id) || textId(item?.id)
    || textId(attention?.work_ref?.item_id) || textId(execution?.item_id)
    || textId(edge?.to_id);
}

/// One activity row as a sentence about the work item, by title ("Freeze
/// the release branch was completed"). The item ID stays out of the sentence:
/// it is on the item. workGraphEventLine keeps the compact
/// "time · kind · id" form.
export function workGraphEventSentence(
  event: WorkGraphWireEvent,
  context: { titleOf: (itemId: string) => string | undefined; directory?: AgentDirectory },
): string {
  const payload = recordOf(event.payload);
  const item = recordOf(payload?.item) as WorkGraphWireItem | null;
  const attention = recordOf(payload?.attention) as WorkGraphWireBinding | null;
  const edge = recordOf(payload?.edge) as WorkGraphWireEdge | null;
  const execution = recordOf(recordOf(payload?.execution_binding)?.work_ref);
  const itemId = textId(event.item_id) || textId(item?.id)
    || textId(attention?.work_ref?.item_id) || textId(execution?.item_id);
  const titleOf = (id: string) => (id ? context.titleOf(id) || "" : "");
  const named = titleOf(itemId) || textId(item?.title).trim();
  const subject = named || "A work item";
  const object = named || "a work item";
  // The agent an attention event is about, by name, when the console knows it.
  const agentOnGoal = bindingAgentName(attention, context.directory);
  const kind = textId(event.kind).trim().toLowerCase().replace(/^item_/, "");
  switch (kind) {
    case "created":
      return `${subject} was created`;
    case "updated":
      return `${subject} was updated`;
    case "ready":
    case "readiness_observed":
      return `${subject} is ready to start`;
    case "claimed": {
      const owner = item ? workGraphItemOwner(item, context.directory) : null;
      return owner ? `${owner.name} claimed ${object}` : `${subject} was claimed`;
    }
    case "released":
      return `${subject} was released`;
    case "blocked":
      return `${subject} became blocked`;
    case "closed":
      if (item?.status === "completed") return `${subject} was completed`;
      if (item?.status === "failed") return `${subject} failed`;
      if (item?.status === "cancelled") return `${subject} was cancelled`;
      return `${subject} was closed`;
    case "linked": {
      const from = titleOf(textId(edge?.from_id)) || "A work item";
      const to = titleOf(textId(edge?.to_id)) || "another work item";
      if (edge?.kind === "parent") return `${from} was placed under ${to}`;
      if (edge?.kind === "blocks") return `${from} now blocks ${to}`;
      return `${from} was linked to ${to}`;
    }
    case "evidence_added":
      return `Evidence was added to ${object}`;
    // Attention events say which agent works toward a goal, never
    // "attention": the agent is put on it, the goal is paused for it, the
    // agent is taken off it.
    case "attention_created":
      return `${agentOnGoal ?? "An agent"} was put on ${object}`;
    case "attention_paused":
      return `${subject} was paused for ${agentOnGoal ?? "its agent"}`;
    case "attention_updated": {
      const state = attention?.status?.state;
      if (state === "paused") return `${subject} was paused for ${agentOnGoal ?? "its agent"}`;
      if (state === "superseded" || state === "stopped") return `${agentOnGoal ?? "An agent"} was taken off ${object}`;
      return `${agentOnGoal ? `${agentOnGoal}'s` : "An agent's"} part in ${object} changed`;
    }
    case "execution_bound":
      return `Work on ${object} started`;
    case "execution_transitioned":
      return `Work on ${object} was updated`;
    case "goal_status_reported":
      return `Progress was reported on ${object}`;
    default: {
      const words = humanizeEnum(kind);
      return words ? `${words}: ${object}` : `Activity on ${object}`;
    }
  }
}
