export type ConsoleDockPanelMode = "console" | "terminal";
export type ConsoleDockSplitDirection = "horizontal" | "vertical";
export type ConsoleDockPanelSplitDirection = "up" | "down" | "left" | "right";
export type ConsoleDockOpenIntent = "replace_focused" | "new_tab" | "split_right" | "split_down";
export type ConsoleDockPresetId = "single" | "two_columns" | "two_rows" | "grid";

export interface ConsoleDockTarget {
  id: string;
  kind: string;
  title: string;
  subtitle?: string | null;
  iconName?: string | null;
  badgeLabel?: string | null;
}

export type BrowserDockTarget = ConsoleDockTarget & {
  id: `browser-panel:${string}`;
  kind: "browser";
  title: "Browser";
  browserPanelId: string;
};

export interface ConsoleDockPanelState<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  id: string;
  target: TTarget | null;
  mode: ConsoleDockPanelMode;
}

export interface ConsoleDockPanelNode {
  kind: "panel";
  panelId: string;
}

export interface ConsoleDockSplitNode {
  kind: "split";
  id: string;
  direction: ConsoleDockSplitDirection;
  ratio?: number | null;
  first: ConsoleDockNode;
  second: ConsoleDockNode;
}

export type ConsoleDockNode = ConsoleDockPanelNode | ConsoleDockSplitNode;

export interface ConsoleDockTabState {
  id: string;
  presetId: ConsoleDockPresetId;
  layout: ConsoleDockNode;
}

export interface ConsoleDockState<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  tabs: ConsoleDockTabState[];
  panels: ConsoleDockPanelState<TTarget>[];
  activeTabId: string | null;
  focusedPanelId: string | null;
}

export interface ConsoleDockPreset {
  id: ConsoleDockPresetId;
  label: string;
  description: string;
  iconName: string;
}

export interface ConsoleDockPanelView<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  id: string;
  title: string;
  subtitle?: string | null;
  iconName?: string | null;
  target?: TTarget | null;
  mode?: ConsoleDockPanelMode;
  statusLabel?: string | null;
  badgeLabel?: string | null;
  closable?: boolean;
  dirty?: boolean;
}

export interface ConsoleDockTabView {
  id: string;
  title: string;
  subtitle?: string | null;
  iconName?: string | null;
  badgeLabel?: string | null;
  closable?: boolean;
  dirty?: boolean;
  layout: ConsoleDockNode;
}

export interface ConsoleDockViewState<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  tabs: ConsoleDockTabView[];
  panels: ConsoleDockPanelView<TTarget>[];
  activeTabId: string | null;
  focusedPanelId: string | null;
}

export interface ConsoleDockSuggestTargetsArgs<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  count: number;
  preferred: TTarget | null;
  excludedIds: string[];
}

export type ConsoleDockSuggestTargets<TTarget extends ConsoleDockTarget = ConsoleDockTarget> = (
  args: ConsoleDockSuggestTargetsArgs<TTarget>,
) => Array<TTarget | null>;

export interface ConsoleDockCreatePanelStateArgs<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  target: TTarget | null;
  sourcePanel?: ConsoleDockPanelState<TTarget> | null;
}

export type ConsoleDockCreatePanelState<TTarget extends ConsoleDockTarget = ConsoleDockTarget> = (
  args: ConsoleDockCreatePanelStateArgs<TTarget>,
) => ConsoleDockPanelState<TTarget>;

export interface ConsoleDockPresetState<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  presetId: ConsoleDockPresetId;
  layout: ConsoleDockNode;
  panels: ConsoleDockPanelState<TTarget>[];
  focusedPanelId: string | null;
}

export interface BuildConsoleDockPresetStateOptions<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  presetId: ConsoleDockPresetId;
  preferredTarget?: TTarget | null;
  preferredPanel?: ConsoleDockPanelState<TTarget> | null;
  createPanelState: ConsoleDockCreatePanelState<TTarget>;
  createSplitId: () => string;
  suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
}

export interface CreateConsoleDockStateOptions<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  initialTarget?: TTarget | null;
  initialPresetId?: ConsoleDockPresetId;
  createPanelState: ConsoleDockCreatePanelState<TTarget>;
  createTabId: () => string;
  createSplitId: () => string;
  suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
}

export interface ApplyConsoleDockPresetOptions<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  presetId: ConsoleDockPresetId;
  createPanelState: ConsoleDockCreatePanelState<TTarget>;
  createSplitId: () => string;
  suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
}

export interface OpenConsoleDockTargetOptions<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  intent?: ConsoleDockOpenIntent;
  createPanelState: ConsoleDockCreatePanelState<TTarget>;
  createTabId: () => string;
  createSplitId: () => string;
  suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
}

