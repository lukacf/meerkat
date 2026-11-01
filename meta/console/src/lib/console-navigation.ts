// Cross-panel navigation (spec A4.7): what the console does when a panel
// opens an entity by its name. An agent opens beside the current pane (its
// details, or its chat); a work item, memory record, approval, route or view
// opens its view, focused on the entity. A view the console does not offer
// has no opener, so the kit renders its links as plain text.

import React from "react";
import {
  collectConsoleDockPanelIds,
  type ConsoleDockAction,
  type ConsoleDockNode,
  type ConsoleDockState,
  type ConsoleDockTarget,
} from "@console-core";
import type { ConsoleNavigation } from "@console-components";
import { normalizeNavKind, type NavKind } from "../panels/Sidebar";
import type { ConsoleAgent } from "../types";

/** The entities a view focuses on request (WorkGraph's items, Routing's routes). */
export type NavigationFocusKind = "work-item" | "route";

/** A request to show one entity. Each request has its own serial, so asking twice focuses twice. */
export interface NavigationFocusRequest {
  kind: NavigationFocusKind;
  id: string;
  serial: number;
}

/** What the console does for each opener, against its dock and its own state. */
export interface ConsoleNavigationHost<TTarget extends ConsoleDockTarget = ConsoleDockTarget> {
  /** The dock as it is now. */
  dockState(): ConsoleDockState<TTarget>;
  dispatch(action: ConsoleDockAction<TTarget>): void;
  /** The agent a reference names: an identity, member id, label or comms name. */
  findAgent(reference: string): ConsoleAgent | null;
  chatTarget(agent: ConsoleAgent): TTarget;
  detailsTarget(agent: ConsoleAgent): TTarget;
  viewTarget(kind: NavKind): TTarget;
  /** What opening an agent's details also does: select it, read it fresh. */
  onAgentDetails(agent: ConsoleAgent): void;
  /** What opening an agent's chat also does: read its timeline fresh. */
  onAgentChat(agent: ConsoleAgent): void;
  /** Approvals at one request. */
  selectApproval(id: string): void;
  /** The record in Memory's drawer. */
  loadMemoryRecord(realm: string, id: string): void;
  /** Ask the view that shows this kind of entity to focus on it. */
  focus(kind: NavigationFocusKind, id: string): void;
}

/** Panes that show an agent: its chat or its details. */
const AGENT_TARGET_KINDS = new Set(["agent-chat", "identity-inspect"]);

function sameTarget(shown: ConsoleDockTarget | null | undefined, target: ConsoleDockTarget): boolean {
  return Boolean(shown) && shown!.kind === target.kind && shown!.id === target.id;
}

/** The tab that holds the focused pane, else the active tab. */
function currentTab<TTarget extends ConsoleDockTarget>(state: ConsoleDockState<TTarget>) {
  const focused = state.focusedPanelId;
  return (focused && state.tabs.find((tab) => collectConsoleDockPanelIds(tab.layout).includes(focused)))
    || state.tabs.find((tab) => tab.id === state.activeTabId)
    || null;
}

/** The pane of the current tab that already shows the target. */
function paneShowing<TTarget extends ConsoleDockTarget>(state: ConsoleDockState<TTarget>, target: TTarget): string | null {
  const tab = currentTab(state);
  if (!tab) return null;
  const ids = collectConsoleDockPanelIds(tab.layout);
  return state.panels.find((panel) => ids.includes(panel.id) && sameTarget(panel.target, target))?.id ?? null;
}

/** The pane that shares a split with `panelId`, when that neighbour is a single pane. */
function splitNeighbour(node: ConsoleDockNode, panelId: string): string | null {
  if (node.kind !== "split") return null;
  const { first, second } = node;
  if (first.kind === "panel" && first.panelId === panelId) return second.kind === "panel" ? second.panelId : null;
  if (second.kind === "panel" && second.panelId === panelId) return first.kind === "panel" ? first.panelId : null;
  return splitNeighbour(first, panelId) ?? splitNeighbour(second, panelId);
}

/** Dock actions that show a view: focus the pane that already shows it, else show it in the focused pane. */
export function showViewActions<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  target: TTarget,
): ConsoleDockAction<TTarget>[] {
  const showing = paneShowing(state, target);
  if (showing) return [{ type: "focus_panel", panelId: showing }];
  return [{ type: "open_target", target, intent: "replace_focused" }];
}

/**
 * Dock actions that show an agent beside the focused pane, keeping one agent
 * pane there instead of adding one per click:
 * 1. a pane of the current tab already shows it: focus that pane;
 * 2. the focused pane shows nothing: show it there;
 * 3. the focused pane's neighbour shows an agent (chat or details) or
 *    nothing: show it there;
 * 4. otherwise split the focused pane and show it on the right.
 */
