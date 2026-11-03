import type {
  ConsoleSidebarAction,
  ConsoleSidebarBlock,
  ConsoleSidebarBlockKind,
  ConsoleSidebarItem,
  ConsoleSidebarMeta,
  ConsoleSidebarSection,
  ConsoleSidebarViewState,
} from "./sidebar";
import { normalizeConsoleSidebarViewState } from "./sidebar";

export type ConsoleNavigationOrientation = "vertical" | "horizontal" | "tree" | "palette";
export type ConsoleNavigationMovePosition = "before" | "after" | "inside";
export type ConsoleNavigationNodeType = "group" | "item";

export interface ConsoleNavigationSourceRef {
  source: "sidebar-compat";
  blockId: string;
  blockKind: ConsoleSidebarBlockKind;
  blockTitle?: string | null;
  blockMeta?: ConsoleSidebarMeta[];
  blockActions?: ConsoleSidebarAction[];
  sectionId?: string;
}

export interface ConsoleNavigationOrderState {
  orderedNodeIds: string[];
}

export interface ConsoleNavigationMeta extends ConsoleSidebarMeta {}
export interface ConsoleNavigationAction extends ConsoleSidebarAction {}

export interface ConsoleNavigationNodeBase<TTarget = unknown> {
  type: ConsoleNavigationNodeType;
  id: string;
  label: string;
  subtitle?: string | null;
  selected?: boolean;
  disabled?: boolean;
  iconName?: string | null;
  meta?: ConsoleNavigationMeta[];
  actions?: ConsoleNavigationAction[];
  ariaLabel?: string;
  sourceRef?: ConsoleNavigationSourceRef;
  target?: TTarget;
}

export interface ConsoleNavigationGroup<TTarget = unknown> extends ConsoleNavigationNodeBase<TTarget> {
  type: "group";
  children: ConsoleNavigationNode<TTarget>[];
  expanded: boolean;
}

export interface ConsoleNavigationItem<TTarget = unknown> extends ConsoleNavigationNodeBase<TTarget> {
  type: "item";
  pinned?: boolean;
  unread?: boolean;
}

export type ConsoleNavigationNode<TTarget = unknown> =
  | ConsoleNavigationGroup<TTarget>
  | ConsoleNavigationItem<TTarget>;

export interface ConsoleNavigationModel<TTarget = unknown> {
  orientation?: ConsoleNavigationOrientation;
  activeNodeId?: string;
  focusNodeId?: string;
  nodes: ConsoleNavigationNode<TTarget>[];
  order: ConsoleNavigationOrderState;
}

export interface ConsoleNavigationMoveInput {
  id: string;
  targetId: string;
  position: ConsoleNavigationMovePosition;
  scope?: "tree" | "siblings";
}

export type ConsoleNavigationReorderInputSource = "keyboard" | "pointer";

export interface ConsoleNavigationReorderIntent extends ConsoleNavigationMoveInput {
  inputSource: ConsoleNavigationReorderInputSource;
}

export interface ConsoleNavigationMoveResult<TTarget = unknown> {
  model: ConsoleNavigationModel<TTarget>;
  focusNodeId: string | null;
  announcement: string;
}

interface LocatedNode<TTarget> {
  node: ConsoleNavigationNode<TTarget>;
  parentId: string | null;
  path: number[];
}

function normalizeMeta(meta: ConsoleNavigationMeta[] | null | undefined): ConsoleNavigationMeta[] {
  return (meta || []).filter((entry) => Boolean(entry?.label));
}

function normalizeActions(actions: ConsoleNavigationAction[] | null | undefined): ConsoleNavigationAction[] {
  return (actions || []).filter((action) => Boolean(action?.id && action?.label));
}

function collectNavigationNodeIds<TTarget>(nodes: ConsoleNavigationNode<TTarget>[]): string[] {
  const ids: string[] = [];
  for (const node of nodes) {
    ids.push(node.id);
    if (node.type === "group") {
      ids.push(...collectNavigationNodeIds(node.children));
    }
  }
  return ids;
}

function normalizeNode<TTarget>(
  node: ConsoleNavigationNode<TTarget> | null | undefined,
  seen: Set<string>,
): ConsoleNavigationNode<TTarget> | null {
  if (!node?.id || !node.label || seen.has(node.id)) {
    return null;
  }
  seen.add(node.id);

  const base = {
    ...node,
    meta: normalizeMeta(node.meta),
    actions: normalizeActions(node.actions),
  };

  if (node.type === "group") {
    return {
      ...base,
      type: "group",
      expanded: node.expanded !== false,
      children: (node.children || [])
        .map((child) => normalizeNode(child, seen))
        .filter(Boolean) as ConsoleNavigationNode<TTarget>[],
    };
  }

  if (node.type === "item") {
    return {
      ...base,
      type: "item",
      pinned: Boolean(node.pinned),
      unread: Boolean(node.unread),
    };
  }

  return null;
}

