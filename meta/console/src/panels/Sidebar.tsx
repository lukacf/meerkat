import { ApprovalAttention } from "../../../packages/console-components/src/conversation/approval-card";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import type { PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import React from "react";
import {
  DEFAULT_CONSOLE_SIDEBAR_VIEW,
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SIDEBAR_AGENT_ORDER_STORAGE_PREFIX,
  SIDEBAR_PINS_STORAGE_PREFIX,
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
  SIDEBAR_VIEW_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
  applyConsoleNavigationReorderIntent,
  applyConsoleSidebarOrder as applySidebarOrder,
  capConsoleSidebarAgentOrder,
  listConsoleSidebarAgentOrder,
  moveConsoleSidebarOrderKey,
  normalizeConsoleNavigationModel,
  normalizeConsoleSidebarGroupBy,
  normalizeConsoleSidebarSortBy,
  pruneStaleSidebarStorage,
  readConsoleSidebarView,
  readSidebarStringList,
  readSidebarStringSet,
  safeConsoleHref,
  sidebarStorageKey,
  writeConsoleSidebarView,
  writeSidebarStringList,
  writeSidebarStringSet,
  type ConsoleNavigationModel,
  type ConsoleNavigationNode,
  type ConsoleSidebarDropPosition,
  type ConsoleSidebarSortBy,
  type ConsoleSidebarStorageLike,
  type ConsoleSidebarViewPreferences,
} from "@console-core";
import type {
  ConsoleAgent,
  ConsoleAgentListConfig,
  ConsoleFrame,
  ConsoleSidebarButtonConfig,
} from "../types";
import { Icon } from "../icon";
import { isAgentPinned, operatorPanelPath, sidebarAgentPinId } from "../lib/adapters";
import { agentKindLabel } from "../lib/agents";
import { countRender } from "../lib/render-counts";

export { sidebarAgentPinId } from "../lib/adapters";
export {
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SIDEBAR_PINS_STORAGE_PREFIX,
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
  pruneStaleSidebarStorage,
  readSidebarStringList,
  readSidebarStringSet,
  sidebarStorageKey,
  writeSidebarStringList,
  writeSidebarStringSet,
};

export type NavKind = "topology" | "timeline" | "gating" | "roster" | "routing" | "logs" | "health" | "access" | "memory" | "workgraph";

interface SidebarProps {
  agents: ConsoleAgent[];
  selectedMemberId: string;
  recentActivity: ConsoleFrame[];
  collapsed: boolean;
  visibleControls?: NavKind[];
  customButtons?: ConsoleSidebarButtonConfig[];
  customPanels?: readonly import("@console-core").ConsolePanelDefinition[];
  onOpenCustomPanel?: (id: string) => void;
  grouping?: ConsoleAgentListConfig;
  storageNamespace?: string;
  /**
   * The principal-aware scope of the per-user preferences: the manual agent
   * order and the roster view. Unlike `storageNamespace`, it does not change
   * when the agent-list configuration does. Defaults to `storageNamespace`.
   */
  preferenceScope?: string;
  pinnedAgentIds?: Set<string>;
  approvals?: PendingApprovalSnapshot;
  onOpenApproval?: (pendingId?: string) => void;
  onSelect: (agent: ConsoleAgent) => void;
  onTogglePinnedAgent?: (agent: ConsoleAgent, familyPinIds?: Set<string>) => void;
  onOpenControl: (kind: NavKind) => void;
  /** Opens a configured operator panel button in the dock. */
  onOpenOperatorPanel?: (buttonId: string) => void;
}

/// Nav-item visibility configuration via URL query params:
///
///   ?hide_nav=timeline,gating          ⇒ hide listed kinds (others visible)
///   ?show_nav=topology,roster          ⇒ show only listed kinds
///   ?show_nav=topology&hide_nav=...    ⇒ whitelist wins; hide_nav ignored
///
/// Embedders can drop the query string into the iframe URL without
/// touching the React tree. Reads once on mount; reload to change.
export const ALL_NAV: NavKind[] = ["topology", "timeline", "gating", "roster", "routing", "logs", "health", "access", "memory", "workgraph"];
const NAV_LABEL: Record<NavKind, string> = {
  topology: "Topology",
  timeline: "Today",
  gating: "Approvals",
  roster: "Roster",
  routing: "Routing",
  logs: "Logs",
  health: "Health",
  access: "Access",
  memory: "Memory",
  workgraph: "WorkGraph",
};

export function normalizeNavKind(value: unknown): NavKind | null {
  return typeof value === "string" && ALL_NAV.includes(value as NavKind)
    ? value as NavKind
    : null;
}

function parseNavList(raw: string | null): Set<NavKind> {
  const out = new Set<NavKind>();
  if (!raw) return out;
  for (const token of raw.split(",")) {
    const trimmed = token.trim();
    if (ALL_NAV.includes(trimmed as NavKind)) out.add(trimmed as NavKind);
  }
  return out;
}

function visibleNavKinds(): NavKind[] {
  if (typeof window === "undefined") return ALL_NAV;
  const params = new URLSearchParams(window.location.search);
  const show = parseNavList(params.get("show_nav"));
  if (show.size > 0) return ALL_NAV.filter((k) => show.has(k));
  const hide = parseNavList(params.get("hide_nav"));
  if (hide.size > 0) return ALL_NAV.filter((k) => !hide.has(k));
  return ALL_NAV;
}

type Bucket = "Personal" | "Coordinators" | "Domains" | "Internal" | "Other";
interface AgentRow {
  agent: ConsoleAgent;
  childOfHost: boolean;
  depth: number;
  parentMemberId?: string | null;
  subgroup?: string | null;
}

type SidebarVirtualRow =
  | {
    kind: "section";
    key: string;
    bucket: string;
    count: number;
    collapsed: boolean;
    pinned?: boolean;
    reorderable: boolean;
    /** The key its collapse state is kept under, when not its name. */
    collapseKey?: string;
  }
  | { kind: "empty"; key: string; bucket: string; sectionConfig: ReturnType<typeof sectionConfigFor> }
  | { kind: "subgroup"; key: string; bucket: string; label: string; count: number; collapsed: boolean; storageKey: string; reorderable: boolean }
  /** `container` names the row's siblings: rows that share it can trade places. */
  | { kind: "agent"; key: string; bucket: string; row: AgentRow; container: string };

type SidebarNavigationModel = ConsoleNavigationModel<SidebarVirtualRow>;
type SidebarNavigationNode = ConsoleNavigationNode<SidebarVirtualRow>;

type SidebarOrderKind = "section" | "subgroup" | "agent";

/** What a reorder moves: a section, a subgroup within `bucket`, or an agent among the rows of `container`. */
interface SidebarOrderItem {
  kind: SidebarOrderKind;
  id: string;
  bucket?: string;
  container?: string;
}

interface SidebarDragOver {
  kind: SidebarOrderKind;
  id: string;
  where: SidebarDropPosition;
  /** The row that draws the drop line: after an agent, its last visible descendant. */
  indicatorId: string;
}

interface SidebarDragPreview {
  x: number;
  y: number;
  width: number;
}

type PendingOrderFocus = SidebarOrderItem;

/**
 * How agents are ordered among their siblings: the stored manual order, by
 * name, or by last activity. `rank` holds each agent's place in the manual
 * order; `lastActivity` answers an agent's newest activity time, if known.
 */
interface SidebarSortContext {
  sortBy: ConsoleSidebarSortBy;
  rank: ReadonlyMap<string, number>;
  lastActivity: (agent: ConsoleAgent) => number | undefined;
}

const SIDEBAR_ROW_HEIGHT = {
  section: 36,
  empty: 58,
  subgroup: 28,
  agent: 72,
} as const;

const SIDEBAR_OVERSCAN_PX = 360;
const PINNED_SECTION_NAME = "Pinned";
const PINNED_SECTION_KEY = "section:__meta_pinned";
/** The roster's one section when it is not grouped by category. */
const FLAT_SECTION_NAME = "All agents";
const FLAT_SECTION_KEY = "section:__meta_all_agents";
/** A drag within this distance of the list's top or bottom edge scrolls the list. */
const SIDEBAR_DRAG_EDGE_PX = 36;
const SIDEBAR_DRAG_EDGE_MAX_STEP_PX = 14;
const AGENT_NAME_COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
const EMPTY_ACTIVITY: ReadonlyMap<string, number> = new Map();

function localSidebarStorage(): ConsoleSidebarStorageLike | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/**
 * The user's pinned agents: the per-user key; else, once, the pins kept
 * under the agent-list configuration namespace before pins moved, copied to
 * the per-user key; else the configured defaults.
 */
export function readSidebarPins(
  storage: ConsoleSidebarStorageLike | null | undefined,
  userKey: string,
  configurationKey: string,
  defaults: Iterable<string>,
): Set<string> {
  const stored = readSidebarStringSet(storage, userKey);
  if (stored) return stored;
  const earlier = readSidebarStringSet(storage, configurationKey);
  if (!earlier) return new Set(defaults);
  writeSidebarStringSet(storage, userKey, earlier);
  return earlier;
}

function isWorkerish(a: ConsoleAgent): boolean {
  const haystack = [a.label, a.identity, a.member_id, a.role].filter(Boolean).join(" ").toLowerCase();
  return (
    haystack.includes("worker")
    || haystack.includes("delegate")
    || haystack.includes("helper")
  );
}

function isCommanderLike(a: ConsoleAgent): boolean {
  if (isWorkerish(a)) return false;
  const haystack = [a.label, a.identity, a.member_id, a.role].filter(Boolean).join(" ").toLowerCase();
  return haystack.includes("commander") || haystack.includes("coordinator");
}

function agentKeys(a: ConsoleAgent | null): string[] {
  return [a?.identity, a?.member_id, a?.agent_id]
    .filter((value): value is string => Boolean(value))
    .map((value) => value.toLowerCase());
}

function referenceMatchesAgentKey(reference: string, key: string): boolean {
  const normalizedReference = reference.toLowerCase();
  const normalizedKey = key.toLowerCase();
  if (normalizedReference === normalizedKey) return true;
  const compactReference = normalizedReference.replace(/[^a-z0-9]+/g, "");
  const compactKey = normalizedKey.replace(/[^a-z0-9]+/g, "");
  if (compactKey && compactReference === compactKey) return true;
  const tokens = normalizedReference
    .split(/[/:#\s]+/)
    .filter(Boolean);
  if (tokens.includes(normalizedKey)) return true;
  if (!compactKey) return false;
  for (let start = 0; start < tokens.length; start++) {
    let compactSlice = "";
    for (let end = start; end < tokens.length; end++) {
      compactSlice += tokens[end].replace(/[^a-z0-9]+/g, "");
      if (compactSlice === compactKey) return true;
      if (compactSlice.length > compactKey.length) break;
    }
  }
  return false;
}

function isWiredTo(a: ConsoleAgent, host: ConsoleAgent | null): boolean {
  if (!host) return false;
  const wiredTo = a.wired_to || [];
  return agentKeys(host).some((key) =>
    wiredTo.some((peer) => referenceMatchesAgentKey(peer, key)),
  );
}

function isSpawnedDelegateLike(a: ConsoleAgent, host: ConsoleAgent | null): boolean {
  if (!isWorkerish(a)) return false;
  if (isWiredTo(a, host)) return true;
  if (a.labels?.group?.trim() || a.labels?.console_group?.trim()) return false;

  const role = (a.role || "").toLowerCase();
  const group = (a.group || "").toLowerCase();
  return (
    !group
    || group === role
    || group === "worker"
    || group === "delegate"
    || group.includes("helper")
  );
}

function explicitHostId(a: ConsoleAgent): string | null {
  return a.labels?.delegate_host_identity
    || a.labels?.host_identity
    || a.labels?.parent_identity
    || a.labels?.spawned_by
    || null;
}

function findSpawnHost(a: ConsoleAgent, agents: ConsoleAgent[], commander: ConsoleAgent | null): ConsoleAgent | null {
  // A side chat lists under the agent it branched from, as the runtime's
  // roster records the fork.
  const sideChatOf = a.side_chat_of;
  if (sideChatOf) {
    const match = agents.find((candidate) =>
      candidate.member_id !== a.member_id
        && agentKeys(candidate).some((key) => referenceMatchesAgentKey(sideChatOf, key)),
    );
    if (match) return match;
  }
  const spawnedBy = a.labels?.spawned_by;
  if (spawnedBy) {
    const match = agents.find((candidate) =>
      candidate.member_id !== a.member_id
        && agentKeys(candidate).some((key) => referenceMatchesAgentKey(spawnedBy, key)),
    );
    if (match) return match;
  }
  if (!isWorkerish(a)) return null;
  const explicitHost = explicitHostId(a);
  if (explicitHost) {
    const match = agents.find((candidate) =>
      candidate.member_id !== a.member_id
        && agentKeys(candidate).some((key) => referenceMatchesAgentKey(explicitHost, key)),
    );
    if (match) return match;
  }

  const commanderHost = agents.find((candidate) =>
    candidate.member_id !== a.member_id && isCommanderLike(candidate) && isWiredTo(a, candidate),
  );
  if (commanderHost) return commanderHost;

  const wiredNonWorkerHost = agents.find((candidate) =>
    candidate.member_id !== a.member_id && !isWorkerish(candidate) && isWiredTo(a, candidate),
  );
  if (wiredNonWorkerHost) return wiredNonWorkerHost;

  const workerHost = agents.find((candidate) =>
    candidate.member_id !== a.member_id && isWorkerish(candidate) && isWiredTo(a, candidate),
  );
  if (workerHost) return workerHost;

  if (commander && commander.member_id !== a.member_id && isSpawnedDelegateLike(a, commander)) return commander;
  return null;
}

export const __sidebarTest = {
  isCommanderLike,
  isSpawnedDelegateLike,
  findSpawnHost,
  groupSidebarAgents,
  orderedSectionNames,
  configuredAgentGroup,
  configuredAgentSubgroup,
  configuredAgentBadges,
  sidebarAgentPinId,
  sidebarStorageKey,
  readSidebarStringSet,
  writeSidebarStringSet,
  readSidebarStringList,
  writeSidebarStringList,
  collapsedSectionsForStorage,
  collapsedSubgroupsForStorage,
  sidebarSubgroupStorageId,
  buildSidebarVirtualRows,
  buildStockSidebarNavigationModel,
  sidebarNavigationRows,
  sidebarNavigationModelFromRows,
  sidebarDragPreviewRows,
  sidebarVirtualOffsets,
  sidebarVisibleRange,
  applySidebarOrder,
  reorderSidebarOrder: reorderSidebarOrderWithNavigationModel,
  isAgentPinned,
  sidebarPinnedFamilyPinIds,
  pruneStaleSidebarStorage,
  safeConsoleHref,
  deriveStateAttr,
  sidebarAgentOrderKey,
  sidebarAgentContainer,
  sidebarAgentOrderAfterMove,
  sidebarStoredOrderKeys,
  sidebarCategorySequence,
  agentSubtreeEnd,
  compareAgentNames,
  compareBySidebarSort,
  newestFrameTimes,
  agentLastActivity,
  agentStatusText,
  agentDetailText,
  relativeActivityText,
  FLAT_SECTION_NAME,
  FLAT_SECTION_KEY,
  SIDEBAR_AGENT_ORDER_STORAGE_PREFIX,
  SIDEBAR_VIEW_STORAGE_PREFIX,
  SIDEBAR_PINS_STORAGE_PREFIX,
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
};

export function sidebarPinnedFamilyPinIds(agent: ConsoleAgent, agents: ConsoleAgent[]): Set<string> {
  const host = agents.find(isCommanderLike);
  const byId = new Map(agents.map((candidate) => [candidate.member_id, candidate]));
  const childrenById = new Map<string, ConsoleAgent[]>();
  for (const candidate of agents) {
    const parent = findSpawnHost(candidate, agents, host || null);
    if (!parent) continue;
    if (!childrenById.has(parent.member_id)) childrenById.set(parent.member_id, []);
    childrenById.get(parent.member_id)!.push(candidate);
  }

  const ids = new Set<string>();
  const visit = (current: ConsoleAgent | undefined): void => {
    if (!current || ids.has(current.member_id)) return;
    ids.add(sidebarAgentPinId(current));
    ids.add(current.member_id);
    for (const child of childrenById.get(current.member_id) || []) visit(byId.get(child.member_id));
  };
  visit(agent);
  return ids;
}

function bucketOf(a: ConsoleAgent): Bucket {
  const g = (a.group || "").toLowerCase();
  const p = (a.role || a.kind || "").toLowerCase();
  if (g.includes("coordinator") || p.includes("coord") || p.includes("triage") || p.includes("router") || p.includes("commander")) return "Coordinators";
  if (g.includes("personal") || p.includes("personal") || p.includes("identity") || p.includes("lead")) return "Personal";
  if (g.includes("internal") || p.includes("gate") || p.includes("monitor") || p.includes("scribe")) return "Internal";
  if (g.includes("domain") || g.includes("responder") || g.includes("communication") || g.includes("specialist")) return "Domains";
  return "Domains";
}

const SECTION_ORDER: Bucket[] = ["Personal", "Coordinators", "Domains", "Internal", "Other"];

function configuredSelectors(config: ConsoleAgentListConfig | undefined, key: "group_by" | "subgroup_by"): string[] {
  return (config?.[key] || []).map((value) => value.trim()).filter(Boolean);
}

function configuredFieldValue(agent: ConsoleAgent, selector: string): string | null {
  const normalized = selector.trim();
  if (!normalized) return null;
  if (normalized.startsWith("labels.")) {
    const key = normalized.slice("labels.".length);
    return agent.labels?.[key]?.trim() || null;
  }
  if (normalized.startsWith("label:")) {
    const key = normalized.slice("label:".length);
    return agent.labels?.[key]?.trim() || null;
  }
  switch (normalized) {
    case "group": return agent.group?.trim() || null;
    case "subgroup": return agent.subgroup?.trim() || null;
    case "role": return agent.role?.trim() || null;
    case "kind": return agent.kind?.trim() || null;
    case "identity": return agent.identity?.trim() || null;
    case "member_id": return agent.member_id?.trim() || null;
    case "agent_id": return agent.agent_id?.trim() || null;
    default:
      return agent.labels?.[normalized]?.trim() || null;
  }
}

function firstConfiguredValue(agent: ConsoleAgent, selectors: string[]): string | null {
  for (const selector of selectors) {
    const value = configuredFieldValue(agent, selector);
    if (value) return value;
  }
  return null;
}

function configuredAgentGroup(
  agent: ConsoleAgent,
  config: ConsoleAgentListConfig | undefined,
  parentById?: Map<string, string>,
  byId?: Map<string, ConsoleAgent>,
): string | null {
  const selectors = configuredSelectors(config, "group_by");
  if (selectors.length === 0) return null;
  const chain: ConsoleAgent[] = [];
  let current: ConsoleAgent | undefined = agent;
  const seen = new Set<string>();
  while (current && !seen.has(current.member_id)) {
    seen.add(current.member_id);
    chain.push(current);
    if (!parentById || !byId) break;
    const parentId = parentById.get(current.member_id);
    if (!parentId) break;
    current = byId.get(parentId);
  }
  const searchOrder = chain.length > 1 ? [...chain].reverse() : chain;
  for (const candidate of searchOrder) {
    const value = firstConfiguredValue(candidate, selectors);
    if (value) return value;
  }
  return config?.fallback_group?.trim() || "Agents";
}

function configuredAgentSubgroup(
  agent: ConsoleAgent,
  config: ConsoleAgentListConfig | undefined,
  parentById?: Map<string, string>,
  byId?: Map<string, ConsoleAgent>,
): string | null {
  const selectors = configuredSelectors(config, "subgroup_by");
  if (selectors.length === 0) return null;
  const chain: ConsoleAgent[] = [];
  let current: ConsoleAgent | undefined = agent;
  const seen = new Set<string>();
  while (current && !seen.has(current.member_id)) {
    seen.add(current.member_id);
    chain.push(current);
    if (!parentById || !byId) break;
    const parentId = parentById.get(current.member_id);
    if (!parentId) break;
    current = byId.get(parentId);
  }
  const searchOrder = chain.length > 1 ? [...chain].reverse() : chain;
  for (const candidate of searchOrder) {
    const value = firstConfiguredValue(candidate, selectors);
    if (value) return value;
  }
  return config?.fallback_subgroup?.trim() || null;
}

function configuredAgentBadges(agent: ConsoleAgent, config: ConsoleAgentListConfig | undefined): Array<{
  id: string;
  label: string;
  value: string;
  tone?: string;
}> {
  return (config?.badges || [])
    .map((badge) => {
      const value = configuredFieldValue(agent, badge.field || "");
      if (!badge.id || !badge.label || !value) return null;
      return {
        id: badge.id,
        label: badge.label,
        value,
        tone: badge.tone,
      };
    })
    .filter((badge): badge is { id: string; label: string; value: string; tone?: string } => Boolean(badge));
}

function bucketForAgent(a: ConsoleAgent, parentById: Map<string, string>, byId: Map<string, ConsoleAgent>): Bucket {
  const seen = new Set<string>();
  let current: ConsoleAgent | undefined = a;
  while (current) {
    if (seen.has(current.member_id)) break;
    seen.add(current.member_id);
    const parentId = parentById.get(current.member_id);
    if (!parentId) break;
    current = byId.get(parentId);
  }
  return bucketOf(current || a);
}

function depthForAgent(a: ConsoleAgent, parentById: Map<string, string>): number {
  const seen = new Set<string>();
  let depth = 0;
  let current = a.member_id;
  while (parentById.has(current) && !seen.has(current)) {
    seen.add(current);
    depth += 1;
    current = parentById.get(current)!;
  }
  return depth;
}

function compareRows(
  host: ConsoleAgent | null,
  orderSubgroups = false,
  order?: SidebarSortContext,
  defaultRank?: ReadonlyMap<string, number>,
) {
  return (a: AgentRow, b: AgentRow): number => {
    if (orderSubgroups && a.subgroup !== b.subgroup) {
      if (!a.subgroup) return 1;
      if (!b.subgroup) return -1;
      return a.subgroup.localeCompare(b.subgroup);
    }
    const chosen = order ? compareBySidebarSort(order, a.agent, b.agent) : 0;
    if (chosen !== 0) return chosen;
    if (defaultRank) {
      const byDefault = (defaultRank.get(a.agent.member_id) ?? Number.MAX_SAFE_INTEGER)
        - (defaultRank.get(b.agent.member_id) ?? Number.MAX_SAFE_INTEGER);
      if (byDefault !== 0) return byDefault;
    }
    if (host) {
      if (a.agent.member_id === host.member_id) return -1;
      if (b.agent.member_id === host.member_id) return 1;
    }
    if (a.childOfHost !== b.childOfHost) return a.childOfHost ? -1 : 1;
    return a.agent.label.localeCompare(b.agent.label);
  };
}

/** The key a manual order (like a pin) stores for an agent: its durable identity first. */
function sidebarAgentOrderKey(agent: ConsoleAgent): string {
  return sidebarAgentPinId(agent);
}

function compareAgentNames(a: ConsoleAgent, b: ConsoleAgent): number {
  const byName = AGENT_NAME_COLLATOR.compare(a.label, b.label);
  if (byName !== 0) return byName;
  const aKey = sidebarAgentOrderKey(a);
  const bKey = sidebarAgentOrderKey(b);
  return aKey < bKey ? -1 : aKey > bKey ? 1 : 0;
}

function manualRank(order: SidebarSortContext, agent: ConsoleAgent): number | undefined {
  return order.rank.get(sidebarAgentOrderKey(agent)) ?? order.rank.get(agent.member_id);
}

/**
 * Compares two sibling agents under the chosen sort. Manual: agents in the
 * stored order come first, in that order, and two agents outside it compare
 * equal so the default rule places them (with nothing stored the roster
 * keeps its default order). Name: numeric, case-insensitive collation, then
 * the agent key. Last activity: newest first, unknown last, then by name.
 */
function compareBySidebarSort(order: SidebarSortContext, a: ConsoleAgent, b: ConsoleAgent): number {
  if (order.sortBy === "name") return compareAgentNames(a, b);
  if (order.sortBy === "activity") {
    const aAt = order.lastActivity(a);
    const bAt = order.lastActivity(b);
    if (aAt !== bAt) {
      if (aAt === undefined) return 1;
      if (bAt === undefined) return -1;
      return bAt - aAt;
    }
    return compareAgentNames(a, b);
  }
  const aRank = manualRank(order, a);
  const bRank = manualRank(order, b);
  if (aRank === bRank) return 0;
  if (aRank === undefined) return 1;
  if (bRank === undefined) return -1;
  return aRank - bRank;
}

/**
 * Frame sources that record an agent acting. History the console rebuilds
 * from a session and frames it synthesizes are stamped when it observes
 * them, not when the agent acted.
 */
const ACTING_FRAME_SOURCES: ReadonlySet<string> = new Set(["console_event", "send", "tool_application"]);
/** Live event kinds that record something done to an agent, not the agent acting. */
const NON_ACTING_FRAME_EVENTS: ReadonlySet<string> = new Set([
  "identity_respawned",
  "identity_retired",
  "identity_reset",
  "identity_deleted",
  "interaction_failed",
]);

/**
 * Whether a frame records its agent acting: the rule the server's
 * `last_activity_ms` follows. A frame that names no source still counts.
 */
function isActingFrame(frame: ConsoleFrame): boolean {
  if (frame.sourceKind !== undefined && !ACTING_FRAME_SOURCES.has(frame.sourceKind)) return false;
  return !NON_ACTING_FRAME_EVENTS.has(frame.event) && !frame.event.startsWith("memory.");
}

/** The newest time this console saw each identity act. */
function newestFrameTimes(frames: readonly ConsoleFrame[]): Map<string, number> {
  const newest = new Map<string, number>();
  for (const frame of frames) {
    const identity = frame.identity;
    const at = frame.timestampMs;
    if (!identity || typeof at !== "number" || !Number.isFinite(at) || !isActingFrame(frame)) continue;
    const known = newest.get(identity);
    if (known === undefined || at > known) newest.set(identity, at);
  }
  return newest;
}

/**
 * An agent's newest known activity: the roster's `last_activity_ms`, or a
 * newer frame this console saw since the roster was read.
 */
function agentLastActivity(agent: ConsoleAgent, frameTimes: ReadonlyMap<string, number>): number | undefined {
  const fromFrames = frameTimes.get(agent.identity || agent.member_id);
  const fromRoster = agent.last_activity_ms;
  if (fromRoster === undefined) return fromFrames;
  if (fromFrames === undefined) return fromRoster;
  return Math.max(fromRoster, fromFrames);
}

function relativeActivityText(elapsedMs: number): string {
  const seconds = Math.max(0, Math.floor(elapsedMs / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}

/** What the agent is doing, in words, or when it was last active. */
function agentStatusText(agent: ConsoleAgent, lastActivityMs: number | undefined, nowMs: number): string {
  const state = (agent.state || "").toLowerCase();
  if (state === "retired" || state === "stopped") return "retired";
  if (state === "retiring") return "retiring";
  if (state === "needs_repair") return "needs repair";
  if (state === "broken") return "broken";
  if (agent.labels?.console_degraded === "true" || state.includes("degrade")) return "degraded";
  if (agent.response_phase === "generating") return "working";
  if (agent.response_phase === "tool-executing") return "using a tool";
  if (agent.response_phase === "waiting") return "waiting";
  if (lastActivityMs !== undefined) return `active ${relativeActivityText(nowMs - lastActivityMs)}`;
  if (agent.response_phase === null) return "idle";
  return "";
}

/**
 * The row's second line, for people: what the agent is (its role, else its
 * group) and its status in words or how long ago it was last active. The
 * identity is not shown here; the row's tooltip and copy action carry it.
 */
function agentDetailParts(
  agent: ConsoleAgent,
  lastActivityMs: number | undefined,
  nowMs: number,
): { kind: string; status: string } {
  const kind = agentKindLabel(agent);
  const status = agentStatusText(agent, lastActivityMs, nowMs);
  return kind ? { kind, status } : { kind: "", status: status ? status[0]!.toUpperCase() + status.slice(1) : "" };
}

function agentDetailText(agent: ConsoleAgent, lastActivityMs: number | undefined, nowMs: number): string {
  const { kind, status } = agentDetailParts(agent, lastActivityMs, nowMs);
  return [kind, status].filter(Boolean).join(" · ");
}

function orderRowsPreorder(
  rows: AgentRow[],
  parentById: Map<string, string>,
  host: ConsoleAgent | null,
  orderSubgroups = false,
  order?: SidebarSortContext,
  defaultRank?: ReadonlyMap<string, number>,
): AgentRow[] {
  const byParent = new Map<string, AgentRow[]>();
  const rowById = new Map(rows.map((row) => [row.agent.member_id, row]));
  const roots: AgentRow[] = [];
  for (const row of rows) {
    const parentId = parentById.get(row.agent.member_id);
    if (parentId && rowById.has(parentId)) {
      if (!byParent.has(parentId)) byParent.set(parentId, []);
      byParent.get(parentId)!.push(row);
    } else {
      roots.push(row);
    }
  }
  const sortRows = compareRows(host, orderSubgroups, order, defaultRank);
  roots.sort(sortRows);
  for (const children of byParent.values()) children.sort(sortRows);

  const ordered: AgentRow[] = [];
  const visit = (row: AgentRow): void => {
    ordered.push(row);
    for (const child of byParent.get(row.agent.member_id) || []) visit(child);
  };
  for (const root of roots) visit(root);
  return ordered;
}

function orderRowsPreorderByIndex(
  rows: AgentRow[],
  orderIndex: Map<string, number>,
  order?: SidebarSortContext,
): AgentRow[] {
  const byParent = new Map<string, AgentRow[]>();
  const rowById = new Map(rows.map((row) => [row.agent.member_id, row]));
  const roots: AgentRow[] = [];
  for (const row of rows) {
    const parentId = row.parentMemberId || undefined;
    if (parentId && rowById.has(parentId)) {
      if (!byParent.has(parentId)) byParent.set(parentId, []);
      byParent.get(parentId)!.push(row);
    } else {
      roots.push(row);
    }
  }
  const sortByExistingOrder = (a: AgentRow, b: AgentRow) => {
    const chosen = order ? compareBySidebarSort(order, a.agent, b.agent) : 0;
    if (chosen !== 0) return chosen;
    return (orderIndex.get(a.agent.member_id) ?? Number.MAX_SAFE_INTEGER)
      - (orderIndex.get(b.agent.member_id) ?? Number.MAX_SAFE_INTEGER);
  };
  roots.sort(sortByExistingOrder);
  for (const children of byParent.values()) children.sort(sortByExistingOrder);

  const ordered: AgentRow[] = [];
  const visit = (row: AgentRow): void => {
    ordered.push(row);
    for (const child of byParent.get(row.agent.member_id) || []) visit(child);
  };
  for (const root of roots) visit(root);
  return ordered;
}

interface SidebarGroupingOptions {
  /** One list instead of categories: every agent in one section, no subgroups. */
  flat?: boolean;
  order?: SidebarSortContext;
  /** The stored category and subgroup orders, which the flat list's default follows. */
  sectionOrder?: string[];
  subgroupOrder?: string[];
}

/** The roster's agents placed in their sections and under their parents, not yet ordered. */
interface SidebarClassification {
  rowsByBucket: Map<string, AgentRow[]>;
  parentById: Map<string, string>;
  host: ConsoleAgent | null;
  orderSubgroups: boolean;
}

function classifySidebarAgents(
  filtered: ConsoleAgent[],
  config?: ConsoleAgentListConfig,
): SidebarClassification {
  const rowsByBucket = new Map<string, AgentRow[]>();
  const host = filtered.find(isCommanderLike) || null;
  const byId = new Map(filtered.map((a) => [a.member_id, a]));
  const parentById = new Map<string, string>();
  for (const a of filtered) {
    const parent = findSpawnHost(a, filtered, host);
    if (parent) parentById.set(a.member_id, parent.member_id);
  }
  for (const a of filtered) {
    const childOfHost = parentById.has(a.member_id);
    const key = configuredAgentGroup(a, config, parentById, byId) || bucketForAgent(a, parentById, byId);
    const subgroup = configuredAgentSubgroup(a, config, parentById, byId);
    if (!rowsByBucket.has(key)) rowsByBucket.set(key, []);
    rowsByBucket.get(key)!.push({
      agent: a,
      childOfHost,
      depth: depthForAgent(a, parentById),
      parentMemberId: parentById.get(a.member_id) || null,
      subgroup,
    });
  }
  return {
    rowsByBucket,
    parentById,
    host,
    orderSubgroups: configuredSelectors(config, "subgroup_by").length > 0,
  };
}

/** The flat list's classification: every agent in one section, without subgroups, nested as before. */
function flattenSidebarClassification(classified: SidebarClassification): SidebarClassification {
  const rows = Array.from(classified.rowsByBucket.values()).flat().map((row) => ({ ...row, subgroup: null }));
  return {
    rowsByBucket: rows.length > 0 ? new Map([[FLAT_SECTION_NAME, rows]]) : new Map(),
    parentById: classified.parentById,
    host: classified.host,
    orderSubgroups: false,
  };
}

/**
 * Orders each section's rows as a preorder tree under the chosen sort. Kept
 * apart from the classification, which walks the whole roster for every
 * agent, so a re-sort (a new last activity, a manual move) stays cheap.
 */
function orderSidebarGroups(
  classified: SidebarClassification,
  order?: SidebarSortContext,
  defaultRank?: ReadonlyMap<string, number>,
): Map<string, AgentRow[]> {
  const grouped = new Map<string, AgentRow[]>();
  for (const [key, rows] of classified.rowsByBucket.entries()) {
    grouped.set(
      key,
      orderRowsPreorder(rows, classified.parentById, classified.host, classified.orderSubgroups, order, defaultRank),
    );
  }
  return grouped;
}

/**
 * The category view's agents in the order it lists them: its sections in
 * their order, each section's subgroups in theirs, a pinned agent at its
 * place in its category. With nothing stored this is the flat list's order
 * too, and a move lists every agent in it first, so either view's first
 * move stores the same list and the other view keeps its order apart from
 * the moved agent.
 */
function sidebarCategorySequence(
  grouped: Map<string, AgentRow[]>,
  config?: ConsoleAgentListConfig,
  sectionOrder?: string[],
  subgroupOrder?: string[],
): AgentRow[] {
  return applySidebarOrder(orderedSectionNames(grouped, config), sectionOrder)
    .flatMap((bucket) => orderRowsBySubgroupOrder(grouped.get(bucket) || [], bucket, subgroupOrder));
}

/** The flat list, ordered by the chosen sort and, where it leaves two agents equal, by the category order. */
function orderFlatSidebarGroup(
  classified: SidebarClassification,
  categorySequence: AgentRow[],
  order?: SidebarSortContext,
): Map<string, AgentRow[]> {
  const defaultRank = new Map(categorySequence.map((row, index) => [row.agent.member_id, index] as const));
  return orderSidebarGroups(flattenSidebarClassification(classified), order, defaultRank);
}

function groupSidebarAgents(
  filtered: ConsoleAgent[],
  config?: ConsoleAgentListConfig,
  options: SidebarGroupingOptions = {},
): Map<string, AgentRow[]> {
  const classified = classifySidebarAgents(filtered, config);
  const grouped = orderSidebarGroups(classified, options.order);
  if (options.flat !== true) return grouped;
  const sequence = sidebarCategorySequence(grouped, config, options.sectionOrder, options.subgroupOrder);
  return orderFlatSidebarGroup(classified, sequence, options.order);
}

function orderedSectionNames(grouped: Map<string, AgentRow[]>, config?: ConsoleAgentListConfig): string[] {
  const names = Array.from(new Set([
    ...Array.from(grouped.keys()),
    ...(config?.sections || []).map((section) => section.name).filter(Boolean),
  ]));
  const configuredOrder = (config?.section_order || []).map((value) => value.trim()).filter(Boolean);
  const order = configuredOrder.length > 0 ? configuredOrder : SECTION_ORDER;
  const rank = new Map(order.map((name, index) => [name.toLowerCase(), index] as const));
  return names.sort((a, b) => {
    const ar = rank.get(a.toLowerCase()) ?? Number.MAX_SAFE_INTEGER;
    const br = rank.get(b.toLowerCase()) ?? Number.MAX_SAFE_INTEGER;
    if (ar !== br) return ar - br;
    return a.localeCompare(b);
  });
}

function sectionConfigFor(name: string, config?: ConsoleAgentListConfig) {
  const needle = name.toLowerCase();
  return (config?.sections || []).find((section) => section.name?.toLowerCase() === needle) || null;
}

function defaultCollapsedSections(config?: ConsoleAgentListConfig): Set<string> {
  return new Set((config?.sections || [])
    .filter((section) => section.collapsed === true)
    .map((section) => section.name));
}

function collapsedSectionsForStorage(
  config: ConsoleAgentListConfig | undefined,
  storageKey: string,
  storage: ConsoleSidebarStorageLike | null | undefined = localSidebarStorage(),
): Set<string> {
  return readSidebarStringSet(storage, storageKey) ?? defaultCollapsedSections(config);
}

function collapsedSubgroupsForStorage(
  storageKey: string,
  storage: ConsoleSidebarStorageLike | null | undefined = localSidebarStorage(),
): Set<string> {
  return readSidebarStringSet(storage, storageKey) ?? new Set();
}

function sidebarSubgroupStorageId(bucket: string, subgroup: string): string {
  return JSON.stringify([bucket, subgroup]);
}

function sidebarSubgroupStorageLabel(storageKey: string): string {
  try {
    const parsed = JSON.parse(storageKey) as unknown;
    if (Array.isArray(parsed) && typeof parsed[1] === "string" && parsed[1].trim()) {
      return parsed[1];
    }
  } catch {
    // Fall through to the raw key when reading legacy or externally-written preferences.
  }
  return storageKey;
}

export type SidebarDropPosition = ConsoleSidebarDropPosition;

function reorderSidebarOrderWithNavigationModel(
  baseOrder: string[],
  draggedId: string,
  target: string,
  where: SidebarDropPosition,
  inputSource: "keyboard" | "pointer",
): string[] {
  const allowed = new Set(baseOrder);
  const result = applyConsoleNavigationReorderIntent({
    orientation: "vertical",
    nodes: baseOrder.map((id) => ({
      type: "item" as const,
      id,
      label: id,
    })),
    order: { orderedNodeIds: baseOrder },
  }, {
    id: draggedId,
    targetId: target,
    position: where,
    scope: "siblings",
    inputSource,
  });
  return result.model.order.orderedNodeIds.filter((id) => allowed.has(id));
}

/**
 * The stored order with each agent's key brought up to date: an agent
 * stored by its member id before it had an identity keeps that place under
 * its identity key, so listing it again does not move it to the end.
 */
function sidebarStoredOrderKeys(stored: readonly string[], agents: readonly ConsoleAgent[]): string[] {
  const storedKeys = new Set(stored);
  const renamed = new Map<string, string>();
  for (const agent of agents) {
    const key = sidebarAgentOrderKey(agent);
    if (key !== agent.member_id && storedKeys.has(agent.member_id) && !storedKeys.has(key)) {
      renamed.set(agent.member_id, key);
    }
  }
  return renamed.size === 0 ? [...stored] : stored.map((key) => renamed.get(key) ?? key);
}

/**
 * The stored manual order after moving `dragged` before or after `target`
 * among `siblings` (the keys of one container, in shown order). Every agent
 * is listed first, those not yet stored in the category order (`present`,
 * the keys of {@link sidebarCategorySequence}), so the first move fixes that
 * order for every agent in both views. Only the dragged key then moves, so
 * a move changes the order of only the pairs that include the moved agent,
 * in either view. `null` when nothing moves.
 */
function sidebarAgentOrderAfterMove(args: {
  stored: readonly string[];
  agents: readonly ConsoleAgent[];
  present: readonly string[];
  siblings: readonly string[];
  dragged: string;
  target: string;
  where: SidebarDropPosition;
  inputSource: "keyboard" | "pointer";
}): string[] | null {
  const siblings = [...args.siblings];
  const moved = reorderSidebarOrderWithNavigationModel(siblings, args.dragged, args.target, args.where, args.inputSource);
  if (moved.length !== siblings.length || moved.every((key, index) => key === siblings[index])) return null;
  const listed = listConsoleSidebarAgentOrder(sidebarStoredOrderKeys(args.stored, args.agents), args.present);
  return capConsoleSidebarAgentOrder(moveConsoleSidebarOrderKey(listed, args.dragged, moved), new Set(args.present));
}

function collectPinnedRows(rows: AgentRow[], pinnedAgentIds: Set<string> | undefined): Set<string> {
  const pinned = new Set<string>();
  if (!pinnedAgentIds || pinnedAgentIds.size === 0) return pinned;

  const rowById = new Map(rows.map((row) => [row.agent.member_id, row]));
  const childrenById = new Map<string, AgentRow[]>();
  for (const row of rows) {
    if (!row.parentMemberId) continue;
    if (!childrenById.has(row.parentMemberId)) childrenById.set(row.parentMemberId, []);
    childrenById.get(row.parentMemberId)!.push(row);
  }

  const includeAncestors = (row: AgentRow): void => {
    let current: AgentRow | undefined = row;
    const seen = new Set<string>();
    while (current && !seen.has(current.agent.member_id)) {
      seen.add(current.agent.member_id);
      pinned.add(current.agent.member_id);
      current = current.parentMemberId ? rowById.get(current.parentMemberId) : undefined;
    }
  };
  const includeDescendants = (row: AgentRow): void => {
    for (const child of childrenById.get(row.agent.member_id) || []) {
      if (pinned.has(child.agent.member_id)) continue;
      pinned.add(child.agent.member_id);
      includeDescendants(child);
    }
  };

  for (const row of rows) {
    if (!isAgentPinned(row.agent, pinnedAgentIds)) continue;
    includeAncestors(row);
    includeDescendants(row);
  }
  return pinned;
}

function orderRowsBySubgroupOrder(rows: AgentRow[], bucket: string, subgroupOrder: string[] | undefined): AgentRow[] {
  if (rows.length <= 1) return rows;
  const orderIndex = new Map(rows.map((row, index) => [row.agent.member_id, index] as const));
  const defaultSubgroups = rows
    .map((row) => row.subgroup)
    .filter((value): value is string => Boolean(value));
  const subgroupIds = applySidebarOrder(
    Array.from(new Set(defaultSubgroups)).map((subgroup) => sidebarSubgroupStorageId(bucket, subgroup)),
    subgroupOrder,
  );
  const subgroupRank = new Map(subgroupIds.map((id, index) => [id, index] as const));
  const byParent = new Map<string, AgentRow[]>();
  const rowById = new Map(rows.map((row) => [row.agent.member_id, row]));
  const roots: AgentRow[] = [];
  for (const row of rows) {
    const parentId = row.parentMemberId || undefined;
    if (parentId && rowById.has(parentId)) {
      if (!byParent.has(parentId)) byParent.set(parentId, []);
      byParent.get(parentId)!.push(row);
    } else {
      roots.push(row);
    }
  }
  const sortBySubgroup = (a: AgentRow, b: AgentRow) => {
    const ar = a.subgroup ? subgroupRank.get(sidebarSubgroupStorageId(bucket, a.subgroup)) ?? Number.MAX_SAFE_INTEGER : Number.MAX_SAFE_INTEGER;
    const br = b.subgroup ? subgroupRank.get(sidebarSubgroupStorageId(bucket, b.subgroup)) ?? Number.MAX_SAFE_INTEGER : Number.MAX_SAFE_INTEGER;
    if (ar !== br) return ar - br;
    return (orderIndex.get(a.agent.member_id) ?? 0) - (orderIndex.get(b.agent.member_id) ?? 0);
  };
  roots.sort(sortBySubgroup);
  for (const children of byParent.values()) children.sort((a, b) =>
    (orderIndex.get(a.agent.member_id) ?? 0) - (orderIndex.get(b.agent.member_id) ?? 0),
  );

  const ordered: AgentRow[] = [];
  const visit = (row: AgentRow): void => {
    ordered.push(row);
    for (const child of byParent.get(row.agent.member_id) || []) visit(child);
  };
  for (const root of roots) visit(root);
  return ordered;
}

function sidebarFamilyPinIdsByMemberId(grouped: Map<string, AgentRow[]>): Map<string, Set<string>> {
  const rows = Array.from(grouped.values()).flat();
  const rowById = new Map(rows.map((row) => [row.agent.member_id, row]));
  const childrenById = new Map<string, AgentRow[]>();
  for (const row of rows) {
    if (!row.parentMemberId) continue;
    if (!childrenById.has(row.parentMemberId)) childrenById.set(row.parentMemberId, []);
    childrenById.get(row.parentMemberId)!.push(row);
  }
  const familyById = new Map<string, Set<string>>();
  const visit = (row: AgentRow | undefined, ids: Set<string>): void => {
    if (!row || ids.has(row.agent.member_id)) return;
    ids.add(sidebarAgentPinId(row.agent));
    ids.add(row.agent.member_id);
    for (const child of childrenById.get(row.agent.member_id) || []) visit(rowById.get(child.agent.member_id), ids);
  };
  for (const row of rows) {
    const ids = new Set<string>();
    visit(row, ids);
    familyById.set(row.agent.member_id, ids);
  }
  return familyById;
}

/** The siblings an agent row can trade places with: same section, subgroup and parent. */
function sidebarAgentContainer(bucket: string, subgroup: string | null, parentMemberId: string | null): string {
  return JSON.stringify([bucket, subgroup, parentMemberId]);
}

function buildSidebarVirtualRows(args: {
  sectionNames: string[];
  grouped: Map<string, AgentRow[]>;
  grouping?: ConsoleAgentListConfig;
  collapsedSections: Set<string>;
  collapsedSubgroups: Set<string>;
  pinnedAgentIds?: Set<string>;
  sectionOrder?: string[];
  subgroupOrder?: string[];
  searchActive?: boolean;
  /** One list ([`groupSidebarAgents`] with `flat`): no category or subgroup headers. */
  flat?: boolean;
  order?: SidebarSortContext;
}): SidebarVirtualRow[] {
  const rows: SidebarVirtualRow[] = [];
  const flat = args.flat === true;
  const orderedSections = flat ? args.sectionNames : applySidebarOrder(args.sectionNames, args.sectionOrder);
  const baseRows = orderedSections.flatMap((bucket) => args.grouped.get(bucket) || []);
  const baseOrderIndex = new Map(baseRows.map((row, index) => [row.agent.member_id, index] as const));
  const pinnedRowIds = collectPinnedRows(baseRows, args.pinnedAgentIds);
  if (pinnedRowIds.size > 0) {
    const pinnedRows = orderRowsPreorderByIndex(
      baseRows.filter((row) => pinnedRowIds.has(row.agent.member_id)),
      baseOrderIndex,
      args.order,
    );
    const collapsedPinned = args.searchActive ? false : args.collapsedSections.has(PINNED_SECTION_NAME);
    rows.push({
      kind: "section",
      key: PINNED_SECTION_KEY,
      bucket: PINNED_SECTION_NAME,
      count: pinnedRows.length,
      collapsed: collapsedPinned,
      pinned: true,
      reorderable: false,
    });
    if (!collapsedPinned) {
      for (const row of pinnedRows) {
        const parent = row.parentMemberId && pinnedRowIds.has(row.parentMemberId) ? row.parentMemberId : null;
        rows.push({
          kind: "agent",
          key: `agent:${PINNED_SECTION_NAME}:${row.agent.member_id}`,
          bucket: PINNED_SECTION_NAME,
          row,
          container: sidebarAgentContainer(PINNED_SECTION_NAME, null, parent),
        });
      }
    }
  }

  const orderSubgroups = !flat && configuredSelectors(args.grouping, "subgroup_by").length > 0;
  for (const bucket of orderedSections) {
    const list = (args.grouped.get(bucket) || []).filter((row) => !pinnedRowIds.has(row.agent.member_id));
    const sectionConfig = flat ? null : sectionConfigFor(bucket, args.grouping);
    if (list.length === 0 && !sectionConfig) continue;
    if (flat) {
      // A flat roster has no category headers; its one header only
      // separates it from the Pinned section above.
      let collapsedFlat = false;
      if (pinnedRowIds.size > 0) {
        // Its collapse state has a key of its own, so a configured category
        // of the same name keeps its own.
        collapsedFlat = args.searchActive ? false : args.collapsedSections.has(FLAT_SECTION_KEY);
        rows.push({
          kind: "section",
          key: FLAT_SECTION_KEY,
          bucket: FLAT_SECTION_NAME,
          count: list.length,
          collapsed: collapsedFlat,
          reorderable: false,
          collapseKey: FLAT_SECTION_KEY,
        });
      }
      if (collapsedFlat) continue;
    } else {
      const collapsedSection = args.searchActive ? false : args.collapsedSections.has(bucket);
      rows.push({
        kind: "section",
        key: `section:${bucket}`,
        bucket,
        count: list.length,
        collapsed: collapsedSection,
        reorderable: true,
      });
      if (collapsedSection) continue;
      if (list.length === 0) {
        rows.push({
          kind: "empty",
          key: `empty:${bucket}`,
          bucket,
          sectionConfig,
        });
        continue;
      }
    }

    const orderedList = orderRowsBySubgroupOrder(list, bucket, args.subgroupOrder);
    // A row's parent is any row of the section, pinned or not: a child left
    // here when a sibling was pinned (taking the parent to Pinned) is still
    // sorted as the parent's child, so it keeps the parent's container.
    const listIds = new Set((args.grouped.get(bucket) || []).map((row) => row.agent.member_id));
    const subgroups = new Set(orderedList.map((row) => row.subgroup).filter((value): value is string => Boolean(value)));
    const showSubgroups = orderSubgroups
      && subgroups.size > (args.grouping?.collapse_single_subgroup === false ? 0 : 1);
    const subgroupCounts = new Map<string, number>();
    for (const row of orderedList) {
      if (!row.subgroup) continue;
      subgroupCounts.set(row.subgroup, (subgroupCounts.get(row.subgroup) || 0) + 1);
    }

    let lastSubgroup: string | null = null;
    let currentSubgroupCollapsed = false;
    for (const row of orderedList) {
      if (showSubgroups && !row.subgroup) {
        lastSubgroup = null;
        currentSubgroupCollapsed = false;
      }
      if (showSubgroups && row.subgroup && row.subgroup !== lastSubgroup) {
        lastSubgroup = row.subgroup;
        const storageKey = sidebarSubgroupStorageId(bucket, row.subgroup);
        currentSubgroupCollapsed = args.searchActive ? false : args.collapsedSubgroups.has(storageKey);
        rows.push({
          kind: "subgroup",
          key: `subgroup:${bucket}:${row.subgroup}`,
          bucket,
          label: row.subgroup,
          count: subgroupCounts.get(row.subgroup) || 0,
          collapsed: currentSubgroupCollapsed,
          storageKey,
          reorderable: true,
        });
      }
      if (currentSubgroupCollapsed) continue;
      const parent = row.parentMemberId && listIds.has(row.parentMemberId) ? row.parentMemberId : null;
      rows.push({
        kind: "agent",
        key: `agent:${row.agent.member_id}`,
        bucket,
        row,
        container: sidebarAgentContainer(bucket, orderSubgroups ? row.subgroup ?? null : null, parent),
      });
    }
  }
  return rows;
}

function sidebarNavigationLabel(row: SidebarVirtualRow): string {
  switch (row.kind) {
    case "section":
      return row.bucket;
    case "subgroup":
      return row.label;
    case "empty":
      return row.sectionConfig?.empty_title || row.sectionConfig?.empty_text || "No agents";
    case "agent":
      return row.row.agent.label;
  }
}

function sidebarNavigationNodeForRow(row: SidebarVirtualRow): SidebarNavigationNode {
  const base = {
    id: row.key,
    label: sidebarNavigationLabel(row),
    target: row,
  };
  if (row.kind === "section" || row.kind === "subgroup") {
    return {
      ...base,
      type: "group",
      expanded: !row.collapsed,
      children: [],
    };
  }
  return {
    ...base,
    type: "item",
  };
}

function sidebarNavigationModelFromRows(rows: SidebarVirtualRow[]): SidebarNavigationModel {
  const nodes: SidebarNavigationNode[] = [];
  let currentSection: SidebarNavigationNode | null = null;
  let currentSubgroup: SidebarNavigationNode | null = null;
  for (const row of rows) {
    const node = sidebarNavigationNodeForRow(row);
    if (row.kind === "section") {
      nodes.push(node);
      currentSection = node;
      currentSubgroup = null;
      continue;
    }
    if (row.kind === "subgroup") {
      if (currentSection?.type === "group") {
        currentSection.children.push(node);
      } else {
        nodes.push(node);
      }
      currentSubgroup = node;
      continue;
    }
    const belongsToCurrentSubgroup = row.kind === "agent"
      && Boolean(row.row.subgroup)
      && currentSubgroup?.type === "group"
      && currentSubgroup.target?.kind === "subgroup"
      && currentSubgroup.target.bucket === row.bucket
      && currentSubgroup.target.label === row.row.subgroup;
    const parent = belongsToCurrentSubgroup
      ? currentSubgroup
      : currentSection?.type === "group"
        ? currentSection
        : null;
    if (parent) {
      parent.children.push(node);
    } else {
      nodes.push(node);
    }
  }
  return normalizeConsoleNavigationModel({
    orientation: "vertical",
    nodes,
    order: { orderedNodeIds: [] },
  });
}

function buildStockSidebarNavigationModel(args: Parameters<typeof buildSidebarVirtualRows>[0]): SidebarNavigationModel {
  return sidebarNavigationModelFromRows(buildSidebarVirtualRows(args));
}

function sidebarNavigationRows(model: SidebarNavigationModel): SidebarVirtualRow[] {
  const normalized = normalizeConsoleNavigationModel(model);
  const rows: SidebarVirtualRow[] = [];
  const visit = (node: SidebarNavigationNode): void => {
    if (node.target) rows.push(node.target);
    if (node.type !== "group" || !node.expanded) return;
    for (const child of node.children) visit(child);
  };
  for (const node of normalized.nodes) visit(node);
  return rows;
}

function deriveStateAttr(agent: ConsoleAgent): "active" | "degraded" | "retired" {
  const state = (agent.state || "").toLowerCase();
  if (state === "retired" || state === "retiring" || state === "stopped") return "retired";
  const degraded = agent.labels?.console_degraded === "true" ||
                   state.includes("degrade") ||
                   state === "needs_repair" ||
                   state === "broken";
  if (degraded) return "degraded";
  return "active";
}

function pulseSamples(activity: ConsoleFrame[], identity: string): number[] {
  const bucket = new Array<number>(10).fill(0);
  const now = Date.now();
  const window = 15 * 60 * 1000;
  for (const f of activity) {
    if (!f.timestampMs || (f.identity || "") !== identity) continue;
    const age = now - f.timestampMs;
    if (age < 0 || age > window) continue;
    const idx = 9 - Math.floor((age / window) * 10);
    if (idx >= 0 && idx < 10) bucket[idx]++;
  }
  return bucket;
}

function virtualRowHeight(row: SidebarVirtualRow): number {
  return SIDEBAR_ROW_HEIGHT[row.kind];
}

/** The index of the last row of the agent row at `index` and its visible descendants. */
function agentSubtreeEnd(rows: SidebarVirtualRow[], index: number): number {
  const start = rows[index];
  if (!start || start.kind !== "agent") return index;
  const inSubtree = new Set([start.row.agent.member_id]);
  let end = index;
  for (let next = index + 1; next < rows.length; next += 1) {
    const row = rows[next]!;
    if (row.kind !== "agent" || row.bucket !== start.bucket) break;
    const parent = row.row.parentMemberId;
    if (!parent || !inSubtree.has(parent)) break;
    inSubtree.add(row.row.agent.member_id);
    end = next;
  }
  return end;
}

function agentOrderRowIndex(rows: SidebarVirtualRow[], item: SidebarOrderItem): number {
  return rows.findIndex((row) => row.kind === "agent"
    && row.container === item.container
    && sidebarAgentOrderKey(row.row.agent) === item.id);
}

function sidebarDragPreviewRows(
  rows: SidebarVirtualRow[],
  item: SidebarOrderItem | null,
): SidebarVirtualRow[] {
  if (!item) return [];
  if (item.kind === "agent") {
    const start = agentOrderRowIndex(rows, item);
    return start < 0 ? [] : rows.slice(start, agentSubtreeEnd(rows, start) + 1);
  }
  const start = rows.findIndex((row) => {
    if (item.kind === "section") return row.kind === "section" && row.bucket === item.id;
    return row.kind === "subgroup" && row.storageKey === item.id && row.bucket === item.bucket;
  });
  if (start < 0) return [];
  const out: SidebarVirtualRow[] = [];
  for (let index = start; index < rows.length; index += 1) {
    const row = rows[index];
    if (index > start) {
      if (item.kind === "section" && row.kind === "section") break;
      if (item.kind === "subgroup" && (row.kind === "section" || row.kind === "subgroup")) break;
    }
    out.push(row);
  }
  return out;
}

function renderSidebarDragPreviewRows(rows: SidebarVirtualRow[]): React.JSX.Element[] {
  return rows.map((row) => {
    if (row.kind === "section") {
      return (
        <div
          className="sidebar__drag-preview-section"
          key={`preview:${row.key}`}
          data-pinned={row.pinned ? "true" : undefined}
        >
          <span className="sidebar__sec-label">{row.bucket}</span>
          <span className="sidebar__sec-spacer" />
          <span className="sidebar__sec-count">{row.count}</span>
        </div>
      );
    }
    if (row.kind === "subgroup") {
      return (
        <div className="sidebar__drag-preview-subgroup" key={`preview:${row.key}`}>
          <span>{row.label}</span>
          <span className="sidebar__sec-spacer" />
          <span className="sidebar__sec-count">{row.count}</span>
        </div>
      );
    }
    if (row.kind === "empty") {
      return (
        <div className="sidebar__drag-preview-empty" key={`preview:${row.key}`}>
          {row.sectionConfig?.empty_title || row.sectionConfig?.empty_text || "No agents"}
        </div>
      );
    }
    return (
      <div
        className={`sidebar__drag-preview-agent ${row.row.childOfHost ? "sidebar__drag-preview-agent--child" : ""}`}
        data-depth={row.row.childOfHost ? String(Math.min(row.row.depth, 3)) : undefined}
        key={`preview:${row.key}`}
      >
        <span className="agent__dot" />
        <span className="sidebar__drag-preview-agent-body">
          <span className="agent__name">{row.row.agent.label}</span>
          {agentKindLabel(row.row.agent) ? <span className="agent__detail">{agentKindLabel(row.row.agent)}</span> : null}
        </span>
      </div>
    );
  });
}

function lowerBound(values: number[], needle: number): number {
  let lo = 0;
  let hi = values.length;
  while (lo < hi) {
    const mid = Math.floor((lo + hi) / 2);
    if (values[mid] < needle) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

function sidebarVirtualOffsets(rows: SidebarVirtualRow[]): { offsets: number[]; total: number } {
  const offsets: number[] = [];
  let total = 0;
  for (const row of rows) {
    offsets.push(total);
    total += virtualRowHeight(row);
  }
  return { offsets, total };
}

function sidebarVisibleRange(args: {
  rowCount: number;
  offsets: number[];
  total: number;
  scrollTop: number;
  listHeight: number;
}): { start: number; end: number } {
  if (args.rowCount === 0) return { start: 0, end: 0 };
  const startNeedle = Math.max(0, args.scrollTop - SIDEBAR_OVERSCAN_PX);
  const endNeedle = Math.min(args.total, args.scrollTop + Math.max(1, args.listHeight) + SIDEBAR_OVERSCAN_PX);
  const start = Math.max(0, lowerBound(args.offsets, startNeedle) - 1);
  const end = Math.min(args.rowCount, lowerBound(args.offsets, endNeedle) + 1);
  return { start, end };
}

function pendingOrderFocusMatchesRow(pending: PendingOrderFocus, row: SidebarVirtualRow): boolean {
  if (pending.kind === "section") {
    return row.kind === "section" && row.bucket === pending.id;
  }
  if (pending.kind === "agent") {
    return row.kind === "agent"
      && row.container === pending.container
      && sidebarAgentOrderKey(row.row.agent) === pending.id;
  }
  return row.kind === "subgroup"
    && row.storageKey === pending.id
    && row.bucket === pending.bucket;
}

function pendingOrderFocusMatchesElement(pending: PendingOrderFocus, element: HTMLElement): boolean {
  if (element.dataset.sidebarOrderKind !== pending.kind) return false;
  if (element.dataset.sidebarOrderId !== pending.id) return false;
  if (pending.kind === "subgroup" && element.dataset.sidebarOrderBucket !== pending.bucket) return false;
  if (pending.kind === "agent" && element.dataset.sidebarOrderContainer !== pending.container) return false;
  return true;
}

function useMeasuredHeight<T extends HTMLElement>(): [React.RefObject<T>, number] {
  const ref = React.useRef<T>(null);
  const [height, setHeight] = React.useState(0);
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const update = () => setHeight(element.clientHeight);
    update();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", update);
      return () => window.removeEventListener("resize", update);
    }
    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      setHeight(box ? box.height : element.clientHeight);
    });
    ro.observe(element);
    return () => ro.disconnect();
  }, []);
  return [ref, height];
}

/** An agent row's part in manual ordering: its container, and the handlers when it can move. */
interface AgentRowReorder {
  container: string;
  /** Manual sort, no search or filter, and a sibling to trade places with. */
  enabled: boolean;
  dragging: boolean;
  dragOver?: SidebarDropPosition;
  onGripPointerDown: (event: React.PointerEvent<HTMLElement>) => void;
  /** Handles Alt+ArrowUp and Alt+ArrowDown; returns whether it did. */
  onKeyDown: (event: React.KeyboardEvent<HTMLElement>) => boolean;
}

interface AgentRowRender {
  row: AgentRow;
  selectedMemberId: string;
  recentActivity: ConsoleFrame[];
  grouping: ConsoleAgentListConfig | undefined;
  pinnedAgentIds: Set<string> | undefined;
  onSelect: (agent: ConsoleAgent) => void;
  onTogglePinnedAgent: ((agent: ConsoleAgent, familyPinIds?: Set<string>) => void) | undefined;
  familyPinIds?: Set<string>;
  lastActivityMs?: number;
  nowMs: number;
  reorder?: AgentRowReorder;
}

function renderAgentRow({
  row,
  selectedMemberId,
  recentActivity,
  grouping,
  pinnedAgentIds,
  onSelect,
  onTogglePinnedAgent,
  familyPinIds,
  lastActivityMs,
  nowMs,
  reorder,
}: AgentRowRender): React.JSX.Element {
  const { agent, childOfHost, depth } = row;
  const stateAttr = deriveStateAttr(agent);
  const identity = agent.identity || agent.member_id;
  const pulse = pulseSamples(recentActivity, identity);
  const inbox = inboxCount(agent);
  const badges = configuredAgentBadges(agent, grouping);
  const pinned = isAgentPinned(agent, pinnedAgentIds);
  const detail = agentDetailParts(agent, lastActivityMs, nowMs);
  const movable = reorder?.enabled === true;
  return (
    <div
      className={`agent ${childOfHost ? "agent--child" : ""} ${agent.member_id === selectedMemberId ? "is-active" : ""}`}
      data-state={stateAttr}
      data-child-of-host={childOfHost ? "true" : undefined}
      data-side-chat={agent.side_chat_of ? "true" : undefined}
      data-depth={childOfHost ? String(Math.min(depth, 3)) : undefined}
      data-testid={`sidebar-agent:${agent.member_id}`}
      data-sidebar-order-kind={movable ? "agent" : undefined}
      data-sidebar-order-id={movable ? sidebarAgentOrderKey(agent) : undefined}
      data-sidebar-order-container={movable ? reorder?.container : undefined}
      data-drag-over={reorder?.dragOver}
      data-dragging={reorder?.dragging ? "true" : undefined}
      aria-keyshortcuts={movable ? "Alt+ArrowUp Alt+ArrowDown" : undefined}
      title={`Identity: ${identity}`}
      onClick={() => onSelect(agent)}
      onKeyDown={(event) => {
        // Keys on the row's own buttons (pin, copy) stay theirs.
        if (event.target !== event.currentTarget) return;
        if (reorder?.onKeyDown(event)) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect(agent);
        }
      }}
      role="button"
      tabIndex={0}
    >
      {movable ? (
        <span
          className="agent__grip"
          aria-hidden="true"
          title="Drag to reorder. Alt+Up and Alt+Down move the focused agent."
          data-testid={`sidebar-agent-grip:${agent.member_id}`}
          onPointerDown={reorder?.onGripPointerDown}
          onClick={(event) => event.stopPropagation()}
        />
      ) : null}
      {reorder?.dragOver ? <span className="agent__drop-line" data-where={reorder.dragOver} aria-hidden="true" /> : null}
      <span className="agent__dot" />
      <span className="agent__body">
        <span className="agent__name">{agent.label}</span>
        {detail.kind || detail.status ? (
          // The kind gives way first, so the status always shows in a narrow roster.
          <span className="agent__detail" data-testid={`sidebar-agent-detail:${agent.member_id}`}>
            {detail.kind ? (
              <span className="agent__detail-kind">
                {detail.kind}
                {detail.status ? <span className="agent__detail-separator"> · </span> : null}
              </span>
            ) : null}
            {detail.status ? <span className="agent__detail-status">{detail.status}</span> : null}
          </span>
        ) : null}
        {badges.length > 0 || agent.side_chat_of ? (
          <span className="agent__badges">
            {agent.side_chat_of ? (
              <span
                className="agent__badge"
                data-tone="neutral"
                data-testid={`sidebar-side-chat:${agent.member_id}`}
                title={`Side chat of ${agent.side_chat_of}`}
              >
                <span>Side chat</span>
              </span>
            ) : null}
            {badges.map((badge) => (
              <span
                className="agent__badge"
                data-tone={badge.tone || "neutral"}
                key={badge.id}
                title={`${badge.label}: ${badge.value}`}
              >
                <span>{badge.label}</span>
                <strong>{badge.value}</strong>
              </span>
            ))}
          </span>
        ) : null}
      </span>
      <span className="agent__actions">
        <span
          className="agent__copy"
          data-testid={`sidebar-agent-copy-identity:${agent.member_id}`}
          onClick={(event) => event.stopPropagation()}
        >
          <CopyButton
            text={identity}
            label={`Copy the identity of ${agent.label}`}
            copiedLabel="Identity copied"
          />
        </span>
        {onTogglePinnedAgent ? (
          <button
            type="button"
            className="agent__pin"
            data-active={pinned ? "true" : undefined}
            aria-label={pinned ? `Unpin ${agent.label}` : `Pin ${agent.label}`}
            aria-pressed={pinned}
            title={pinned ? "Unpin agent" : "Pin agent"}
            data-testid={`sidebar-agent-pin:${agent.member_id}`}
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onTogglePinnedAgent(agent, familyPinIds);
            }}
          >
            <Icon name="i-pin" className="agent__pin-icon" />
          </button>
        ) : null}
      </span>
      <span className="agent__meta">
        <span className="agent__pulse">
          {pulse.map((v, i) => (
            <span key={i} style={{ height: `${Math.max(1, Math.min(12, v * 2 + 1))}px` }} />
          ))}
        </span>
        {inbox > 0 && <span className="agent__inbox">{inbox}</span>}
      </span>
    </div>
  );
}

