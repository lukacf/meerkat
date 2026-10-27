import React from "react";
import { ApprovalCard } from "../../../packages/console-components/src/conversation/approval-card";
import { normalizeGatingAuditEntry } from "../../../packages/console-core/src/control-plane";
import type { PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";

interface GatingInboxPanelProps {
  pending: unknown[];
  audit: unknown[];
  onDecide: (pendingId: string, decision: "approve" | "reject" | "escalate") => void;
  readOnly?: boolean;
  resource?: PendingApprovalSnapshot;
  onRefresh?: () => void;
  selectedPendingId?: string;
}

type Tab = "pending" | "auto" | "audit" | "policies";

function formatWaited(entry: Record<string, unknown>): string {
  const waited = entry.waited_ms || entry.waited || entry.age_ms;
  if (typeof waited !== "number") return "—";
  const seconds = Math.floor(waited / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

function payloadSummary(entry: Record<string, unknown>): string {
  const payload = entry.payload;
  if (typeof payload === "string") return payload;
  if (payload && typeof payload === "object") {
    try {
      const parts: string[] = [];
      for (const [k, v] of Object.entries(payload).slice(0, 3)) {
        parts.push(`${k}=${String(v).slice(0, 20)}`);
      }
      return parts.join(" ");
    } catch { return ""; }
  }
  return String(entry.summary || entry.reason || "");
}

export function GatingInboxPanel({
  pending,
  audit,
  onDecide,
  readOnly = false,
  resource,
  onRefresh,
  selectedPendingId,
}: GatingInboxPanelProps): React.JSX.Element {
  const [tab, setTab] = React.useState<Tab>("pending");
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const listRef = React.useRef<HTMLDivElement>(null);
  const pendingRequests = resource?.requests.filter((request) => request.status === "pending" && resource.decisions[request.pendingId]?.phase !== "settled");
  const pendingLabel = !resource || resource.status === "ready" ? String(pendingRequests ? pendingRequests.length : pending.length) : "?";
  const selectedRequestAvailable = resource?.requests.some((request) => request.pendingId === selectedPendingId) === true;
  React.useEffect(() => { if (selectedPendingId) { setSelectedId(selectedPendingId); setTab("pending"); } }, [selectedPendingId]);
  React.useEffect(() => {
    if (tab !== "pending" || !selectedPendingId || !selectedRequestAvailable) return;
    const selected = Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-approval-id]") || [])
      .find((element) => element.dataset.approvalId === selectedPendingId);
    selected?.scrollIntoView?.({ block: "nearest" });
    selected?.focus({ preventScroll: true });
  }, [selectedPendingId, selectedRequestAvailable, tab]);

  const auditEntries = audit.map(normalizeGatingAuditEntry);
  const automaticallyAllowed = auditEntries.filter((entry) => entry.automaticAllowance);

  const currentList = tab === "pending"
    ? pending.map((raw) => ({ raw, record: normalizeGatingAuditEntry(raw) }))
    : (tab === "auto" ? automaticallyAllowed : auditEntries).map((record) => ({ raw: undefined, record }));

  return (
    <div className="gating" data-testid="gating-panel">
      <div className="gating__head">
        <h2>Approvals</h2>
        <p>· {pendingLabel} pending · {automaticallyAllowed.length} automatically allowed</p>
      </div>
      <div className="gating__tabs">
        <button
          className={`gating__tab ${tab === "pending" ? "is-active" : ""}`}
          onClick={() => setTab("pending")}
          data-testid="gating-tab:pending"
        >
          Pending <span className="n">{pendingLabel}</span>
        </button>
        <button
          className={`gating__tab ${tab === "auto" ? "is-active" : ""}`}
          onClick={() => setTab("auto")}
          data-testid="gating-tab:auto"
        >
          Auto <span className="n">{automaticallyAllowed.length}</span>
        </button>
        <button
          className={`gating__tab ${tab === "audit" ? "is-active" : ""}`}
          onClick={() => setTab("audit")}
          data-testid="gating-tab:audit"
        >
          Audit <span className="n">{audit.length}</span>
        </button>
        <button
          className={`gating__tab ${tab === "policies" ? "is-active" : ""}`}
          onClick={() => setTab("policies")}
          data-testid="gating-tab:policies"
        >
          Policies
        </button>
      </div>
      <div className="gating__list" ref={listRef}>
        {tab === "pending" && resource ? (
          <div>
            {resource.status !== "ready" ? <p role="status">{resource.status === "forbidden" ? "Approval access denied" : resource.status === "unsupported" ? "Approvals are not available for this connection" : resource.status === "loading" ? "Loading approvals" : resource.status === "stale" ? "Approvals may be out of date" : "Approvals unavailable"}</p> : null}
            {onRefresh ? <button type="button" onClick={onRefresh}>Refresh approvals</button> : null}
            {resource.status === "ready" && pendingRequests?.length === 0 ? <p>No pending approvals.</p> : null}
            {resource.requests.map((request) => <div key={request.pendingId} tabIndex={-1} data-approval-id={request.pendingId} data-selected={selectedId === request.pendingId} className={selectedId === request.pendingId ? "is-selected" : undefined}>
              <ApprovalCard request={request} resourceStatus={resource.status} decision={resource.decisions[request.pendingId]} readOnly={readOnly || resource.readOnly} onDecide={onDecide} />
            </div>)}
          </div>
        ) : tab === "policies" ? (
          <div className="gating__empty" role="status">Policy details are not available in this console.</div>
        ) : (
          <>
          {tab !== "pending" && <p className="gating__audit-note">Approval records do not confirm execution.</p>}
          {currentList.length === 0 && (
          <div className="gating__empty">No {tab} items.</div>
          )}
          {currentList.map(({ raw, record }, index) => {
          const r = raw && typeof raw === "object" ? raw as Record<string, unknown> : {};
          const pid = (tab === "pending" ? record.pendingId : record.auditId) || `item-${index}`;
          const action = record.actionId || record.eventType || "Unknown action";
          const agent = record.actorId || (tab === "pending" ? String(r.agent || r.identity || r.actor || "") : "");
          const waited = formatWaited(r);
          const risk = record.riskTier;
          const tierLabel = risk === "unknown" ? `Unknown${record.recordedRiskTier ? ` (${record.recordedRiskTier})` : ""}` : risk.toUpperCase();
          const payload = tab === "pending" ? payloadSummary(r) : "";

          const selected = selectedId === pid;
          const showActions = tab === "pending" && !readOnly;

          return (
            <div
              className={`gitem ${selected ? "is-selected" : ""}`}
              data-risk={risk}
              data-testid={`gating-${tab === "pending" ? "pending" : "audit"}:${pid}`}
              key={pid}
              onClick={() => setSelectedId(pid)}
            >
              <span className="gitem__risk" aria-hidden="true" />
              <span className="gitem__id">{pid.slice(0, 8)}</span>
              <span>
                <div className="gitem__action">{action}</div>
                {payload && <div className="gitem__payload">{payload}</div>}
                <div className="gitem__agent">Tier: {tierLabel}</div>
                <div className="gitem__agent">Actor: {agent || "Not recorded"}</div>
                {tab !== "pending" && <>
                  <div className="gitem__agent">Event: {record.eventType || "Not recorded"}</div>
                  <div className="gitem__agent">Decision: {record.decision || "Not recorded"}</div>
                  {record.approverId && <div className="gitem__agent">Approver: {record.approverId}</div>}
                  <div className="gitem__agent">Outcome: {record.outcome || "Not recorded"}</div>
                </>}
              </span>
              {showActions ? (
                <span className="gitem__actions">
                  <button
                    className="approve"
                    data-testid={`gating-action:${pid}:approve`}
                    onClick={(e) => { e.stopPropagation(); onDecide(pid, "approve"); }}
                  >Approve</button>
                  <button
                    className="reject"
                    data-testid={`gating-action:${pid}:reject`}
                    onClick={(e) => { e.stopPropagation(); onDecide(pid, "reject"); }}
                  >Reject</button>
                  <button
                    data-testid={`gating-action:${pid}:escalate`}
                    onClick={(e) => { e.stopPropagation(); onDecide(pid, "escalate"); }}
                  >Escalate</button>
                </span>
              ) : (
                <span className="gitem__actions" />
              )}
              {tab === "pending" && <span className="gitem__waited">waited<br />{waited}</span>}
            </div>
          );
          })}
          </>
        )}
      </div>
    </div>
  );
}