function mapNavigationNodes<TTarget>(
  nodes: ConsoleNavigationNode<TTarget>[],
  mapper: (node: ConsoleNavigationNode<TTarget>) => ConsoleNavigationNode<TTarget>,
): ConsoleNavigationNode<TTarget>[] {
  return nodes.map((node) => {
    const mapped = mapper(node);
    if (mapped.type !== "group") {
      return mapped;
    }
    return {
      ...mapped,
      children: mapNavigationNodes(mapped.children, mapper),
    };
  });
}

function findNavigationNode<TTarget>(
  nodes: ConsoleNavigationNode<TTarget>[],
  id: string,
  parentId: string | null = null,
  path: number[] = [],
): LocatedNode<TTarget> | null {
  for (let index = 0; index < nodes.length; index += 1) {
    const node = nodes[index]!;
    const nodePath = [...path, index];
    if (node.id === id) {
      return { node, parentId, path: nodePath };
    }
    if (node.type === "group") {
      const child = findNavigationNode(node.children, id, node.id, nodePath);
      if (child) return child;
    }
  }
  return null;
}

function isDescendantPath(path: number[], possibleDescendant: number[]): boolean {
  return path.length < possibleDescendant.length
    && path.every((segment, index) => possibleDescendant[index] === segment);
}

function removeNodeAtPath<TTarget>(
  nodes: ConsoleNavigationNode<TTarget>[],
  path: number[],
): { nodes: ConsoleNavigationNode<TTarget>[]; removed: ConsoleNavigationNode<TTarget> | null } {
  if (path.length === 0) {
    return { nodes, removed: null };
  }
  const [head, ...tail] = path;
  if (head === undefined || head < 0 || head >= nodes.length) {
    return { nodes, removed: null };
  }
  if (tail.length === 0) {
    const next = [...nodes];
    const [removed] = next.splice(head, 1);
    return { nodes: next, removed: removed || null };
  }
  const node = nodes[head]!;
  if (node.type !== "group") {
    return { nodes, removed: null };
  }
  const childResult = removeNodeAtPath(node.children, tail);
  const next = [...nodes];
  next[head] = { ...node, children: childResult.nodes };
  return { nodes: next, removed: childResult.removed };
}

function insertNode<TTarget>(
  nodes: ConsoleNavigationNode<TTarget>[],
  targetId: string,
  position: ConsoleNavigationMovePosition,
  nodeToInsert: ConsoleNavigationNode<TTarget>,
): { nodes: ConsoleNavigationNode<TTarget>[]; inserted: boolean } {
  const next = [...nodes];
  for (let index = 0; index < next.length; index += 1) {
    const node = next[index]!;
    if (node.id === targetId) {
      if (position === "inside" && node.type === "group") {
        next[index] = { ...node, expanded: true, children: [...node.children, nodeToInsert] };
        return { nodes: next, inserted: true };
      }
      const offset = position === "after" ? 1 : 0;
      next.splice(index + offset, 0, nodeToInsert);
      return { nodes: next, inserted: true };
    }
    if (node.type === "group") {
      const childResult = insertNode(node.children, targetId, position, nodeToInsert);
      if (childResult.inserted) {
        next[index] = {
          ...node,
          children: childResult.nodes,
        };
        return { nodes: next, inserted: true };
      }
    }
  }
  return { nodes, inserted: false };
}

function navigationMoveAnnouncement(
  moved: ConsoleNavigationNode,
  target: ConsoleNavigationNode,
  position: ConsoleNavigationMovePosition,
): string {
  if (position === "inside") {
    return `Moved ${moved.label} into ${target.label}.`;
  }
  return `Moved ${moved.label} ${position} ${target.label}.`;
}

export function normalizeConsoleNavigationModel<TTarget>(
  model: ConsoleNavigationModel<TTarget> | null | undefined,
): ConsoleNavigationModel<TTarget> {
  const seen = new Set<string>();
  const nodes = (model?.nodes || [])
    .map((node) => normalizeNode(node, seen))
    .filter(Boolean) as ConsoleNavigationNode<TTarget>[];
  const ids = collectNavigationNodeIds(nodes);
  const idSet = new Set(ids);
  const activeNodeId = model?.activeNodeId && idSet.has(model.activeNodeId) ? model.activeNodeId : undefined;
  const focusNodeId = model?.focusNodeId && idSet.has(model.focusNodeId)
    ? model.focusNodeId
    : activeNodeId;
  const orderedNodeIds = (model?.order?.orderedNodeIds || []).filter((id) => idSet.has(id));

  return {
    orientation: model?.orientation,
    activeNodeId,
    focusNodeId,
    nodes,
    order: { orderedNodeIds: orderedNodeIds.length ? orderedNodeIds : ids },
  };
}

