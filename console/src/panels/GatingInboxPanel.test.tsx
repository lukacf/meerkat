import React from "react";
import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GatingInboxPanel } from "./GatingInboxPanel";
import { createPendingApprovalResource, normalizePendingApproval, type PendingApprovalResource, type PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import { CONSOLE_RPC_METHODS } from "../lib/contract";

afterEach(cleanup);
const request = normalizePendingApproval({ pending_id: "p1", action_id: "a1", action: "Deploy release", rationale: "Publish reviewed changes" })!;
const snapshot = (status: PendingApprovalSnapshot["status"]): PendingApprovalSnapshot => ({ scopeKey: "scope", status, requests: [request], decisions: {}, readOnly: false });
describe("GatingInboxPanel shared resource", () => {
  it("does not mistake unavailable data for an empty inbox", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("unavailable"), requests: [] }} />);
    expect(view.getByText("Approvals unavailable")).toBeTruthy();
    expect(view.queryByText("No pending approvals.")).toBeNull();
    view.rerender(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("ready"), requests: [] }} />);
    expect(view.getByText("No pending approvals.")).toBeTruthy();
  });
  it("navigates attention to the exact request without a second fetch owner", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={snapshot("ready")} selectedPendingId="p1" />);
    const selected = view.container.querySelector<HTMLElement>('[data-approval-id="p1"]');
    expect(selected?.dataset.selected).toBe("true");
    expect(document.activeElement).toBe(selected);
    expect(view.getByText("Publish reviewed changes")).toBeTruthy();
  });
  it.each([
    { name: "an empty audit", audit: [] },
    { name: "recorded decisions", audit: [
      { audit_id: "audit-approve", action_id: "deploy-release", decision: "approve" },
      { audit_id: "audit-reject", action_id: "delete-database", decision: "reject" },
      { audit_id: "audit-escalate", action_id: "grant-access", decision: "escalate" },
    ] },
  ])("does not manufacture policy counts or rules from $name", ({ audit }) => {
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={vi.fn()} resource={snapshot("ready")} />);
    expect(view.container.querySelector(".gating__head")?.textContent).not.toMatch(/\d+ polic/);
    fireEvent.click(view.getByRole("button", { name: "Policies", exact: true }));
    expect(view.getByRole("status").textContent).toBe("Policy details are not available in this console.");
    for (const invented of ["scope: *", "active", "paused", "Auto on low risk", "High rejection rate"]) {
      expect(view.queryByText(invented, { exact: true })).toBeNull();
    }
    expect(view.queryByText(/No .*policies/)).toBeNull();
    expect(view.queryByText("deploy-release", { exact: true })).toBeNull();
    expect(view.queryByText("delete-database", { exact: true })).toBeNull();
    expect(view.queryByText("grant-access", { exact: true })).toBeNull();
  });
  it("preserves pending selection and audit access after viewing unavailable policies", () => {
    const audit = [
      { audit_id: "auto-approval", action_id: "automatic-check", event_type: "evaluated", outcome: "allowed_with_audit", risk_tier: "r2" },
      { audit_id: "manual-approval", action_id: "manual-review", event_type: "approval_decided", outcome: "allowed", risk_tier: "r3", detail: { decision: "approve" } },
    ];
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={vi.fn()} resource={snapshot("ready")} selectedPendingId="p1" />);
    fireEvent.click(view.getByTestId("gating-tab:policies"));
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    expect(view.getByText("automatic-check", { exact: true })).toBeTruthy();
    expect(view.getByText("manual-review", { exact: true })).toBeTruthy();
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    expect(view.getByText("automatic-check", { exact: true })).toBeTruthy();
    expect(view.queryByText("manual-review", { exact: true })).toBeNull();
    fireEvent.click(view.getByTestId("gating-tab:pending"));
    expect(view.getByText("Publish reviewed changes")).toBeTruthy();
    expect(document.activeElement).toBe(view.container.querySelector('[data-approval-id="p1"]'));
  });
  it("explains an unsupported approval capability without claiming access was denied", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("unsupported"), requests: [], readOnly: true }} />);
    expect(view.getByText("Approvals are not available for this connection")).toBeTruthy();
    expect(view.queryByText("Approval access denied")).toBeNull();
    expect(view.queryByText("No pending approvals.")).toBeNull();
  });
});

