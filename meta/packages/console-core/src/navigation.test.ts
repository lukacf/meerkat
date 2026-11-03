import assert from "node:assert/strict";

import {
  applyConsoleNavigationReorderIntent,
  canMoveConsoleNavigationNode,
  consoleNavigationFromSidebarViewState,
  consoleNavigationToSidebarViewState,
  moveConsoleNavigationNode,
  normalizeConsoleNavigationModel,
  pinConsoleNavigationNode,
  selectConsoleNavigationNode,
  toggleConsoleNavigationGroup,
  type ConsoleNavigationModel,
} from "./navigation";
import { normalizeConsoleSidebarViewState } from "./sidebar";

// Dual-runner: meta's console gates bundle this file with esbuild and run it
// under `node --test`; the meerkat-studio desktop app picks it up with vitest
// (globals enabled). Resolve the registrar for whichever runner is active.
type TestFn = (name: string, fn: () => void | Promise<void>) => void;
const nodeTestModule = "node:test";
const test: TestFn = process.env.VITEST
  ? ((globalThis as Record<string, unknown>).test as TestFn)
  : ((await import(/* @vite-ignore */ nodeTestModule)).default as unknown as TestFn);

test("ConsoleNavigationModel normalizes invalid nodes and preserves layout-neutral order", () => {
  const model = normalizeConsoleNavigationModel({
    orientation: "horizontal",
    activeNodeId: "thread-alpha",
    nodes: [
      {
        type: "group",
        id: "threads",
        label: "Threads",
        expanded: true,
        children: [
          { type: "item", id: "thread-alpha", label: "Alpha" },
          { type: "item", id: "", label: "" },
        ],
      },
    ],
    order: { orderedNodeIds: [] },
  });

  assert.equal(model.orientation, "horizontal");
  assert.equal(model.activeNodeId, "thread-alpha");
  assert.deepEqual(model.order.orderedNodeIds, ["threads", "thread-alpha"]);
});

test("navigation operations select, toggle, and pin without depending on sidebar layout", () => {
  const model = baseNavigationModel();

  const selected = selectConsoleNavigationNode(model, "thread-beta");
  assert.equal(selected.activeNodeId, "thread-beta");
  assert.equal(selected.focusNodeId, "thread-beta");

  const collapsed = toggleConsoleNavigationGroup(selected, "threads");
  const group = collapsed.nodes[0];
  assert.equal(group?.type, "group");
  assert.equal(group?.type === "group" ? group.expanded : true, false);

  const pinned = pinConsoleNavigationNode(collapsed, "thread-beta", true);
  const item = pinned.nodes[0]?.type === "group" ? pinned.nodes[0].children[1] : null;
  assert.equal(item?.type, "item");
  assert.equal(item?.type === "item" ? item.pinned : false, true);
});

test("navigation toggle and pin ignore stale or wrong-type node ids", () => {
  const model = baseNavigationModel();

  const staleToggle = toggleConsoleNavigationGroup(model, "missing");
  assert.equal(staleToggle.focusNodeId, model.focusNodeId);
  assert.deepEqual(staleToggle.nodes, model.nodes);

  const itemToggle = toggleConsoleNavigationGroup(model, "thread-alpha");
  assert.equal(itemToggle.focusNodeId, model.focusNodeId);
  assert.equal(itemToggle.nodes[0]?.type === "group" ? itemToggle.nodes[0].expanded : false, true);

  const stalePin = pinConsoleNavigationNode(model, "missing", true);
  assert.equal(stalePin.focusNodeId, model.focusNodeId);
  assert.deepEqual(stalePin.nodes, model.nodes);

  const groupPin = pinConsoleNavigationNode(model, "threads", true);
  assert.equal(groupPin.focusNodeId, model.focusNodeId);
  assert.deepEqual(groupPin.nodes, model.nodes);
});

test("navigation move validates disabled, descendant, inside, and sibling constraints", () => {
  const model = baseNavigationModel();

  assert.equal(canMoveConsoleNavigationNode(model, {
    id: "thread-alpha",
    targetId: "thread-beta",
    position: "after",
    scope: "siblings",
  }), true);
  assert.equal(canMoveConsoleNavigationNode(model, {
    id: "threads",
    targetId: "thread-alpha",
    position: "after",
  }), false);
  assert.equal(canMoveConsoleNavigationNode(model, {
    id: "thread-alpha",
    targetId: "thread-beta",
    position: "inside",
  }), false);

  const moved = moveConsoleNavigationNode(model, {
    id: "thread-alpha",
    targetId: "thread-beta",
    position: "after",
    scope: "siblings",
  });

  const children = moved.model.nodes[0]?.type === "group" ? moved.model.nodes[0].children : [];
  assert.deepEqual(children.map((child) => child.id), ["thread-beta", "thread-alpha"]);
  assert.equal(moved.focusNodeId, "thread-alpha");
  assert.equal(moved.announcement, "Moved Alpha after Beta.");
});

