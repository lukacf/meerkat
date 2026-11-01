import { act, render } from "@testing-library/react";
import React from "react";
import { describe, expect, it, vi } from "vitest";

import {
  applyConsoleDockAction,
  collectConsoleDockPanelIds,
  createConsoleDockState,
  type ConsoleDockState,
  type ConsoleDockTarget,
} from "@console-core";
import { ALL_NAV, type NavKind } from "../panels/Sidebar";
import type { ConsoleAgent } from "../types";
import {
  NavigationFocusProvider,
  buildConsoleNavigation,
  useNavigationFocus,
  useNavigationFocusSource,
  type ConsoleNavigationHost,
  type NavigationFocusKind,
} from "./console-navigation";

const agents = [
  { identity: "domain:billing", member_id: "billing-member", label: "Billing" },
  { identity: "domain:support", member_id: "support-member", label: "Support" },
] as ConsoleAgent[];

const chat = (agent: ConsoleAgent): ConsoleDockTarget => ({ id: agent.member_id, kind: "agent-chat", title: agent.label || "" });
const details = (agent: ConsoleAgent): ConsoleDockTarget => ({ id: `inspect:${agent.identity}`, kind: "identity-inspect", title: `${agent.label} Details` });
const view = (kind: string): ConsoleDockTarget => ({ id: kind, kind, title: kind });

/** A console whose dock applies the openers' actions, and what else each opener did. */
function consoleWith(initial: ConsoleDockTarget | null, offered: readonly NavKind[] = ALL_NAV) {
  let panes = 0;
  let splits = 0;
  let tabs = 0;
  const options = {
    createPanelState: ({ target }: { target?: ConsoleDockTarget | null }) => ({ id: `pane-${++panes}`, target: target ?? null, mode: "console" as const }),
    createSplitId: () => `split-${++splits}`,
    createTabId: () => `tab-${++tabs}`,
  };
  let state: ConsoleDockState<ConsoleDockTarget> = createConsoleDockState<ConsoleDockTarget>({ initialTarget: initial, ...options });
  const did: string[] = [];
  const host: ConsoleNavigationHost<ConsoleDockTarget> = {
    dockState: () => state,
    dispatch: (action) => {
      state = applyConsoleDockAction(state, action, options);
    },
    findAgent: (reference) => agents.find((agent) => [agent.identity, agent.member_id, agent.label].includes(reference)) ?? null,
    chatTarget: chat,
    detailsTarget: details,
    viewTarget: view,
    onAgentDetails: (agent) => did.push(`select ${agent.member_id}`),
    onAgentChat: (agent) => did.push(`read ${agent.identity}`),
    selectApproval: (id) => did.push(`approval ${id}`),
    loadMemoryRecord: (realm, id) => did.push(`memory ${realm}/${id}`),
    focus: (kind: NavigationFocusKind, id: string) => did.push(`focus ${kind} ${id}`),
  };
  const navigation = buildConsoleNavigation(() => host, offered);
  /** The active tab's panes in layout order, the focused one marked with "*". */
  const panesShown = () => {
    const tab = state.tabs.find((candidate) => candidate.id === state.activeTabId)!;
    return collectConsoleDockPanelIds(tab.layout).map((id) => {
      const pane = state.panels.find((candidate) => candidate.id === id)!;
      return `${id === state.focusedPanelId ? "*" : ""}${pane.target?.id ?? "empty"}`;
    });
  };
  const focusPane = (index: number) => {
    const tab = state.tabs.find((candidate) => candidate.id === state.activeTabId)!;
    host.dispatch({ type: "focus_panel", panelId: collectConsoleDockPanelIds(tab.layout)[index]! });
  };
  return { navigation, did, panesShown, focusPane, host };
}

describe("openAgent", () => {
  it("opens an agent's details beside the current pane, and keeps one agent pane there", () => {
    const app = consoleWith(view("logs"));
    app.navigation.openAgent!("Billing");
    expect(app.panesShown()).toEqual(["logs", "*inspect:domain:billing"]);
    expect(app.did).toEqual(["select billing-member"]);
    // From Logs again: the agent pane beside it shows the next agent.
    app.focusPane(0);
    app.navigation.openAgent!("domain:support", "details");
    expect(app.panesShown()).toEqual(["logs", "*inspect:domain:support"]);
    // An agent already shown is focused, not opened twice.
    app.focusPane(0);
    app.navigation.openAgent!("support-member");
    expect(app.panesShown()).toEqual(["logs", "*inspect:domain:support"]);
    // An empty pane shows it in place; an unknown agent opens nothing.
    const empty = consoleWith(null);
    empty.navigation.openAgent!("Billing");
    expect(empty.panesShown()).toEqual(["*inspect:domain:billing"]);
    empty.navigation.openAgent!("nobody");
    expect(empty.panesShown()).toEqual(["*inspect:domain:billing"]);
    expect(empty.did).toEqual(["select billing-member"]);
  });

  it("opens an agent's chat beside the current pane, never over another view", () => {
    const app = consoleWith(view("logs"));
    app.navigation.openAgent!("billing-member", "chat");
    expect(app.panesShown()).toEqual(["logs", "*billing-member"]);
    expect(app.did).toEqual(["read domain:billing"]);
    // Logs beside WorkGraph: the chat splits Logs instead of replacing WorkGraph.
    const busy = consoleWith(view("logs"));
    busy.host.dispatch({ type: "open_target", target: view("workgraph"), intent: "split_right" });
    busy.focusPane(0);
    busy.navigation.openAgent!("Support", "chat");
    expect(busy.panesShown()).toEqual(["logs", "*support-member", "workgraph"]);
  });
});

