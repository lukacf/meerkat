import {
  applyConsoleDockPreset,
  buildConsoleDockViewState,
  collectConsoleDockPanelIds,
  consoleDockSplitDirectionAxis,
  consoleDockSplitDirectionPrecedes,
  createConsoleDockState,
  createConsoleDockTab,
  findConsoleDockFirstPanelId,
  normalizeConsoleDockViewState,
  openConsoleDockTarget,
  removeConsoleDockPanelNode,
  replaceConsoleDockPanelNode,
  resizeConsoleDockSplit,
  splitConsoleDockPanel,
  updateConsoleDockSplitRatio,
  type ConsoleDockTarget,
  type ConsoleDockNode,
  type ConsoleDockPanelState,
} from "./dock";

describe("dock helpers", () => {
  const layout: ConsoleDockNode = {
    kind: "split",
    id: "root",
    direction: "horizontal",
    first: { kind: "panel", panelId: "left" },
    second: {
      kind: "split",
      id: "right-stack",
      direction: "vertical",
      first: { kind: "panel", panelId: "top" },
      second: { kind: "panel", panelId: "bottom" },
    },
  };

  test("collects and replaces panel nodes", () => {
    expect(collectConsoleDockPanelIds(layout)).toEqual(["left", "top", "bottom"]);
    expect(findConsoleDockFirstPanelId(layout)).toBe("left");

    const replaced = replaceConsoleDockPanelNode(layout, "top", {
      kind: "split",
      id: "top-replaced",
      direction: "horizontal",
      first: { kind: "panel", panelId: "alpha" },
      second: { kind: "panel", panelId: "beta" },
    });

    expect(collectConsoleDockPanelIds(replaced)).toEqual(["left", "alpha", "beta", "bottom"]);
  });

  test("removes panel nodes and collapses empty splits", () => {
    expect(removeConsoleDockPanelNode(layout, "bottom")).toEqual({
      kind: "split",
      id: "root",
      direction: "horizontal",
      first: { kind: "panel", panelId: "left" },
      second: { kind: "panel", panelId: "top" },
    });

    expect(removeConsoleDockPanelNode({ kind: "panel", panelId: "solo" }, "solo")).toBeNull();
  });

  test("updates split ratios and clamps them to a safe range", () => {
    expect(updateConsoleDockSplitRatio(layout, "right-stack", 0.74)).toEqual({
      kind: "split",
      id: "root",
      direction: "horizontal",
      first: { kind: "panel", panelId: "left" },
      second: {
        kind: "split",
        id: "right-stack",
        direction: "vertical",
        ratio: 0.74,
        first: { kind: "panel", panelId: "top" },
        second: { kind: "panel", panelId: "bottom" },
      },
    });

    expect(updateConsoleDockSplitRatio(layout, "root", 4)).toEqual({
      kind: "split",
      id: "root",
      direction: "horizontal",
      ratio: 0.88,
      first: { kind: "panel", panelId: "left" },
      second: {
        kind: "split",
        id: "right-stack",
        direction: "vertical",
        first: { kind: "panel", panelId: "top" },
        second: { kind: "panel", panelId: "bottom" },
      },
    });
  });

  test("normalizes tabs, active tab, and focused panel", () => {
    const viewState = normalizeConsoleDockViewState({
      panels: [
        { id: "alpha", title: "Alpha" },
        { id: "beta", title: "Beta" },
      ],
      tabs: [
        {
          id: "one",
          title: "One",
          layout: {
            kind: "split",
            id: "root",
            direction: "horizontal",
            first: { kind: "panel", panelId: "missing" },
            second: { kind: "panel", panelId: "beta" },
          },
        },
      ],
      activeTabId: "missing",
      focusedPanelId: "alpha",
    });

    expect(viewState.activeTabId).toBe("one");
    expect(viewState.focusedPanelId).toBe("beta");
    expect(collectConsoleDockPanelIds(viewState.tabs[0]?.layout || null)).toEqual(["beta"]);
  });

  test("preserves host-defined target payloads through normalization", () => {
    type TestTarget = ConsoleDockTarget & {
      kind: "thread";
      threadId: string;
    };

    const viewState = normalizeConsoleDockViewState<TestTarget>({
      panels: [
        {
          id: "alpha",
          title: "Alpha",
          target: {
            id: "thread:alpha",
            kind: "thread",
            title: "Alpha",
            threadId: "thread-alpha",
          },
        },
      ],
      tabs: [
        {
          id: "one",
          title: "One",
          layout: { kind: "panel", panelId: "alpha" },
        },
      ],
      activeTabId: "one",
      focusedPanelId: "alpha",
    });

    expect(viewState.panels[0]?.target).toMatchObject({
      kind: "thread",
      threadId: "thread-alpha",
    });
  });

  test("maps split directions to axes and ordering", () => {
    expect(consoleDockSplitDirectionAxis("left")).toBe("horizontal");
    expect(consoleDockSplitDirectionAxis("down")).toBe("vertical");
    expect(consoleDockSplitDirectionPrecedes("left")).toBe(true);
    expect(consoleDockSplitDirectionPrecedes("right")).toBe(false);
  });

  test("creates, splits, opens, resizes, and presets dock state with host target payloads", () => {
    type TestTarget = ConsoleDockTarget & {
      kind: "thread";
      threadId: string;
    };

    let panelIdCounter = 0;
    let tabIdCounter = 0;
    let splitIdCounter = 0;
    const suggestionCalls: Array<{
      count: number;
      preferredId: string | null;
      excludedIds: string[];
    }> = [];

    const alpha: TestTarget = {
      id: "thread:alpha",
      kind: "thread",
      title: "Alpha",
      threadId: "alpha",
    };
    const beta: TestTarget = {
      id: "thread:beta",
      kind: "thread",
      title: "Beta",
      threadId: "beta",
    };
    const gamma: TestTarget = {
      id: "thread:gamma",
      kind: "thread",
      title: "Gamma",
      threadId: "gamma",
    };

    const createPanelState = ({ target }: { target: TestTarget | null }): ConsoleDockPanelState<TestTarget> => ({
      id: `panel-${++panelIdCounter}`,
      target,
      mode: "console",
    });

    const createTabId = () => `tab-${++tabIdCounter}`;
    const createSplitId = () => `split-${++splitIdCounter}`;
    const suggestTargets = ({ count, preferred, excludedIds }: {
      count: number;
      preferred: TestTarget | null;
      excludedIds: string[];
    }) => {
      suggestionCalls.push({
        count,
        preferredId: preferred?.id || null,
        excludedIds: [...excludedIds],
      });
      const pool = [preferred, alpha, beta, gamma].filter(Boolean) as TestTarget[];
      const results: Array<TestTarget | null> = [];
      for (const target of pool) {
        if (excludedIds.includes(target.id)) {
          continue;
        }
        results.push(target);
        if (results.length >= count) {
          return results;
        }
      }
      while (results.length < count) {
        results.push(null);
      }
      return results;
    };

    let state = createConsoleDockState<TestTarget>({
      initialTarget: alpha,
      createPanelState,
      createSplitId,
      createTabId,
      suggestTargets,
    });

    expect(state.panels).toHaveLength(1);
    expect(state.panels[0]?.target?.threadId).toBe("alpha");

    state = splitConsoleDockPanel(state, state.focusedPanelId!, "right", {
      createPanelState,
      createSplitId,
      suggestTargets,
    });

    expect(state.panels).toHaveLength(2);
    expect(collectConsoleDockPanelIds(state.tabs[0]?.layout || null)).toHaveLength(2);

    state = openConsoleDockTarget(state, gamma, {
      intent: "new_tab",
      createPanelState,
      createSplitId,
      createTabId,
      suggestTargets,
    });

    expect(state.tabs).toHaveLength(2);
    expect(state.activeTabId).toBe("tab-2");

    const activeTab = state.tabs.find((tab) => tab.id === state.activeTabId)!;
    const firstSplitId = activeTab.layout.kind === "split" ? activeTab.layout.id : null;
    if (firstSplitId) {
      state = resizeConsoleDockSplit(state, firstSplitId, 0.8);
      const resizedActiveTab = state.tabs.find((tab) => tab.id === state.activeTabId)!;
      expect(resizedActiveTab.layout.kind === "split" ? resizedActiveTab.layout.ratio : null).toBe(0.8);
    }

    state = applyConsoleDockPreset(state, {
      presetId: "grid",
      createPanelState,
      createSplitId,
      suggestTargets,
    });

    const gridPanelIds = collectConsoleDockPanelIds(
      state.tabs.find((tab) => tab.id === state.activeTabId)?.layout || null,
    );
    expect(gridPanelIds).toHaveLength(4);
    const gridPanels = gridPanelIds.map((panelId) => state.panels.find((panel) => panel.id === panelId)!);
    expect(gridPanels.map((panel) => panel.target?.id || null)).toEqual([
      "thread:gamma",
      "thread:beta",
      "thread:alpha",
      null,
    ]);
    expect(gridPanels.filter((panel) => panel.target?.id === "thread:gamma")).toHaveLength(1);
    expect(suggestionCalls.at(-1)).toEqual({
      count: 3,
      preferredId: null,
      excludedIds: ["thread:gamma"],
    });
  });

  test("splitting a focused panel clones its target when no alternate suggestion is available", () => {
    type TestTarget = ConsoleDockTarget & {
      kind: "thread";
      threadId: string;
    };

    const alpha: TestTarget = {
      id: "thread:alpha",
      kind: "thread",
      title: "Alpha",
      threadId: "alpha",
    };

    let panelIdCounter = 0;
    let splitIdCounter = 0;
    let tabIdCounter = 0;

    const createPanelState = ({ target }: { target: TestTarget | null }): ConsoleDockPanelState<TestTarget> => ({
      id: `panel-${++panelIdCounter}`,
      target,
      mode: "console",
    });

    let state = createConsoleDockState<TestTarget>({
      initialTarget: alpha,
      createPanelState,
      createSplitId: () => `split-${++splitIdCounter}`,
      createTabId: () => `tab-${++tabIdCounter}`,
      suggestTargets: ({ count }) => Array.from({ length: count }, () => null),
    });

    state = splitConsoleDockPanel(state, state.focusedPanelId!, "right", {
      createPanelState,
      createSplitId: () => `split-${++splitIdCounter}`,
      suggestTargets: ({ count }) => Array.from({ length: count }, () => null),
    });

    expect(state.panels).toHaveLength(2);
    expect(state.panels.map((panel) => panel.target?.id)).toEqual(["thread:alpha", "thread:alpha"]);
  });

  test("can create an explicitly blank tab without changing the default clone behavior", () => {
    type TestTarget = ConsoleDockTarget & { kind: "thread"; threadId: string };
    const alpha: TestTarget = { id: "thread:alpha", kind: "thread", title: "Alpha", threadId: "alpha" };
    let panelIdCounter = 0;
    let tabIdCounter = 0;
    const createPanelState = ({ target }: { target: TestTarget | null }): ConsoleDockPanelState<TestTarget> => ({
      id: `panel-${++panelIdCounter}`,
      target,
      mode: "console",
    });
    const createTabId = () => `tab-${++tabIdCounter}`;
    const createSplitId = () => "split-unused";
    const initial = createConsoleDockState<TestTarget>({
      initialTarget: alpha,
      createPanelState,
      createTabId,
      createSplitId,
    });

    const cloned = createConsoleDockTab(initial, { createPanelState, createTabId, createSplitId });
    expect(cloned.panels.find((panel) => panel.id === cloned.focusedPanelId)?.target?.id).toBe(alpha.id);

    const blank = createConsoleDockTab(initial, { createPanelState, createTabId, createSplitId }, { blank: true });
    expect(blank.panels.find((panel) => panel.id === blank.focusedPanelId)?.target).toBeNull();
  });

  test("presets do not create empty panels when there are no suggested targets", () => {
    type TestTarget = ConsoleDockTarget & {
      kind: "thread";
      threadId: string;
    };

    const alpha: TestTarget = {
      id: "thread:alpha",
      kind: "thread",
      title: "Alpha",
      threadId: "alpha",
    };

    let panelIdCounter = 0;
    let splitIdCounter = 0;
    let tabIdCounter = 0;

    const createPanelState = ({ target }: { target: TestTarget | null }): ConsoleDockPanelState<TestTarget> => ({
      id: `panel-${++panelIdCounter}`,
      target,
      mode: "console",
    });
    const suggestTargets = ({ preferred }: { preferred: TestTarget | null }) => [preferred];

    let state = createConsoleDockState<TestTarget>({
      initialTarget: alpha,
      initialPresetId: "two_columns",
      createPanelState,
      createSplitId: () => `split-${++splitIdCounter}`,
      createTabId: () => `tab-${++tabIdCounter}`,
      suggestTargets,
    });

    expect(state.panels).toHaveLength(1);
    expect(state.panels[0]?.target?.id).toBe("thread:alpha");
    expect(state.tabs[0]?.presetId).toBe("single");

    state = applyConsoleDockPreset(state, {
      presetId: "grid",
      createPanelState,
      createSplitId: () => `split-${++splitIdCounter}`,
      suggestTargets,
    });

    expect(state.panels).toHaveLength(1);
    expect(state.panels.every((panel) => panel.target)).toBe(true);
    expect(state.tabs[0]?.presetId).toBe("single");
  });

  test("builds dock view state from host resolvers instead of baked-in chrome", () => {
    type TestTarget = ConsoleDockTarget & {
      kind: "agent";
      memberId: string;
    };

    const state = createConsoleDockState<TestTarget>({
      initialTarget: {
        id: "agent:alpha",
        kind: "agent",
        title: "Alpha",
        memberId: "alpha",
      },
      createPanelState: ({ target }) => ({
        id: "panel-1",
        target,
        mode: "console",
      }),
      createTabId: () => "tab-1",
      createSplitId: () => "split-1",
    });

    const viewState = buildConsoleDockViewState(state, {
      resolvePanelView: ({ panel, focused }) => ({
        title: `Panel for ${panel.target?.memberId}`,
        statusLabel: focused ? "Focused" : "Idle",
      }),
      resolveTabView: ({ panels }) => ({
        title: `Tab ${panels[0]?.target?.memberId}`,
      }),
    });

    expect(viewState.panels[0]).toMatchObject({
      title: "Panel for alpha",
      statusLabel: "Focused",
    });
    expect(viewState.tabs[0]).toMatchObject({
      title: "Tab alpha",
    });
  });
});
