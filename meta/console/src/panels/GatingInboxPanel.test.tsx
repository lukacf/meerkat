import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GatingInboxPanel } from "./GatingInboxPanel";
import { normalizePendingApproval, type PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";

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
