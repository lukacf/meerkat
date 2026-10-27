import { render, fireEvent, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApprovalAttention, ApprovalCard } from "./approval-card";
import { normalizePendingApproval, type PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";

afterEach(cleanup);
const request = normalizePendingApproval({ pending_id: "request:1", action_id: "action:1", action: "Publish release artifacts", rationale: "Make the tested release available", actor_id: "actor", deadline_at_ms: 1, payload: { destination: "production", full: "complete request contents" } })!;
const snapshot = (status: PendingApprovalSnapshot["status"] = "ready"): PendingApprovalSnapshot => ({ scopeKey: "authority", status, requests: [request], decisions: {}, readOnly: false });
describe("ApprovalCard", () => {
  it("shows the requested approver separately from actor and describes notification IDs as references", () => {
    const routed = normalizePendingApproval({ ...request.raw, requested_approver: "human:reviewer",
      approval_recipient: "review-queue", approval_channel: "notification",
      approval_route_id: "route:1", approval_delivery_id: "delivery:1" })!;
    const view = render(<ApprovalCard request={routed} resourceStatus="ready" onDecide={() => {}} />);
    expect(view.getByText("Actor").nextElementSibling?.textContent).toBe("actor");
    expect(view.getByText("Requested approver").nextElementSibling?.textContent).toBe("human:reviewer");
    expect(view.getByText("Notification recipient").nextElementSibling?.textContent).toBe("review-queue");
    expect(view.getByText("Notification channel").nextElementSibling?.textContent).toBe("notification");
    expect(view.getByText("Route reference").nextElementSibling?.textContent).toBe("route:1");
    expect(view.getByText("Delivery reference").nextElementSibling?.textContent).toBe("delivery:1");
    expect(view.getByText("Notification references do not confirm receipt or a human decision.")).toBeTruthy();
    expect(view.queryByText("Delivered")).toBeNull();
    expect(view.queryByText("Approved")).toBeNull();
  });

  it("does not infer an approver or delivery when the owner omits them", () => {
    const view = render(<ApprovalCard request={request} resourceStatus="ready" onDecide={() => {}} />);
    expect(view.getByText("Requested approver").nextElementSibling?.textContent).toBe("Not specified");
    expect(view.queryByText("Route reference")).toBeNull();
    expect(view.queryByText("Delivery reference")).toBeNull();
    expect(view.queryByText("Notification references do not confirm receipt or a human decision.")).toBeNull();
  });

  it("shows readable scope, full request details and supported decisions", () => {
    const decide = vi.fn();
    const view = render(<ApprovalCard request={request} resourceStatus="ready" onDecide={decide} />);
    expect(view.getByText("Publish release artifacts")).toBeTruthy();
    expect(view.getByText("Make the tested release available")).toBeTruthy();
    expect(view.getByText("Complete request details")).toBeTruthy();
    expect(view.container.querySelector("pre")?.textContent).toContain("complete request contents");
    fireEvent.click(view.getByRole("button", { name: "Approve" }));
    expect(decide).toHaveBeenCalledWith("request:1", "approve");
    expect(view.getByText("Approval needed")).toBeTruthy();
  });
  it("disables all decisions while submitting, stale or read-only", () => {
    const decide = vi.fn();
    const view = render(<ApprovalCard request={request} resourceStatus="ready" onDecide={decide} decision={{ phase: "submitting", action: "approve" }} />);
    expect((view.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    view.rerender(<ApprovalCard request={request} resourceStatus="stale" onDecide={decide} />);
    expect((view.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    view.rerender(<ApprovalCard request={request} resourceStatus="ready" onDecide={decide} readOnly />);
    expect((view.getByRole("button", { name: "Approve" }) as HTMLButtonElement).disabled).toBe(true);
    expect(decide).not.toHaveBeenCalled();
  });
  it("labels a known pre-dispatch refusal as unavailable with neutral state", () => {
    const decide = vi.fn();
    const view = render(<ApprovalCard request={request} resourceStatus="ready" onDecide={decide} readOnly
      decision={{ phase: "unavailable", action: "approve", error: "Approval decisions are unavailable with current access" }} />);
    expect(view.getByRole("status").textContent).toBe("Decision unavailable");
    expect(view.getByTestId("gating-pending:request:1").dataset.state).toBe("unavailable");
    expect(view.getByText("Read-only access")).toBeTruthy();
    expect(view.queryByText("Decision unconfirmed")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "Approve" }));
    expect(decide).not.toHaveBeenCalled();
  });
  it("keeps a lost decision acknowledgement unconfirmed even after access becomes read-only", () => {
    const view = render(<ApprovalCard request={request} resourceStatus="ready" onDecide={() => {}} readOnly
      decision={{ phase: "failed", action: "approve", error: "Approval decisions are unavailable with current access" }} />);
    expect(view.getByRole("status").textContent).toBe("Decision unconfirmed");
    expect(view.getByTestId("gating-pending:request:1").dataset.state).toBe("failed");
    expect(view.queryByText("Decision unavailable")).toBeNull();
  });
  it("keeps decision context visible and exact opaque scope available in the disclosure", () => {
    const scopedRequest = { ...request, origin: { identity: "router:main" }, riskTier: "r3" };
    const view = render(<ApprovalCard request={scopedRequest} resourceStatus="ready" onDecide={() => {}} />);
    const details = view.container.querySelector("details")!;
    expect(details.hasAttribute("open")).toBe(false);
    expect(details.textContent).toContain("request:1");
    expect(details.textContent).toContain("action:1");
    expect(view.getByText("Request").closest("details")).toBe(details);
    expect(view.getByText("Action scope").closest("details")).toBe(details);
    expect(details.querySelector("pre")?.textContent).toBe(JSON.stringify(request.raw, null, 2));
    expect(view.getByText("router:main").closest("details")).toBeNull();
    expect(view.getByText("r3").closest("details")).toBeNull();
    expect(view.container.querySelector("time")?.closest("details")).toBeNull();
    for (const action of ["approve", "reject", "escalate"]) {
      expect(view.getByTestId(`gating-action:request:1:${action}`).dataset.action).toBe(action);
    }
  });
  it("cannot treat a local deadline as expired and shows authoritative expiry only", () => {
    const view = render(<ApprovalCard request={request} resourceStatus="ready" onDecide={() => {}} />);
    expect(view.getByText("Approval needed")).toBeTruthy();
    view.rerender(<ApprovalCard request={{ ...request, status: "expired" }} resourceStatus="ready" onDecide={() => {}} />);
    expect(view.getByText("Expired")).toBeTruthy();
    expect(view.queryByRole("button", { name: "Approve" })).toBeNull();
  });
  it("attention has unavailable states instead of a false zero count and opens the request in the inbox", () => {
    const onOpen = vi.fn();
    const view = render(<ApprovalAttention snapshot={{ ...snapshot("unavailable"), requests: [] }} onOpen={onOpen} />);
    expect(view.getByText("Approvals unavailable")).toBeTruthy();
    expect(view.queryByText("0 pending")).toBeNull();
    view.rerender(<ApprovalAttention snapshot={snapshot()} onOpen={onOpen} />);
    expect(view.queryByText("Publish release artifacts")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "Needs you, 1 pending approval" }));
    expect(onOpen).toHaveBeenCalledWith("request:1");
  });
  it.each(["forbidden", "unsupported"] as const)("omits global attention when approval access is %s", (status) => {
    const view = render(<ApprovalAttention snapshot={snapshot(status)} onOpen={vi.fn()} />);
    expect(view.queryByTestId("approval-attention")).toBeNull();
  });
  it("retains global attention for an authorized stale request and unconfirmed decision", () => {
    const view = render(<ApprovalAttention snapshot={{ ...snapshot("stale"), decisions: { "request:1": { phase: "failed", action: "approve", error: "Network response lost" } } }} onOpen={vi.fn()} />);
    expect(view.getByTestId("approval-attention")).toBeTruthy();
    expect(view.getByText("Approvals may be out of date")).toBeTruthy();
    expect(view.queryByText("Publish release artifacts")).toBeNull();
    expect(view.getAllByRole("button")).toHaveLength(1);
  });
  it("keeps a large inbox to one navigation row and opens all requests", () => {
    const onOpen = vi.fn();
    const requests = Array.from({ length: 25 }, (_, index) => ({ ...request, pendingId: `request:${index}` }));
    const view = render(<ApprovalAttention snapshot={{ ...snapshot(), requests }} onOpen={onOpen} />);
    expect(view.getAllByRole("button")).toHaveLength(1);
    expect(view.queryByText("Publish release artifacts")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "Needs you, 25 pending approvals" }));
    expect(onOpen).toHaveBeenCalledWith(undefined);
  });
  it("counts only unresolved pending requests and announces an empty ready inbox", () => {
    const view = render(<ApprovalAttention snapshot={{ ...snapshot(), decisions: { "request:1": { phase: "settled", action: "approve" } } }} onOpen={vi.fn()} />);
    expect(view.getByRole("button", { name: "Needs you, 0 pending approvals" })).toBeTruthy();
    expect(view.getByRole("status").textContent).toBe("0 pending approvals");
  });
});