export interface ConsoleDockAction<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  type:
    | "create_tab"
    | "select_tab"
    | "close_tab"
    | "focus_panel"
    | "set_panel_target"
    | "set_panel_mode"
    | "open_target"
    | "resize_split"
    | "split_panel"
    | "close_panel"
    | "apply_preset";
  tabId?: string;
  panelId?: string;
  splitId?: string;
  ratio?: number;
  mode?: ConsoleDockPanelMode;
  target?: TTarget | null;
  intent?: ConsoleDockOpenIntent;
  direction?: ConsoleDockPanelSplitDirection;
  presetId?: ConsoleDockPresetId;
  /** Start a genuinely empty tab instead of cloning the focused panel. */
  blank?: boolean;
}

export interface ConsoleDockResolvePanelViewArgs<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  panel: ConsoleDockPanelState<TTarget>;
  activePanelCount: number;
  focused: boolean;
}

export interface ConsoleDockResolveTabViewArgs<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  tab: ConsoleDockTabState;
  panels: ConsoleDockPanelState<TTarget>[];
  active: boolean;
  focusedPanelId: string | null;
}

export interface BuildConsoleDockViewStateOptions<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  resolvePanelView?: (
    args: ConsoleDockResolvePanelViewArgs<TTarget>,
  ) => Partial<Omit<ConsoleDockPanelView<TTarget>, "id" | "target" | "mode">>;
  resolveTabView?: (
    args: ConsoleDockResolveTabViewArgs<TTarget>,
  ) => Partial<Omit<ConsoleDockTabView, "id" | "layout">>;
}

const CONSOLE_DOCK_PRESETS: ConsoleDockPreset[] = [
  {
    id: "single",
    label: "Single",
    description: "One focused panel.",
    iconName: "i-compose",
  },
  {
    id: "two_columns",
    label: "Two columns",
    description: "Side-by-side work.",
    iconName: "i-sidebar-toggle",
  },
  {
    id: "two_rows",
    label: "Two rows",
    description: "Top and bottom pairing.",
    iconName: "i-swap",
  },
  {
    id: "grid",
    label: "Grid",
    description: "A 2x2 comparison layout.",
    iconName: "i-team",
  },
];

function isDockPanelNode(node: ConsoleDockNode | null | undefined): node is ConsoleDockPanelNode {
  return Boolean(node && node.kind === "panel" && node.panelId);
}

function isDockSplitNode(node: ConsoleDockNode | null | undefined): node is ConsoleDockSplitNode {
  return Boolean(
    node
    && node.kind === "split"
    && node.id
    && (node.direction === "horizontal" || node.direction === "vertical")
    && node.first
    && node.second,
  );
}

function normalizeTarget<TTarget extends ConsoleDockTarget>(
  target: TTarget | null | undefined,
): TTarget | null {
  if (!target?.id || !target?.kind || !target?.title) {
    return null;
  }
  return target;
}

function normalizePanelState<TTarget extends ConsoleDockTarget>(
  panel: ConsoleDockPanelState<TTarget> | null | undefined,
): ConsoleDockPanelState<TTarget> | null {
  if (!panel?.id) {
    return null;
  }
  return {
    id: panel.id,
    target: normalizeTarget(panel.target),
    mode: panel.mode === "terminal" ? "terminal" : "console",
  };
}

function normalizeNode(
  node: ConsoleDockNode | null | undefined,
  validPanelIds: Set<string>,
): ConsoleDockNode | null {
  if (isDockPanelNode(node)) {
    return validPanelIds.has(node.panelId)
      ? { kind: "panel", panelId: node.panelId }
      : null;
  }

  if (!isDockSplitNode(node)) {
    return null;
  }

  const first = normalizeNode(node.first, validPanelIds);
  const second = normalizeNode(node.second, validPanelIds);

  if (first && second) {
    return {
      kind: "split",
      id: node.id,
      direction: node.direction,
      ratio: typeof node.ratio === "number" && node.ratio > 0 && node.ratio < 1 ? node.ratio : 0.5,
      first,
      second,
    };
  }

  return first || second;
}

function panelNode(panelId: string): ConsoleDockNode {
  return { kind: "panel", panelId };
}

function presetMeta(presetId: ConsoleDockPresetId): ConsoleDockPreset {
  return CONSOLE_DOCK_PRESETS.find((entry) => entry.id === presetId) || CONSOLE_DOCK_PRESETS[0]!;
}

function uniqueTargets<TTarget extends ConsoleDockTarget>(
  values: Array<TTarget | null>,
  excludedIds: string[],
): Array<TTarget | null> {
  const usedIds = new Set(excludedIds);
  const results: Array<TTarget | null> = [];

  for (const target of values) {
    if (!target) {
      results.push(null);
      continue;
    }
    if (usedIds.has(target.id)) {
      continue;
    }
    usedIds.add(target.id);
    results.push(target);
  }

  return results;
}

