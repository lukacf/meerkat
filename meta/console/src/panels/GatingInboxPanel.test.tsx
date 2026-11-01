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
    fireEvent.click(view.getByRole("tab", { name: "Policies", exact: true }));
    expect(view.getByRole("tab", { name: "Policies", exact: true }).getAttribute("aria-selected")).toBe("true");
    expect(view.getByRole("status").textContent).toBe("Policy details are not available in this console.");
    for (const invented of ["scope: *", "active", "paused", "Auto on low risk", "High rejection rate"]) {
      expect(view.queryByText(invented, { exact: true })).toBeNull();
    }
    expect(view.queryByText(/No .*policies/)).toBeNull();
    expect(view.queryByTitle("deploy-release", { exact: true })).toBeNull();
    expect(view.queryByTitle("delete-database", { exact: true })).toBeNull();
    expect(view.queryByTitle("grant-access", { exact: true })).toBeNull();
  });
  it("preserves pending selection and audit access after viewing unavailable policies", () => {
    const audit = [
      { audit_id: "auto-approval", action_id: "automatic-check", event_type: "evaluated", outcome: "allowed_with_audit", risk_tier: "r2" },
      { audit_id: "manual-approval", action_id: "manual-review", event_type: "approval_decided", outcome: "allowed", risk_tier: "r3", detail: { decision: "approve" } },
    ];
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={vi.fn()} resource={snapshot("ready")} selectedPendingId="p1" />);
    fireEvent.click(view.getByTestId("gating-tab:policies"));
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    expect(view.getByTitle("automatic-check", { exact: true })).toBeTruthy();
    expect(view.getByTitle("manual-review", { exact: true })).toBeTruthy();
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    expect(view.getByTitle("automatic-check", { exact: true })).toBeTruthy();
    expect(view.queryByTitle("manual-review", { exact: true })).toBeNull();
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
    { risk_tier: "r0", outcome: "allowed", outcomeWords: "Allowed", tone: "neutral", riskWords: "Minimal (R0)" },
    { risk_tier: "r1", outcome: "allowed", outcomeWords: "Allowed", tone: "neutral", riskWords: "Low (R1)" },
    { risk_tier: "r2", outcome: "allowed_with_audit", outcomeWords: "Allowed with audit", tone: "warn", riskWords: "Elevated (R2)" },
  ])("shows the recorded $risk_tier automatic allowance with its risk in words", ({ risk_tier, outcome, outcomeWords, tone, riskWords }) => {
    const view = render(<GatingInboxPanel pending={[]} audit={[{ ...evaluated, risk_tier, outcome }]} onDecide={vi.fn()} />);
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    // The raw action id is the title's tooltip; the title reads the action.
    const row = view.getByTitle("send-message", { exact: true }).closest<HTMLElement>(".gitem")!;
    expect(row.querySelector(".cc-row__title")?.textContent).toBe("Send message");
    expect(row.getAttribute("data-risk")).toBe(risk_tier);
    expect(row.querySelector(".gitem__risk")?.getAttribute("data-tone")).toBe(tone);
    expect(within(row).getByText(`Risk: ${riskWords}`)).toBeTruthy();
    expect(within(row).getByText("Actor: Household agent")).toBeTruthy();
    expect(within(row).getByText(`Outcome: ${outcomeWords}`)).toBeTruthy();
    expect(within(row).getByText("Decision: Not recorded")).toBeTruthy();
    expect(within(row).getByText("Allowed automatically")).toBeTruthy();
    expect(view.queryByText(outcome, { exact: true })).toBeNull();
    expect(view.getByText("Approval records do not confirm execution.")).toBeTruthy();
    expect(view.container.querySelector('[data-risk="low"]')).toBeNull();
  });

  it.each([
    { event_type: "approval_decided", decision: "approve", outcome: "allowed", decisionWords: "Approve", outcomeWords: "Allowed", verdict: "Approved" },
    { event_type: "rejection_decided", decision: "reject", outcome: "safe_draft", decisionWords: "Reject", outcomeWords: "Kept as a draft", verdict: "Rejected" },
    { event_type: "escalation_decided", decision: "escalate", outcome: "pending_approval", decisionWords: "Escalate", outcomeWords: "Waiting for approval", verdict: "Escalated" },
  ])("shows the R3 human $decision decision and its recorded outcome", ({ event_type, decision, outcome, decisionWords, outcomeWords, verdict }) => {
    const audit = [{ ...evaluated, audit_id: "human-decision", pending_id: "pending-1", event_type, risk_tier: "r3", outcome, timestamp_ms: Date.now() - 2 * 60 * 60 * 1000, detail: { approver_id: "duty-approver", decision } }];
    const onDecide = vi.fn();
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={onDecide} />);
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    const row = view.getByTitle("send-message", { exact: true }).closest<HTMLElement>(".gitem")!;
    expect(row.getAttribute("data-risk")).toBe("r3");
    expect(row.querySelector(".gitem__risk")?.getAttribute("data-tone")).toBe("crit");
    expect(within(row).getByText("Risk: High (R3)")).toBeTruthy();
    expect(within(row).getByText("Actor: Household agent")).toBeTruthy();
    expect(within(row).getByText("Approver: Duty approver")).toBeTruthy();
    // The verdict chip carries the decision; the facts do not repeat it.
    expect(within(row).queryByText(`Decision: ${decisionWords}`) === null).toBe(true);
    expect(row.textContent?.includes("Decision:")).toBe(false);
    expect(within(row).getByText(`Outcome: ${outcomeWords}`)).toBeTruthy();
    expect(within(row).getByText(verdict)).toBeTruthy();
    expect(within(row).getByText("2h ago").tagName).toBe("TIME");
    // The ID column is gone: the IDs sit in the row's technical details.
    expect(view.queryByText("human-decision", { exact: true })).toBeNull();
    expect(within(row).queryByRole("button")).toBeNull();
    expect(row.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(row);
    expect(onDecide).not.toHaveBeenCalled();
    expect(row.getAttribute("aria-expanded")).toBe("true");
    expect(view.getByText("Audit record ID")).toBeTruthy();
    expect(view.getByTitle("human-decision").classList.contains("cc-tech-id")).toBe(true);
    expect(view.getByTitle("pending-1").classList.contains("cc-tech-id")).toBe(true);
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
    expect(within(row).getByText(`Risk: ${label}`)).toBeTruthy();
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
    expect(view.getByTitle("send-message", { exact: true })).toBeTruthy();
    for (const action of ["automation-failed", "blocked-evaluation", "invented-decision"]) {
      expect(view.queryByTitle(action, { exact: true })).toBeNull();
    }
    expect(within(view.getByTitle("send-message", { exact: true }).closest<HTMLElement>(".gitem")!).queryByRole("button")).toBeNull();
    expect(onDecide).not.toHaveBeenCalled();
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    for (const action of ["automation-failed", "blocked-evaluation", "invented-decision"]) {
      expect(view.getByTitle(action, { exact: true })).toBeTruthy();
    }
  });
});

