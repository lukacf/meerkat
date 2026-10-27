import assert from "node:assert/strict";

import { normalizeSidebarWatchFields } from "./control-plane";
import { normalizeConsoleSidebarViewState } from "./sidebar";

// Dual-runner: meta's phase0 gate bundles this file with esbuild and runs it
// under `node --test`; the meerkat-studio desktop app picks it up with vitest
// (globals enabled). Resolve the registrar for whichever runner is active.
type TestFn = (name: string, fn: () => void | Promise<void>) => void;
const nodeTestModule = "node:test";
const test: TestFn = process.env.VITEST
  ? ((globalThis as Record<string, unknown>).test as TestFn)
  : ((await import(/* @vite-ignore */ nodeTestModule)).default as unknown as TestFn);

test("normalizeSidebarWatchFields preserves the phase-0 contract", () => {
  assert.deepEqual(
    normalizeSidebarWatchFields({
      watched: true,
      alertLevel: "elevated",
      degraded: true,
      degradedReason: " lease_expired ",
    }),
    {
      watched: true,
      alertLevel: "elevated",
      degraded: true,
      degradedReason: "lease_expired",
    },
  );
});

test("normalizeConsoleSidebarViewState keeps valid action-strip and list blocks while filtering incomplete data", () => {
  const viewState = normalizeConsoleSidebarViewState({
    blocks: [
      {
        id: "primary",
        kind: "action_strip",
        actions: [
          { id: "new", label: "New thread", iconName: "i-new-thread" },
          { id: "", label: "" },
        ],
      },
      {
        id: "threads",
        kind: "list",
        title: "Threads",
        actions: [{ id: "filter", label: "Filter", iconName: "i-sliders" }],
        sections: [
          {
            id: "workspace",
            title: "workspace",
            actions: [{ id: "create", label: "New thread", iconName: "i-plus" }],
            items: [
              {
                id: "thread-1",
                title: "Sidebar extraction",
                actions: [{ id: "pin", label: "Pin thread", iconName: "i-pin" }],
              },
              {
                id: "",
                title: "",
              },
            ],
          },
          {
            id: "empty",
            title: "",
            items: [],
          },
        ],
      },
    ],
  });

  const threadBlock = viewState.blocks[1];
  const firstSection = threadBlock?.sections?.[0];

  assert.equal(viewState.blocks.length, 2);
  assert.equal(viewState.blocks[0]?.actions?.length, 1);
  assert.equal(threadBlock?.sections?.length, 1);
  assert.equal(firstSection?.items?.length, 1);
});

test("normalizeConsoleSidebarViewState preserves watch fields and warning tones on sidebar items", () => {
  const viewState = normalizeConsoleSidebarViewState({
    blocks: [
      {
        id: "agents",
        kind: "list",
        sections: [
          {
            id: "ops",
            title: "Ops",
            items: [
              {
                id: "identity:luka",
                title: "Luka",
                watched: true,
                alertLevel: "critical",
                degraded: true,
                degradedReason: "peer_unreachable",
                meta: [{ id: "alert", label: "Critical", tone: "warning" }],
              },
            ],
          },
        ],
      },
    ],
  });

  const item = viewState.blocks[0]?.sections?.[0]?.items?.[0];
  assert.equal(item?.watched, true);
  assert.equal(item?.alertLevel, "critical");
  assert.equal(item?.degraded, true);
  assert.equal(item?.degradedReason, "peer_unreachable");
  assert.equal(item?.meta?.[0]?.tone, "warning");
});