function suggestDockTargets<TTarget extends ConsoleDockTarget>({
  count,
  preferred = null,
  excludedIds = [],
  suggestTargets,
}: {
  count: number;
  preferred?: TTarget | null;
  excludedIds?: string[];
  suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
}): Array<TTarget | null> {
  const suggested = uniqueTargets(
    suggestTargets?.({ count, preferred: preferred || null, excludedIds }) || [],
    excludedIds,
  );
  const results: Array<TTarget | null> = [];
  const usedIds = new Set(excludedIds);

  for (const target of suggested) {
    if (!target) {
      continue;
    }
    if (usedIds.has(target.id)) {
      continue;
    }
    usedIds.add(target.id);
    results.push(target);
    if (results.length >= count) {
      return results;
    }
  }

  while (results.length < count) {
    if (preferred && !usedIds.has(preferred.id)) {
      usedIds.add(preferred.id);
      results.push(preferred);
    } else {
      results.push(null);
    }
  }

  return results;
}

function replacePanelStates<TTarget extends ConsoleDockTarget>(
  panels: ConsoleDockPanelState<TTarget>[],
  nextPanels: ConsoleDockPanelState<TTarget>[],
): ConsoleDockPanelState<TTarget>[] {
  const nextById = new Map(nextPanels.map((panel) => [panel.id, panel] as const));
  const filtered = panels.filter((panel) => !nextById.has(panel.id));
  return [...filtered, ...nextPanels];
}

export function consoleDockPresets(): ConsoleDockPreset[] {
  return CONSOLE_DOCK_PRESETS;
}

export function collectConsoleDockPanelIds(node: ConsoleDockNode | null | undefined): string[] {
  if (isDockPanelNode(node)) {
    return [node.panelId];
  }

  if (!isDockSplitNode(node)) {
    return [];
  }

  return [
    ...collectConsoleDockPanelIds(node.first),
    ...collectConsoleDockPanelIds(node.second),
  ];
}

export function findConsoleDockFirstPanelId(node: ConsoleDockNode | null | undefined): string | null {
  if (isDockPanelNode(node)) {
    return node.panelId;
  }

  if (!isDockSplitNode(node)) {
    return null;
  }

  return findConsoleDockFirstPanelId(node.first) || findConsoleDockFirstPanelId(node.second);
}

export function replaceConsoleDockPanelNode(
  node: ConsoleDockNode,
  panelId: string,
  replacement: ConsoleDockNode,
): ConsoleDockNode {
  if (node.kind === "panel") {
    return node.panelId === panelId ? replacement : node;
  }

  return {
    ...node,
    first: replaceConsoleDockPanelNode(node.first, panelId, replacement),
    second: replaceConsoleDockPanelNode(node.second, panelId, replacement),
  };
}

export function removeConsoleDockPanelNode(
  node: ConsoleDockNode | null | undefined,
  panelId: string,
): ConsoleDockNode | null {
  if (!node) {
    return null;
  }

  if (node.kind === "panel") {
    return node.panelId === panelId ? null : node;
  }

  const nextFirst = removeConsoleDockPanelNode(node.first, panelId);
  const nextSecond = removeConsoleDockPanelNode(node.second, panelId);

  if (nextFirst && nextSecond) {
    return {
      ...node,
      first: nextFirst,
      second: nextSecond,
    };
  }

  return nextFirst || nextSecond;
}

function clampConsoleDockSplitRatio(ratio: number | null | undefined): number {
  if (typeof ratio !== "number" || Number.isNaN(ratio)) {
    return 0.5;
  }
  return Math.min(0.88, Math.max(0.12, ratio));
}

export function updateConsoleDockSplitRatio(
  node: ConsoleDockNode,
  splitId: string,
  ratio: number,
): ConsoleDockNode {
  if (node.kind === "panel") {
    return node;
  }

  if (node.id === splitId) {
    return {
      ...node,
      ratio: clampConsoleDockSplitRatio(ratio),
    };
  }

  return {
    ...node,
    first: updateConsoleDockSplitRatio(node.first, splitId, ratio),
    second: updateConsoleDockSplitRatio(node.second, splitId, ratio),
  };
}

export function consoleDockSplitDirectionAxis(
  direction: ConsoleDockPanelSplitDirection,
): ConsoleDockSplitDirection {
  return direction === "left" || direction === "right"
    ? "horizontal"
    : "vertical";
}

export function consoleDockSplitDirectionPrecedes(
  direction: ConsoleDockPanelSplitDirection,
): boolean {
  return direction === "left" || direction === "up";
}