describe("GatingInboxPanel frame and requests without the shared resource", () => {
  it("names the counts in the header and refreshes from a header button", () => {
    const onRefresh = vi.fn();
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={snapshot("ready")} onRefresh={onRefresh} />);
    const head = view.container.querySelector(".gating__head");
    expect(head?.textContent?.includes("1 pending · 0 allowed automatically")).toBe(true);
    expect(view.getByRole("heading", { name: "Approvals" }).tagName).toBe("H2");
    const refresh = view.getByRole("button", { name: "Refresh approvals", exact: true });
    expect(head?.contains(refresh)).toBe(true);
    fireEvent.click(refresh);
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(view.getByRole("tab", { name: "Pending 1" }).getAttribute("aria-selected")).toBe("true");
  });

  it("does not show a pending count it does not know", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("loading"), requests: [] }} />);
    expect(view.container.querySelector(".gating__head")?.textContent?.includes("pending")).toBe(false);
    expect(view.getByTestId("gating-tab:pending").textContent).toBe("Pending");
  });

  it("decides a request from its row without opening the row", () => {
    const pendingEntry = { pending_id: "pending-7", action_id: "deploy-release", actor_id: "release-manager", risk_tier: "r3", waited_ms: 18 * 60 * 1000, payload: { region: "eu-west" } };
    const onDecide = vi.fn();
    const view = render(<GatingInboxPanel pending={[pendingEntry]} audit={[]} onDecide={onDecide} />);
    const row = view.getByTestId("gating-pending:pending-7");
    expect(row.classList.contains("gitem")).toBe(true);
    expect(row.querySelector(".cc-row__title")?.textContent).toBe("Deploy release");
    expect(within(row).getByText("Risk: High (R3)")).toBeTruthy();
    expect(within(row).getByText("Actor: Release manager")).toBeTruthy();
    expect(within(row).getByText("Waiting 18 min")).toBeTruthy();
    fireEvent.click(view.getByTestId("gating-action:pending-7:approve"));
    expect(onDecide).toHaveBeenCalledWith("pending-7", "approve");
    expect(row.querySelector(".cc-row__hit")?.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(view.getByTestId("gating-action:pending-7:reject"));
    fireEvent.click(view.getByTestId("gating-action:pending-7:escalate"));
    expect(onDecide.mock.calls.map((call) => call[1])).toEqual(["approve", "reject", "escalate"]);
    view.rerender(<GatingInboxPanel pending={[pendingEntry]} audit={[]} onDecide={onDecide} readOnly />);
    expect(view.queryByTestId("gating-action:pending-7:approve")).toBeNull();
    view.rerender(<GatingInboxPanel pending={[]} audit={[]} onDecide={onDecide} />);
    expect(view.getByText("No pending items.")).toBeTruthy();
  });
});

