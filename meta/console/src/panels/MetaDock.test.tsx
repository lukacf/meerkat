import { fireEvent, render, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { buildConsoleDockViewState, createConsoleDockState } from "@console-core";
import type { ConsoleAgent } from "../types";
import { buildDockTarget, type MetaDockTarget } from "../lib/adapters";
import { MetaDock } from "./MetaDock";

// DOM assertions compare booleans and strings, never elements.

const unnamed = {
  agent_id: "router:main",
  member_id: "router:main",
  identity: "router:main",
  label: "router:main",
  kind: "identity",
  role: "router",
  state: "active",
} as ConsoleAgent;

const generated = {
  agent_id: "agent:01HXYZ8Q9ABCDEF",
  member_id: "agent:01HXYZ8Q9ABCDEF",
  identity: "agent:01HXYZ8Q9ABCDEF",
  label: "agent:01HXYZ8Q9ABCDEF",
  kind: "identity",
  role: "reviewer",
  state: "active",
} as ConsoleAgent;

function renderDock(agents: ConsoleAgent[]) {
  let next = 1;
  const state = createConsoleDockState<MetaDockTarget>({
    initialTarget: buildDockTarget(agents[0]!),
    initialPresetId: "two_columns",
    createPanelState: ({ target }) => ({ id: `panel-${next++}`, target: target || null, mode: "console" }),
    createSplitId: () => `split-${next++}`,
    createTabId: () => `tab-${next++}`,
  });
  const noop = vi.fn();
  const view = render(
    <MetaDock
      viewState={buildConsoleDockViewState(state)}
      agents={agents}
      renderPanelBody={() => null}
      onSelectTab={noop}
      onCloseTab={noop}
      onCreateTab={noop}
      onFocusPanel={noop}
      onSplitPanel={noop}
      onClosePanel={noop}
      onResizeSplit={noop}
      onOpenTargetInPanel={noop}
    />,
  );
  const chatPaneId = state.panels.find((panel) => panel.target?.kind === "agent-chat")?.id || "";
  return { view, chatPaneId };
}

describe("MetaDock pane titles", () => {
  it("name an agent the way the sidebar does and keep its identity in the tooltip", () => {
    const { view, chatPaneId } = renderDock([unnamed]);
    const pane = view.getByTestId(`pane:${chatPaneId}`);
    const title = within(pane).getByTestId(`pane-title:${chatPaneId}`);
    expect(title.textContent?.includes("Router main")).toBe(true);
    expect(pane.querySelector(".pane__bar")?.textContent?.includes("router:main")).toBe(false);
    expect(title.querySelector(".pane__title-text")?.getAttribute("title")).toBe("router:main");

    fireEvent.click(title);
    const item = within(pane).getByTestId("pane-menu-agent:router:main");
    expect(item.textContent?.includes("Router main")).toBe(true);
  });

  it("keeps an agent's identity in the pane menu as demoted, middle-truncated detail", () => {
    const { view, chatPaneId } = renderDock([unnamed, generated]);
    fireEvent.click(view.getByTestId(`pane-title:${chatPaneId}`));
    const item = view.getByTestId("pane-menu-agent:agent:01HXYZ8Q9ABCDEF");
    expect(item.querySelector(".pane-menu__name")?.textContent).toBe("Reviewer abcdef");
    const id = item.querySelector(".pane-menu__id");
    expect(id?.textContent).toBe("agent:01HXYZ8Q9ABCDEF");
    expect(id?.getAttribute("title")).toBe("agent:01HXYZ8Q9ABCDEF");
    expect(id?.querySelector(".pane-menu__id-head")?.textContent).toBe("agent:01HXYZ8Q9");
    expect(id?.querySelector(".pane-menu__id-tail")?.textContent).toBe("ABCDEF");
    // The text a real-backend scenario selects agents by is unchanged.
    expect(item.textContent?.includes("agent:01HXYZ8Q9ABCDEF")).toBe(true);
  });
});