export function normalizeConsoleDockState<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget> | null | undefined,
): ConsoleDockState<TTarget> {
  const panels = (state?.panels || [])
    .map((panel) => normalizePanelState(panel))
    .filter(Boolean) as ConsoleDockPanelState<TTarget>[];
  const validPanelIds = new Set(panels.map((panel) => panel.id));

  const tabs = (state?.tabs || [])
    .filter((tab) => Boolean(tab?.id))
    .map((tab) => ({
      id: tab.id,
      presetId: tab.presetId || "single",
      layout: normalizeNode(tab.layout, validPanelIds),
    }))
    .filter((tab) => Boolean(tab.layout)) as Array<ConsoleDockTabState & { layout: ConsoleDockNode }>;

  const activeTabId = tabs.some((tab) => tab.id === state?.activeTabId)
    ? state?.activeTabId || null
    : tabs[0]?.id || null;
  const activeTab = tabs.find((tab) => tab.id === activeTabId) || null;
  const activePanelIds = activeTab ? collectConsoleDockPanelIds(activeTab.layout) : [];
  const focusedPanelId = state?.focusedPanelId && activePanelIds.includes(state.focusedPanelId)
    ? state.focusedPanelId
    : activePanelIds[0] || null;

  return {
    tabs,
    panels,
    activeTabId,
    focusedPanelId,
  };
}

export function buildConsoleDockPresetState<TTarget extends ConsoleDockTarget>({
  presetId,
  preferredTarget = null,
  preferredPanel = null,
  createPanelState,
  createSplitId,
  suggestTargets,
}: BuildConsoleDockPresetStateOptions<TTarget>): ConsoleDockPresetState<TTarget> {
  const requestedCount = presetId === "grid" ? 4 : presetId === "single" ? 1 : 2;
  const reservedPrimaryTarget = preferredTarget ?? preferredPanel?.target ?? null;
  const suggestionCount = requestedCount - (reservedPrimaryTarget ? 1 : 0);
  const suggestedTargets = suggestionCount > 0
    ? suggestDockTargets({
      count: suggestionCount,
      preferred: reservedPrimaryTarget ? null : preferredTarget,
      excludedIds: reservedPrimaryTarget ? [reservedPrimaryTarget.id] : [],
      suggestTargets,
    })
    : [];
  const [firstTarget, ...remainingTargets] = reservedPrimaryTarget
    ? [reservedPrimaryTarget, ...suggestedTargets]
    : suggestedTargets;
  const [secondTarget, thirdTarget, suggestedFourthTarget] = remainingTargets.filter(
    (target): target is TTarget => Boolean(target),
  );
  const fourthTarget = suggestedFourthTarget || null;
  const createEmptyFourthSlot = presetId === "grid" && Boolean(thirdTarget) && !fourthTarget;

  const primary = createPanelState({
    target: preferredPanel ? (preferredTarget ?? preferredPanel.target) : (firstTarget || null),
    sourcePanel: preferredPanel || null,
  });

  const singlePanelState = (): ConsoleDockPresetState<TTarget> => ({
    presetId: "single",
    layout: panelNode(primary.id),
    panels: [primary],
    focusedPanelId: primary.id,
  });

  if (presetId === "single") {
    return singlePanelState();
  }

  if (presetId === "two_columns") {
    if (!secondTarget) return singlePanelState();
    const right = createPanelState({ target: secondTarget || null, sourcePanel: preferredPanel || primary });
    return {
      presetId,
      layout: {
        kind: "split",
        id: createSplitId(),
        direction: "horizontal",
        ratio: 0.5,
        first: panelNode(primary.id),
        second: panelNode(right.id),
      },
      panels: [primary, right],
      focusedPanelId: primary.id,
    };
  }

  if (presetId === "two_rows") {
    if (!secondTarget) return singlePanelState();
    const bottom = createPanelState({ target: secondTarget || null, sourcePanel: preferredPanel || primary });
    return {
      presetId,
      layout: {
        kind: "split",
        id: createSplitId(),
        direction: "vertical",
        ratio: 0.5,
        first: panelNode(primary.id),
        second: panelNode(bottom.id),
      },
      panels: [primary, bottom],
      focusedPanelId: primary.id,
    };
  }

  if (!secondTarget && !thirdTarget && !fourthTarget) {
    return singlePanelState();
  }

  const rightTop = createPanelState({ target: secondTarget || null, sourcePanel: preferredPanel || primary });
  if (!thirdTarget) {
    return {
      presetId: "two_columns",
      layout: {
        kind: "split",
        id: createSplitId(),
        direction: "horizontal",
        ratio: 0.5,
        first: panelNode(primary.id),
        second: panelNode(rightTop.id),
      },
      panels: [primary, rightTop],
      focusedPanelId: primary.id,
    };
  }

  const leftBottom = createPanelState({ target: thirdTarget, sourcePanel: preferredPanel || primary });
  if (!fourthTarget && !createEmptyFourthSlot) {
    return {
      presetId,
      layout: {
        kind: "split",
        id: createSplitId(),
        direction: "horizontal",
        ratio: 0.5,
        first: panelNode(primary.id),
        second: {
          kind: "split",
          id: createSplitId(),
          direction: "vertical",
          ratio: 0.5,
          first: panelNode(rightTop.id),
          second: panelNode(leftBottom.id),
        },
      },
      panels: [primary, rightTop, leftBottom],
      focusedPanelId: primary.id,
    };
  }

  const rightBottom = createPanelState({
    target: fourthTarget,
    sourcePanel: preferredPanel || primary,
  });

  return {
    presetId,
    layout: {
      kind: "split",
      id: createSplitId(),
      direction: "horizontal",
      ratio: 0.5,
      first: {
        kind: "split",
        id: createSplitId(),
        direction: "vertical",
        ratio: 0.5,
        first: panelNode(primary.id),
        second: panelNode(leftBottom.id),
      },
      second: {
        kind: "split",
        id: createSplitId(),
        direction: "vertical",
        ratio: 0.5,
        first: panelNode(rightTop.id),
        second: panelNode(rightBottom.id),
      },
    },
    panels: [primary, rightTop, leftBottom, rightBottom],
    focusedPanelId: primary.id,
  };
}