describe("GatingInboxPanel recorded audit", () => {
  const evaluated = {
    audit_id: "evaluation-1",
    action_id: "send-message",
    event_type: "evaluated",
    actor_id: "household-agent",
    risk_tier: "r2",
    outcome: "allowed_with_audit",
    detail: { policy: "consequence_mode_allow_with_audit_v0_1", action: "Send message" },
  };

  it.each([
    { risk_tier: "r0", outcome: "allowed" },
    { risk_tier: "r1", outcome: "allowed" },
    { risk_tier: "r2", outcome: "allowed_with_audit" },
  ])("shows the recorded $risk_tier automatic allowance without inventing a risk scale", ({ risk_tier, outcome }) => {
    const view = render(<GatingInboxPanel pending={[]} audit={[{ ...evaluated, risk_tier, outcome }]} onDecide={vi.fn()} />);
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    const row = view.getByText("send-message", { exact: true }).closest<HTMLElement>(".gitem")!;
    expect(row.getAttribute("data-risk")).toBe(risk_tier);
    expect(within(row).getByText(`Tier: ${risk_tier.toUpperCase()}`)).toBeTruthy();
    expect(within(row).getByText("Actor: household-agent")).toBeTruthy();
    expect(within(row).getByText(`Outcome: ${outcome}`)).toBeTruthy();
    expect(within(row).getByText("Decision: Not recorded")).toBeTruthy();
    expect(view.getByText("Approval records do not confirm execution.")).toBeTruthy();
    expect(view.container.querySelector('[data-risk="low"]')).toBeNull();
  });

  it.each([
    { event_type: "approval_decided", decision: "approve", outcome: "allowed" },
    { event_type: "rejection_decided", decision: "reject", outcome: "safe_draft" },
    { event_type: "escalation_decided", decision: "escalate", outcome: "pending_approval" },
  ])("shows the R3 human $decision decision and its recorded outcome", ({ event_type, decision, outcome }) => {
    const audit = [{ ...evaluated, audit_id: "human-decision", pending_id: "pending-1", event_type, risk_tier: "r3", outcome, detail: { approver_id: "luka", decision } }];
    const onDecide = vi.fn();
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={onDecide} />);
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    const row = view.getByText("send-message", { exact: true }).closest<HTMLElement>(".gitem")!;
    expect(row.getAttribute("data-risk")).toBe("r3");
    expect(within(row).getByText("Tier: R3")).toBeTruthy();
    expect(within(row).getByText("Actor: household-agent")).toBeTruthy();
    expect(within(row).getByText("Approver: luka")).toBeTruthy();
    expect(within(row).getByText(`Decision: ${decision}`)).toBeTruthy();
    expect(within(row).getByText(`Outcome: ${outcome}`)).toBeTruthy();
    expect(within(row).queryByRole("button")).toBeNull();
    fireEvent.click(row);
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    expect(view.getByText("No auto items.")).toBeTruthy();
  });

  it.each([
    { risk_tier: "future-tier", label: "Unknown (future-tier)" },
    { risk_tier: "high", label: "Unknown (high)" },
    { risk_tier: undefined, label: "Unknown" },
  ])("keeps an unrecognized or missing tier explicit: $label", ({ risk_tier, label }) => {
    const view = render(<GatingInboxPanel pending={[]} audit={[{ audit_id: "unknown-tier", risk_tier }]} onDecide={vi.fn()} />);
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    const row = view.container.querySelector<HTMLElement>(".gitem")!;
    expect(row.getAttribute("data-risk")).toBe("unknown");
    expect(within(row).getByText(`Tier: ${label}`)).toBeTruthy();
    expect(within(row).getByText("Actor: Not recorded")).toBeTruthy();
    expect(within(row).getByText("Outcome: Not recorded")).toBeTruthy();
    expect(within(row).getByText("Decision: Not recorded")).toBeTruthy();
  });

  it("classifies Auto only from an exact evaluated allowance, not event substrings or invented decisions", () => {
    const audit = [
      evaluated,
      { ...evaluated, audit_id: "unrelated", action_id: "automation-failed", event_type: "automation_failed" },
      { ...evaluated, audit_id: "blocked", action_id: "blocked-evaluation", outcome: "pending_approval" },
      { audit_id: "invented", action_id: "invented-decision", decision: "auto_approve" },
    ];
    const onDecide = vi.fn();
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={onDecide} />);
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    expect(view.getByText("send-message", { exact: true })).toBeTruthy();
    for (const action of ["automation-failed", "blocked-evaluation", "invented-decision"]) {
      expect(view.queryByText(action, { exact: true })).toBeNull();
    }
    expect(within(view.getByText("send-message", { exact: true }).closest<HTMLElement>(".gitem")!).queryByRole("button")).toBeNull();
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    for (const action of ["automation-failed", "blocked-evaluation", "invented-decision"]) {
      expect(view.getByText(action, { exact: true })).toBeTruthy();
    }
  });
});