test("navigation move preserves nodes when moving inside later groups and across groups", () => {
  const model = nestedNavigationModel();

  const withinLaterGroup = moveConsoleNavigationNode(model, {
    id: "worker-two",
    targetId: "worker-one",
    position: "before",
    scope: "siblings",
  });
  const workers = withinLaterGroup.model.nodes[1]?.type === "group"
    ? withinLaterGroup.model.nodes[1].children
    : [];
  assert.deepEqual(workers.map((child) => child.id), ["worker-two", "worker-one"]);
  assert.equal(withinLaterGroup.focusNodeId, "worker-two");

  const acrossGroups = moveConsoleNavigationNode(model, {
    id: "thread-alpha",
    targetId: "workers",
    position: "inside",
  });
  const threadChildren = acrossGroups.model.nodes[0]?.type === "group"
    ? acrossGroups.model.nodes[0].children
    : [];
  const workerChildren = acrossGroups.model.nodes[1]?.type === "group"
    ? acrossGroups.model.nodes[1].children
    : [];
  assert.deepEqual(threadChildren.map((child) => child.id), ["thread-beta"]);
  assert.deepEqual(workerChildren.map((child) => child.id), ["worker-one", "worker-two", "thread-alpha"]);
  assert.deepEqual(acrossGroups.model.order.orderedNodeIds, [
    "threads",
    "thread-beta",
    "workers",
    "worker-one",
    "worker-two",
    "thread-alpha",
  ]);
});

test("keyboard and pointer reorder intents use the same navigation move operation", () => {
  const model = baseNavigationModel();
  const keyboard = applyConsoleNavigationReorderIntent(model, {
    inputSource: "keyboard",
    id: "thread-alpha",
    targetId: "thread-beta",
    position: "after",
    scope: "siblings",
  });
  const pointer = applyConsoleNavigationReorderIntent(model, {
    inputSource: "pointer",
    id: "thread-alpha",
    targetId: "thread-beta",
    position: "after",
    scope: "siblings",
  });

  assert.deepEqual(
    keyboard.model.order.orderedNodeIds,
    pointer.model.order.orderedNodeIds,
  );
  assert.equal(keyboard.focusNodeId, pointer.focusNodeId);
  assert.equal(keyboard.announcement, pointer.announcement);
});

test("sidebar compatibility adapters round-trip existing ConsoleSidebarViewState", () => {
  const sidebar = {
    blocks: [
      {
        id: "primary",
        kind: "action_strip" as const,
        actions: [{ id: "new", label: "New thread", iconName: "i-plus" }],
      },
      {
        id: "agents",
        kind: "list" as const,
        title: "Agents",
        meta: [{ id: "count", label: "2" }],
        sections: [
          {
            id: "coordinators",
            title: "Coordinators",
            iconName: "i-bolt",
            actions: [{ id: "create", label: "Create", iconName: "i-plus" }],
            items: [
              {
                id: "identity:luka",
                title: "Luka",
                subtitle: "identity:luka",
                selected: true,
                pinned: true,
                meta: [{ id: "state", label: "Running", tone: "accent" as const }],
                actions: [{ id: "inspect", label: "Inspect", iconName: "i-terminal" }],
              },
            ],
          },
        ],
      },
    ],
  };

  const navigation = consoleNavigationFromSidebarViewState(sidebar);
  assert.equal(navigation.nodes.length, 2);
  assert.equal(navigation.nodes[1]?.type, "group");
  assert.equal(navigation.nodes[1]?.label, "Coordinators");

  assert.deepEqual(
    consoleNavigationToSidebarViewState(navigation),
    normalizeConsoleSidebarViewState(sidebar),
  );
});

function baseNavigationModel(): ConsoleNavigationModel {
  return normalizeConsoleNavigationModel({
    nodes: [
      {
        type: "group",
        id: "threads",
        label: "Threads",
        expanded: true,
        children: [
          { type: "item", id: "thread-alpha", label: "Alpha" },
          { type: "item", id: "thread-beta", label: "Beta" },
        ],
      },
      {
        type: "group",
        id: "disabled",
        label: "Disabled",
        disabled: true,
        expanded: true,
        children: [],
      },
    ],
    order: { orderedNodeIds: [] },
  });
}

function nestedNavigationModel(): ConsoleNavigationModel {
  return normalizeConsoleNavigationModel({
    nodes: [
      {
        type: "group",
        id: "threads",
        label: "Threads",
        expanded: true,
        children: [
          { type: "item", id: "thread-alpha", label: "Alpha" },
          { type: "item", id: "thread-beta", label: "Beta" },
        ],
      },
      {
        type: "group",
        id: "workers",
        label: "Workers",
        expanded: true,
        children: [
          { type: "item", id: "worker-one", label: "Worker One" },
          { type: "item", id: "worker-two", label: "Worker Two" },
        ],
      },
    ],
    order: { orderedNodeIds: [] },
  });
}