export function createConsoleDockState<TTarget extends ConsoleDockTarget>({
  initialTarget = null,
  initialPresetId = "single",
  createPanelState,
  createTabId,
  createSplitId,
  suggestTargets,
}: CreateConsoleDockStateOptions<TTarget>): ConsoleDockState<TTarget> {
  const initial = buildConsoleDockPresetState({
    presetId: initialPresetId,
    preferredTarget: initialTarget,
    createPanelState,
    createSplitId,
    suggestTargets,
  });
  const firstTabId = createTabId();

  return {
    tabs: [{
      id: firstTabId,
      presetId: initial.presetId,
      layout: initial.layout,
    }],
    panels: initial.panels,
    activeTabId: firstTabId,
    focusedPanelId: initial.focusedPanelId,
  };
}

export function selectConsoleDockTab<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  tabId: string,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const tab = normalized.tabs.find((entry) => entry.id === tabId) || null;
  const focusedPanelId = tab ? findConsoleDockFirstPanelId(tab.layout) : normalized.focusedPanelId;
  return {
    ...normalized,
    activeTabId: tab ? tab.id : normalized.activeTabId,
    focusedPanelId,
  };
}

export function focusConsoleDockPanel<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  panelId: string,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  return normalized.panels.some((panel) => panel.id === panelId)
    ? {
        ...normalized,
        focusedPanelId: panelId,
      }
    : normalized;
}

export function setConsoleDockPanelTarget<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  panelId: string,
  target: TTarget | null,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  return {
    ...normalized,
    panels: normalized.panels.map((panel) => (
      panel.id === panelId
        ? {
            ...panel,
            target: normalizeTarget(target),
          }
        : panel
    )),
  };
}

export function setConsoleDockPanelMode<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  panelId: string,
  mode: ConsoleDockPanelMode,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  return {
    ...normalized,
    panels: normalized.panels.map((panel) => (
      panel.id === panelId
        ? {
            ...panel,
            mode,
          }
        : panel
    )),
  };
}

export function createConsoleDockTab<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  options: Omit<CreateConsoleDockStateOptions<TTarget>, "initialPresetId" | "initialTarget">,
  behavior: { blank?: boolean } = {},
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const preferredPanel = !behavior.blank && normalized.focusedPanelId
    ? normalized.panels.find((panel) => panel.id === normalized.focusedPanelId) || null
    : null;
  const presetState = buildConsoleDockPresetState({
    presetId: "single",
    preferredTarget: preferredPanel?.target || null,
    preferredPanel,
    createPanelState: options.createPanelState,
    createSplitId: options.createSplitId,
    // An explicitly blank tab is a drafting surface. Target suggestions are
    // useful for layout presets, but must not silently turn New tab into a
    // duplicate or arbitrary conversation.
    suggestTargets: behavior.blank ? undefined : options.suggestTargets,
  });
  const tabId = options.createTabId();

  return {
    ...normalized,
    tabs: [
      ...normalized.tabs,
      {
        id: tabId,
        presetId: "single",
        layout: presetState.layout,
      },
    ],
    panels: replacePanelStates(normalized.panels, presetState.panels),
    activeTabId: tabId,
    focusedPanelId: presetState.focusedPanelId,
  };
}

