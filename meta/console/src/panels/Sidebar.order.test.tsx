import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Sidebar } from "./Sidebar";
import type { ConsoleAgent } from "../types";

afterEach(() => { cleanup(); localStorage.clear(); });

const agent = (id: string, label: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  agent_id: id, member_id: id, identity: `agent:${id}`, label, kind: "mob_agent", role: "analyst", group: "Team", ...extra,
});
const team = [agent("a", "Alpha"), agent("b", "Bravo"), agent("c", "Charlie")];
const props = {
  agents: team, selectedMemberId: "", recentActivity: [], collapsed: false, visibleControls: [],
  onSelect: vi.fn(), onOpenControl: vi.fn(), grouping: { group_by: ["group"] },
  storageNamespace: "configuration-a", preferenceScope: "principal-a",
};
const ORDER_KEY = "meta-console-sidebar-agent-order:principal-a";
const VIEW_KEY = "meta-console-sidebar-view:principal-a";
const rows = (container: HTMLElement) => [...container.querySelectorAll('[data-testid^="sidebar-agent:"]')]
  .map(node => node.getAttribute("data-testid")?.slice("sidebar-agent:".length));
const announced = (container: HTMLElement) =>
  container.querySelector('[data-testid="sidebar-reorder-live"]')?.textContent || "";
// The roster view lives in the mast's view options popover (A5): open it
// unless it is open, then press an option. A change keeps it open.
const chooseView = (view: ReturnType<typeof render>, option: string) => {
  if (!view.queryByTestId("sidebar-view-options")) fireEvent.click(view.getByTestId("sidebar-view-menu"));
  fireEvent.click(view.getByTestId(option));
};
const pressed = (view: ReturnType<typeof render>, option: string) => view.getByTestId(option).getAttribute("aria-pressed");
// A control inside another control: a button or link in a button, or in
// an element that claims the button role.
const NESTED_CONTROLS = [
  "button button", "button a", "button [role='button']",
  "[role='button'] button", "[role='button'] a", "[role='button'] [role='button']",
].join(", ");

