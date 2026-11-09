import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { buildConsoleDockViewState, createConsoleDockState } from "@console-core";
import type { ConsoleAgent } from "../types";
import { buildDockTarget, type MetaDockTarget } from "../lib/adapters";
import { MetaDock } from "./MetaDock";
import type { NavKind } from "./Sidebar";

// DOM assertions compare booleans and strings, never elements.

afterEach(() => { cleanup(); });

const agent = (id: string, label: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  agent_id: id, member_id: id, identity: id, label, kind: "mob_agent", role: "analyst", state: "active", ...extra,
});

function renderDock(agents: ConsoleAgent[], visibleControls?: NavKind[]) {
  let next = 1;
  const state = createConsoleDockState<MetaDockTarget>({
    initialTarget: buildDockTarget(agents[0]!),
    initialPresetId: "two_columns",
    createPanelState: ({ target }) => ({ id: `panel-${next++}`, target: target || null, mode: "console" }),
    createSplitId: () => `split-${next++}`,
    createTabId: () => `tab-${next++}`,
  });
  const handlers = {
    onSelectTab: vi.fn(),
    onCloseTab: vi.fn(),
    onCreateTab: vi.fn(),
    onFocusPanel: vi.fn(),
    onSplitPanel: vi.fn(),
    onClosePanel: vi.fn(),
    onResizeSplit: vi.fn(),
    onOpenTargetInPanel: vi.fn(),
  };
  const view = render(
    <MetaDock
      viewState={buildConsoleDockViewState(state)}
      agents={agents}
      renderPanelBody={() => null}
      visibleControls={visibleControls}
      {...handlers}
    />,
  );
  const chatPaneId = state.panels.find((panel) => panel.target?.kind === "agent-chat")?.id || "";
  return { view, chatPaneId, handlers, focusedPanelId: state.focusedPanelId };
}

const ALL_VIEWS: NavKind[] = ["topology", "timeline", "gating", "roster", "routing", "logs", "health", "access", "memory", "workgraph", "schedules"];
const ids = (container: HTMLElement, prefix: string) => [...container.querySelectorAll(`[data-testid^="${prefix}"]`)]
  .map((node) => node.getAttribute("data-testid")?.slice(prefix.length));

describe("MetaDock pane menu (B16)", () => {
  it("lists every view the sidebar lists, in its order and with icons", () => {
    // The host's controls arrive in its own order; the menu shows the nav's.
    const { view, chatPaneId } = renderDock([agent("router", "Router")], ["schedules", "memory", "topology", "access", "workgraph", "gating"]);
    fireEvent.click(view.getByTestId(`pane-title:${chatPaneId}`));
    expect(ids(view.container, "pane-menu-view:")).toEqual(["topology", "gating", "access", "memory", "workgraph", "schedules"]);
    for (const kind of ["memory", "workgraph", "access", "schedules"]) {
      expect(view.getByTestId(`pane-menu-view:${kind}`).querySelector("svg.pane-menu__icon use")?.getAttribute("href")).toBe(`#i-${kind}`);
    }
    expect(view.getByTestId("pane-menu-view:gating").textContent).toBe("Approvals");
  });

  it("lists every view when the host names none", () => {
    const { view, chatPaneId } = renderDock([agent("router", "Router")]);
    fireEvent.click(view.getByTestId(`pane-title:${chatPaneId}`));
    expect(ids(view.container, "pane-menu-view:")).toEqual(ALL_VIEWS);
  });

  it("opens on its search field, filters views and agents, and picks with the keyboard", () => {
    const agents = [agent("router", "Router"), agent("billing", "Billing", { role: "domain" }), agent("memory-keeper", "Archivist")];
    const { view, chatPaneId, handlers } = renderDock(agents, ALL_VIEWS);
    const title = view.getByTestId(`pane-title:${chatPaneId}`);
    expect(title.getAttribute("aria-haspopup")).toBe("dialog");
    fireEvent.click(title);
    expect(title.getAttribute("aria-expanded")).toBe("true");
    const dialog = view.getByRole("dialog", { name: "Show in this pane" });
    const search = within(dialog).getByTestId("pane-menu-search");
    expect(document.activeElement?.getAttribute("data-testid")).toBe("pane-menu-search");
    expect(search.getAttribute("role")).toBe("combobox");

    fireEvent.change(search, { target: { value: "mem" } });
    expect(ids(view.container, "pane-menu-view:")).toEqual(["memory"]);
    expect(ids(view.container, "pane-menu-agent:")).toEqual(["memory-keeper"]);
    // The first match is active; the arrows move through what is listed.
    const active = () => view.container.querySelector('[role="option"][aria-selected="true"]')?.getAttribute("data-testid");
    expect(active()).toBe("pane-menu-view:memory");
    expect(search.getAttribute("aria-activedescendant")).toBe(view.getByTestId("pane-menu-view:memory").id);
    fireEvent.keyDown(search, { key: "ArrowDown" });
    expect(active()).toBe("pane-menu-agent:memory-keeper");
    fireEvent.keyDown(search, { key: "ArrowDown" });
    expect(active()).toBe("pane-menu-view:memory");
    fireEvent.keyDown(search, { key: "ArrowUp" });
    fireEvent.keyDown(search, { key: "Enter" });
    expect(handlers.onOpenTargetInPanel.mock.calls.at(-1)?.[0]).toBe(chatPaneId);
    expect((handlers.onOpenTargetInPanel.mock.calls.at(-1)?.[1] as MetaDockTarget).kind).toBe("agent-chat");
    expect(view.queryByRole("dialog") === null).toBe(true);

    fireEvent.click(title);
    fireEvent.change(view.getByTestId("pane-menu-search"), { target: { value: "zzz" } });
    expect(view.getByRole("dialog").textContent?.includes('Nothing matches "zzz".')).toBe(true);
    fireEvent.keyDown(view.getByRole("dialog"), { key: "Escape" });
    expect(view.queryByRole("dialog") === null).toBe(true);
    expect(document.activeElement?.getAttribute("data-testid")).toBe(`pane-title:${chatPaneId}`);
  });

  it("opens a view in the pane it was opened from", () => {
    const { view, chatPaneId, handlers } = renderDock([agent("router", "Router")], ALL_VIEWS);
    fireEvent.click(view.getByTestId(`pane-title:${chatPaneId}`));
    fireEvent.click(view.getByTestId("pane-menu-view:workgraph"));
    expect(handlers.onOpenTargetInPanel.mock.calls.at(-1)?.[0]).toBe(chatPaneId);
    expect((handlers.onOpenTargetInPanel.mock.calls.at(-1)?.[1] as MetaDockTarget).kind).toBe("workgraph");
  });

  it("renders only the rows in sight of a long roster, and every agent stays findable", () => {
    const agents = Array.from({ length: 300 }, (_, index) => agent(`agent-${String(index).padStart(3, "0")}`, `Agent ${index}`));
    const { view, chatPaneId } = renderDock(agents, ALL_VIEWS);
    fireEvent.click(view.getByTestId(`pane-title:${chatPaneId}`));
    const rendered = ids(view.container, "pane-menu-agent:");
    expect(rendered.length > 0 && rendered.length < 40).toBe(true);
    expect(view.container.querySelector(".pane-menu__count")?.textContent).toBe("300");
    fireEvent.change(view.getByTestId("pane-menu-search"), { target: { value: "Agent 299" } });
    expect(ids(view.container, "pane-menu-agent:")).toEqual(["agent-299"]);
  });
});

