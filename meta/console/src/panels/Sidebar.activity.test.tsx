import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Sidebar } from "./Sidebar";
import type { ConsoleAgent } from "../types";

afterEach(() => { cleanup(); localStorage.clear(); });
const agent = (name: string, phase?: ConsoleAgent["response_phase"]): ConsoleAgent => ({
  agent_id: name, member_id: name, identity: name, label: name, kind: "module_agent",
  group: "Reviewers", ...(phase === undefined ? {} : { response_phase: phase }),
});
const agents = [agent("Working reviewer", "generating"), agent("Quiet reviewer", null), agent("Unknown reviewer")];
const props = { agents, selectedMemberId: "", recentActivity: [], collapsed: false,
  visibleControls: [], onSelect: vi.fn(), onOpenControl: vi.fn(),
  grouping: { group_by: ["group"], sections: [{ name: "Reviewers", collapsed: true }] },
};
const rows = (container: HTMLElement) => [...container.querySelectorAll('[data-testid^="sidebar-agent:"]')]
  .map(node => node.getAttribute("data-testid")?.split(":")[1]);
// The Show filter lives in the roster view options popover (A5): open it
// unless it is open; its buttons keep their names and aria-pressed.
const show = (view: ReturnType<typeof render>, name: "All" | "Working" | "Quiet" | "Unknown") => {
  if (!view.queryByTestId("sidebar-view-options")) fireEvent.click(view.getByTestId("sidebar-view-menu"));
  fireEvent.click(view.getByRole("button", { name, exact: true }));
};

describe("Roster activity filters", () => {
  it("filters typed activity, reveals matching collapsed rows, and combines with search", () => {
    const view = render(<Sidebar {...props} />);
    expect(rows(view.container)).toEqual([]);
    show(view, "Working");
    expect(rows(view.container)).toEqual(["Working reviewer"]);
    show(view, "Quiet");
    expect(rows(view.container)).toEqual(["Quiet reviewer"]);
    show(view, "Unknown");
    expect(rows(view.container)).toEqual(["Unknown reviewer"]);
    fireEvent.change(view.getByPlaceholderText("Search agents"), { target: { value: "Working" } });
    expect(rows(view.container)).toEqual([]);
    expect(view.getByText("No matching agents.")).toBeTruthy();
    show(view, "All");
    expect(rows(view.container)).toEqual(["Working reviewer"]);
    fireEvent.change(view.getByPlaceholderText("Search agents"), { target: { value: "" } });
    expect(rows(view.container)).toEqual([]);
  });

  it("updates from owner phases without inferring activity from a running state or prose", () => {
    const view = render(<Sidebar {...props} />);
    show(view, "Working");
    view.rerender(<Sidebar {...props} agents={[
      { ...agent("Working reviewer", null), state: "running" },
      agent("Quiet reviewer", "tool-executing"), agent("Unknown reviewer", "waiting"),
    ]} />);
    expect(rows(view.container)).toEqual(["Quiet reviewer", "Unknown reviewer"]);
    show(view, "Quiet");
    expect(rows(view.container)).toEqual(["Working reviewer"]);
    expect(view.getByRole("button", { name: "Quiet", exact: true }).getAttribute("aria-pressed")).toBe("true");
  });
  it("keeps pinned order and exposes unknown only for missing phase", () => {
    const view = render(<Sidebar {...props} agents={[
      agent("Z pinned", "waiting"), agent("A reviewer", "generating"), agent("No phase"),
    ]} pinnedAgentIds={new Set(["Z pinned"])} />);
    show(view, "Working");
    expect(rows(view.container)).toEqual(["Z pinned", "A reviewer"]);
    show(view, "Unknown");
    expect(rows(view.container)).toEqual(["No phase"]);
  });

});

describe("Roster order under activity", () => {
  const open = { ...props, grouping: { group_by: ["group"] }, preferenceScope: "activity-order" };
  const names = ["Quiet reviewer", "Working reviewer", "Unknown reviewer"];

  it("keeps the manual order through phase changes and re-sorts by last activity only when chosen", () => {
    localStorage.setItem("meta-console-sidebar-agent-order:activity-order", JSON.stringify(names));
    const view = render(<Sidebar {...open} />);
    expect(rows(view.container)).toEqual(names);

    view.rerender(<Sidebar {...open} agents={[
      agent("Working reviewer", null), agent("Quiet reviewer", "generating"), agent("Unknown reviewer", "waiting"),
    ]} />);
    expect(rows(view.container)).toEqual(names);

    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    fireEvent.click(view.getByTestId("sidebar-view-sort:activity"));
    const timed = [
      { ...agent("Quiet reviewer", null), last_activity_ms: 10 },
      { ...agent("Working reviewer", "generating"), last_activity_ms: 20 },
      { ...agent("Unknown reviewer"), last_activity_ms: 30 },
    ];
    view.rerender(<Sidebar {...open} agents={timed} />);
    expect(rows(view.container)).toEqual(["Unknown reviewer", "Working reviewer", "Quiet reviewer"]);

    view.rerender(<Sidebar {...open} agents={timed} recentActivity={[
      { id: "quiet-frame", event: "text_delta", identity: "Quiet reviewer", timestampMs: 40 },
    ]} />);
    expect(rows(view.container)).toEqual(["Quiet reviewer", "Unknown reviewer", "Working reviewer"]);
  });
});

describe("approval attention availability", () => {
  it.each(["forbidden", "unsupported"] as const)("does not turn %s approval access into an operator task", (status) => {
    const view = render(<Sidebar {...props} approvals={{ scopeKey: "scope", status, requests: [], decisions: {}, readOnly: true }} />);
    expect(view.queryByTestId("approval-attention")).toBeNull();
    expect(view.getByText("Roster")).toBeTruthy();
  });
});