describe("Approvals accessibility", () => {
  // A control inside another control: a button or link in a button, or in
  // an element that claims the button role.
  const NESTED_CONTROLS = [
    "button button", "button a", "button [role='button']",
    "[role='button'] button", "[role='button'] a", "[role='button'] [role='button']",
  ].join(", ");

  it("keeps a request's decisions beside the row's button, never inside it", () => {
    const pendingEntry = { pending_id: "pending-7", action_id: "deploy-release", actor_id: "release-manager", risk_tier: "r3", waited_ms: 18 * 60 * 1000 };
    const onDecide = vi.fn();
    const view = render(<GatingInboxPanel pending={[pendingEntry]} audit={[]} onDecide={onDecide} />);
    const row = view.getByTestId("gating-pending:pending-7");
    expect(view.container.querySelectorAll(NESTED_CONTROLS).length).toBe(0);
    expect(row.getAttribute("role") === null).toBe(true);
    const open = within(row).getByRole("button", { name: "Deploy release" });
    expect(open.getAttribute("aria-expanded")).toBe("false");
    for (const decision of ["approve", "reject", "escalate"]) {
      expect(open.contains(view.getByTestId(`gating-action:pending-7:${decision}`))).toBe(false);
    }
    fireEvent.click(open);
    expect(open.getAttribute("aria-expanded")).toBe("true");
    expect(view.getByText("Request ID")).toBeTruthy();
    expect(onDecide).not.toHaveBeenCalled();
  });

  it("ties each tab to the panel it shows", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={snapshot("ready")} />);
    for (const tab of ["pending", "auto", "audit", "policies"]) {
      const control = view.getByTestId(`gating-tab:${tab}`);
      fireEvent.click(control);
      expect(control.getAttribute("aria-selected")).toBe("true");
      const panel = document.getElementById(control.getAttribute("aria-controls") ?? "");
      expect(panel?.getAttribute("role")).toBe("tabpanel");
      expect(panel?.getAttribute("aria-labelledby")).toBe(control.getAttribute("id"));
      expect(view.getAllByRole("tabpanel").length).toBe(1);
    }
    expect(view.getByRole("tabpanel").textContent?.includes("Policy details are not available in this console.")).toBe(true);
  });
});