export function closeConsoleDockTab<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  tabId: string,
  options: Omit<CreateConsoleDockStateOptions<TTarget>, "initialPresetId" | "initialTarget">,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const closingIndex = normalized.tabs.findIndex((tab) => tab.id === tabId);
  if (closingIndex < 0) {
    return normalized;
  }

  if (normalized.tabs.length <= 1) {
    return createConsoleDockState({
      initialPresetId: "single",
      createPanelState: options.createPanelState,
      createSplitId: options.createSplitId,
      createTabId: () => normalized.tabs[0]?.id || options.createTabId(),
      suggestTargets: options.suggestTargets,
    });
  }

  const closingTab = normalized.tabs[closingIndex]!;
  const removePanelIds = new Set(collectConsoleDockPanelIds(closingTab.layout));
  const nextTabs = normalized.tabs.filter((tab) => tab.id !== tabId);
  const nextActiveTabId = normalized.activeTabId === tabId
    ? (nextTabs[Math.max(0, closingIndex - 1)]?.id || nextTabs[0]?.id || null)
    : normalized.activeTabId;
  const nextState = {
    tabs: nextTabs,
    panels: normalized.panels.filter((panel) => !removePanelIds.has(panel.id)),
    activeTabId: nextActiveTabId,
    focusedPanelId: normalized.focusedPanelId,
  };

  return normalizeConsoleDockState(nextState);
}

export function openConsoleDockTarget<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  target: TTarget,
  options: OpenConsoleDockTargetOptions<TTarget>,
): ConsoleDockState<TTarget> {
  const intent = options.intent || "replace_focused";
  const normalized = normalizeConsoleDockState<TTarget>(state);

  if (intent === "new_tab") {
    const presetState = buildConsoleDockPresetState({
      presetId: "single",
      preferredTarget: target,
      createPanelState: options.createPanelState,
      createSplitId: options.createSplitId,
      suggestTargets: options.suggestTargets,
    });
    const tabId = options.createTabId();
    return {
      ...normalized,
      tabs: [
        ...normalized.tabs,
        {
          id: tabId,
          presetId: "single",
          layout: presetState.layout,
        },
      ],
      panels: replacePanelStates(normalized.panels, presetState.panels),
      activeTabId: tabId,
      focusedPanelId: presetState.focusedPanelId,
    };
  }

  if (intent === "split_right" || intent === "split_down") {
    const activeTab = normalized.tabs.find((tab) => tab.id === normalized.activeTabId) || null;
    const focusedPanel = normalized.focusedPanelId
      ? normalized.panels.find((panel) => panel.id === normalized.focusedPanelId) || null
      : null;
    if (!activeTab || !focusedPanel) {
      return normalized;
    }

    const direction = intent === "split_right" ? "right" : "down";
    const nextPanel = options.createPanelState({
      target,
      sourcePanel: focusedPanel,
    });
    const replacement: ConsoleDockNode = {
      kind: "split",
      id: options.createSplitId(),
      direction: consoleDockSplitDirectionAxis(direction),
      ratio: 0.5,
      first: panelNode(focusedPanel.id),
      second: panelNode(nextPanel.id),
    };

    return {
      ...normalized,
      tabs: normalized.tabs.map((tab) => (
        tab.id === activeTab.id
          ? {
              ...tab,
              layout: replaceConsoleDockPanelNode(tab.layout, focusedPanel.id, replacement),
            }
          : tab
      )),
      panels: replacePanelStates(normalized.panels, [nextPanel]),
      focusedPanelId: nextPanel.id,
    };
  }

  if (!normalized.focusedPanelId) {
    return normalized;
  }

  return setConsoleDockPanelTarget(normalized, normalized.focusedPanelId, target);
}

export function resizeConsoleDockSplit<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  splitId: string,
  ratio: number,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const activeTab = normalized.tabs.find((tab) => tab.id === normalized.activeTabId) || null;
  if (!activeTab) {
    return normalized;
  }
  return {
    ...normalized,
    tabs: normalized.tabs.map((tab) => (
      tab.id === activeTab.id
        ? {
            ...tab,
            layout: updateConsoleDockSplitRatio(tab.layout, splitId, ratio),
          }
        : tab
    )),
  };
}

export function splitConsoleDockPanel<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  panelId: string,
  direction: ConsoleDockPanelSplitDirection,
  options: Pick<OpenConsoleDockTargetOptions<TTarget>, "createPanelState" | "createSplitId" | "suggestTargets">,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const activeTab = normalized.tabs.find((tab) => tab.id === normalized.activeTabId) || null;
  const panel = normalized.panels.find((entry) => entry.id === panelId) || null;

  if (!activeTab || !panel) {
    return normalized;
  }

  const excludedIds = collectConsoleDockPanelIds(activeTab.layout)
    .map((id) => normalized.panels.find((entry) => entry.id === id)?.target?.id || "")
    .filter(Boolean);
  const suggestedTarget = suggestDockTargets({
    count: 1,
    preferred: panel.target,
    excludedIds,
    suggestTargets: options.suggestTargets,
  })[0] || panel.target || null;
  const nextPanel = options.createPanelState({
    target: suggestedTarget,
    sourcePanel: panel,
  });
  const replacement: ConsoleDockNode = {
    kind: "split",
    id: options.createSplitId(),
    direction: consoleDockSplitDirectionAxis(direction),
    ratio: 0.5,
    first: consoleDockSplitDirectionPrecedes(direction) ? panelNode(nextPanel.id) : panelNode(panelId),
    second: consoleDockSplitDirectionPrecedes(direction) ? panelNode(panelId) : panelNode(nextPanel.id),
  };

  return {
    ...normalized,
    tabs: normalized.tabs.map((tab) => (
      tab.id === activeTab.id
        ? {
            ...tab,
            layout: replaceConsoleDockPanelNode(tab.layout, panelId, replacement),
          }
        : tab
    )),
    panels: replacePanelStates(normalized.panels, [nextPanel]),
    focusedPanelId: nextPanel.id,
  };
}