describe("MetaDock pane chrome (B16)", () => {
  it("marks the focused pane quietly and names its controls", () => {
    const { view, focusedPanelId } = renderDock([agent("router", "Router")]);
    const pane = view.getByTestId(`pane:${focusedPanelId}`);
    expect(pane.classList.contains("is-focused")).toBe(true);
    expect(pane.getAttribute("data-focused")).toBe("true");
    const bar = pane.querySelector(".pane__bar");
    expect(bar?.classList.contains("cc-reveal-host")).toBe(true);
    for (const [part, label] of [["pane-split-right", "Split right"], ["pane-split-down", "Split down"], ["pane-close", "Close pane"]] as const) {
      const button = view.getByTestId(`${part}:${focusedPanelId}`);
      expect(button.getAttribute("aria-label")).toBe(label);
      // Revealed on hover or focus of the bar, never out of the tab order.
      expect(button.classList.contains("cc-reveal")).toBe(true);
      expect(button.tabIndex).toBe(0);
    }
    // No Unicode glyphs left in the bar.
    expect(/[◨⬓×▾]/.test(bar?.textContent || "")).toBe(false);
  });

  it("splits and closes from the bar", () => {
    const { view, focusedPanelId, handlers } = renderDock([agent("router", "Router")]);
    fireEvent.click(view.getByTestId(`pane-split-right:${focusedPanelId}`));
    fireEvent.click(view.getByTestId(`pane-split-down:${focusedPanelId}`));
    fireEvent.click(view.getByTestId(`pane-close:${focusedPanelId}`));
    expect(handlers.onSplitPanel.mock.calls).toEqual([[focusedPanelId, "right"], [focusedPanelId, "down"]]);
    expect(handlers.onClosePanel.mock.calls).toEqual([[focusedPanelId]]);
  });

  it("shows workspaces as tabs and a new-workspace button", () => {
    const { view, handlers } = renderDock([agent("router", "Router")]);
    const tabs = view.getAllByRole("tab");
    expect(tabs.length).toBe(1);
    expect(tabs[0]?.getAttribute("aria-selected")).toBe("true");
    // One pane: the name alone; the count appears from two panes on.
    expect(tabs[0]?.textContent).toBe("Router");
    expect(tabs[0]?.querySelector(".wstab__count") === null).toBe(true);
    const add = view.getByTestId("wstab-add");
    expect(add.getAttribute("aria-label")).toBe("New workspace");
    fireEvent.click(add);
    expect(handlers.onCreateTab).toHaveBeenCalledTimes(1);
    // One workspace: nothing to close.
    expect(view.queryByTestId(/^wstab-close:/) === null).toBe(true);
  });
});