describe("Roster manual order", () => {
  it("moves the focused agent with Alt+ArrowDown, names it in the announcement and keeps focus", async () => {
    const view = render(<Sidebar {...props} />);
    const alpha = view.getByTestId("sidebar-agent:a");
    alpha.focus();
    fireEvent.keyDown(alpha, { key: "ArrowDown", altKey: true });

    await waitFor(() => expect(rows(view.container)).toEqual(["b", "a", "c"]));
    expect(announced(view.container)).toBe("Moved Alpha below Bravo.");
    expect(JSON.parse(localStorage.getItem(ORDER_KEY) || "[]")).toEqual(["agent:b", "agent:a", "agent:c"]);
    await waitFor(() => expect(document.activeElement?.getAttribute("data-testid")).toBe("sidebar-agent:a"));
    expect(view.getByTestId("sidebar-agent:a").getAttribute("aria-keyshortcuts")).toBe("Alt+ArrowUp Alt+ArrowDown");
  });

  it("explains why an agent cannot move at an edge, in another sort or while filtered", () => {
    const view = render(<Sidebar {...props} />);
    const alpha = view.getByTestId("sidebar-agent:a");
    fireEvent.keyDown(alpha, { key: "ArrowUp", altKey: true });
    expect(announced(view.container)).toBe("Alpha is already first.");

    chooseView(view, "sidebar-view-sort:name");
    fireEvent.keyDown(view.getByTestId("sidebar-agent:a"), { key: "ArrowDown", altKey: true });
    expect(announced(view.container)).toBe("Choose Manual under Sort by in the roster view options to reorder agents.");
    expect(view.queryByTestId("sidebar-agent-grip:a") === null).toBe(true);

    chooseView(view, "sidebar-view-sort:manual");
    expect(view.queryByTestId("sidebar-agent-grip:a") !== null).toBe(true);
    fireEvent.change(view.getByPlaceholderText("Search agents"), { target: { value: "a" } });
    fireEvent.keyDown(view.getByTestId("sidebar-agent:a"), { key: "ArrowDown", altKey: true });
    expect(announced(view.container)).toBe("Clear the search and the activity filter to reorder agents.");
    expect(view.queryByTestId("sidebar-agent-grip:a") === null).toBe(true);
    expect(localStorage.getItem(ORDER_KEY)).toBeNull();
  });

  it("applies a stored order and keeps a new agent at the end of its category", () => {
    localStorage.setItem(ORDER_KEY, JSON.stringify(["agent:c", "agent:a", "agent:b"]));
    const view = render(<Sidebar {...props} agents={[...team, agent("d", "Aaron")]} />);
    expect(rows(view.container)).toEqual(["c", "a", "b", "d"]);
  });

  it("keeps the view and the order per user across remounts and configuration changes", () => {
    localStorage.setItem(ORDER_KEY, JSON.stringify(["agent:b", "agent:a", "agent:c"]));
    const first = render(<Sidebar {...props} />);
    chooseView(first, "sidebar-view-group:none");
    expect(first.queryByTestId("sidebar-section-toggle:Team") === null).toBe(true);
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toEqual({ v: 1, group_by: "none", sort_by: "manual" });
    first.unmount();

    const second = render(<Sidebar {...props} storageNamespace="configuration-b" />);
    fireEvent.click(second.getByTestId("sidebar-view-menu"));
    expect(pressed(second, "sidebar-view-group:none")).toBe("true");
    expect(pressed(second, "sidebar-view-group:category")).toBe("false");
    expect(rows(second.container)).toEqual(["b", "a", "c"]);
  });

  it("sorts by name and by last activity, and takes newer frames this console saw", () => {
    const agents = [
      agent("a", "Agent 10", { last_activity_ms: 9 }),
      agent("b", "agent 2", { last_activity_ms: 5 }),
      agent("c", "Agent 1"),
    ];
    const view = render(<Sidebar {...props} agents={agents} />);
    chooseView(view, "sidebar-view-sort:name");
    expect(rows(view.container)).toEqual(["c", "b", "a"]);
    chooseView(view, "sidebar-view-sort:activity");
    expect(rows(view.container)).toEqual(["a", "b", "c"]);

    view.rerender(<Sidebar {...props} agents={agents} recentActivity={[
      { id: "frame-c", event: "text_delta", identity: "agent:c", timestampMs: 20 },
    ]} />);
    expect(rows(view.container)).toEqual(["c", "a", "b"]);
  });

  it("shows a human detail line and keeps the identity in the tooltip and a copy action", () => {
    const onSelect = vi.fn();
    const view = render(<Sidebar {...props} onSelect={onSelect}
      agents={[agent("a", "Alpha", { role: "router", response_phase: "generating" })]} />);
    const detail = view.getByTestId("sidebar-agent-detail:a");
    expect(detail.textContent).toBe("Router · writing a reply");
    // The kind truncates on its own; the separator and the activity are its
    // siblings, so a narrow roster never swallows them.
    expect(detail.querySelector(".agent__detail-kind")?.textContent).toBe("Router");
    expect(detail.querySelector(".agent__detail-separator")?.parentElement?.classList.contains("agent__detail")).toBe(true);
    expect(detail.querySelector(".agent__detail-status")?.textContent).toBe("writing a reply");
    const row = view.getByTestId("sidebar-agent:a");
    expect(row.getAttribute("title")).toBe("Identity: agent:a");
    expect((row.textContent || "").includes("agent:a")).toBe(false);

    fireEvent.click(view.getByRole("button", { name: "Copy the identity of Alpha" }));
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("names an agent without a display name for people and keeps its identity in the tooltip and copy action", () => {
    const view = render(<Sidebar {...props} onTogglePinnedAgent={vi.fn()} agents={[
      agent("router", "router:main", { identity: "router:main", role: "lead" }),
      agent("bravo", "Bravo"),
    ]} />);
    const row = view.getByTestId("sidebar-agent:router");
    expect(row.querySelector(".agent__name")?.textContent).toBe("Router main");
    expect((row.textContent || "").includes("router:main")).toBe(false);
    expect(row.getAttribute("title")).toBe("Identity: router:main");
    expect(view.queryByRole("button", { name: "Copy the identity of Router main" }) !== null).toBe(true);
    expect(view.queryByRole("button", { name: "Pin Router main" }) !== null).toBe(true);
    fireEvent.change(view.getByPlaceholderText("Search agents"), { target: { value: "router main" } });
    expect(rows(view.container)).toEqual(["router"]);
  });

  it("keeps the relative order when Group switches to None and back", () => {
    const agents = [agent("a", "Alpha"), agent("b", "Bravo"), agent("m", "Mike", { group: "Ops" }), agent("z", "Zulu", { group: "Ops" })];
    const view = render(<Sidebar {...props} agents={agents} />);
    expect(rows(view.container)).toEqual(["m", "z", "a", "b"]);
    chooseView(view, "sidebar-view-group:none");
    expect(rows(view.container)).toEqual(["m", "z", "a", "b"]);

    fireEvent.keyDown(view.getByTestId("sidebar-agent:a"), { key: "ArrowUp", altKey: true });
    expect(rows(view.container)).toEqual(["m", "a", "z", "b"]);
    chooseView(view, "sidebar-view-group:category");
    expect(rows(view.container)).toEqual(["m", "z", "a", "b"]);
    chooseView(view, "sidebar-view-group:none");
    expect(rows(view.container)).toEqual(["m", "a", "z", "b"]);
  });

  it("refuses a move while an activity filter is on, and sorts by last activity within it", () => {
    const agents = [
      agent("a", "Alpha", { response_phase: "generating", last_activity_ms: 10 }),
      agent("b", "Bravo", { response_phase: "generating", last_activity_ms: 30 }),
      agent("c", "Charlie", { response_phase: null, last_activity_ms: 50 }),
    ];
    const view = render(<Sidebar {...props} agents={agents} />);
    chooseView(view, "sidebar-view-activity:working");
    expect(view.getByRole("button", { name: "Working", exact: true }).getAttribute("aria-pressed")).toBe("true");
    expect(rows(view.container)).toEqual(["a", "b"]);
    expect(view.queryByTestId("sidebar-agent-grip:a") === null).toBe(true);
    fireEvent.keyDown(view.getByTestId("sidebar-agent:a"), { key: "ArrowDown", altKey: true });
    expect(announced(view.container)).toBe("Clear the search and the activity filter to reorder agents.");
    expect(localStorage.getItem(ORDER_KEY)).toBeNull();

    chooseView(view, "sidebar-view-sort:activity");
    expect(rows(view.container)).toEqual(["b", "a"]);
    fireEvent.click(view.getByRole("button", { name: "All", exact: true }));
    expect(rows(view.container)).toEqual(["c", "b", "a"]);
  });

  it("lets the pin button take Enter without opening the agent", () => {
    const onSelect = vi.fn();
    const onTogglePinnedAgent = vi.fn();
    const view = render(<Sidebar {...props} onSelect={onSelect} onTogglePinnedAgent={onTogglePinnedAgent} />);
    fireEvent.keyDown(view.getByTestId("sidebar-agent-pin:a"), { key: "Enter" });
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.keyDown(view.getByTestId("sidebar-agent:a"), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
  });

  it("keeps an agent's pin and copy actions beside its row's button, never inside it", () => {
    const onSelect = vi.fn();
    const onTogglePinnedAgent = vi.fn();
    const view = render(<Sidebar {...props} onSelect={onSelect} onTogglePinnedAgent={onTogglePinnedAgent} />);
    const row = view.getByTestId("sidebar-agent:a");
    expect(row.getAttribute("role")).toBe("button");
    expect(view.container.querySelectorAll(NESTED_CONTROLS).length).toBe(0);
    expect(row.contains(view.getByTestId("sidebar-agent-pin:a"))).toBe(false);
    expect(row.contains(view.getByTestId("sidebar-agent-copy-identity:a"))).toBe(false);
    fireEvent.click(view.getByTestId("sidebar-agent-pin:a"));
    expect(onTogglePinnedAgent.mock.calls.length).toBe(1);
    expect(onSelect).not.toHaveBeenCalled();
  });
});

describe("Roster pointer drag", () => {
  beforeAll(() => {
    // jsdom has no PointerEvent; a MouseEvent carries everything the drag reads.
    if (!("PointerEvent" in window)) {
      class TestPointerEvent extends MouseEvent {}
      Object.defineProperty(window, "PointerEvent", { value: TestPointerEvent, configurable: true });
    }
  });
  afterEach(() => { Reflect.deleteProperty(document, "elementFromPoint"); });

  const dragCharlieOverAlpha = (view: ReturnType<typeof render>) => {
    // jsdom does no layout: the pointer is always over Alpha's row.
    Object.defineProperty(document, "elementFromPoint", {
      value: vi.fn(() => view.getByTestId("sidebar-agent:a")),
      configurable: true,
    });
    fireEvent.pointerDown(view.getByTestId("sidebar-agent-grip:c"), { button: 0, clientX: 4, clientY: 80 });
    fireEvent.pointerMove(window, { clientX: 4, clientY: 20 });
  };

  it("drops on pointerup and ends without moving on pointercancel", () => {
    const view = render(<Sidebar {...props} />);
    dragCharlieOverAlpha(view);
    fireEvent.pointerCancel(window);
    expect(view.queryByTestId("sidebar-drag-preview") === null).toBe(true);
    expect(rows(view.container)).toEqual(["a", "b", "c"]);
    expect(localStorage.getItem(ORDER_KEY)).toBeNull();
    expect(announced(view.container)).toBe("");

    dragCharlieOverAlpha(view);
    fireEvent.pointerUp(window);
    expect(rows(view.container)).toEqual(["a", "c", "b"]);
    expect(JSON.parse(localStorage.getItem(ORDER_KEY) || "[]")).toEqual(["agent:a", "agent:c", "agent:b"]);
    expect(announced(view.container)).toBe("Moved Charlie below Alpha.");
  });

  it("drops on a row while the pointer is on the row's pin", () => {
    const view = render(<Sidebar {...props} onTogglePinnedAgent={vi.fn()} />);
    Object.defineProperty(document, "elementFromPoint", {
      value: vi.fn(() => view.getByTestId("sidebar-agent-pin:a")),
      configurable: true,
    });
    fireEvent.pointerDown(view.getByTestId("sidebar-agent-grip:c"), { button: 0, clientX: 4, clientY: 80 });
    fireEvent.pointerMove(window, { clientX: 4, clientY: 20 });
    expect(view.getByTestId("sidebar-agent:a").getAttribute("data-drag-over")).toBe("after");
    fireEvent.pointerUp(window);
    expect(rows(view.container)).toEqual(["a", "c", "b"]);
  });

  it("shows no drop line over a row of another container and drops nothing there", () => {
    const agents = [
      agent("p", "Planner"),
      agent("w1", "Worker one", { labels: { spawned_by: "agent:p" } }),
      agent("w2", "Worker two", { labels: { spawned_by: "agent:p" } }),
      agent("q", "Queue"),
    ];
    const view = render(<Sidebar {...props} agents={agents} />);
    // Queue is a root; the workers are Planner's children, another container.
    Object.defineProperty(document, "elementFromPoint", {
      value: vi.fn(() => view.getByTestId("sidebar-agent:w1")),
      configurable: true,
    });
    expect(view.getByTestId("sidebar-agent:w1").getAttribute("data-sidebar-order-kind")).toBe("agent");
    fireEvent.pointerDown(view.getByTestId("sidebar-agent-grip:q"), { button: 0, clientX: 4, clientY: 80 });
    fireEvent.pointerMove(window, { clientX: 4, clientY: 20 });
    expect(view.container.querySelectorAll("[data-drag-over]").length).toBe(0);
    fireEvent.pointerUp(window);
    expect(rows(view.container)).toEqual(["p", "w1", "w2", "q"]);
    expect(localStorage.getItem(ORDER_KEY)).toBeNull();
  });

  it("previews the dragged agent's kind as its row shows it", () => {
    const view = render(<Sidebar {...props} />);
    dragCharlieOverAlpha(view);
    const preview = view.getByTestId("sidebar-drag-preview").textContent || "";
    expect(preview.includes("Analyst")).toBe(true);
    expect(preview.includes("analyst")).toBe(false);
    fireEvent.pointerCancel(window);
  });
});