export function closeConsoleDockPanel<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  panelId: string,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const activeTab = normalized.tabs.find((tab) => tab.id === normalized.activeTabId) || null;
  const panel = normalized.panels.find((entry) => entry.id === panelId) || null;

  if (!activeTab || !panel) {
    return normalized;
  }

  if (collectConsoleDockPanelIds(activeTab.layout).length <= 1) {
    return {
      ...normalized,
      panels: normalized.panels.map((entry) => (
        entry.id === panelId
          ? {
              ...entry,
              target: null,
            }
          : entry
      )),
      focusedPanelId: panelId,
    };
  }

  const nextLayout = removeConsoleDockPanelNode(activeTab.layout, panelId);
  if (!nextLayout) {
    return normalized;
  }

  return {
    ...normalized,
    tabs: normalized.tabs.map((tab) => (
      tab.id === activeTab.id
        ? {
            ...tab,
            layout: nextLayout,
          }
        : tab
    )),
    panels: normalized.panels.filter((entry) => entry.id !== panelId),
    focusedPanelId: findConsoleDockFirstPanelId(nextLayout),
  };
}

export function applyConsoleDockPreset<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  options: ApplyConsoleDockPresetOptions<TTarget>,
): ConsoleDockState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const activeTab = normalized.tabs.find((tab) => tab.id === normalized.activeTabId) || null;
  const focusedPanel = normalized.focusedPanelId
    ? normalized.panels.find((panel) => panel.id === normalized.focusedPanelId) || null
    : null;
  if (!activeTab) {
    return normalized;
  }

  const presetState = buildConsoleDockPresetState({
    presetId: options.presetId,
    preferredTarget: focusedPanel?.target || null,
    preferredPanel: focusedPanel,
    createPanelState: options.createPanelState,
    createSplitId: options.createSplitId,
    suggestTargets: options.suggestTargets,
  });
  const currentPanelIds = new Set(collectConsoleDockPanelIds(activeTab.layout));

  return {
    ...normalized,
    tabs: normalized.tabs.map((tab) => (
      tab.id === activeTab.id
        ? {
            ...tab,
            presetId: presetState.presetId,
            layout: presetState.layout,
          }
        : tab
    )),
    panels: replacePanelStates(
      normalized.panels.filter((panel) => !currentPanelIds.has(panel.id)),
      presetState.panels,
    ),
    focusedPanelId: presetState.focusedPanelId,
  };
}

export function applyConsoleDockAction<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  action: ConsoleDockAction<TTarget>,
  options: {
    createPanelState: ConsoleDockCreatePanelState<TTarget>;
    createSplitId: () => string;
    createTabId: () => string;
    suggestTargets?: ConsoleDockSuggestTargets<TTarget>;
  },
): ConsoleDockState<TTarget> {
  switch (action.type) {
    case "create_tab":
      return createConsoleDockTab(state, options, { blank: action.blank });
    case "select_tab":
      return action.tabId ? selectConsoleDockTab(state, action.tabId) : state;
    case "close_tab":
      return action.tabId ? closeConsoleDockTab(state, action.tabId, options) : state;
    case "focus_panel":
      return action.panelId ? focusConsoleDockPanel(state, action.panelId) : state;
    case "set_panel_target":
      return action.panelId ? setConsoleDockPanelTarget(state, action.panelId, action.target || null) : state;
    case "set_panel_mode":
      return action.panelId && action.mode ? setConsoleDockPanelMode(state, action.panelId, action.mode) : state;
    case "open_target":
      return action.target
        ? openConsoleDockTarget(state, action.target, {
            ...options,
            intent: action.intent,
          })
        : state;
    case "resize_split":
      return action.splitId && typeof action.ratio === "number"
        ? resizeConsoleDockSplit(state, action.splitId, action.ratio)
        : state;
    case "split_panel":
      return action.panelId && action.direction
        ? splitConsoleDockPanel(state, action.panelId, action.direction, options)
        : state;
    case "close_panel":
      return action.panelId ? closeConsoleDockPanel(state, action.panelId) : state;
    case "apply_preset":
      return action.presetId
        ? applyConsoleDockPreset(state, {
            presetId: action.presetId,
            createPanelState: options.createPanelState,
            createSplitId: options.createSplitId,
            suggestTargets: options.suggestTargets,
          })
        : state;
    default:
      return state;
  }
}

