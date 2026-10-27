import assert from "node:assert/strict";
import test from "node:test";

import {
  focusConsoleDockPanel,
  normalizeConsoleDockState,
  type BrowserDockTarget,
  type ConsoleDockTarget,
} from "./index";

type ConversationDockTarget = ConsoleDockTarget & {
  kind: "conversation";
  projectId: string;
};

type TestDockTarget = BrowserDockTarget | ConversationDockTarget;

test("BrowserDockTarget remains a placement payload in a mixed-project dock", () => {
  const browserTarget: BrowserDockTarget = {
    id: "browser-panel:browser-one",
    kind: "browser",
    title: "Browser",
    browserPanelId: "browser-one",
  };
  const browserId: `browser-panel:${string}` = browserTarget.id;
  const browserKind: "browser" = browserTarget.kind;
  const browserTitle: "Browser" = browserTarget.title;

  const state = normalizeConsoleDockState<TestDockTarget>({
    activeTabId: "mixed",
    focusedPanelId: "conversation-a",
    panels: [
      {
        id: "conversation-a",
        mode: "console",
        target: {
          id: "conversation:a",
          kind: "conversation",
          title: "Conversation A",
          projectId: "project-a",
        },
      },
      { id: "browser", mode: "console", target: browserTarget },
      {
        id: "conversation-b",
        mode: "console",
        target: {
          id: "conversation:b",
          kind: "conversation",
          title: "Conversation B",
          projectId: "project-b",
        },
      },
    ],
    tabs: [{
      id: "mixed",
      presetId: "grid",
      layout: {
        kind: "split",
        id: "root",
        direction: "horizontal",
        first: { kind: "panel", panelId: "conversation-a" },
        second: {
          kind: "split",
          id: "right",
          direction: "vertical",
          first: { kind: "panel", panelId: "browser" },
          second: { kind: "panel", panelId: "conversation-b" },
        },
      },
    }],
  });

  const focused = focusConsoleDockPanel(state, "browser");
  assert.equal(browserId, "browser-panel:browser-one");
  assert.equal(browserKind, "browser");
  assert.equal(browserTitle, "Browser");
  assert.equal(focused.focusedPanelId, "browser");
  assert.strictEqual(focused.panels.find((panel) => panel.id === "browser")?.target, browserTarget);
  assert.deepEqual(
    focused.panels
      .map((panel) => panel.target)
      .filter((target): target is ConversationDockTarget => target?.kind === "conversation")
      .map((target) => target.projectId),
    ["project-a", "project-b"],
  );
});