export function selectConsoleNavigationNode<TTarget>(
  model: ConsoleNavigationModel<TTarget>,
  id: string,
): ConsoleNavigationModel<TTarget> {
  const normalized = normalizeConsoleNavigationModel(model);
  if (!findNavigationNode(normalized.nodes, id)) {
    return normalized;
  }
  return {
    ...normalized,
    activeNodeId: id,
    focusNodeId: id,
    nodes: mapNavigationNodes(normalized.nodes, (node) => ({
      ...node,
      selected: node.id === id,
    })),
  };
}

export function toggleConsoleNavigationGroup<TTarget>(
  model: ConsoleNavigationModel<TTarget>,
  id: string,
): ConsoleNavigationModel<TTarget> {
  const normalized = normalizeConsoleNavigationModel(model);
  const located = findNavigationNode(normalized.nodes, id);
  if (!located || located.node.type !== "group") {
    return normalized;
  }
  return {
    ...normalized,
    focusNodeId: id,
    nodes: mapNavigationNodes(normalized.nodes, (node) => (
      node.type === "group" && node.id === id
        ? { ...node, expanded: !node.expanded }
        : node
    )),
  };
}

export function pinConsoleNavigationNode<TTarget>(
  model: ConsoleNavigationModel<TTarget>,
  id: string,
  pinned = true,
): ConsoleNavigationModel<TTarget> {
  const normalized = normalizeConsoleNavigationModel(model);
  const located = findNavigationNode(normalized.nodes, id);
  if (!located || located.node.type !== "item") {
    return normalized;
  }
  return {
    ...normalized,
    focusNodeId: id,
    nodes: mapNavigationNodes(normalized.nodes, (node) => (
      node.type === "item" && node.id === id
        ? { ...node, pinned }
        : node
    )),
  };
}

export function canMoveConsoleNavigationNode<TTarget>(
  model: ConsoleNavigationModel<TTarget>,
  input: ConsoleNavigationMoveInput,
): boolean {
  const normalized = normalizeConsoleNavigationModel(model);
  const moved = findNavigationNode(normalized.nodes, input.id);
  const target = findNavigationNode(normalized.nodes, input.targetId);
  if (!moved || !target || moved.node.disabled || target.node.disabled) {
    return false;
  }
  if (moved.node.id === target.node.id) {
    return false;
  }
  if (isDescendantPath(moved.path, target.path)) {
    return false;
  }
  if (input.position === "inside" && target.node.type !== "group") {
    return false;
  }
  if (input.scope === "siblings" && moved.parentId !== target.parentId) {
    return false;
  }
  return true;
}

export function moveConsoleNavigationNode<TTarget>(
  model: ConsoleNavigationModel<TTarget>,
  input: ConsoleNavigationMoveInput,
): ConsoleNavigationMoveResult<TTarget> {
  const normalized = normalizeConsoleNavigationModel(model);
  const moved = findNavigationNode(normalized.nodes, input.id);
  const target = findNavigationNode(normalized.nodes, input.targetId);
  if (!moved || !target || !canMoveConsoleNavigationNode(normalized, input)) {
    return {
      model: normalized,
      focusNodeId: normalized.focusNodeId || null,
      announcement: "Move unavailable.",
    };
  }

  const removed = removeNodeAtPath(normalized.nodes, moved.path);
  if (!removed.removed) {
    return {
      model: normalized,
      focusNodeId: normalized.focusNodeId || null,
      announcement: "Move unavailable.",
    };
  }

  const inserted = insertNode(removed.nodes, input.targetId, input.position, removed.removed);
  if (!inserted.inserted) {
    return {
      model: normalized,
      focusNodeId: normalized.focusNodeId || null,
      announcement: "Move unavailable.",
    };
  }
  const nodes = inserted.nodes;
  const next = normalizeConsoleNavigationModel({
    ...normalized,
    focusNodeId: removed.removed.id,
    nodes,
    order: { orderedNodeIds: collectNavigationNodeIds(nodes) },
  });

  return {
    model: next,
    focusNodeId: removed.removed.id,
    announcement: navigationMoveAnnouncement(removed.removed, target.node, input.position),
  };
}

export function applyConsoleNavigationReorderIntent<TTarget>(
  model: ConsoleNavigationModel<TTarget>,
  intent: ConsoleNavigationReorderIntent,
): ConsoleNavigationMoveResult<TTarget> {
  return moveConsoleNavigationNode(model, intent);
}