export function besideActions<TTarget extends ConsoleDockTarget>(
  state: ConsoleDockState<TTarget>,
  target: TTarget,
): ConsoleDockAction<TTarget>[] {
  const showing = paneShowing(state, target);
  if (showing) return [{ type: "focus_panel", panelId: showing }];
  const focusedId = state.focusedPanelId;
  const focused = focusedId ? state.panels.find((panel) => panel.id === focusedId) : undefined;
  const tab = currentTab(state);
  if (!focused || !tab || !focused.target) return [{ type: "open_target", target, intent: "replace_focused" }];
  const neighbourId = splitNeighbour(tab.layout, focused.id);
  const neighbour = neighbourId ? state.panels.find((panel) => panel.id === neighbourId) : undefined;
  if (neighbour && (!neighbour.target || AGENT_TARGET_KINDS.has(neighbour.target.kind))) {
    return [
      { type: "set_panel_target", panelId: neighbour.id, target },
      { type: "focus_panel", panelId: neighbour.id },
    ];
  }
  return [{ type: "open_target", target, intent: "split_right" }];
}

/**
 * The console's openers for the kit's links (ConsoleNavigationProvider).
 * `host` is read at each call, so the openers stay one object while the
 * console's state moves on; `offered` are the views the console offers (its
 * nav), and an entity whose view is not offered has no opener.
 */
export function buildConsoleNavigation<TTarget extends ConsoleDockTarget>(
  host: () => ConsoleNavigationHost<TTarget>,
  offered: readonly NavKind[],
): ConsoleNavigation {
  const offers = new Set(offered);
  const run = (current: ConsoleNavigationHost<TTarget>, actions: ConsoleDockAction<TTarget>[]) => {
    for (const action of actions) current.dispatch(action);
  };
  const showView = (current: ConsoleNavigationHost<TTarget>, kind: NavKind) =>
    run(current, showViewActions(current.dockState(), current.viewTarget(kind)));
  const navigation: ConsoleNavigation = {
    openAgent(identity, how = "details") {
      const current = host();
      const agent = current.findAgent(identity);
      if (!agent) return;
      if (how === "chat") {
        current.onAgentChat(agent);
        run(current, besideActions(current.dockState(), current.chatTarget(agent)));
        return;
      }
      current.onAgentDetails(agent);
      run(current, besideActions(current.dockState(), current.detailsTarget(agent)));
    },
    openPanel(kind) {
      const view = normalizeNavKind(kind);
      if (view && offers.has(view)) showView(host(), view);
    },
  };
  if (offers.has("workgraph")) {
    navigation.openWorkItem = (id) => {
      const current = host();
      showView(current, "workgraph");
      current.focus("work-item", id);
    };
  }
  if (offers.has("memory")) {
    navigation.openMemoryRecord = (realm, id) => {
      const current = host();
      showView(current, "memory");
      current.loadMemoryRecord(realm, id);
    };
  }
  if (offers.has("gating")) {
    navigation.openApproval = (id) => {
      const current = host();
      current.selectApproval(id);
      showView(current, "gating");
    };
  }
  if (offers.has("routing")) {
    navigation.openRoute = (key) => {
      const current = host();
      showView(current, "routing");
      current.focus("route", key);
    };
  }
  return navigation;
}

// ── Focus requests ────────────────────────────────────────────────────

interface NavigationFocusValue {
  request: NavigationFocusRequest | null;
  /** The view showed the entity: the request is spent. */
  done(serial: number): void;
}

const NavigationFocusContext = React.createContext<NavigationFocusValue | null>(null);

/** The console's focus requests: `focus` asks, `value` goes to NavigationFocusProvider. */
export function useNavigationFocusSource(): { value: NavigationFocusValue; focus(kind: NavigationFocusKind, id: string): void } {
  const [request, setRequest] = React.useState<NavigationFocusRequest | null>(null);
  const serial = React.useRef(0);
  const focus = React.useCallback((kind: NavigationFocusKind, id: string) => {
    serial.current += 1;
    setRequest({ kind, id, serial: serial.current });
  }, []);
  const done = React.useCallback((handled: number) => {
    setRequest((current) => (current && current.serial === handled ? null : current));
  }, []);
  const value = React.useMemo(() => ({ request, done }), [request, done]);
  return { value, focus };
}

export function NavigationFocusProvider({ value, children }: { value: NavigationFocusValue; children?: React.ReactNode }): React.JSX.Element {
  return React.createElement(NavigationFocusContext.Provider, { value }, children);
}

/**
 * For a view that shows entities of one kind: `onFocus` gets the entity's id
 * once per request (when the view is open, or as it opens). Nothing happens
 * without a provider.
 */
export function useNavigationFocus(kind: NavigationFocusKind, onFocus: (id: string) => void): void {
  const context = React.useContext(NavigationFocusContext);
  const onFocusRef = React.useRef(onFocus);
  onFocusRef.current = onFocus;
  const request = context?.request ?? null;
  const done = context?.done;
  React.useEffect(() => {
    if (!request || request.kind !== kind || !done) return;
    onFocusRef.current(request.id);
    done(request.serial);
  }, [request, kind, done]);
}