describe("GatingInboxPanel host-provided attention", () => {
  const item = { sourceId: "host", sourceLabel: "Host", id: "a", title: "Confirm a setting", href: "/host/items/a", details: [] };
  it("lists the approvals first and host items after them", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={snapshot("ready")}
      hostAttention={{ status: "ready", items: [item], failedSources: ["Other"] }} />);
    const host = view.getByTestId("host-attention-list");
    expect(within(host).getByRole("link", { name: "Open: Confirm a setting" }).getAttribute("href")).toBe("/host/items/a");
    expect(within(host).getByText("Could not read: Other").getAttribute("role")).toBe("status");
    expect(view.getByText("Publish reviewed changes")).toBeTruthy();
    expect(host.compareDocumentPosition(view.getByText("Publish reviewed changes")) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
  });
  it.each(["forbidden", "unsupported"] as const)("shows the %s note and the host items together", (status) => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot(status), requests: [] }}
      hostAttention={{ status: "ready", items: [item], failedSources: [] }} />);
    expect(view.getByText(status === "forbidden" ? "Approval access denied" : "Approvals are not available for this connection")).toBeTruthy();
    expect(view.getByTestId("host-attention:host:a")).toBeTruthy();
  });
  it("fills each source's decide link for a read-only session and drops a cross-origin one", () => {
    const encoded = normalizePendingApproval({ ...request.raw, pending_id: "p 1/2" })!;
    const sources = [
      { id: "host", label: "Host", url: "/host/attention", decide_href: "/host/decide/{decision_id}" },
      { id: "remote", label: "Remote", url: "/host/attention", decide_href: "https://evil.example/{decision_id}" },
      { id: "plain", label: "Plain", url: "/host/attention" },
    ];
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} readOnly hostDecideSources={sources}
      resource={{ ...snapshot("ready"), requests: [encoded] }} />);
    const links = view.getAllByRole("link", { name: /^Decide in / });
    expect(links.map((link) => link.getAttribute("href"))).toEqual(["/host/decide/p%201%2F2"]);
    expect(view.queryByRole("button", { name: "Approve" })).toBeNull();
  });
  it("keeps the disabled buttons for a read-only session when no source gives a usable link", () => {
    const sources = [
      { id: "remote", label: "Remote", url: "/host/attention", decide_href: "https://evil.example/{decision_id}" },
      { id: "plain", label: "Plain", url: "/host/attention" },
    ];
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} readOnly hostDecideSources={sources} resource={snapshot("ready")} />);
    expect(view.queryByRole("link", { name: /^Decide in / })).toBeNull();
    expect((view.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    expect(view.getByText("Read-only access")).toBeTruthy();
  });
  it("links the host's decide page once a decision is refused for the session's access", async () => {
    const resource = createPendingApprovalResource({
      scopeKey: "scope",
      load: async () => ({ pending: [{ pending_id: "p1", action_id: "a1", action: "Deploy release" }] }),
      decide: async () => { throw Object.assign(new Error("decide unavailable"), { kind: "console-capability-unavailable", method: CONSOLE_RPC_METHODS.gatingDecide, availableMethods: [CONSOLE_RPC_METHODS.gatingPending] }); },
    });
    function Inbox({ owner }: { owner: PendingApprovalResource }) {
      const current = React.useSyncExternalStore(owner.subscribe, owner.getSnapshot);
      return <GatingInboxPanel pending={[]} audit={[]} resource={current} onDecide={(id, action) => { void owner.decide(id, action); }}
        hostDecideSources={[{ id: "host", label: "Host", url: "/host/attention", decide_href: "/host/decide/{decision_id}" }]} />;
    }
    const view = render(<Inbox owner={resource} />);
    try {
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
      expect(view.queryByRole("link", { name: /^Decide in / })).toBeNull();
      await act(async () => { fireEvent.click(view.getByRole("button", { name: "Approve" })); await new Promise((resolve) => setTimeout(resolve, 20)); });
      expect(resource.getSnapshot().readOnly).toBe(true);
      expect(view.getByRole("link", { name: "Decide in Host: Deploy release" }).getAttribute("href")).toBe("/host/decide/p1");
    } finally {
      resource.dispose();
    }
  });
  it("shows nothing extra without host items", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={snapshot("ready")}
      hostAttention={{ status: "ready", items: [], failedSources: [] }} />);
    expect(view.queryByTestId("host-attention-list")).toBeNull();
  });
});
