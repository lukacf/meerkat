import * as React from "react";
import type { ApprovalAction, ApprovalDecisionState, PendingApproval, PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";
import {
  AgentName,
  Button,
  Chip,
  KeyValueList,
  Notice,
  RelativeTime,
  TechnicalId,
  formatDuration,
  humanizeEnum,
  riskTone,
  useNow,
} from "../panel-kit";
import type { ButtonVariant, Tone } from "../panel-kit";
import { RowDetails } from "./presentation-policy";

const labels: Record<ApprovalAction, string> = { approve: "Approve", reject: "Reject", escalate: "Escalate" };
const variants: Record<ApprovalAction, ButtonVariant> = { approve: "primary", reject: "danger", escalate: "default" };
export interface ApprovalCardProps {
  request: PendingApproval;
  resourceStatus: PendingApprovalSnapshot["status"];
  decision?: ApprovalDecisionState;
  readOnly?: boolean;
  onDecide(pendingId: string, action: ApprovalAction): void | Promise<void>;
}

type ApprovalCardState = "pending" | "submitting" | "stale" | "failed" | "unavailable" | "expired" | "settled";

const STATE_WORDS: Record<ApprovalCardState, { text: string; tone: Tone }> = {
  pending: { text: "Approval needed", tone: "warn" },
  submitting: { text: "Submitting decision", tone: "info" },
  stale: { text: "Approval state may be out of date", tone: "warn" },
  failed: { text: "Decision unconfirmed", tone: "crit" },
  unavailable: { text: "Decision unavailable", tone: "neutral" },
  expired: { text: "Expired", tone: "neutral" },
  settled: { text: "Resolved", tone: "ok" },
};

/** Risk tiers read as recorded, with the tier codes in capitals ("R3"). */
function riskLabel(tier: string): string {
  return /^r\d$/i.test(tier) ? tier.toUpperCase() : tier;
}

/** The same request and decision owner can feed an inbox and several panes. */
export function ApprovalCard({ request, resourceStatus, decision, readOnly = false, onDecide }: ApprovalCardProps) {
  const now = useNow();
  const pending = request.status === "pending" && decision?.phase !== "settled";
  const submitting = decision?.phase === "submitting";
  const stale = resourceStatus !== "ready";
  const state: ApprovalCardState = submitting ? "submitting" : decision?.phase === "failed" ? "failed" : request.status === "expired" ? "expired" : !pending ? "settled" : stale ? "stale" : decision?.phase === "unavailable" ? "unavailable" : "pending";
  const words = STATE_WORDS[state];
  const waiting = pending && request.createdAtMs !== undefined
    ? formatDuration(Math.max(0, now - request.createdAtMs))
    : "";
  // A request without its own label falls back to the action ID: read it as
  // words, with the exact ID in the tooltip (and in the request details).
  const titleIsId = request.action === request.actionId;
  return (
    <article className="cc-approval" data-state={state} data-testid={`gating-pending:${request.pendingId}`}>
      <header className="cc-approval__header">
        <div className="cc-approval__heading">
          <strong className="cc-approval__title" title={titleIsId ? request.actionId : undefined}>
            {titleIsId ? humanizeEnum(request.actionId) || request.action : request.action}
          </strong>
          {waiting ? <span className="cc-approval__waiting">{`Waiting ${waiting}`}</span> : null}
        </div>
        <span className="cc-approval__status" role="status">
          <Chip tone={words.tone}>{words.text}</Chip>
        </span>
      </header>
      {request.rationale ? <p className="cc-approval__rationale">{request.rationale}</p> : null}
      <KeyValueList
        className="cc-approval__scope"
        items={[
          request.actorId ? { key: "actor", label: "Actor", value: <AgentName id={request.actorId} /> } : null,
          { key: "approver", label: "Requested approver", value: request.requestedApprover || "Not specified" },
          request.origin ? { key: "origin", label: "Origin", value: <AgentName id={request.origin.identity} /> } : null,
          request.riskTier ? {
            key: "risk",
            label: "Risk",
            value: <Chip tone={riskTone(request.riskTier)}>{riskLabel(request.riskTier)}</Chip>,
          } : null,
          request.approvalRecipient ? { key: "recipient", label: "Notification recipient", value: request.approvalRecipient } : null,
          request.approvalChannel ? { key: "channel", label: "Notification channel", value: request.approvalChannel } : null,
          request.deadlineAtMs !== undefined ? {
            key: "deadline",
            label: "Deadline",
            value: <RelativeTime value={request.deadlineAtMs} />,
          } : null,
        ]}
      />
      <RowDetails part={`approval-details:${request.pendingId}`} className="cc-approval__details cc-tech">
        <summary className="cc-tech__summary">Complete request details</summary>
        <div className="cc-tech__body">
          <KeyValueList
            items={[
              { key: "request", label: "Request", value: <code>{request.pendingId}</code> },
              { key: "scope", label: "Action scope", value: <code>{request.actionId}</code> },
              request.approvalRouteId ? { key: "route", label: "Route reference", value: <code>{request.approvalRouteId}</code> } : null,
              request.approvalDeliveryId ? { key: "delivery", label: "Delivery reference", value: <code>{request.approvalDeliveryId}</code> } : null,
            ]}
          />
          {request.approvalRouteId || request.approvalDeliveryId ? (
            <p className="cc-approval__note">Notification references do not confirm receipt or a human decision.</p>
          ) : null}
          <pre>{JSON.stringify(request.raw, null, 2)}</pre>
        </div>
      </RowDetails>
      {decision?.error ? <Notice tone="crit" role="alert">{decision.error}</Notice> : null}
      {decision?.result?.next_pending_id ? (
        <Notice tone="info">
          Escalated to a new request <TechnicalId value={decision.result.next_pending_id} label="Request ID" />
        </Notice>
      ) : null}
      {readOnly ? <Notice tone="locked">Read-only access</Notice> : null}
      {resourceStatus === "forbidden" ? <Notice tone="locked">Approval access denied</Notice> : null}
      {pending ? <div className="cc-approval__actions">{request.actions.map((action) => (
        <Button
          key={action}
          variant={variants[action]}
          disabled={readOnly || stale || submitting}
          data-action={action}
          data-testid={`gating-action:${request.pendingId}:${action}`}
          onClick={() => { void onDecide(request.pendingId, action); }}
        >
          {labels[action]}
        </Button>
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