describe("refresh and freshness follow the shown tab", () => {
  it("refreshes and dates the audit list on Audit and Auto, the queue on Pending, and nothing on Policies", () => {
    const now = Date.now();
    const onRefresh = vi.fn();
    const onRefreshAudit = vi.fn();
    const audit = [{ audit_id: "audit-1", action_id: "tag-release", event_type: "approval_decided", outcome: "allowed", risk_tier: "r2", detail: { decision: "approve" } }];
    const view = render(
      <GatingInboxPanel
        pending={[]}
        audit={audit}
        auditUpdatedAtMs={now - 5 * 60_000}
        onDecide={vi.fn()}
        resource={{ ...snapshot("ready"), updatedAtMs: now }}
        onRefresh={onRefresh}
        onRefreshAudit={onRefreshAudit}
      />,
    );
    expect(view.getByText("Updated just now").tagName).toBe("TIME");
    fireEvent.click(view.getByRole("button", { name: "Refresh approvals" }));
    expect(onRefresh.mock.calls.length).toBe(1);

    fireEvent.click(view.getByTestId("gating-tab:audit"));
    expect(view.getByText("Updated 5m ago").tagName).toBe("TIME");
    expect(view.queryByText("Updated just now") === null).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "Refresh approvals" }));
    expect(onRefreshAudit.mock.calls.length).toBe(1);
    expect(onRefresh.mock.calls.length).toBe(1);

    fireEvent.click(view.getByTestId("gating-tab:auto"));
    fireEvent.click(view.getByRole("button", { name: "Refresh approvals" }));
    expect(onRefreshAudit.mock.calls.length).toBe(2);

    fireEvent.click(view.getByTestId("gating-tab:policies"));
    expect(view.queryByRole("button", { name: "Refresh approvals" }) === null).toBe(true);
    expect(view.queryByText(/^Updated /) === null).toBe(true);
  });
});

describe("GatingInboxPanel audit refresh errors", () => {
  it("says a refresh of the recorded approvals failed and keeps the loaded records", () => {
    const audit = [{ audit_id: "audit-1", action_id: "tag-release", event_type: "evaluated", outcome: "allowed", risk_tier: "r1" }];
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={vi.fn()} onRefreshAudit={vi.fn()} auditError="audit store unavailable" />);
    expect(view.queryByTestId("gating-audit-error") === null).toBe(true);
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    const error = view.getByTestId("gating-audit-error");
    expect(error.getAttribute("role")).toBe("alert");
    expect(error.textContent).toBe("Couldn't refresh the approval records (audit store unavailable). Showing the ones loaded before.");
    expect(view.container.querySelectorAll(".gitem").length).toBe(1);
  });
});

