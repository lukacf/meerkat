import * as React from "react";
import type { ApprovalAction, ApprovalDecisionState, PendingApproval, PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";
import { RowDetails } from "./presentation-policy";

const labels: Record<ApprovalAction, string> = { approve: "Approve", reject: "Reject", escalate: "Escalate" };
export interface ApprovalCardProps {
  request: PendingApproval;
  resourceStatus: PendingApprovalSnapshot["status"];
  decision?: ApprovalDecisionState;
  readOnly?: boolean;
  onDecide(pendingId: string, action: ApprovalAction): void | Promise<void>;
}

/** The same request and decision owner can feed an inbox and several panes. */
export function ApprovalCard({ request, resourceStatus, decision, readOnly = false, onDecide }: ApprovalCardProps) {
  const pending = request.status === "pending" && decision?.phase !== "settled";
  const submitting = decision?.phase === "submitting";
  const stale = resourceStatus !== "ready";
  const state = submitting ? "submitting" : decision?.phase === "failed" ? "failed" : request.status === "expired" ? "expired" : !pending ? "settled" : stale ? "stale" : decision?.phase === "unavailable" ? "unavailable" : "pending";
  return (
    <article className="cc-approval" data-state={state} data-testid={`gating-pending:${request.pendingId}`}>
      <header className="cc-approval__header">
        <strong>{request.action}</strong>
        <span className="cc-approval__status" role="status">{state === "pending" ? "Approval needed" : state === "submitting" ? "Submitting decision" : state === "stale" ? "Approval state may be out of date" : state === "failed" ? "Decision unconfirmed" : state === "unavailable" ? "Decision unavailable" : state === "expired" ? "Expired" : "Resolved"}</span>
      </header>
      {request.rationale ? <p>{request.rationale}</p> : null}
      <dl className="cc-approval__scope">
        {request.actorId ? <><dt>Actor</dt><dd>{request.actorId}</dd></> : null}
        <dt>Requested approver</dt><dd>{request.requestedApprover || "Not specified"}</dd>
        {request.origin ? <><dt>Origin</dt><dd>{request.origin.identity}</dd></> : null}
        {request.riskTier ? <><dt>Risk</dt><dd>{request.riskTier}</dd></> : null}
        {request.approvalRecipient ? <><dt>Notification recipient</dt><dd>{request.approvalRecipient}</dd></> : null}
        {request.approvalChannel ? <><dt>Notification channel</dt><dd>{request.approvalChannel}</dd></> : null}
        {request.deadlineAtMs !== undefined ? <><dt>Deadline</dt><dd><time dateTime={new Date(request.deadlineAtMs).toISOString()}>{new Date(request.deadlineAtMs).toLocaleString()}</time></dd></> : null}
      </dl>
      <RowDetails part={`approval-details:${request.pendingId}`} className="cc-approval__details"><summary>Complete request details</summary>
        <dl className="cc-approval__scope">
          <dt>Request</dt><dd><code>{request.pendingId}</code></dd>
          <dt>Action scope</dt><dd><code>{request.actionId}</code></dd>
          {request.approvalRouteId ? <><dt>Route reference</dt><dd><code>{request.approvalRouteId}</code></dd></> : null}
          {request.approvalDeliveryId ? <><dt>Delivery reference</dt><dd><code>{request.approvalDeliveryId}</code></dd></> : null}
        </dl>
        {request.approvalRouteId || request.approvalDeliveryId ? <p>Notification references do not confirm receipt or a human decision.</p> : null}
        <pre>{JSON.stringify(request.raw, null, 2)}</pre>
      </RowDetails>
      {decision?.error ? <p role="alert">{decision.error}</p> : null}
      {decision?.result?.next_pending_id ? <p>Escalated to <code>{decision.result.next_pending_id}</code></p> : null}
      {readOnly ? <p>Read-only access</p> : null}
      {resourceStatus === "forbidden" ? <p>Approval access denied</p> : null}
      {pending ? <div className="cc-approval__actions">{request.actions.map((action) => (
        <button key={action} type="button" disabled={readOnly || stale || submitting} data-action={action} data-testid={`gating-action:${request.pendingId}:${action}`} onClick={() => { void onDecide(request.pendingId, action); }}>{labels[action]}</button>
      ))}</div> : null}
    </article>
  );
}

export function ApprovalAttention({ snapshot, onOpen }: {
  snapshot: PendingApprovalSnapshot;
  onOpen(pendingId?: string): void;
}) {
  if (snapshot.status === "forbidden" || snapshot.status === "unsupported") return null;
  const requests = snapshot.requests.filter((request) => request.status === "pending" && snapshot.decisions[request.pendingId]?.phase !== "settled");
  const ready = snapshot.status === "ready";
  const status = ready ? `${requests.length} pending approval${requests.length === 1 ? "" : "s"}` : snapshot.status === "loading" ? "Checking approvals" : snapshot.status === "stale" ? "Approvals may be out of date" : "Approvals unavailable";
  return (
    <section className="cc-approval-attention" aria-label="Needs you" data-testid="approval-attention" data-state={snapshot.status} data-pending={ready && requests.length > 0}>
      <button className="cc-approval-attention__open" type="button" aria-label={`Needs you, ${status}`} title={status}
        onClick={() => onOpen(ready && requests.length === 1 ? requests[0].pendingId : undefined)}>
        <svg className="cc-approval-attention__icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m4 5-2 9v5h20v-5l-2-9H4Z" /><path d="M2 14h6l2 3h4l2-3h6" />
        </svg>
        <span className="cc-approval-attention__label">Needs you</span>
        <span className="cc-approval-attention__count" aria-hidden="true">{ready ? requests.length : snapshot.status === "loading" ? "..." : "!"}</span>
        <svg className="cc-approval-attention__chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 4 4 4-4 4" /></svg>
      </button>
      <span className="cc-approval-attention__status" role="status">{status}</span>
    </section>
  );
}