export function buildConsoleDockViewState<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget> | null | undefined,
  options: BuildConsoleDockViewStateOptions<TTarget> = {},
): ConsoleDockViewState<TTarget> {
  const normalized = normalizeConsoleDockState<TTarget>(state);
  const panelsById = new Map(normalized.panels.map((panel) => [panel.id, panel] as const));

  return {
    activeTabId: normalized.activeTabId,
    focusedPanelId: normalized.focusedPanelId,
    tabs: normalized.tabs.map((tab) => {
      const panelStates = collectConsoleDockPanelIds(tab.layout)
        .map((panelId) => panelsById.get(panelId))
        .filter(Boolean) as ConsoleDockPanelState<TTarget>[];
      const firstTarget = panelStates.find((panel) => panel.target)?.target || null;
      const preset = presetMeta(tab.presetId);
      const resolved = options.resolveTabView?.({
        tab,
        panels: panelStates,
        active: tab.id === normalized.activeTabId,
        focusedPanelId: normalized.focusedPanelId,
      }) || {};

      return {
        id: tab.id,
        title: resolved.title || firstTarget?.title || preset.label,
        subtitle: resolved.subtitle ?? firstTarget?.subtitle ?? preset.description,
        iconName: resolved.iconName ?? firstTarget?.iconName ?? preset.iconName,
        badgeLabel: resolved.badgeLabel ?? (panelStates.length > 1 ? `x${panelStates.length}` : null),
        closable: resolved.closable ?? true,
        dirty: resolved.dirty ?? false,
        layout: tab.layout,
      };
    }),
    panels: normalized.tabs.flatMap((tab) => {
      const activePanelIds = collectConsoleDockPanelIds(tab.layout);
      const activePanelCount = activePanelIds.length;
      return activePanelIds.flatMap((panelId) => {
        const panel = panelsById.get(panelId);
        if (!panel) {
          return [];
        }
          const resolved = options.resolvePanelView?.({
            panel,
            activePanelCount,
            focused: normalized.focusedPanelId === panel.id,
          }) || {};
          return [{
            id: panel.id,
            title: resolved.title || panel.target?.title || "Choose a target",
            subtitle: resolved.subtitle ?? panel.target?.subtitle ?? "Use the launcher or activity rail to open a target.",
            iconName: resolved.iconName ?? panel.target?.iconName ?? "i-compose",
            target: panel.target,
            mode: panel.mode,
            statusLabel: resolved.statusLabel ?? (panel.target ? "Active target" : "Ready"),
            badgeLabel: resolved.badgeLabel ?? panel.target?.badgeLabel ?? null,
            dirty: resolved.dirty ?? false,
            closable: resolved.closable ?? activePanelCount > 1,
          }];
        });
    }),
  };
}

export function normalizeConsoleDockViewState<TTarget extends ConsoleDockTarget>(
  viewState: ConsoleDockViewState<TTarget> | null | undefined,
): ConsoleDockViewState<TTarget> {
  const panels = (viewState?.panels || [])
    .filter((panel) => Boolean(panel?.id && panel?.title))
    .map((panel) => ({
      ...panel,
      target: normalizeTarget(panel.target),
      mode: panel.mode === "terminal" ? "terminal" : "console",
    })) as ConsoleDockPanelView<TTarget>[];
  const validPanelIds = new Set(panels.map((panel) => panel.id));

  const tabs = (viewState?.tabs || [])
    .filter((tab) => Boolean(tab?.id && tab?.title))
    .map((tab) => ({
      ...tab,
      layout: normalizeNode(tab.layout, validPanelIds),
    }))
    .filter((tab) => Boolean(tab.layout)) as Array<ConsoleDockTabView & { layout: ConsoleDockNode }>;

  const activeTabId = tabs.some((tab) => tab.id === viewState?.activeTabId)
    ? viewState?.activeTabId || null
    : tabs[0]?.id || null;
  const activeTab = tabs.find((tab) => tab.id === activeTabId) || null;
  const activePanelIds = activeTab ? collectConsoleDockPanelIds(activeTab.layout) : [];
  const focusedPanelId = viewState?.focusedPanelId && activePanelIds.includes(viewState.focusedPanelId)
    ? viewState.focusedPanelId
    : activePanelIds[0] || null;

  return {
    tabs,
    panels,
    activeTabId,
    focusedPanelId,
  };
}
