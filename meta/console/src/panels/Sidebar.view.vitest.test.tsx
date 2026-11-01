import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import { Sidebar, sidebarAgentsWithLineage } from "./Sidebar";
import type { ConsoleAgent, ConsoleFrame } from "../types";

// DOM assertions compare booleans and strings, never elements.

afterEach(() => { cleanup(); localStorage.clear(); });

const agent = (id: string, label: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  agent_id: id, member_id: id, identity: `agent:${id}`, label, kind: "mob_agent", role: "analyst", group: "Team", ...extra,
});
const progress = (health: string, run_state: string) => ({
  health, run_state, in_flight_work: 1, last_progress_at_ms: 1_000, last_progress_event: "became_busy",
});
const team = [agent("a", "Alpha", { response_phase: "generating" }), agent("b", "Bravo", { response_phase: null }), agent("c", "Charlie")];
const props = {
  agents: team, selectedMemberId: "", recentActivity: [] as ConsoleFrame[], collapsed: false, visibleControls: [],
  onSelect: vi.fn(), onOpenControl: vi.fn(), grouping: { group_by: ["group"] },
  storageNamespace: "configuration-v", preferenceScope: "principal-v",
};
const VIEW_KEY = "meta-console-sidebar-view:principal-v";
const rows = (container: HTMLElement) => [...container.querySelectorAll('[data-testid^="sidebar-agent:"]')]
  .map((node) => node.getAttribute("data-testid")?.slice("sidebar-agent:".length));
const focused = () => document.activeElement?.getAttribute("data-testid") ?? null;
const describedBy = (element: HTMLElement) => {
  const id = element.getAttribute("aria-describedby");
  return id ? document.getElementById(id)?.textContent ?? null : null;
};