export function consoleNavigationFromSidebarViewState(
  viewState: ConsoleSidebarViewState,
): ConsoleNavigationModel {
  const normalized = normalizeConsoleSidebarViewState(viewState);
  const nodes: ConsoleNavigationNode[] = [];

  for (const block of normalized.blocks) {
    if (block.kind === "action_strip") {
      nodes.push({
        type: "group",
        id: `sidebar:block:${block.id}`,
        label: block.title || block.id,
        meta: block.meta,
        actions: block.actions,
        expanded: true,
        children: [],
        sourceRef: sidebarBlockSourceRef(block),
      });
      continue;
    }

    for (const section of block.sections || []) {
      nodes.push({
        type: "group",
        id: `sidebar:section:${block.id}:${section.id}`,
        label: section.title,
        subtitle: section.subtitle,
        iconName: section.iconName,
        meta: section.meta,
        selected: section.selected,
        actions: section.actions,
        expanded: true,
        children: section.items.map((item) => sidebarItemToNavigationNode(block, section, item)),
        sourceRef: {
          ...sidebarBlockSourceRef(block),
          sectionId: section.id,
        },
      });
    }
  }

  return normalizeConsoleNavigationModel({ nodes, order: { orderedNodeIds: [] } });
}

function sidebarBlockSourceRef(block: ConsoleSidebarBlock): ConsoleNavigationSourceRef {
  return {
    source: "sidebar-compat",
    blockId: block.id,
    blockKind: block.kind,
    blockTitle: block.title,
    blockMeta: block.meta,
    blockActions: block.actions,
  };
}

function sidebarItemToNavigationNode(
  block: ConsoleSidebarBlock,
  section: ConsoleSidebarSection,
  item: ConsoleSidebarItem,
): ConsoleNavigationItem {
  return {
    type: "item",
    id: item.id,
    label: item.title,
    subtitle: item.subtitle,
    selected: item.selected,
    unread: item.unread,
    pinned: item.pinned,
    disabled: item.disabled,
    iconName: item.iconName,
    meta: item.meta,
    actions: item.actions,
    sourceRef: {
      ...sidebarBlockSourceRef(block),
      sectionId: section.id,
    },
  };
}

export function consoleNavigationToSidebarViewState(
  model: ConsoleNavigationModel,
): ConsoleSidebarViewState {
  const normalized = normalizeConsoleNavigationModel(model);
  const blocks = new Map<string, ConsoleSidebarBlock>();

  for (const node of normalized.nodes) {
    const ref = node.sourceRef;
    if (!ref || ref.source !== "sidebar-compat") {
      continue;
    }

    if (ref.blockKind === "action_strip") {
      const block: ConsoleSidebarBlock = {
        id: ref.blockId,
        kind: "action_strip",
        actions: node.actions || ref.blockActions || [],
      };
      if (ref.blockTitle) block.title = ref.blockTitle;
      if (ref.blockMeta) block.meta = ref.blockMeta;
      blocks.set(ref.blockId, block);
      continue;
    }

    const block = blocks.get(ref.blockId) || {
      id: ref.blockId,
      kind: "list",
      sections: [],
    };
    if (ref.blockTitle) block.title = ref.blockTitle;
    if (ref.blockMeta) block.meta = ref.blockMeta;
    if (ref.blockActions) block.actions = ref.blockActions;
    block.sections = block.sections || [];
    block.sections.push(navigationGroupToSidebarSection(node));
    blocks.set(ref.blockId, block);
  }

  return normalizeConsoleSidebarViewState({ blocks: Array.from(blocks.values()) });
}

function navigationGroupToSidebarSection(node: ConsoleNavigationNode): ConsoleSidebarSection {
  const group = node.type === "group" ? node : null;
  const section: ConsoleSidebarSection = {
    id: node.sourceRef?.sectionId || node.id,
    title: node.label,
    items: (group?.children || [])
      .filter((child): child is ConsoleNavigationItem => child.type === "item")
      .map(navigationItemToSidebarItem),
  };
  if (node.subtitle) section.subtitle = node.subtitle;
  if (node.iconName) section.iconName = node.iconName;
  if (node.meta) section.meta = node.meta;
  if (node.selected) section.selected = node.selected;
  if (node.actions) section.actions = node.actions;
  return section;
}

function navigationItemToSidebarItem(child: ConsoleNavigationItem): ConsoleSidebarItem {
  const item: ConsoleSidebarItem = {
    id: child.id,
    title: child.label,
  };
  if (child.subtitle) item.subtitle = child.subtitle;
  if (child.selected) item.selected = child.selected;
  if (child.unread) item.unread = child.unread;
  if (child.pinned) item.pinned = child.pinned;
  if (child.disabled) item.disabled = child.disabled;
  if (child.iconName) item.iconName = child.iconName;
  if (child.meta) item.meta = child.meta;
  if (child.actions) item.actions = child.actions;
  return item;
}
