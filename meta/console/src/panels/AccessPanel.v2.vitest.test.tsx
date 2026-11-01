import React from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AccessPanel, type AccessPreviewResult } from "./AccessPanel";
import type { ConsoleAccessConfig } from "../types";

// The seeded preview's access: enforced, an anonymous console that manages
// access through the catch-all rule, two groups and five rules.
const ACTIONS = ["agent.view", "agent.send", "agent.memory.read", "mob.memory.read", "gating.view", "gating.decide", "workgraph.manage", "access.admin", "mobpack.author", "mobpack.deploy"];
const CONFIG: ConsoleAccessConfig = {
  enabled: true,
  admins: ["ops-lead@example.com"],
  groups: {
    contractors: { description: "Seasonal support contractors", members: ["temp1@example.com", "temp2@example.com"] },
    "support-leads": { members: ["ana.lima@example.com"] },
  },
  rules: [
    { id: "console-operators", effect: "allow", actions: ["*"], description: "Anyone who can open this console may operate the team" },
    { id: "no-host-deploys", effect: "deny", actions: ["mobpack.deploy"], description: "Mobpack deploys run only from the release pipeline" },
    { id: "leads-decide-approvals", effect: "allow", groups: ["support-leads"], actions: ["gating.view", "gating.decide", "workgraph.manage"], description: "Support leads decide approvals" },
    { id: "contractors-domain-desks", effect: "allow", groups: ["contractors"], actions: ["agent.view", "agent.send", "agent.memory.read"], agents: ["billing"], description: "Contractors talk to the domain desks only" },
    { id: "contractors-no-memory", effect: "deny", groups: ["contractors"], actions: ["agent.memory.read", "mob.memory.read"], description: "Contractors never read team memory" },
  ],
};

function props(overrides: Partial<React.ComponentProps<typeof AccessPanel>> = {}) {
  return {
    status: { available: true, enabled: true, revision: 5, owner_instance: "owner", conditional_mutations: "checked_v1", subject: null, can_administer: true, is_admin: false, actions: ACTIONS },
    config: CONFIG,
    agents: [{ identity: "billing", label: "Billing" }, { identity: "shipping", label: "Shipping" }],
    onRefresh: vi.fn(), onSetEnabled: vi.fn(), onSaveAdmins: vi.fn(), onUpsertRule: vi.fn(),
    onDeleteRule: vi.fn(), onSaveGroup: vi.fn(), onDeleteGroup: vi.fn(),
    onPreview: vi.fn(async (subject: string, action: string): Promise<AccessPreviewResult> => ({ allowed: action !== "mobpack.deploy", reason: subject ? null : "no subject" })),
    ...overrides,
  } as React.ComponentProps<typeof AccessPanel>;
}

describe("Console access at a glance", () => {
  it("says it is enforcing and who is signed in, and how they manage access", () => {
    render(<AccessPanel {...props()} />);
    expect(screen.getByTestId("access-state").textContent).toBe("Enforcing");
    expect(screen.getByTestId("access-state").getAttribute("data-tone")).toBe("ok");
    const summary = screen.getByTestId("access-summary").textContent ?? "";
    expect(summary.includes("5 rules · 2 groups · Not signed in · you manage access through a rule")).toBe(true);
    expect(screen.getByRole("heading", { name: "Console access", exact: true }).tagName).toBe("H2");
  });
});