function inboxCount(agent: ConsoleAgent): number {
  const n = Number(agent.labels?.console_inbox_count ?? 0);
  return Number.isFinite(n) ? n : 0;
}

/// Memoised: the app root re-renders on every coalesced frame flush and on
/// each debounced draft publish, and the props here are stable references
/// (see ConsoleApp), so unchanged input skips the whole subtree.
export const Sidebar = React.memo(function Sidebar({
  agents,
  selectedMemberId,
  recentActivity,
  collapsed,
  visibleControls,
  customButtons,
  customPanels = [],
  onOpenCustomPanel,
  grouping,
  storageNamespace,
  preferenceScope,
  pinnedAgentIds,
  onSelect,
  onTogglePinnedAgent,
  onOpenControl,
  onOpenOperatorPanel,
  approvals,
  onOpenApproval,
}: SidebarProps): React.JSX.Element {
  countRender("Sidebar");
  const [q, setQ] = React.useState("");
  const [activityFilter, setActivityFilter] = React.useState<"all" | "working" | "quiet" | "unknown">("all");
  const [draggingOrder, setDraggingOrder] = React.useState<SidebarOrderItem | null>(null);
  const [dragOverOrder, setDragOverOrder] = React.useState<SidebarDragOver | null>(null);
  const [dragPreview, setDragPreview] = React.useState<SidebarDragPreview | null>(null);
  const [orderAnnouncement, setOrderAnnouncement] = React.useState("");
  const pendingOrderFocusRef = React.useRef<PendingOrderFocus | null>(null);
  const draggingOrderRef = React.useRef<SidebarOrderItem | null>(null);
  const pointerDragRef = React.useRef<(SidebarOrderItem & {
    startX: number;
    startY: number;
    lastX: number;
    lastY: number;
    previewWidth: number;
    moved: boolean;
    over: { id: string; bucket?: string; where: SidebarDropPosition } | null;
  }) | null>(null);
  const suppressOrderClickRef = React.useRef(false);
  const virtualRowsRef = React.useRef<SidebarVirtualRow[]>([]);
  React.useEffect(() => {
    draggingOrderRef.current = draggingOrder;
  }, [draggingOrder]);
  // Computed once; URL-driven config doesn't change without a reload.
  const navKinds = React.useMemo(() => {
    const configured = visibleNavKinds();
    if (!visibleControls) return configured;
    const allowed = new Set(visibleControls);
    return configured.filter((kind) => allowed.has(kind));
  }, [visibleControls]);

  // Per-user preferences: the roster view and the manual agent order. Their
  // keys take the principal scope, so an agent-list configuration edit
  // leaves them as they are.
  const preferenceNamespace = preferenceScope ?? storageNamespace;
  const viewStorageKey = React.useMemo(
    () => sidebarStorageKey(SIDEBAR_VIEW_STORAGE_PREFIX, preferenceNamespace),
    [preferenceNamespace],
  );
  const agentOrderStorageKey = React.useMemo(
    () => sidebarStorageKey(SIDEBAR_AGENT_ORDER_STORAGE_PREFIX, preferenceNamespace),
    [preferenceNamespace],
  );
  const [view, setView] = React.useState<ConsoleSidebarViewPreferences>(
    () => readConsoleSidebarView(localSidebarStorage(), viewStorageKey) ?? { ...DEFAULT_CONSOLE_SIDEBAR_VIEW },
  );
  React.useEffect(() => {
    setView(readConsoleSidebarView(localSidebarStorage(), viewStorageKey) ?? { ...DEFAULT_CONSOLE_SIDEBAR_VIEW });
  }, [viewStorageKey]);
  const [agentOrder, setAgentOrder] = React.useState<string[]>(
    () => readSidebarStringList(localSidebarStorage(), agentOrderStorageKey) || [],
  );
  React.useEffect(() => {
    setAgentOrder(readSidebarStringList(localSidebarStorage(), agentOrderStorageKey) || []);
  }, [agentOrderStorageKey]);
  // Another tab of this console reordered agents or changed the view.
  React.useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const onStorage = (event: StorageEvent) => {
      if (event.key === agentOrderStorageKey) {
        setAgentOrder(readSidebarStringList(localSidebarStorage(), agentOrderStorageKey) || []);
      } else if (event.key === viewStorageKey) {
        setView(readConsoleSidebarView(localSidebarStorage(), viewStorageKey) ?? { ...DEFAULT_CONSOLE_SIDEBAR_VIEW });
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [agentOrderStorageKey, viewStorageKey]);
  const updateView = React.useCallback((patch: Partial<ConsoleSidebarViewPreferences>) => {
    setView((current) => {
      const next = { ...current, ...patch };
      writeConsoleSidebarView(localSidebarStorage(), viewStorageKey, next);
      return next;
    });
  }, [viewStorageKey]);

  // The clock behind "active 3 min ago"; frames re-render the roster too.
  const [nowMs, setNowMs] = React.useState(() => Date.now());
  React.useEffect(() => {
    if (collapsed || typeof window === "undefined") return undefined;
    const timer = window.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [collapsed]);
  const frameTimes = React.useMemo(() => newestFrameTimes(recentActivity), [recentActivity]);
  const liveActivity = React.useMemo(() => {
    const byMember = new Map<string, number>();
    for (const agent of agents) {
      const at = agentLastActivity(agent, frameTimes);
      if (at !== undefined) byMember.set(agent.member_id, at);
    }
    return byMember;
  }, [agents, frameTimes]);
  // Sorting by last activity holds still while a drag is in progress.
  const heldActivityRef = React.useRef(liveActivity);
  if (!draggingOrder) heldActivityRef.current = liveActivity;
  const sortActivity = view.sortBy !== "activity"
    ? EMPTY_ACTIVITY
    : draggingOrder ? heldActivityRef.current : liveActivity;
  const sortContext = React.useMemo<SidebarSortContext>(() => ({
    sortBy: view.sortBy,
    rank: new Map(agentOrder.map((key, index) => [key, index] as const)),
    lastActivity: (agent) => sortActivity.get(agent.member_id),
  }), [agentOrder, sortActivity, view.sortBy]);
  const flat = view.groupBy === "none";
  const searchActive = Boolean(q) || activityFilter !== "all";

  const filtered = React.useMemo(() => {
    const needle = q.toLowerCase();
    return agents.filter((agent) => {
      // Null is an owner-reported quiet phase; absent activity stays unknown.
      const activity = agent.response_phase === null ? "quiet"
        : agent.response_phase === "waiting" || agent.response_phase === "tool-executing" || agent.response_phase === "generating"
          ? "working" : "unknown";
      return (activityFilter === "all" || activity === activityFilter) && (!needle ||
        [agent.label, agent.identity, agent.member_id, agent.role].some(value => value?.toLowerCase().includes(needle)));
    });
  }, [agents, q, activityFilter]);

  const sectionOrderStorageKey = React.useMemo(
    () => sidebarStorageKey(SIDEBAR_SECTION_ORDER_STORAGE_PREFIX, storageNamespace),
    [storageNamespace],
  );
  const subgroupOrderStorageKey = React.useMemo(
    () => sidebarStorageKey(SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX, storageNamespace),
    [storageNamespace],
  );
  const [sectionOrder, setSectionOrder] = React.useState<string[]>(() => {
    return readSidebarStringList(localSidebarStorage(), sectionOrderStorageKey) || [];
  });
  React.useEffect(() => {
    setSectionOrder(readSidebarStringList(localSidebarStorage(), sectionOrderStorageKey) || []);
  }, [sectionOrderStorageKey]);
  const [subgroupOrder, setSubgroupOrder] = React.useState<string[]>(() => {
    return readSidebarStringList(localSidebarStorage(), subgroupOrderStorageKey) || [];
  });
  React.useEffect(() => {
    setSubgroupOrder(readSidebarStringList(localSidebarStorage(), subgroupOrderStorageKey) || []);
  }, [subgroupOrderStorageKey]);
  const classified = React.useMemo(() => classifySidebarAgents(filtered, grouping), [filtered, grouping]);
  // The category view's rows. The flat list needs them only for Manual,
  // where they give its default order and the one list both views store.
  const categoryGrouped = React.useMemo(
    () => (flat && view.sortBy !== "manual" ? null : orderSidebarGroups(classified, sortContext)),
    [classified, flat, sortContext, view.sortBy],
  );
  const categorySequence = React.useMemo(
    () => (categoryGrouped && view.sortBy === "manual"
      ? sidebarCategorySequence(categoryGrouped, grouping, sectionOrder, subgroupOrder)
      : []),
    [categoryGrouped, grouping, sectionOrder, subgroupOrder, view.sortBy],
  );
  const grouped = React.useMemo(
    () => (!flat && categoryGrouped ? categoryGrouped : orderFlatSidebarGroup(classified, categorySequence, sortContext)),
    [categoryGrouped, categorySequence, classified, flat, sortContext],
  );
  const familyPinIdsByMemberId = React.useMemo(() => sidebarFamilyPinIdsByMemberId(grouped), [grouped]);
  const sectionNames = React.useMemo(
    () => flat
      ? (grouped.has(FLAT_SECTION_NAME) ? [FLAT_SECTION_NAME] : [])
      : orderedSectionNames(grouped, grouping),
    [flat, grouped, grouping],
  );
  const defaultCollapsedKey = React.useMemo(
    () => JSON.stringify((grouping?.sections || []).map((section) => [section.name, section.collapsed === true])),
    [grouping?.sections],
  );
  const sectionCollapseStorageKey = React.useMemo(
    () => sidebarStorageKey(SECTION_COLLAPSE_STORAGE_PREFIX, storageNamespace),
    [storageNamespace],
  );
  const subgroupCollapseStorageKey = React.useMemo(
    () => sidebarStorageKey(SUBGROUP_COLLAPSE_STORAGE_PREFIX, storageNamespace),
    [storageNamespace],
  );
  const [collapsedSections, setCollapsedSections] = React.useState<Set<string>>(() => {
    return collapsedSectionsForStorage(grouping, sectionCollapseStorageKey);
  });
  React.useEffect(() => {
    setCollapsedSections(collapsedSectionsForStorage(grouping, sectionCollapseStorageKey));
  }, [defaultCollapsedKey, grouping, sectionCollapseStorageKey]);
  const [collapsedSubgroups, setCollapsedSubgroups] = React.useState<Set<string>>(() => {
    return collapsedSubgroupsForStorage(subgroupCollapseStorageKey);
  });
  React.useEffect(() => {
    setCollapsedSubgroups(collapsedSubgroupsForStorage(subgroupCollapseStorageKey));
  }, [subgroupCollapseStorageKey]);
  const customSidebarButtons = React.useMemo(
    () => (customButtons || []).filter((button) => button.id && button.label && (button.control || button.href || button.panel)),
    [customButtons],
  );
  const completeSectionDrop = React.useCallback((
    target: string,
    where: SidebarDropPosition,
    draggedId = draggingOrderRef.current?.id,
    inputSource: "keyboard" | "pointer" = "pointer",
  ) => {
    if (!draggedId || draggedId === target) return;
    if (inputSource === "keyboard") {
      pendingOrderFocusRef.current = { kind: "section", id: draggedId };
    }
    setSectionOrder((current) => {
      const baseOrder = applySidebarOrder(sectionNames, current);
      const next = reorderSidebarOrderWithNavigationModel(baseOrder, draggedId, target, where, inputSource);
      writeSidebarStringList(localSidebarStorage(), sectionOrderStorageKey, next);
      return next;
    });
    setOrderAnnouncement(`Moved section ${draggedId} ${where} ${target}.`);
  }, [sectionNames, sectionOrderStorageKey]);
  const subgroupIdsForBucket = React.useCallback((bucket: string): string[] => {
    const list = grouped.get(bucket) || [];
    const ids = list
      .map((row) => row.subgroup)
      .filter((value): value is string => Boolean(value))
      .map((subgroup) => sidebarSubgroupStorageId(bucket, subgroup));
    return Array.from(new Set(ids));
  }, [grouped]);
  const completeSubgroupDrop = React.useCallback((
    target: string,
    bucket: string,
    where: SidebarDropPosition,
    draggedId = draggingOrderRef.current?.id,
    draggedBucket = draggingOrderRef.current?.bucket,
    inputSource: "keyboard" | "pointer" = "pointer",
  ) => {
    if (
      !draggedId ||
      draggedBucket !== bucket ||
      draggedId === target
    ) return;
    if (inputSource === "keyboard") {
      pendingOrderFocusRef.current = { kind: "subgroup", id: draggedId, bucket };
    }
    setSubgroupOrder((current) => {
      const bucketOrder = applySidebarOrder(subgroupIdsForBucket(bucket), current);
      const nextBucketOrder = reorderSidebarOrderWithNavigationModel(bucketOrder, draggedId, target, where, inputSource);
      const nextBucketSet = new Set(nextBucketOrder);
      const next = [
        ...current.filter((id) => !nextBucketSet.has(id)),
        ...nextBucketOrder,
      ];
      writeSidebarStringList(localSidebarStorage(), subgroupOrderStorageKey, next);
      return next;
    });
    setOrderAnnouncement(`Moved subgroup ${sidebarSubgroupStorageLabel(draggedId)} ${where} ${sidebarSubgroupStorageLabel(target)}.`);
  }, [subgroupIdsForBucket, subgroupOrderStorageKey]);
  // Every agent's key in the category order, whichever view is shown: a
  // move lists them all first, so either view's first move stores the same
  // list and the other view keeps its order apart from the moved agent.
  const presentAgentKeys = React.useMemo(
    () => categorySequence.map((row) => sidebarAgentOrderKey(row.agent)),
    [categorySequence],
  );
  // Why agents cannot be reordered right now, or null when they can.
  const agentReorderBlocked = view.sortBy !== "manual"
    ? "Choose Sort: Manual to reorder agents."
    : searchActive
      ? "Clear the search and the activity filter to reorder agents."
      : null;
  const completeAgentDrop = React.useCallback((
    target: string,
    where: SidebarDropPosition,
    draggedId: string,
    container: string,
    inputSource: "keyboard" | "pointer",
  ) => {
    if (!draggedId || draggedId === target) return;
    const siblings = virtualRowsRef.current.filter(
      (row): row is Extract<SidebarVirtualRow, { kind: "agent" }> => row.kind === "agent" && row.container === container,
    );
    const next = sidebarAgentOrderAfterMove({
      stored: agentOrder,
      agents,
      present: presentAgentKeys,
      siblings: siblings.map((row) => sidebarAgentOrderKey(row.row.agent)),
      dragged: draggedId,
      target,
      where,
      inputSource,
    });
    if (!next) return;
    writeSidebarStringList(localSidebarStorage(), agentOrderStorageKey, next);
    setAgentOrder(next);
    if (inputSource === "keyboard") {
      pendingOrderFocusRef.current = { kind: "agent", id: draggedId, container };
    }
    const labelOf = (key: string) =>
      siblings.find((row) => sidebarAgentOrderKey(row.row.agent) === key)?.row.agent.label || key;
    setOrderAnnouncement(`Moved ${labelOf(draggedId)} ${where === "before" ? "above" : "below"} ${labelOf(target)}.`);
  }, [agentOrder, agentOrderStorageKey, agents, presentAgentKeys]);
  const handleSectionOrderKeyDown = React.useCallback((event: React.KeyboardEvent<HTMLElement>, bucket: string) => {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    const ordered = applySidebarOrder(sectionNames, sectionOrder);
    const index = ordered.indexOf(bucket);
    const target = event.key === "ArrowUp" ? ordered[index - 1] : ordered[index + 1];
    if (!target) return;
    event.preventDefault();
    completeSectionDrop(target, event.key === "ArrowUp" ? "before" : "after", bucket, "keyboard");
  }, [completeSectionDrop, sectionNames, sectionOrder]);
  const handleSubgroupOrderKeyDown = React.useCallback((event: React.KeyboardEvent<HTMLElement>, storageKey: string, bucket: string) => {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return;
    const ordered = applySidebarOrder(subgroupIdsForBucket(bucket), subgroupOrder);
    const index = ordered.indexOf(storageKey);
    const target = event.key === "ArrowUp" ? ordered[index - 1] : ordered[index + 1];
    if (!target) return;
    event.preventDefault();
    completeSubgroupDrop(target, bucket, event.key === "ArrowUp" ? "before" : "after", storageKey, bucket, "keyboard");
  }, [completeSubgroupDrop, subgroupIdsForBucket, subgroupOrder]);
  const handleAgentOrderKeyDown = React.useCallback((
    event: React.KeyboardEvent<HTMLElement>,
    row: Extract<SidebarVirtualRow, { kind: "agent" }>,
  ): boolean => {
    if (!event.altKey || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return false;
    event.preventDefault();
    if (agentReorderBlocked) {
      setOrderAnnouncement(agentReorderBlocked);
      return true;
    }
    const key = sidebarAgentOrderKey(row.row.agent);
    const siblings = virtualRowsRef.current
      .filter((candidate): candidate is Extract<SidebarVirtualRow, { kind: "agent" }> =>
        candidate.kind === "agent" && candidate.container === row.container)
      .map((candidate) => sidebarAgentOrderKey(candidate.row.agent));
    const index = siblings.indexOf(key);
    const up = event.key === "ArrowUp";
    const target = index < 0 ? undefined : up ? siblings[index - 1] : siblings[index + 1];
    if (!target) {
      setOrderAnnouncement(`${row.row.agent.label} is already ${up ? "first" : "last"}.`);
      return true;
    }
    completeAgentDrop(target, up ? "before" : "after", key, row.container, "keyboard");
    return true;
  }, [agentReorderBlocked, completeAgentDrop]);
  const beginPointerOrderDrag = React.useCallback((
    event: React.PointerEvent<HTMLElement>,
    item: SidebarOrderItem,
  ) => {
    if (event.button !== 0) return;
    event.preventDefault();
    pointerDragRef.current = {
      ...item,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      previewWidth: event.currentTarget.closest(".sidebar")?.getBoundingClientRect().width || event.currentTarget.getBoundingClientRect().width,
      moved: false,
      over: null,
    };
    setDraggingOrder(item);
    draggingOrderRef.current = item;
  }, []);
  // The drop target under the pointer: a row of the dragged kind (for an
  // agent, of its container), above or below its middle.
  const updatePointerOrderTarget = React.useCallback((clientX: number, clientY: number) => {
    const drag = pointerDragRef.current;
    if (!drag) return;
    const target = document
      .elementFromPoint(clientX, clientY)
      ?.closest<HTMLElement>("[data-sidebar-order-kind]");
    const kind = target?.dataset.sidebarOrderKind as SidebarOrderKind | undefined;
    const id = target?.dataset.sidebarOrderId;
    const bucket = target?.dataset.sidebarOrderBucket;
    const container = target?.dataset.sidebarOrderContainer;
    if (
      !target || !kind || !id || kind !== drag.kind || id === drag.id
      || (kind === "subgroup" && bucket !== drag.bucket)
      || (kind === "agent" && container !== drag.container)
    ) {
      drag.over = null;
      setDragOverOrder(null);
      return;
    }
    const rect = target.getBoundingClientRect();
    const where: SidebarDropPosition = clientY > rect.top + rect.height / 2 ? "after" : "before";
    drag.over = { id, bucket, where };
    let indicatorId = id;
    if (kind === "agent") {
      // "After" an agent means after its whole subtree, so its last
      // visible descendant draws the line.
      const rows = virtualRowsRef.current;
      const index = agentOrderRowIndex(rows, { kind, id, container });
      const indicatorRow = index < 0 ? undefined : rows[where === "after" ? agentSubtreeEnd(rows, index) : index];
      indicatorId = indicatorRow?.key || id;
    }
    setDragOverOrder({ kind, id, where, indicatorId });
  }, []);
  const movePointerOrderDrag = React.useCallback((event: Pick<PointerEvent | React.PointerEvent<HTMLElement>, "clientX" | "clientY" | "preventDefault">) => {
    const drag = pointerDragRef.current;
    if (!drag) return;
    drag.lastX = event.clientX;
    drag.lastY = event.clientY;
    if (!drag.moved && Math.max(Math.abs(event.clientX - drag.startX), Math.abs(event.clientY - drag.startY)) < 4) return;
    drag.moved = true;
    event.preventDefault();
    setDragPreview({
      x: event.clientX,
      y: event.clientY,
      width: drag.previewWidth,
    });
    updatePointerOrderTarget(event.clientX, event.clientY);
  }, [updatePointerOrderTarget]);
  // A pointerup drops the dragged row where it is over; a pointercancel
  // (the browser took the pointer away) ends the drag without moving it.
  const finishPointerOrderDrag = React.useCallback((drop: boolean) => {
    const drag = pointerDragRef.current;
    if (!drag) return;
    pointerDragRef.current = null;
    if (drop && drag.moved && drag.over) {
      if (drag.kind === "section") {
        completeSectionDrop(drag.over.id, drag.over.where, drag.id, "pointer");
      } else if (drag.kind === "subgroup") {
        if (drag.over.bucket) {
          completeSubgroupDrop(drag.over.id, drag.over.bucket, drag.over.where, drag.id, drag.bucket, "pointer");
        }
      } else if (drag.container) {
        completeAgentDrop(drag.over.id, drag.over.where, drag.id, drag.container, "pointer");
      }
      suppressOrderClickRef.current = true;
      window.setTimeout(() => {
        suppressOrderClickRef.current = false;
      }, 0);
    }
    draggingOrderRef.current = null;
    setDraggingOrder(null);
    setDragOverOrder(null);
    setDragPreview(null);
  }, [completeAgentDrop, completeSectionDrop, completeSubgroupDrop]);
  React.useEffect(() => {
    if (!draggingOrder) return undefined;
    const onMove = (event: PointerEvent) => movePointerOrderDrag(event);
    const onDrop = () => finishPointerOrderDrag(true);
    const onCancel = () => finishPointerOrderDrag(false);
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onDrop);
    window.addEventListener("pointercancel", onCancel);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onDrop);
      window.removeEventListener("pointercancel", onCancel);
    };
  }, [draggingOrder, finishPointerOrderDrag, movePointerOrderDrag]);
  const sidebarNavigationModel = React.useMemo<SidebarNavigationModel>(() => {
    return buildStockSidebarNavigationModel({
      sectionNames,
      grouped,
      grouping,
      collapsedSections,
      collapsedSubgroups,
      pinnedAgentIds,
      sectionOrder,
      subgroupOrder,
      searchActive,
      flat,
      order: sortContext,
    });
  }, [sectionNames, grouped, grouping, collapsedSections, collapsedSubgroups, pinnedAgentIds, sectionOrder, subgroupOrder, searchActive, flat, sortContext]);
  const virtualRows = React.useMemo<SidebarVirtualRow[]>(
    () => sidebarNavigationRows(sidebarNavigationModel),
    [sidebarNavigationModel],
  );
  virtualRowsRef.current = virtualRows;
  const agentSiblingCounts = React.useMemo(() => {
    const counts = new Map<string, number>();
    for (const row of virtualRows) {
      if (row.kind === "agent") counts.set(row.container, (counts.get(row.container) || 0) + 1);
    }
    return counts;
  }, [virtualRows]);
  const virtualOffsets = React.useMemo(() => sidebarVirtualOffsets(virtualRows), [virtualRows]);
  const [listRef, listHeight] = useMeasuredHeight<HTMLDivElement>();
  const [scrollTop, setScrollTop] = React.useState(0);
  React.useEffect(() => {
    setScrollTop(0);
    if (listRef.current) listRef.current.scrollTop = 0;
  }, [q, activityFilter, grouping, view.groupBy, view.sortBy, listRef]);
  // A drag near the list's top or bottom edge scrolls the list, so a row
  // can move past what is on screen.
  React.useEffect(() => {
    if (!draggingOrder || typeof window === "undefined" || typeof window.requestAnimationFrame !== "function") return undefined;
    let frame = 0;
    const step = () => {
      const drag = pointerDragRef.current;
      const list = listRef.current;
      if (drag?.moved && list) {
        const rect = list.getBoundingClientRect();
        const fromTop = drag.lastY - rect.top;
        const fromBottom = rect.bottom - drag.lastY;
        const inside = drag.lastX >= rect.left && drag.lastX <= rect.right;
        let delta = 0;
        if (inside && fromTop < SIDEBAR_DRAG_EDGE_PX) {
          delta = -Math.ceil(((SIDEBAR_DRAG_EDGE_PX - Math.max(0, fromTop)) / SIDEBAR_DRAG_EDGE_PX) * SIDEBAR_DRAG_EDGE_MAX_STEP_PX);
        } else if (inside && fromBottom < SIDEBAR_DRAG_EDGE_PX) {
          delta = Math.ceil(((SIDEBAR_DRAG_EDGE_PX - Math.max(0, fromBottom)) / SIDEBAR_DRAG_EDGE_PX) * SIDEBAR_DRAG_EDGE_MAX_STEP_PX);
        }
        if (delta !== 0) {
          const before = list.scrollTop;
          list.scrollTop = Math.max(0, Math.min(list.scrollHeight - list.clientHeight, before + delta));
          if (list.scrollTop !== before) {
            setScrollTop(list.scrollTop);
            updatePointerOrderTarget(drag.lastX, drag.lastY);
          }
        }
      }
      frame = window.requestAnimationFrame(step);
    };
    frame = window.requestAnimationFrame(step);
    return () => window.cancelAnimationFrame(frame);
  }, [draggingOrder, listRef, updatePointerOrderTarget]);
  const visibleRange = React.useMemo(() => sidebarVisibleRange({
    rowCount: virtualRows.length,
    offsets: virtualOffsets.offsets,
    total: virtualOffsets.total,
    scrollTop,
    listHeight,
  }), [listHeight, scrollTop, virtualOffsets, virtualRows.length]);
  const visibleRows = React.useMemo(
    () => virtualRows.slice(visibleRange.start, visibleRange.end),
    [virtualRows, visibleRange],
  );
  React.useLayoutEffect(() => {
    const pending = pendingOrderFocusRef.current;
    const list = listRef.current;
    if (!pending || !list) return;
    const rowIndex = virtualRows.findIndex((row) => pendingOrderFocusMatchesRow(pending, row));
    if (rowIndex < 0) {
      pendingOrderFocusRef.current = null;
      return;
    }
    const rowTop = virtualOffsets.offsets[rowIndex] || 0;
    const rowBottom = rowTop + virtualRowHeight(virtualRows[rowIndex]!);
    const currentTop = list.scrollTop;
    if (listHeight > 0) {
      const currentBottom = currentTop + listHeight;
      const nextTop = rowTop < currentTop
        ? rowTop
        : rowBottom > currentBottom
          ? Math.max(0, rowBottom - listHeight)
          : currentTop;
      if (nextTop !== currentTop) {
        list.scrollTop = nextTop;
        setScrollTop(nextTop);
        return;
      }
    }
    const restoreFocus = () => {
      const target = Array.from(list.querySelectorAll<HTMLElement>("[data-sidebar-order-kind]"))
        .find((element) => pendingOrderFocusMatchesElement(pending, element));
      target?.focus();
      pendingOrderFocusRef.current = null;
    };
    if (typeof window !== "undefined" && typeof window.requestAnimationFrame === "function") {
      window.requestAnimationFrame(restoreFocus);
    } else {
      window.setTimeout(restoreFocus, 0);
    }
  }, [listHeight, listRef, scrollTop, virtualOffsets, virtualRows]);
  const dragPreviewRows = React.useMemo(
    () => sidebarDragPreviewRows(virtualRows, draggingOrder),
    [virtualRows, draggingOrder],
  );
  // A click that ends a drag does not open the agent it ended on.
  const selectUnlessDropped = React.useCallback((agent: ConsoleAgent) => {
    if (suppressOrderClickRef.current) return;
    onSelect(agent);
  }, [onSelect]);

  if (collapsed) {
    return (
      <aside
        className="sidebar sidebar--collapsed"
        data-collapsed="true"
        data-testid="sidebar-root"
      >
        <i className="sidebar__grip" aria-hidden="true" />
      </aside>
    );
  }

  return (
    <aside className="sidebar" data-testid="sidebar-root">
      <div
        aria-live="polite"
        data-testid="sidebar-reorder-live"
        style={{
          position: "absolute",
          width: 1,
          height: 1,
          padding: 0,
          margin: -1,
          overflow: "hidden",
          clip: "rect(0 0 0 0)",
          whiteSpace: "nowrap",
          border: 0,
        }}
      >
        {orderAnnouncement}
      </div>
      {approvals ? <ApprovalAttention snapshot={approvals} onOpen={onOpenApproval || (() => onOpenControl("gating"))} /> : null}
      <div className="sidebar__mast">
        <div>
          <div className="sidebar__mast-title">Roster</div>
          <div className="sidebar__mast-sub">{agents.length} agents</div>
        </div>
      </div>
      <div className="sidebar__search">
        <input
          placeholder="Search roster..."
          value={q}
          onChange={(e) => setQ(e.target.value)}
          data-testid="sidebar-search"
        />
        <div className="sidebar__activity-filters" role="group" aria-label="Agent activity">
          {(["all", "working", "quiet", "unknown"] as const).map(filter => (
            <button key={filter} type="button" aria-pressed={activityFilter === filter}
              onClick={() => setActivityFilter(filter)}>
              {filter[0].toUpperCase() + filter.slice(1)}
            </button>
          ))}
        </div>
        <div className="sidebar__view" role="group" aria-label="Roster view">
          <label className="sidebar__view-control">
            <span>Group</span>
            <select
              value={view.groupBy}
              data-testid="sidebar-view-group"
              onChange={(event) => updateView({ groupBy: normalizeConsoleSidebarGroupBy(event.target.value) })}
            >
              <option value="category">Category</option>
              <option value="none">None</option>
            </select>
          </label>
          <label className="sidebar__view-control">
            <span>Sort</span>
            <select
              value={view.sortBy}
              data-testid="sidebar-view-sort"
              onChange={(event) => updateView({ sortBy: normalizeConsoleSidebarSortBy(event.target.value) })}
            >
              <option value="manual">Manual</option>
              <option value="name">Name</option>
              <option value="activity">Last activity</option>
            </select>
          </label>
        </div>
      </div>

      {(navKinds.length > 0 || customSidebarButtons.length > 0 || customPanels.length > 0) && (
          <div className="sidebar__section sidebar__section--nav">
            <div className="sidebar__sec-head">
            <span className="sidebar__sec-label">Workbench</span>
          </div>
          <div className="sidebar__navgrid">
          {navKinds.map((kind) => (
            <button
              key={kind}
              className="sidebar__navitem"
              onClick={() => onOpenControl(kind)}
              data-testid={`nav:${kind}`}
            >
              {NAV_LABEL[kind]}
            </button>
          ))}
          {customPanels.map(panel => <button key={panel.id} className="sidebar__navitem"
            data-testid={`nav-custom-panel:${panel.id}`} onClick={() => onOpenCustomPanel?.(panel.id)}>
            {panel.title}
          </button>)}
          {customSidebarButtons.map((button) => {
            if (button.panel) {
              if (!operatorPanelPath(button.panel) || !onOpenOperatorPanel) return null;
              return (
                <button
                  key={button.id}
                  className="sidebar__navitem"
                  onClick={() => onOpenOperatorPanel(button.id)}
                  data-testid={`nav-panel:${button.id}`}
                  title={button.label}
                >
                  {button.label}
                </button>
              );
            }
            const control = normalizeNavKind(button.control);
            if (control) {
              return (
                <button
                  key={button.id}
                  className="sidebar__navitem"
                  onClick={() => onOpenControl(control)}
                  data-testid={`nav-custom:${button.id}`}
                  title={button.label}
                >
                  {button.label}
                </button>
              );
            }
            if (button.href) {
              const safeHref = safeConsoleHref(button.href);
              if (!safeHref) {
                return null;
              }
              const target = button.target || undefined;
              const rel = target === "_blank" ? "noopener noreferrer" : undefined;
              return (
                <a
                  key={button.id}
                  className="sidebar__navitem"
                  href={safeHref}
                  target={target}
                  rel={rel}
                  data-testid={`nav-custom:${button.id}`}
                  title={button.label}
                >
                  {button.label}
                </a>
              );
            }
            return null;
          })}
          </div>
        </div>
      )}

      <div
        className="sidebar__virtual-list"
        ref={listRef}
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
        data-testid="sidebar-agent-list"
      >
        {filtered.length === 0 && (q || activityFilter !== "all") ? <div className="sidebar__empty" role="status">No matching agents.</div> : null}
        <div className="sidebar__virtual-space" style={{ height: `${virtualOffsets.total}px` }}>
          {visibleRows.map((row, index) => {
            const rowIndex = visibleRange.start + index;
            const top = virtualOffsets.offsets[rowIndex] || 0;
            const height = virtualRowHeight(row);
            return (
              <div
                className={`sidebar__virtual-row sidebar__virtual-row--${row.kind}`}
                key={row.key}
                style={{ transform: `translateY(${top}px)`, height: `${height}px` }}
              >
                {row.kind === "section" ? (
                  <div
                    className="sidebar__section"
                    data-collapsed={row.collapsed ? "true" : undefined}
                    data-pinned={row.pinned ? "true" : undefined}
                    data-drag-over={dragOverOrder?.kind === "section" && dragOverOrder.id === row.bucket ? dragOverOrder.where : undefined}
                  >
                    <button
                      type="button"
                      className={`sidebar__sec-head sidebar__sec-head--button ${row.reorderable ? "sidebar__order-target" : ""}`}
                      aria-expanded={!row.collapsed}
                      aria-keyshortcuts={row.reorderable ? "Alt+ArrowUp Alt+ArrowDown" : undefined}
                      data-sidebar-order-kind={row.reorderable ? "section" : undefined}
                      data-sidebar-order-id={row.reorderable ? row.bucket : undefined}
                      data-reorderable={row.reorderable ? "true" : undefined}
                      onPointerDown={row.reorderable ? (event) => beginPointerOrderDrag(event, { kind: "section", id: row.bucket }) : undefined}
                      onKeyDown={row.reorderable ? (event) => handleSectionOrderKeyDown(event, row.bucket) : undefined}
                      onClick={() => {
                        if (suppressOrderClickRef.current) return;
                        const collapseKey = row.collapseKey ?? row.bucket;
                        setCollapsedSections((current) => {
                          const next = new Set(current);
                          if (next.has(collapseKey)) next.delete(collapseKey);
                          else next.add(collapseKey);
                          writeSidebarStringSet(localSidebarStorage(), sectionCollapseStorageKey, next);
                          return next;
                        });
                      }}
                      data-testid={`sidebar-section-toggle:${row.bucket}`}
                    >
                      <span className="sidebar__sec-label">{row.bucket}</span>
                      <span className="sidebar__sec-spacer" />
                      <span className="sidebar__sec-count">{row.count}</span>
                    </button>
                  </div>
                ) : row.kind === "empty" ? (
                  <div className="sidebar__empty" data-testid={`sidebar-section-empty:${row.bucket}`}>
                    {row.sectionConfig?.empty_title ? <span className="sidebar__empty-title">{row.sectionConfig.empty_title}</span> : null}
                    <span>{row.sectionConfig?.empty_text || "No agents in this section."}</span>
                  </div>
                ) : row.kind === "subgroup" ? (
                  <button
                    type="button"
                    className={`sidebar__subgroup sidebar__subgroup--button ${row.reorderable ? "sidebar__order-target" : ""}`}
                    data-collapsed={row.collapsed ? "true" : undefined}
                    data-drag-over={dragOverOrder?.kind === "subgroup" && dragOverOrder.id === row.storageKey ? dragOverOrder.where : undefined}
                    aria-expanded={!row.collapsed}
                    aria-keyshortcuts={row.reorderable ? "Alt+ArrowUp Alt+ArrowDown" : undefined}
                    data-sidebar-order-kind={row.reorderable ? "subgroup" : undefined}
                    data-sidebar-order-id={row.reorderable ? row.storageKey : undefined}
                    data-sidebar-order-bucket={row.reorderable ? row.bucket : undefined}
                    data-reorderable={row.reorderable ? "true" : undefined}
                    data-testid={`sidebar-subgroup-toggle:${row.bucket}:${row.label}`}
                    onPointerDown={row.reorderable ? (event) => beginPointerOrderDrag(event, { kind: "subgroup", id: row.storageKey, bucket: row.bucket }) : undefined}
                    onKeyDown={row.reorderable ? (event) => handleSubgroupOrderKeyDown(event, row.storageKey, row.bucket) : undefined}
                    onClick={() => {
                      if (suppressOrderClickRef.current) return;
                      setCollapsedSubgroups((current) => {
                        const next = new Set(current);
                        if (next.has(row.storageKey)) next.delete(row.storageKey);
                        else next.add(row.storageKey);
                        writeSidebarStringSet(localSidebarStorage(), subgroupCollapseStorageKey, next);
                        return next;
                      });
                    }}
                  >
                    <span>{row.label}</span>
                    <span className="sidebar__sec-spacer" />
                    <span className="sidebar__sec-count">{row.count}</span>
                  </button>
                ) : (
                  renderAgentRow({
                    row: row.row,
                    selectedMemberId,
                    recentActivity,
                    grouping,
                    pinnedAgentIds,
                    onSelect: selectUnlessDropped,
                    onTogglePinnedAgent,
                    familyPinIds: familyPinIdsByMemberId.get(row.row.agent.member_id),
                    lastActivityMs: liveActivity.get(row.row.agent.member_id),
                    nowMs,
                    reorder: {
                      container: row.container,
                      enabled: agentReorderBlocked === null && (agentSiblingCounts.get(row.container) || 0) > 1,
                      dragging: draggingOrder?.kind === "agent"
                        && draggingOrder.container === row.container
                        && draggingOrder.id === sidebarAgentOrderKey(row.row.agent),
                      dragOver: dragOverOrder?.kind === "agent" && dragOverOrder.indicatorId === row.key
                        ? dragOverOrder.where
                        : undefined,
                      onGripPointerDown: (event) => beginPointerOrderDrag(event, {
                        kind: "agent",
                        id: sidebarAgentOrderKey(row.row.agent),
                        container: row.container,
                      }),
                      onKeyDown: (event) => handleAgentOrderKeyDown(event, row),
                    },
                  })
                )}
              </div>
            );
          })}
        </div>
      </div>
      {dragPreview && dragPreviewRows.length > 0 ? (
        <div
          className="sidebar__drag-preview"
          data-testid="sidebar-drag-preview"
          style={{
            width: `${Math.max(160, dragPreview.width)}px`,
            transform: `translate3d(${dragPreview.x + 12}px, ${dragPreview.y + 12}px, 0)`,
          }}
          aria-hidden="true"
        >
          {renderSidebarDragPreviewRows(dragPreviewRows)}
        </div>
      ) : null}
    </aside>
  );
});
