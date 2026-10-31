import * as React from "react";
import type { ApprovalAction, ApprovalDecisionState, PendingApproval, PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";
import type { HostAttentionItem, HostAttentionSourceConfig, HostDecideLink } from "../../../console-core/src/host-attention";
import { RowDetails } from "./presentation-policy";

const labels: Record<ApprovalAction, string> = { approve: "Approve", reject: "Reject", escalate: "Escalate" };

const HostDecideSourcesContext = React.createContext<readonly HostAttentionSourceConfig[] | undefined>(undefined);
/** The attention sources whose `decide_href` a read-only session's approval
 *  cards link to, for cards placed in a conversation. */
export const HostDecideSourcesProvider = HostDecideSourcesContext.Provider;
export function useHostDecideSources(): readonly HostAttentionSourceConfig[] | undefined {
  return React.useContext(HostDecideSourcesContext);
}
export interface ApprovalCardProps {
  request: PendingApproval;
  resourceStatus: PendingApprovalSnapshot["status"];
  decision?: ApprovalDecisionState;
  readOnly?: boolean;
  /** Host pages where a read-only session's decision is made instead. */
  decideLinks?: readonly HostDecideLink[];
  onDecide(pendingId: string, action: ApprovalAction): void | Promise<void>;
}

/** The same request and decision owner can feed an inbox and several panes. */
export function ApprovalCard({ request, resourceStatus, decision, readOnly = false, decideLinks, onDecide }: ApprovalCardProps) {
  const pending = request.status === "pending" && decision?.phase !== "settled";
  const submitting = decision?.phase === "submitting";
  const stale = resourceStatus !== "ready";
  // A session that cannot decide shows the host's decide pages, never a dead Approve.
  const hostDecide = readOnly && pending && decideLinks?.length ? decideLinks : null;
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
      {readOnly && !hostDecide ? <p>Read-only access</p> : null}
      {resourceStatus === "forbidden" ? <p>Approval access denied</p> : null}
      {hostDecide ? <div className="cc-approval__actions">{hostDecide.map((link) => (
        <a key={link.sourceId} className="cc-host-attention__open" href={link.href} target="_top" aria-label={`Decide in ${link.label}: ${request.action}`} data-testid={`gating-decide-link:${request.pendingId}:${link.sourceId}`}>Decide in {link.label}</a>
      ))}</div> : pending ? <div className="cc-approval__actions">{request.actions.map((action) => (
        <button key={action} type="button" disabled={readOnly || stale || submitting} data-action={action} data-testid={`gating-action:${request.pendingId}:${action}`} onClick={() => { void onDecide(request.pendingId, action); }}>{labels[action]}</button>
      ))}</div> : null}
    </article>
  );
}

/** A host-provided attention item: a link card. The console never decides it;
 *  "Open" loads the host's own page top-level, which owns the decision. */
export function HostAttentionCard({ item }: { item: HostAttentionItem }) {
  return (
    <article className="cc-approval cc-host-attention" data-kind="host" data-tone={item.tone || "attention"} data-testid={`host-attention:${item.sourceId}:${item.id}`}>
      <header className="cc-approval__header">
        <strong>{item.title}</strong>
        {item.sourceLabel ? <span className="cc-host-attention__source">From {item.sourceLabel}</span> : null}
      </header>
      {item.details.length ? <dl className="cc-approval__scope">{item.details.map((detail, index) => (
        <React.Fragment key={`${detail.label}:${index}`}><dt>{detail.label}</dt><dd>{detail.value}</dd></React.Fragment>
      ))}</dl> : null}
      <div className="cc-approval__actions">
        <a className="cc-host-attention__open" href={item.href} target="_top" aria-label={`Open: ${item.title}`} data-testid={`host-attention-open:${item.sourceId}:${item.id}`}>Open</a>
      </div>
    </article>
  );
}

export function ApprovalAttention({ snapshot, onOpen, hostItems = 0 }: {
  snapshot?: PendingApprovalSnapshot;
  onOpen(pendingId?: string): void;
  /** Host-provided attention items, counted with the approvals. */
  hostItems?: number;
}) {
  const approvalsShown = Boolean(snapshot) && snapshot?.status !== "forbidden" && snapshot?.status !== "unsupported";
  if (!approvalsShown && hostItems === 0) return null;
  const requests = approvalsShown && snapshot ? snapshot.requests.filter((request) => request.status === "pending" && snapshot.decisions[request.pendingId]?.phase !== "settled") : [];
  const ready = !approvalsShown || snapshot?.status === "ready";
  const approvalStatus = !approvalsShown ? "" : ready ? `${requests.length} pending approval${requests.length === 1 ? "" : "s"}` : snapshot?.status === "loading" ? "Checking approvals" : snapshot?.status === "stale" ? "Approvals may be out of date" : "Approvals unavailable";
  const hostStatus = hostItems ? `${hostItems} item${hostItems === 1 ? "" : "s"} from the host` : "";
  const status = [approvalStatus, hostStatus].filter(Boolean).join(", ");
  const count = requests.length + hostItems;
  return (
    <section className="cc-approval-attention" aria-label="Needs you" data-testid="approval-attention" data-state={approvalsShown ? snapshot?.status : "ready"} data-pending={ready && count > 0}>
      <button className="cc-approval-attention__open" type="button" aria-label={`Needs you, ${status}`} title={status}
        onClick={() => onOpen(ready && requests.length === 1 ? requests[0].pendingId : undefined)}>
        <svg className="cc-approval-attention__icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m4 5-2 9v5h20v-5l-2-9H4Z" /><path d="M2 14h6l2 3h4l2-3h6" />
        </svg>
        <span className="cc-approval-attention__label">Needs you</span>
        <span className="cc-approval-attention__count" aria-hidden="true">{ready ? count : snapshot?.status === "loading" ? "..." : "!"}</span>
        <svg className="cc-approval-attention__chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 4 4 4-4 4" /></svg>
      </button>
      <span className="cc-approval-attention__status" role="status">{status}</span>
    </section>
  );
}