describe("the access matrix", () => {
  it("marks each row and family from the rules, in words for assistive tech", () => {
    render(<AccessPanel {...props()} />);
    const matrix = screen.getByTestId("access-matrix");
    expect(within(matrix).getByText("Computed from the rules; click a cell to check it.")).toBeTruthy();
    const everyoneAdmin = screen.getByTestId("access-matrix-cell:everyone:admin");
    expect(everyoneAdmin.getAttribute("data-state")).toBe("some");
    expect(everyoneAdmin.getAttribute("aria-label")?.startsWith("Everyone, Admin: partly allowed: 2 of 3 permissions")).toBe(true);
    expect(screen.getByTestId("access-matrix-cell:admins:memory").getAttribute("data-state")).toBe("all");
    // Both memory reads are denied to contractors, whatever else allows them.
    expect(screen.getByTestId("access-matrix-cell:group:contractors:memory").getAttribute("data-state")).toBe("denied");
    expect(screen.getByTestId("access-matrix-cell:group:contractors:memory").getAttribute("aria-label")).toBe("contractors group, Memory: denied by a rule");
  });

  it("is one tab stop the arrow keys move through", () => {
    render(<AccessPanel {...props()} />);
    const first = screen.getByTestId("access-matrix-cell:admins:view");
    expect(first.getAttribute("tabindex")).toBe("0");
    expect(screen.getByTestId("access-matrix-cell:admins:message").getAttribute("tabindex")).toBe("-1");
    first.focus();
    fireEvent.keyDown(first, { key: "ArrowRight" });
    expect(document.activeElement?.getAttribute("data-testid")).toBe("access-matrix-cell:admins:message");
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement?.getAttribute("data-testid")).toBe("access-matrix-cell:group:contractors:message");
    fireEvent.keyDown(document.activeElement!, { key: "End" });
    expect(document.activeElement?.getAttribute("data-testid")).toBe("access-matrix-cell:group:contractors:admin");
  });

  it("checks a cell with the runtime as someone the row stands for, and shows the verified answers", async () => {
    const input = props();
    render(<AccessPanel {...input} />);
    await act(async () => { fireEvent.click(screen.getByTestId("access-matrix-cell:group:contractors:admin")); });
    const drawer = screen.getByTestId("access-check");
    expect(drawer.getAttribute("aria-label")).toBe("Check access: contractors group, Admin");
    expect(within(drawer).getByText("Checked as temp1@example.com, a member of contractors.")).toBeTruthy();
    expect((input.onPreview as ReturnType<typeof vi.fn>).mock.calls.map((call) => call.slice(0, 2))).toEqual([
      ["temp1@example.com", "access.admin"], ["temp1@example.com", "mobpack.author"], ["temp1@example.com", "mobpack.deploy"],
    ]);
    expect(within(drawer).getByTestId("access-check-answer:mobpack.deploy").getAttribute("data-allowed")).toBe("false");
    expect(within(drawer).getAllByText("verified").length).toBe(3);
    expect(within(drawer).getByText('Denied by "Mobpack deploys run only from the release pipeline"')).toBeTruthy();
    fireEvent.click(within(drawer).getByTestId("access-check-close"));
    expect(screen.queryByTestId("access-check") === null).toBe(true);
  });

  it("checks the everyone row as someone not signed in", async () => {
    const input = props();
    render(<AccessPanel {...input} />);
    await act(async () => { fireEvent.click(screen.getByTestId("access-matrix-cell:everyone:message")); });
    expect((input.onPreview as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual(["", "agent.send", undefined]);
    expect(screen.getByText("Checked as someone who is not signed in.")).toBeTruthy();
  });

  it("checks an agent-scoped permission on an agent the rule names", async () => {
    const input = props({ config: { ...CONFIG, rules: CONFIG.rules!.filter((rule) => rule.id !== "console-operators") } });
    render(<AccessPanel {...input} />);
    await act(async () => { fireEvent.click(screen.getByTestId("access-matrix-cell:group:contractors:message")); });
    expect((input.onPreview as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual(["temp1@example.com", "agent.send", "billing"]);
    expect((screen.getByTestId("access-check-agent") as HTMLSelectElement).value).toBe("billing");
  });
});

describe("rules, groups and the rule editor", () => {
  it("says which deny rule takes permissions from an allow rule", () => {
    render(<AccessPanel {...props()} />);
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    const coverage = screen.getByTestId("access-rule-coverage:console-operators");
    expect(coverage.textContent).toBe('Partly overridden: Deploy team packages denied by "Mobpack deploys run only from the release pipeline".');
    expect(coverage.getAttribute("data-tone")).toBe("warn");
    expect(screen.getByTestId("access-rule-coverage:no-host-deploys").textContent?.startsWith('Overrides "Anyone who can open this console may operate the team"')).toBe(true);
    expect(screen.queryByTestId("access-rule-coverage:leads-decide-approvals") === null).toBe(true);
    // A wildcard reads in words, its key in the tooltip.
    const all = within(screen.getByTestId("access-rule:console-operators")).getByText("All actions");
    expect(all.getAttribute("title")).toBe("*");
  });

  it("edits a rule that holds a pattern: the pattern is removable and saves as it is", async () => {
    const input = props({ status: { available: true, enabled: true, revision: 5, owner_instance: "owner", conditional_mutations: "checked_v1", subject: "ops-lead@example.com", can_administer: true, is_admin: true, actions: ACTIONS } });
    input.onUpsertRule = vi.fn(async () => true);
    render(<AccessPanel {...input} />);
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    fireEvent.click(screen.getByTestId("access-rule-edit:console-operators"));
    const pattern = screen.getByTestId("access-rule-action:*");
    expect(pattern.textContent).toBe("All actions");
    expect((screen.getByTestId("access-rule-save") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(pattern);
    expect(screen.queryByTestId("access-rule-action:*") === null).toBe(true);
    fireEvent.click(screen.getByTestId("access-rule-action:agent.view"));
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    expect((input.onUpsertRule as ReturnType<typeof vi.fn>).mock.calls[0][0].actions).toEqual(["agent.view"]);
  });

  it("blocks a save only while a permission the runtime does not list remains", () => {
    const stale = { ...CONFIG, rules: [{ id: "old", effect: "allow" as const, actions: ["agent.view", "custom.retired.action"] }] };
    render(<AccessPanel {...props({ config: stale, status: { available: true, enabled: true, revision: 5, owner_instance: "owner", conditional_mutations: "checked_v1", subject: "ops-lead@example.com", can_administer: true, is_admin: true, actions: ACTIONS } })} />);
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    fireEvent.click(screen.getByTestId("access-rule-edit:old"));
    expect((screen.getByTestId("access-rule-save") as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("access-rule-unknown-actions").textContent).toBe("This runtime does not list custom.retired.action. Remove it to save the rule.");
    fireEvent.click(screen.getByTestId("access-rule-action:custom.retired.action"));
    expect((screen.getByTestId("access-rule-save") as HTMLButtonElement).disabled).toBe(false);
  });

  it("lists group members as chips that inspect their access, and the rules that use the group", () => {
    render(<AccessPanel {...props()} />);
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    const contractors = screen.getByTestId("access-group:contractors");
    expect(within(contractors).getByText("temp1@example.com").classList.contains("access-member__name")).toBe(true);
    const used = screen.getByTestId("access-group-rules:contractors").textContent ?? "";
    expect(used.includes("Contractors talk to the domain desks only")).toBe(true);
    expect(used.includes("Contractors never read team memory")).toBe(true);
    fireEvent.click(within(contractors).getByRole("button", { name: "Inspect access for temp2@example.com" }));
    expect((screen.getByTestId("access-preview-subject") as HTMLInputElement).value).toBe("temp2@example.com");
    expect(screen.getByRole("option", { name: "No specific agent" })).toBeTruthy();
    expect(screen.queryByRole("option", { name: "Any agent" }) === null).toBe(true);
  });

  it("shows the approvers read only", () => {
    const withApprovers = { ...CONFIG, approvers: { "role:release": { subjects: ["carol@example.com"], groups: ["support-leads"] } } } as ConsoleAccessConfig;
    render(<AccessPanel {...props({ config: withApprovers })} />);
    const approvers = screen.getByTestId("access-approvers");
    expect(within(approvers).getByText("role:release")).toBeTruthy();
    expect(approvers.textContent?.includes("carol@example.com · support-leads group")).toBe(true);
    expect(within(approvers).queryByRole("button") === null).toBe(true);
  });
});
