import assert from "node:assert/strict";

import { normalizeConsoleDockState, type ConsoleDockState } from "./dock";
import {
  migrateConsoleWorkbenchTarget,
  type ConsoleWorkbenchTarget,
} from "./targets";

// Dual-runner: meta's console gates bundle this file with esbuild and run it
// under `node --test`; the meerkat-studio desktop app picks it up with vitest
// (globals enabled). Resolve the registrar for whichever runner is active.
type TestFn = (name: string, fn: () => void | Promise<void>) => void;
const nodeTestModule = "node:test";
const test: TestFn = process.env.VITEST
  ? ((globalThis as Record<string, unknown>).test as TestFn)
  : ((await import(/* @vite-ignore */ nodeTestModule)).default as unknown as TestFn);

test("migrateConsoleWorkbenchTarget maps legacy agent-chat targets to namespaced identity chat", () => {
  const target = migrateConsoleWorkbenchTarget({
    id: "member-alex",
    kind: "agent-chat",
    title: "Alex",
    identity: "identity:alex",
    memberId: "member-alex",
  });

  assert.equal(target?.kind, "meta/identity-chat");
  assert.equal(target?.kind === "meta/identity-chat" && "identity" in target ? target.identity : null, "identity:alex");
  assert.equal(target?.kind === "meta/identity-chat" && "addressingMode" in target ? target.addressingMode : null, "identity");
});

test("migrateConsoleWorkbenchTarget maps legacy identity-inspect, routing, and gating targets", () => {
  assert.deepEqual(migrateConsoleWorkbenchTarget({
    id: "inspect:identity:alex",
    kind: "identity-inspect",
    title: "Alex Details",
    identity: "identity:alex",
  }), {
    id: "inspect:identity:alex",
    kind: "meta/identity-inspect",
    title: "Alex Details",
    subtitle: undefined,
    iconName: undefined,
    badgeLabel: undefined,
    identity: "identity:alex",
    memberId: undefined,
  });

  assert.equal(migrateConsoleWorkbenchTarget({ id: "routing", kind: "routing", title: "Routing" })?.kind, "meta/routing");
  assert.equal(migrateConsoleWorkbenchTarget({ id: "gating", kind: "gating", title: "Approvals" })?.kind, "meta/gating");
  assert.equal(migrateConsoleWorkbenchTarget({ id: "workgraph", kind: "workgraph", title: "WorkGraph" })?.kind, "meta/workgraph");
  assert.equal(migrateConsoleWorkbenchTarget({ id: "workgraph", kind: "meta/workgraph", title: "WorkGraph" })?.kind, "meta/workgraph");
});

test("migrated legacy targets hydrate through dock normalization", () => {
  const state: ConsoleDockState<ConsoleWorkbenchTarget> = normalizeConsoleDockState({
    activeTabId: "one",
    focusedPanelId: "p-chat",
    panels: [
      {
        id: "p-chat",
        mode: "console",
        target: migrateConsoleWorkbenchTarget({
          id: "member-alex",
          kind: "agent-chat",
          title: "Alex",
          identity: "identity:alex",
          memberId: "member-alex",
        }),
      },
      {
        id: "p-inspect",
        mode: "console",
        target: migrateConsoleWorkbenchTarget({
          id: "inspect:identity:alex",
          kind: "identity-inspect",
          title: "Alex Details",
          identity: "identity:alex",
        }),
      },
      {
        id: "p-routing",
        mode: "console",
        target: migrateConsoleWorkbenchTarget({ id: "routing", kind: "routing", title: "Routing" }),
      },
      {
        id: "p-gating",
        mode: "console",
        target: migrateConsoleWorkbenchTarget({ id: "gating", kind: "gating", title: "Approvals" }),
      },
    ],
    tabs: [
      {
        id: "one",
        presetId: "grid",
        layout: {
          kind: "split",
          id: "root",
          direction: "horizontal",
          first: { kind: "panel", panelId: "p-chat" },
          second: {
            kind: "split",
            id: "right",
            direction: "vertical",
            first: { kind: "panel", panelId: "p-inspect" },
            second: {
              kind: "split",
              id: "bottom",
              direction: "horizontal",
              first: { kind: "panel", panelId: "p-routing" },
              second: { kind: "panel", panelId: "p-gating" },
            },
          },
        },
      },
    ],
  });

  assert.deepEqual(state.panels.map((panel) => panel.target?.kind), [
    "meta/identity-chat",
    "meta/identity-inspect",
    "meta/routing",
    "meta/gating",
  ]);
  assert.equal(state.focusedPanelId, "p-chat");
});

test("unknown host targets remain inertly persistable when namespaced", () => {
  const target = migrateConsoleWorkbenchTarget({
    id: "project:alpha",
    kind: "host/project",
    title: "Project Alpha",
    payloadVersion: 3,
    payload: { projectId: "alpha" },
  });

  assert.equal(target?.kind, "host/project");
  assert.equal(target?.kind === "host/project" ? target.provenance : null, "host");
  assert.equal(target?.kind === "host/project" ? target.payloadVersion : null, 3);
  assert.equal(migrateConsoleWorkbenchTarget({ id: "bad", kind: "project", title: "Bad" }), null);
});