describe("GatingInboxPanel host-provided attention", () => {
  const item = { sourceId: "host", sourceLabel: "Host", id: "a", title: "Confirm a setting", href: "/host/items/a", details: [] };
  it("lists the approvals first and host items after them", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={snapshot("ready")}
      hostAttention={{ status: "ready", items: [item], failedSources: ["Other"] }} />);
    const host = view.getByTestId("host-attention-list");
    expect(within(host).getByRole("link", { name: "Open: Confirm a setting" }).getAttribute("href")).toBe("/host/items/a");
    // A source that could not be read is named in a kit notice, by its label.
    const failed = within(host).getByText("Could not read: Other").closest(".cc-notice");
    expect(failed?.getAttribute("role")).toBe("status");
    expect(failed?.getAttribute("data-tone")).toBe("warn");
    expect(view.getByText("Publish reviewed changes")).toBeTruthy();
    expect(host.compareDocumentPosition(view.getByText("Publish reviewed changes")) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
    // The host's cards sit in the same card column as the approvals.
    expect(host.classList.contains("gating-inbox__cards")).toBe(true);
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
    expect(view.queryByRole("button", { name: "Approve" }) === null).toBe(true);
  });
  it("keeps the disabled buttons for a read-only session when no source gives a usable link", () => {
    const sources = [
      { id: "remote", label: "Remote", url: "/host/attention", decide_href: "https://evil.example/{decision_id}" },
      { id: "plain", label: "Plain", url: "/host/attention" },
    ];
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} readOnly hostDecideSources={sources} resource={snapshot("ready")} />);
    expect(view.queryByRole("link", { name: /^Decide in / }) === null).toBe(true);
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
      expect(view.queryByRole("link", { name: /^Decide in / }) === null).toBe(true);
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
    expect(view.queryByTestId("host-attention-list") === null).toBe(true);
  });
  it("says approvals are not available on this connection instead of counting them", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} readOnly
      resource={{ ...snapshot("unsupported"), requests: [], readOnly: true }}
      hostAttention={{ status: "ready", items: [item], failedSources: [] }} />);
    const head = view.container.querySelector(".gating__head")?.textContent || "";
    expect(head.includes("Not available on this connection")).toBe(true);
    expect(head.includes("pending")).toBe(false);
    expect(head.includes("allowed automatically")).toBe(false);
    expect(view.getByTestId("gating-tab:pending").textContent).toBe("Pending");
  });
  it("keeps an empty approval list to one line while host items show", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("ready"), requests: [] }}
      hostAttention={{ status: "ready", items: [item], failedSources: [] }} />);
    const empty = view.getByText("No pending approvals.").closest(".cc-panel-empty");
    expect(empty?.classList.contains("cc-panel-empty--inline")).toBe(true);
    view.rerender(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("unsupported"), requests: [] }}
      hostAttention={{ status: "ready", items: [item], failedSources: [] }} />);
    const unsupported = view.getByText("Approvals are not available for this connection").closest(".cc-panel-empty");
    expect(unsupported?.classList.contains("cc-panel-empty--inline")).toBe(true);
    view.rerender(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ ...snapshot("ready"), requests: [] }}
      hostAttention={{ status: "ready", items: [], failedSources: [] }} />);
    expect(view.getByText("No pending approvals.").closest(".cc-panel-empty")?.classList.contains("cc-panel-empty--inline")).toBe(false);
  });
  it("shows Pending again when Needs you opens an inbox that shows another tab", () => {
    const props = { pending: [], audit: [], onDecide: vi.fn(), resource: { ...snapshot("ready"), requests: [] },
      hostAttention: { status: "ready" as const, items: [item], failedSources: [] } };
    const view = render(<GatingInboxPanel {...props} showPending={1} />);
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    expect(view.queryByTestId("host-attention-list") === null).toBe(true);
    // Another render of the same open keeps the tab the viewer chose.
    view.rerender(<GatingInboxPanel {...props} showPending={1} />);
    expect(view.getByTestId("gating-tab:audit").getAttribute("aria-selected")).toBe("true");
    view.rerender(<GatingInboxPanel {...props} showPending={2} />);
    expect(view.getByTestId("gating-tab:pending").getAttribute("aria-selected")).toBe("true");
    expect(view.getByTestId("host-attention:host:a")).toBeTruthy();
  });
  it("lists host items on Pending only, after requests without the shared resource too", () => {
    const pendingEntry = { pending_id: "pending-7", action_id: "deploy-release", actor_id: "release-manager", risk_tier: "r3" };
    const view = render(<GatingInboxPanel pending={[pendingEntry]} audit={[]} onDecide={vi.fn()}
      hostAttention={{ status: "ready", items: [item], failedSources: [] }} />);
    const host = view.getByTestId("host-attention-list");
    expect(host.compareDocumentPosition(view.getByTestId("gating-pending:pending-7")) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    expect(view.queryByTestId("host-attention-list") === null).toBe(true);
    fireEvent.click(view.getByTestId("gating-tab:pending"));
    expect(view.getByTestId("host-attention:host:a")).toBeTruthy();
  });
});