describe("Roster view menu (A5)", () => {
  it("keeps the view options behind a quiet trigger that stays in the tab order", () => {
    const view = render(<Sidebar {...props} />);
    expect(view.container.querySelectorAll("select").length).toBe(0);
    expect(view.queryByRole("group", { name: "Agent activity" }) === null).toBe(true);
    const trigger = view.getByTestId("sidebar-view-menu");
    expect(trigger.getAttribute("aria-label")).toBe("Roster view options");
    expect(trigger.getAttribute("aria-haspopup")).toBe("dialog");
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    // Revealed by opacity on hover or focus (cc-reveal), never taken out of
    // the tab order or the accessibility tree.
    expect(trigger.classList.contains("cc-reveal")).toBe(true);
    expect(trigger.tabIndex).toBe(0);
    expect(trigger.hasAttribute("disabled") || trigger.getAttribute("aria-hidden") === "true").toBe(false);
    // Its reveal host is the mast and the search row together.
    const host = trigger.closest(".cc-reveal-host");
    expect(host !== null && host.contains(view.getByTestId("sidebar-search"))).toBe(true);
    trigger.focus();
    expect(focused()).toBe("sidebar-view-menu");
  });

  it("opens the Roster view on the pressed option, stays open on a change and returns focus on Escape", () => {
    const view = render(<Sidebar {...props} />);
    const trigger = view.getByTestId("sidebar-view-menu");
    fireEvent.click(trigger);
    const dialog = view.getByRole("dialog", { name: "Roster view" });
    expect(dialog.getAttribute("aria-modal")).toBe("false");
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(trigger.getAttribute("aria-controls")).toBe(dialog.id);
    expect(within(dialog).getAllByRole("group").map((group) => group.getAttribute("aria-label"))).toEqual(["Agent activity", "Group by", "Sort by"]);
    expect(within(dialog).getAllByRole("button").map((button) => button.textContent)).toEqual([
      "All", "Working", "Quiet", "Unknown", "Category", "None", "Manual", "Name", "Last activity", "Reset view",
    ]);
    for (const id of ["sidebar-view-activity:all", "sidebar-view-group:category", "sidebar-view-sort:manual"]) {
      expect(view.getByTestId(id).getAttribute("aria-pressed")).toBe("true");
    }
    expect(focused()).toBe("sidebar-view-activity:all");
    expect(within(dialog).queryByText("Manual order: drag agents to reorder") !== null).toBe(true);

    fireEvent.click(view.getByTestId("sidebar-view-sort:name"));
    expect(view.queryByRole("dialog", { name: "Roster view" }) !== null).toBe(true);
    expect(view.getByTestId("sidebar-view-sort:name").getAttribute("aria-pressed")).toBe("true");
    expect(within(dialog).queryByText("Switch to Manual to drag agents") !== null).toBe(true);

    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(view.queryByRole("dialog") === null).toBe(true);
    expect(focused()).toBe("sidebar-view-menu");
  });

  it("shows the summary, the dot and a reset exactly while the view is not the default", () => {
    const view = render(<Sidebar {...props} />);
    const trigger = view.getByTestId("sidebar-view-menu");
    const summary = () => view.queryByTestId("sidebar-view-summary")?.textContent ?? null;
    const count = () => view.getByTestId("sidebar-agent-count").textContent;
    expect(summary()).toBe(null);
    expect(count()).toBe("3 agents");
    expect(view.queryByTestId("sidebar-view-reset") === null).toBe(true);
    expect(trigger.classList.contains("has-badge")).toBe(false);
    expect(trigger.hasAttribute("data-customized")).toBe(false);

    fireEvent.click(trigger);
    fireEvent.click(view.getByTestId("sidebar-view-activity:working"));
    expect(summary()).toBe(" · Working");
    // The Show filter hides rows: the count says how many of the roster show.
    expect(count()).toBe("1 of 3 agents");
    // Customised: always shown, with the accent dot.
    expect(trigger.classList.contains("has-badge")).toBe(true);
    expect(trigger.classList.contains("cc-reveal")).toBe(false);
    expect(trigger.getAttribute("data-customized")).toBe("true");

    fireEvent.click(view.getByTestId("sidebar-view-sort:name"));
    fireEvent.click(view.getByTestId("sidebar-view-group:none"));
    expect(summary()).toBe(" · Working · By name · Ungrouped");
    expect(view.container.querySelector(".sidebar__mast-sub")?.getAttribute("title")).toBe("1 of 3 agents · Working · By name · Ungrouped");

    // The summary's words open the view options.
    fireEvent.keyDown(view.getByRole("dialog", { name: "Roster view" }), { key: "Escape" });
    fireEvent.click(view.getByTestId("sidebar-view-summary"));
    expect(view.queryByRole("dialog", { name: "Roster view" }) !== null).toBe(true);
    fireEvent.keyDown(view.getByRole("dialog", { name: "Roster view" }), { key: "Escape" });

    fireEvent.click(view.getByTestId("sidebar-view-reset"));
    expect(summary()).toBe(null);
    expect(view.queryByTestId("sidebar-view-reset") === null).toBe(true);
    expect(trigger.classList.contains("has-badge")).toBe(false);
    expect(trigger.classList.contains("cc-reveal")).toBe(true);
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toEqual({ v: 1, group_by: "category", sort_by: "manual" });
    expect(rows(view.container)).toEqual(["a", "b", "c"]);
    expect(count()).toBe("3 agents");

    // Grouping and sorting hide nothing: the count stays the roster's.
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    fireEvent.click(view.getByTestId("sidebar-view-sort:activity"));
    fireEvent.click(view.getByTestId("sidebar-view-group:none"));
    expect(summary()).toBe(" · By last activity · Ungrouped");
    expect(count()).toBe("3 agents");
    expect(view.container.querySelector(".sidebar__mast-sub")?.getAttribute("title")).toBe("3 agents · By last activity · Ungrouped");
  });

  it("names Status unknown and By last activity in the summary", () => {
    const view = render(<Sidebar {...props} />);
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    fireEvent.click(view.getByTestId("sidebar-view-activity:unknown"));
    fireEvent.click(view.getByTestId("sidebar-view-sort:activity"));
    expect(view.getByTestId("sidebar-view-summary").textContent).toBe(" · Status unknown · By last activity");
    expect(rows(view.container)).toEqual(["c"]);
  });

  it("Reset view restores All, Category and Manual and is disabled while the view is the default", () => {
    const view = render(<Sidebar {...props} />);
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    const reset = view.getByTestId("sidebar-view-options-reset");
    expect(reset.hasAttribute("disabled")).toBe(true);
    fireEvent.click(view.getByTestId("sidebar-view-activity:quiet"));
    fireEvent.click(view.getByTestId("sidebar-view-group:none"));
    expect(reset.hasAttribute("disabled")).toBe(false);
    expect(rows(view.container)).toEqual(["b"]);
    fireEvent.click(reset);
    for (const id of ["sidebar-view-activity:all", "sidebar-view-group:category", "sidebar-view-sort:manual"]) {
      expect(view.getByTestId(id).getAttribute("aria-pressed")).toBe("true");
    }
    expect(reset.hasAttribute("disabled")).toBe(true);
    expect(rows(view.container)).toEqual(["a", "b", "c"]);
  });

  it("'/' focuses the search unless a text field has focus or a panel took the key", () => {
    const view = render(<><input data-testid="other-field" /><Sidebar {...props} /></>);
    expect(view.getByTestId("sidebar-search").getAttribute("placeholder")).toBe("Search agents");
    fireEvent.keyDown(document.body, { key: "/" });
    expect(focused()).toBe("sidebar-search");

    const other = view.getByTestId("other-field");
    other.focus();
    fireEvent.keyDown(other, { key: "/" });
    expect(focused()).toBe("other-field");
    other.blur();

    // A panel's own search takes "/" by preventing the default.
    const claim = (event: KeyboardEvent) => event.preventDefault();
    document.body.addEventListener("keydown", claim);
    fireEvent.keyDown(document.body, { key: "/" });
    document.body.removeEventListener("keydown", claim);
    expect(focused() === "sidebar-search").toBe(false);
  });

  it("closes the roster view when an agent is opened, by pointer or keyboard", () => {
    const onSelect = vi.fn();
    const view = render(<Sidebar {...props} onSelect={onSelect} />);
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    expect(view.queryByRole("dialog", { name: "Roster view" }) !== null).toBe(true);
    fireEvent.click(view.getByTestId("sidebar-agent:a"));
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(view.queryByRole("dialog") === null).toBe(true);
    expect(view.getByTestId("sidebar-view-menu").getAttribute("aria-expanded")).toBe("false");

    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    fireEvent.keyDown(view.getByTestId("sidebar-agent:b"), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(2);
    expect(view.queryByRole("dialog") === null).toBe(true);
  });

  it("offers Clear filters when nothing matches", () => {
    const view = render(<Sidebar {...props} />);
    fireEvent.change(view.getByTestId("sidebar-search"), { target: { value: "nobody" } });
    expect(view.getByRole("status").textContent?.startsWith("No matching agents.")).toBe(true);
    fireEvent.click(view.getByTestId("sidebar-clear-filters"));
    expect((view.getByTestId("sidebar-search") as HTMLInputElement).value).toBe("");
    expect(rows(view.container)).toEqual(["a", "b", "c"]);
  });
});

describe("Workbench nav (B14)", () => {
  const approvals: PendingApprovalSnapshot = {
    scopeKey: "scope",
    status: "ready",
    requests: ["p1", "p2"].map((pendingId) => ({ pendingId, actionId: pendingId, action: "send", status: "pending", actions: [], raw: {} })),
    decisions: {},
    readOnly: false,
  };
  const withAttention = {
    ...props,
    agents: [...team, agent("s", "Stuck", { progress: progress("wedged", "run_open") })],
    visibleControls: ["topology", "gating", "logs", "health", "memory", "workgraph"] as const,
    approvals,
    activeControl: "topology" as const,
    navAttention: { workgraph: { count: 2, tone: "warn" as const, description: "2 blocked work items" } },
  };

  it("lists views with exact names, describes their attention badges and marks the focused pane's view", () => {
    const view = render(<Sidebar {...withAttention} visibleControls={[...withAttention.visibleControls]} />);
    const nav = view.getByRole("navigation", { name: "Workbench" });
    expect(within(nav).getAllByRole("button").map((button) => button.getAttribute("data-testid"))).toEqual([
      "sidebar-workbench-toggle", "nav:topology", "nav:gating", "nav:logs", "nav:health", "nav:memory", "nav:workgraph",
    ]);
    // A badge never changes a button's name; it is its description.
    for (const [name, kind] of [["Approvals", "gating"], ["WorkGraph", "workgraph"], ["Memory", "memory"], ["Health", "health"]]) {
      expect(view.getByRole("button", { name, exact: true }).getAttribute("data-testid")).toBe(`nav:${kind}`);
    }
    expect(view.getByTestId("nav-badge:gating").textContent).toBe("2");
    expect(view.getByTestId("nav-badge:gating").getAttribute("data-tone")).toBe("warn");
    expect(describedBy(view.getByTestId("nav:gating"))).toBe("2 pending approvals");
    expect(view.getByTestId("nav-badge:health").getAttribute("data-tone")).toBe("crit");
    expect(describedBy(view.getByTestId("nav:health"))).toBe("1 stalled");
    expect(view.getByTestId("nav-badge:workgraph").textContent).toBe("2");
    expect(describedBy(view.getByTestId("nav:workgraph"))).toBe("2 blocked work items");
    // Nothing counted, nothing shown: no zero.
    expect(view.queryByTestId("nav-badge:memory") === null).toBe(true);
    expect(view.queryByTestId("nav-badge:topology") === null).toBe(true);
    expect(view.getByTestId("nav:memory").hasAttribute("aria-describedby")).toBe(false);

    expect(view.getByTestId("nav:topology").getAttribute("aria-current")).toBe("true");
    expect(view.getByTestId("nav:topology").classList.contains("is-active")).toBe(true);
    expect(view.getByTestId("nav:gating").hasAttribute("aria-current")).toBe(false);
    fireEvent.click(view.getByTestId("nav:gating"));
    expect(withAttention.onOpenControl.mock.calls.at(-1)?.[0]).toBe("gating");
  });

  it("folds the Workbench away, keeps the choice per user and sums the attention on its head", () => {
    const first = render(<Sidebar {...withAttention} visibleControls={[...withAttention.visibleControls]} />);
    const toggle = first.getByTestId("sidebar-workbench-toggle");
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(first.getByTestId("sidebar-section-collapse:workbench").getAttribute("aria-hidden")).toBe("true");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(first.queryByTestId("nav:topology") === null).toBe(true);
    expect(toggle.querySelector(".sidebar__navbadge")?.textContent).toBe("5");
    expect(toggle.querySelector(".sidebar__navbadge")?.getAttribute("data-tone")).toBe("crit");
    expect(describedBy(toggle)).toBe("5 things need attention");
    first.unmount();

    const second = render(<Sidebar {...withAttention} visibleControls={[...withAttention.visibleControls]} />);
    expect(second.getByTestId("sidebar-workbench-toggle").getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(second.getByTestId("sidebar-workbench-toggle"));
    expect(second.queryByTestId("nav:topology") !== null).toBe(true);
  });

  it("counts Logs errors since the last visit, and a shown Logs view is a visit", () => {
    const failure: ConsoleFrame = { id: "f", event: "interaction_failed", identity: "agent:a", timestampMs: Date.now() - 1_000, data: { error: "boom" } };
    const logs = { ...props, visibleControls: ["logs" as const] };
    const view = render(<Sidebar {...logs} recentActivity={[failure]} />);
    expect(view.getByTestId("nav-badge:logs").textContent).toBe("1");
    expect(view.getByTestId("nav-badge:logs").getAttribute("data-tone")).toBe("crit");
    view.rerender(<Sidebar {...logs} recentActivity={[failure]} openControls={["logs"]} />);
    expect(view.queryByTestId("nav-badge:logs") === null).toBe(true);
    view.rerender(<Sidebar {...logs} recentActivity={[failure]} openControls={[]} />);
    expect(view.queryByTestId("nav-badge:logs") === null).toBe(true);
    const later: ConsoleFrame = { ...failure, id: "g", timestampMs: Date.now() + 60_000 };
    view.rerender(<Sidebar {...logs} recentActivity={[later, failure]} openControls={[]} />);
    expect(view.getByTestId("nav-badge:logs").textContent).toBe("1");
  });
});

describe("Roster rows (B14)", () => {
  it("marks a working agent live and says what it does, never Unknown", () => {
    const busy = agent("busy", "Busy", { role: "router", response_phase: "generating", progress: progress("unknown", "run_open") });
    const stuck = agent("stuck", "Stuck", { role: "router", response_phase: "generating", progress: progress("wedged", "run_open") });
    const idle = agent("idle", "Idle", { role: "router", response_phase: null, last_activity_ms: Date.now() - 5 * 60_000 });
    const view = render(<Sidebar {...props} agents={[busy, stuck, idle]} />);
    expect(view.getByTestId("sidebar-agent-mark:busy").getAttribute("data-tone")).toBe("live");
    expect(view.getByTestId("sidebar-agent-detail:busy").textContent).toBe("Router · writing a reply");
    expect(view.getByTestId("sidebar-agent:busy").getAttribute("data-health")).toBe("working");
    expect(view.getByTestId("sidebar-agent-mark:stuck").getAttribute("data-tone")).toBe("crit");
    expect(view.getByTestId("sidebar-agent-detail:stuck").textContent).toBe("Router · stalled");
    expect(view.getByTestId("sidebar-agent-mark:idle").getAttribute("data-tone")).toBe("ok");
    expect(view.getByTestId("sidebar-agent-detail:idle").textContent).toBe("Router · idle");
    expect(view.getByTestId("sidebar-agent:idle").querySelector("time")?.textContent).toBe("5m ago");
    expect((view.getByTestId("sidebar-root").textContent || "").includes("nknown")).toBe(false);
    // Working shows exactly the rows whose mark pulses.
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    fireEvent.click(view.getByTestId("sidebar-view-activity:working"));
    expect(rows(view.container)).toEqual(["busy"]);
  });

  it("lays rows out at 44 px, and at 32 px on one line in compact density", () => {
    const view = render(<Sidebar {...props} />);
    const heightOf = (id: string) => (view.getByTestId(`sidebar-agent:${id}`).closest(".sidebar__virtual-row") as HTMLElement | null)?.style.height;
    expect(heightOf("a")).toBe("44px");
    expect(view.getByTestId("sidebar-root").getAttribute("data-density")).toBe("comfortable");
    view.rerender(<Sidebar {...props} density="compact" />);
    expect(heightOf("a")).toBe("32px");
    expect(view.getByTestId("sidebar-root").getAttribute("data-density")).toBe("compact");
  });

  it("shows each section's count beside its label and a collapse chevron", () => {
    const view = render(<Sidebar {...props} />);
    const head = view.getByTestId("sidebar-section-toggle:Team");
    expect(head.textContent).toBe("Team3");
    expect(view.getByTestId("sidebar-section-collapse:Team").getAttribute("aria-hidden")).toBe("true");
    fireEvent.click(head);
    expect(head.getAttribute("aria-expanded")).toBe("false");
    expect(rows(view.container)).toEqual([]);
  });

  it("lists a helper under the agent the topology read says spawned it", () => {
    const parent = agent("ship", "Shipping");
    const helper = agent("watch", "Carrier Watch", { identity: "watch", group: "Specialists" });
    const query = { revision: 1, nodes: [{ endpoint: { identity: "watch" }, labels: { spawned_by: "agent:ship" } }], edges: [] };
    const roster = [parent, helper];
    const enriched = sidebarAgentsWithLineage(roster, query);
    expect(enriched[1]?.labels?.spawned_by).toBe("agent:ship");
    expect(sidebarAgentsWithLineage(roster, null) === roster).toBe(true);
    expect(sidebarAgentsWithLineage(enriched, query) === enriched).toBe(true);
    const view = render(<Sidebar {...props} agents={enriched} />);
    expect(view.getByTestId("sidebar-agent:watch").getAttribute("data-child-of-host")).toBe("true");
    expect(rows(view.container)).toEqual(["ship", "watch"]);
  });
});