describe("entity openers", () => {
  it("openWorkItem opens WorkGraph and asks it to focus the item", () => {
    const app = consoleWith(view("logs"));
    app.navigation.openWorkItem!("item-7");
    expect(app.panesShown()).toEqual(["*workgraph"]);
    expect(app.did).toEqual(["focus work-item item-7"]);
    // WorkGraph already open beside: that pane takes focus, Logs stays.
    const split = consoleWith(view("logs"));
    split.host.dispatch({ type: "open_target", target: view("workgraph"), intent: "split_right" });
    split.focusPane(0);
    split.navigation.openWorkItem!("item-8");
    expect(split.panesShown()).toEqual(["logs", "*workgraph"]);
    expect(consoleWith(view("logs"), ALL_NAV.filter((kind) => kind !== "workgraph")).navigation.openWorkItem).toBeUndefined();
  });

  it("openMemoryRecord opens Memory with the record in its drawer", () => {
    const app = consoleWith(view("timeline"));
    app.navigation.openMemoryRecord!("realm-a", "mem-1");
    expect(app.panesShown()).toEqual(["*memory"]);
    expect(app.did).toEqual(["memory realm-a/mem-1"]);
    expect(consoleWith(view("logs"), ["logs"]).navigation.openMemoryRecord).toBeUndefined();
  });

  it("openApproval opens Approvals at the request", () => {
    const app = consoleWith(view("logs"));
    app.navigation.openApproval!("pending-3");
    expect(app.panesShown()).toEqual(["*gating"]);
    expect(app.did).toEqual(["approval pending-3"]);
    expect(consoleWith(view("logs"), ["logs", "roster"]).navigation.openApproval).toBeUndefined();
  });

  it("openRoute opens Routing and asks it to focus the route", () => {
    const app = consoleWith(view("topology"));
    app.navigation.openRoute!("route:billing->support");
    expect(app.panesShown()).toEqual(["*routing"]);
    expect(app.did).toEqual(["focus route route:billing->support"]);
    // A console without a team control surface offers no Routing.
    expect(consoleWith(view("logs"), ["topology", "timeline", "roster", "logs", "health"]).navigation.openRoute).toBeUndefined();
  });

  it("openPanel opens a view the console offers, and nothing else", () => {
    const app = consoleWith(view("logs"), ["logs", "health", "schedules"]);
    app.navigation.openPanel!("schedules");
    expect(app.panesShown()).toEqual(["*schedules"]);
    app.navigation.openPanel!("access");
    app.navigation.openPanel!("no-such-view");
    expect(app.panesShown()).toEqual(["*schedules"]);
    expect(app.did).toEqual([]);
  });
});

describe("focus requests", () => {
  function Host({ onFocusSource, children }: { onFocusSource: (focus: (kind: NavigationFocusKind, id: string) => void) => void; children?: React.ReactNode }) {
    const source = useNavigationFocusSource();
    onFocusSource(source.focus);
    return <NavigationFocusProvider value={source.value}>{children}</NavigationFocusProvider>;
  }
  function WorkItems({ onFocus }: { onFocus: (id: string) => void }) {
    useNavigationFocus("work-item", onFocus);
    return null;
  }

  it("reach the view of their kind once each, as it opens or while it is open", () => {
    let focus: (kind: NavigationFocusKind, id: string) => void = () => {};
    const seen = vi.fn();
    const view = render(<Host onFocusSource={(next) => { focus = next; }}><WorkItems onFocus={seen} /></Host>);
    act(() => focus("work-item", "item-1"));
    act(() => focus("route", "route-1"));
    expect(seen.mock.calls).toEqual([["item-1"]]);
    // A request is spent once shown: a view opened later does not replay it.
    view.rerender(<Host onFocusSource={(next) => { focus = next; }}>{null}</Host>);
    view.rerender(<Host onFocusSource={(next) => { focus = next; }}><WorkItems onFocus={seen} /></Host>);
    expect(seen.mock.calls).toEqual([["item-1"]]);
    // Asking again for the same item focuses it again; a view that opens with the request sees it.
    act(() => focus("work-item", "item-1"));
    expect(seen.mock.calls).toEqual([["item-1"], ["item-1"]]);
    view.rerender(<Host onFocusSource={(next) => { focus = next; }}>{null}</Host>);
    act(() => focus("work-item", "item-2"));
    view.rerender(<Host onFocusSource={(next) => { focus = next; }}><WorkItems onFocus={seen} /></Host>);
    expect(seen.mock.calls).toEqual([["item-1"], ["item-1"], ["item-2"]]);
  });

  it("do nothing without a provider", () => {
    const seen = vi.fn();
    render(<WorkItems onFocus={seen} />);
    expect(seen).not.toHaveBeenCalled();
  });
});