describe("Approvals at a glance (v2)", () => {
  const now = Date.now();
  const DAY = 24 * 3_600_000;
  const requestRow = (id: string, action: string, deadlineIn: number, extra: Record<string, unknown> = {}) => normalizePendingApproval({
    pending_id: id, action_id: `action:${id}`, action, actor_id: "billing", risk_tier: "r3",
    created_at_ms: now - 10 * 60_000, deadline_at_ms: now + deadlineIn, ...extra,
  })!;
  const ready = (requests: ReturnType<typeof requestRow>[]): PendingApprovalSnapshot => ({ scopeKey: "scope", status: "ready", requests, decisions: {}, readOnly: false, updatedAtMs: now });
  const seededAudit = [
    { audit_id: "a1", event_type: "pending_created", action_id: "refund", risk_tier: "r3", actor_id: "billing", outcome: "pending_approval", timestamp_ms: now - 50 * 60_000, detail: { action: "Refund 1,240 EUR on order 77310" } },
    { audit_id: "a2", event_type: "approval_decided", action_id: "refund", risk_tier: "r3", actor_id: "billing", outcome: "allowed", timestamp_ms: now - 40 * 60_000, detail: { decision: "approve", approver_id: "ana.lima@example.com" } },
    { audit_id: "a3", event_type: "pending_created", action_id: "waive", risk_tier: "r3", actor_id: "dispatch", outcome: "pending_approval", timestamp_ms: now - 30 * 60_000, detail: { action: "Waive shipping fees this week" } },
    { audit_id: "a4", event_type: "rejection_decided", action_id: "waive", risk_tier: "r3", actor_id: "dispatch", outcome: "safe_draft", timestamp_ms: now - 20 * 60_000, detail: { decision: "reject", approver_id: "jonas.berg@example.com" } },
    { audit_id: "a5", event_type: "evaluated", action_id: "credit", risk_tier: "r2", actor_id: "billing", outcome: "allowed_with_audit", timestamp_ms: now - 10 * 60_000, detail: { action: "Apply a 10 EUR goodwill credit" } },
    { audit_id: "a6", event_type: "evaluated", action_id: "label", risk_tier: "r0", actor_id: "returns", outcome: "allowed", timestamp_ms: now - 5 * 60_000, detail: { action: "Re-send a return label" } },
    { audit_id: "a7", event_type: "timeout_fallback", action_id: "cancel", risk_tier: "r3", actor_id: "billing", outcome: "safe_draft", timestamp_ms: now - 2 * DAY, detail: { fallback: "safe_draft" } },
  ];

  it("answers what waits and how long there is, soonest deadline first", () => {
    const requests = [
      requestRow("late", "Raise the refund threshold", 2 * 3_600_000),
      requestRow("soon", "Publish the incident on the status page", 50_000),
      requestRow("mid", "Send the delay notice", 15 * 60_000),
    ];
    const view = render(<GatingInboxPanel pending={[]} audit={seededAudit} onDecide={vi.fn()} resource={ready(requests)} auditLimit={50} />);
    const cards = Array.from(view.container.querySelectorAll("[data-approval-id]"), (card) => card.getAttribute("data-approval-id"));
    expect(cards).toEqual(["soon", "mid", "late"]);
    expect(view.getByTestId("gating-glance:waiting").textContent?.includes("3")).toBe(true);
    expect(view.getByTestId("gating-glance:waiting").textContent?.includes("3 high risk")).toBe(true);
    const deadline = view.getByTestId("gating-glance:deadline");
    expect(deadline.getAttribute("data-tone")).toBe("warn");
    expect(/in \d+s/.test(deadline.textContent ?? "")).toBe(true);
    expect(deadline.textContent?.includes("Publish the incident on the status page")).toBe(true);
    expect(view.getByTestId("gating-summary").textContent?.includes("3 pending · 2 allowed automatically · next deadline in")).toBe(true);
    expect(view.getByTestId("gating-countdown:soon").getAttribute("data-tone")).toBe("warn");
  });

  it("counts today's decisions and the risk mix of the recorded window", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={seededAudit} onDecide={vi.fn()} resource={ready([])} />);
    const decided = view.getByTestId("gating-glance:decided").textContent ?? "";
    expect(decided.includes("1 approved · 1 rejected")).toBe(true);
    expect(decided.includes("timed out")).toBe(false);
    const risk = view.getByTestId("gating-risk-mix");
    expect(risk.querySelector('[role="img"]')?.getAttribute("aria-label")).toBe("Risk of the recorded actions: Minimal 1, Low 0, Elevated 1, High 3");
    expect(Array.from(risk.querySelectorAll("[data-tier]"), (segment) => segment.getAttribute("data-tier"))).toEqual(["r0", "r2", "r3"]);
    expect(view.getByText("No pending approvals.")).toBeTruthy();
  });

  it("titles a decision by the request it decided, newest first, by day", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={seededAudit} onDecide={vi.fn()} resource={ready([])} />);
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    const decision = view.getByTestId("gating-audit:a2");
    expect(decision.querySelector(".cc-row__title")?.textContent).toBe("Refund 1,240 EUR on order 77310");
    expect(view.getByTestId("gating-audit:a4").querySelector(".cc-row__title")?.textContent).toBe("Waive shipping fees this week");
    expect(view.container.textContent?.includes("Gate action")).toBe(false);
    const order = Array.from(view.container.querySelectorAll<HTMLElement>("[data-testid^='gating-audit:']"), (row) => row.getAttribute("data-testid"));
    expect(order.slice(0, 6)).toEqual(["gating-audit:a6", "gating-audit:a5", "gating-audit:a4", "gating-audit:a3", "gating-audit:a2", "gating-audit:a1"]);
    const days = Array.from(view.container.querySelectorAll(".gating-day__label"), (label) => label.textContent);
    expect(days[0]?.startsWith("Today")).toBe(true);
    expect(days.length).toBe(2);
    // The decisions strip: one mark per decision of the last day, in its shape.
    const strip = view.getByTestId("gating-decisions-strip");
    expect(strip.querySelector("[data-testid='gating-decision:a2']")?.getAttribute("data-shape")).toBe("dot");
    expect(strip.querySelector("[data-testid='gating-decision:a4']")?.getAttribute("data-shape")).toBe("diamond");
    expect(strip.querySelector("[data-testid='gating-decision:a7']") === null).toBe(true);
  });

  it("says the counts cover the newest records when the read is full", () => {
    const audit = Array.from({ length: 50 }, (_, index) => ({ audit_id: `e${index}`, event_type: "evaluated", action_id: `act${index}`, risk_tier: "r1", outcome: "allowed", timestamp_ms: now - index * 60_000 }));
    const view = render(<GatingInboxPanel pending={[]} audit={audit} onDecide={vi.fn()} resource={ready([])} auditLimit={50} />);
    expect(view.getByTestId("gating-summary").textContent?.includes("50 allowed automatically in the last 50 records")).toBe(true);
    fireEvent.click(view.getByTestId("gating-tab:auto"));
    expect(view.getByTestId("gating-audit-window").textContent).toBe("The newest 50 records. 50 of them were allowed automatically.");
  });

  it("hides the glance strip where approvals do not apply", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={{ scopeKey: "scope", status: "unsupported", requests: [], decisions: {}, readOnly: true }} />);
    expect(view.queryByTestId("gating-glance") === null).toBe(true);
  });
});
