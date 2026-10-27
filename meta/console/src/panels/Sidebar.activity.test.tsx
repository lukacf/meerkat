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

describe("Roster activity filters", () => {
  it("filters typed activity, reveals matching collapsed rows, and combines with search", () => {
    const view = render(<Sidebar {...props} />);
    expect(rows(view.container)).toEqual([]);
    fireEvent.click(view.getByRole("button", { name: "Working", exact: true }));
    expect(rows(view.container)).toEqual(["Working reviewer"]);
    fireEvent.click(view.getByRole("button", { name: "Quiet", exact: true }));
    expect(rows(view.container)).toEqual(["Quiet reviewer"]);
    fireEvent.click(view.getByRole("button", { name: "Unknown", exact: true }));
    expect(rows(view.container)).toEqual(["Unknown reviewer"]);
    fireEvent.change(view.getByPlaceholderText("Search roster..."), { target: { value: "Working" } });
    expect(rows(view.container)).toEqual([]);
    expect(view.getByText("No matching agents.")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "All", exact: true }));
    expect(rows(view.container)).toEqual(["Working reviewer"]);
    fireEvent.change(view.getByPlaceholderText("Search roster..."), { target: { value: "" } });
    expect(rows(view.container)).toEqual([]);
  });

  it("updates from owner phases without inferring activity from a running state or prose", () => {
    const view = render(<Sidebar {...props} />);
    fireEvent.click(view.getByRole("button", { name: "Working", exact: true }));
    view.rerender(<Sidebar {...props} agents={[
      { ...agent("Working reviewer", null), state: "running" },
      agent("Quiet reviewer", "tool-executing"), agent("Unknown reviewer", "waiting"),
    ]} />);
    expect(rows(view.container)).toEqual(["Quiet reviewer", "Unknown reviewer"]);
    fireEvent.click(view.getByRole("button", { name: "Quiet", exact: true }));
    expect(rows(view.container)).toEqual(["Working reviewer"]);
    expect(view.getByRole("button", { name: "Quiet", exact: true }).getAttribute("aria-pressed")).toBe("true");
  });
  it("keeps pinned order and exposes unknown only for missing phase", () => {
    const view = render(<Sidebar {...props} agents={[
      agent("Z pinned", "waiting"), agent("A reviewer", "generating"), agent("No phase"),
    ]} pinnedAgentIds={new Set(["Z pinned"])} />);
    fireEvent.click(view.getByRole("button", { name: "Working", exact: true }));
    expect(rows(view.container)).toEqual(["Z pinned", "A reviewer"]);
    fireEvent.click(view.getByRole("button", { name: "Unknown", exact: true }));
    expect(rows(view.container)).toEqual(["No phase"]);
  });

});

describe("approval attention availability", () => {
  it.each(["forbidden", "unsupported"] as const)("does not turn %s approval access into an operator task", (status) => {
    const view = render(<Sidebar {...props} approvals={{ scopeKey: "scope", status, requests: [], decisions: {}, readOnly: true }} />);
    expect(view.queryByTestId("approval-attention")).toBeNull();
    expect(view.getByText("Roster")).toBeTruthy();
  });
});
