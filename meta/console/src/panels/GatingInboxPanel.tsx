import React from "react";
import { ApprovalCard, HostAttentionCard } from "../../../packages/console-components/src/conversation/approval-card";
import { normalizeGatingAuditEntry } from "../../../packages/console-core/src/control-plane";
import type { GatingAuditEntryView } from "../../../packages/console-core/src/control-plane";
import type { HostAttentionSnapshot } from "../../../packages/console-core/src/host-attention";
import type { PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import {
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  KitGlyph,
  ListRow,
  LoadingState,
  Notice,
  PanelBody,
  PanelHeader,
  PanelShell,
  PanelTabPanel,
  PanelTabs,
  RelativeTime,
  RowList,
  StatusDot,
  StatusRow,
  TechnicalId,
  agentDisplayName,
  formatAbsoluteTime,
  formatDuration,
  humanizeEnum,
  riskTierWord,
  riskTone,
  toEpochMs,
  useAgentDirectory,
} from "../../../packages/console-components/src/panel-kit";
import type { AgentDirectory, EmptyStateKind, Tone } from "../../../packages/console-components/src/panel-kit";

interface GatingInboxPanelProps {
  pending: unknown[];
  audit: unknown[];
  onDecide: (pendingId: string, decision: "approve" | "reject" | "escalate") => void;
  readOnly?: boolean;
  resource?: PendingApprovalSnapshot;
  /** Refreshes the pending queue (the Pending tab). */
  onRefresh?: () => void;
  /** Refreshes the recorded approvals (the Audit and Auto tabs). */
  onRefreshAudit?: () => void;
  /** When the recorded approvals last loaded. */
  auditUpdatedAtMs?: number | null;
  /** Why the last refresh of the recorded approvals failed, if it did. */
  auditError?: string | null;
  selectedPendingId?: string;
  /** Host-provided attention items, listed on Pending after the approvals. */
  hostAttention?: HostAttentionSnapshot;
}

type Tab = "pending" | "auto" | "audit" | "policies";

type RawRecord = Record<string, unknown>;

function recordOf(value: unknown): RawRecord {
  return value && typeof value === "object" && !Array.isArray(value) ? value as RawRecord : {};
}

function textOf(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function waitedMs(entry: RawRecord): number | null {
  const waited = entry.waited_ms ?? entry.waited ?? entry.age_ms;
  return typeof waited === "number" && Number.isFinite(waited) && waited >= 0 ? waited : null;
}

function payloadSummary(entry: RawRecord): string {
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

/// The action people read: the label the request carried ("Send message"),
/// else the action ID in words. The raw ID stays in the row's tooltip.
function actionTitle(raw: RawRecord, record: GatingAuditEntryView): string {
  const label = textOf(recordOf(raw.detail).action) || textOf(raw.action);
  return label || humanizeEnum(record.actionId) || humanizeEnum(record.eventType) || "Unknown action";
}

const OUTCOME_WORDS: Record<string, string> = {
  allowed: "Allowed",
  allowed_with_audit: "Allowed with audit",
  safe_draft: "Kept as a draft",
  pending_approval: "Waiting for approval",
};

function outcomeWords(outcome: string | undefined): string {
  return outcome ? OUTCOME_WORDS[outcome] || humanizeEnum(outcome) : "";
}

/// The human decisions a recorded event shows as its chip (recordVerdict),
/// so its facts do not repeat them.
const DECISION_VERDICTS = new Set(["approve", "reject", "escalate"]);

/// The headline status of a recorded event: the human decision when there
/// was one, else what happened, else the outcome.
function recordVerdict(record: GatingAuditEntryView): { label: string; tone: Tone } | null {
  if (record.decision === "approve") return { label: "Approved", tone: "ok" };
  if (record.decision === "reject") return { label: "Rejected", tone: "crit" };
  if (record.decision === "escalate") return { label: "Escalated", tone: "warn" };
  if (record.automaticAllowance) return { label: "Allowed automatically", tone: "ok" };
  switch (record.eventType) {
    case "timeout_fallback": return { label: "Timed out", tone: "warn" };
    case "pending_created": return { label: "Requested", tone: "neutral" };
    case "approval_notification": return { label: "Approver notified", tone: "neutral" };
    case "conflict_blocked": return { label: "Blocked", tone: "warn" };
    default: break;
  }
  if (record.outcome === "pending_approval") return { label: "Waiting for approval", tone: "warn" };
  if (record.outcome === "safe_draft") return { label: "Kept as a draft", tone: "warn" };
  if (record.outcome === "allowed" || record.outcome === "allowed_with_audit") return { label: "Allowed", tone: "ok" };
  return record.eventType ? { label: humanizeEnum(record.eventType), tone: "neutral" } : null;
}

/// The risk in words with its tier code: "High (R3)"; a missing tier reads
/// "Unknown", an unrecognized one "Unknown (as recorded)".
function riskWords(record: GatingAuditEntryView): string {
  if (record.riskTier === "unknown") {
    return `Unknown${record.recordedRiskTier ? ` (${record.recordedRiskTier})` : ""}`;
  }
  const word = riskTierWord(record.riskTier);
  const code = record.riskTier.toUpperCase();
  return word ? `${word} (${code})` : code;
}

/// The recorded facts of a row, each one "Label: value" string. People read
/// names; an identity the roster does not know reads as its key.
function recordFacts(
  record: GatingAuditEntryView,
  actor: string,
  directory: AgentDirectory,
  withDecision: boolean,
): string[] {
  const facts = [
    `Risk: ${riskWords(record)}`,
    `Actor: ${actor ? agentDisplayName(actor, directory) : "Not recorded"}`,
  ];
  if (withDecision) {
    // A recorded human decision is the row's chip (Approved, Rejected,
    // Escalated); the facts do not repeat it.
    if (!record.decision || !DECISION_VERDICTS.has(record.decision)) {
      facts.push(`Decision: ${record.decision ? humanizeEnum(record.decision) : "Not recorded"}`);
    }
    if (record.approverId) facts.push(`Approver: ${agentDisplayName(record.approverId, directory)}`);
    facts.push(`Outcome: ${outcomeWords(record.outcome) || "Not recorded"}`);
  }
  return facts;
}

/// One recorded event (Audit and Auto) or one pending request without the
/// shared approval resource. The row opens to its technical details.
function GatingRecordRow({
  pid,
  raw,
  record,
  kind,
  expanded,
  onToggle,
  actions,
}: {
  /// The row's key in its test ID: the request or audit ID, or a position.
  pid: string;
  raw: RawRecord;
  record: GatingAuditEntryView;
  kind: "pending" | "audit";
  expanded: boolean;
  onToggle: () => void;
  actions?: React.ReactNode;
}) {
  const directory = useAgentDirectory();
  const actor = record.actorId || (kind === "pending" ? textOf(raw.agent) || textOf(raw.identity) || textOf(raw.actor) : "");
  const verdict = kind === "audit" ? recordVerdict(record) : null;
  const facts = recordFacts(record, actor, directory, kind === "audit");
  const payload = kind === "pending" ? payloadSummary(raw) : "";
  if (payload) facts.push(`Details: ${payload}`);
  const recordedAt = toEpochMs(raw.timestamp_ms as number | undefined);
  const waited = kind === "pending" ? waitedMs(raw) : null;
  const policy = textOf(recordOf(raw.detail).policy);
  return (
    <div className="gitem-group">
      <ListRow
        className={expanded ? "gitem is-expanded" : "gitem"}
        testId={`gating-${kind}:${pid}`}
        dataAttributes={{ "data-risk": record.riskTier }}
        lead={<StatusDot tone={riskTone(record.riskTier)} className="gitem__risk" />}
        title={actionTitle(raw, record)}
        titleAttr={record.actionId || undefined}
        subtitle={(
          <span className="gitem__facts">
            {facts.map((fact) => <span key={fact} className="gitem__fact">{fact}</span>)}
          </span>
        )}
        meta={(
          <>
            {verdict ? <Chip tone={verdict.tone}>{verdict.label}</Chip> : null}
            {waited !== null ? <span className="gitem__waited">{`Waiting ${formatDuration(waited)}`}</span> : null}
            {recordedAt !== null ? <RelativeTime value={recordedAt} /> : null}
          </>
        )}
        actions={actions}
        onActivate={onToggle}
        ariaExpanded={expanded}
      />
      {expanded ? (
        <div className="gitem__details">
          <KeyValueList
            items={[
              record.auditId ? { key: "audit", label: "Audit record ID", value: <TechnicalId value={record.auditId} label="Audit record ID" /> } : null,
              record.pendingId ? { key: "pending", label: "Request ID", value: <TechnicalId value={record.pendingId} label="Request ID" /> } : null,
              record.eventType ? { key: "event", label: "Event", value: <code className="gitem__code">{record.eventType}</code> } : null,
              policy ? { key: "policy", label: "Policy", value: <code className="gitem__code">{policy}</code> } : null,
              recordedAt !== null ? { key: "recorded", label: "Recorded", value: formatAbsoluteTime(recordedAt) } : null,
            ]}
          />
          {!record.auditId && !record.pendingId && !record.eventType && !policy && recordedAt === null ? (
            <p className="gitem__none">No technical details were recorded.</p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

const RESOURCE_STATE: Record<
  Exclude<PendingApprovalSnapshot["status"], "ready">,
  { text: string; empty: EmptyStateKind; tone: Tone }
> = {
  forbidden: { text: "Approval access denied", empty: "no-access", tone: "locked" },
  unsupported: { text: "Approvals are not available for this connection", empty: "not-configured", tone: "locked" },
  loading: { text: "Loading approvals", empty: "empty", tone: "neutral" },
  stale: { text: "Approvals may be out of date", empty: "unavailable", tone: "warn" },
  unavailable: { text: "Approvals unavailable", empty: "unavailable", tone: "warn" },
};

export function GatingInboxPanel({
  pending,
  audit,
  onDecide,
  readOnly = false,
  resource,
  onRefresh,
  onRefreshAudit,
  auditUpdatedAtMs,
  auditError,
  selectedPendingId,
  hostAttention,
}: GatingInboxPanelProps): React.JSX.Element {
  const [tab, setTab] = React.useState<Tab>("pending");
  const tabIds = React.useId();
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [expandedId, setExpandedId] = React.useState<string | null>(null);
  const listRef = React.useRef<HTMLDivElement>(null);
  const pendingRequests = resource?.requests.filter((request) => request.status === "pending" && resource.decisions[request.pendingId]?.phase !== "settled");
  const pendingKnown = !resource || resource.status === "ready";
  // A connection without approvals (an aggregate console) says so instead of
  // showing counts that cannot apply to it.
  const approvalsUnsupported = resource?.status === "unsupported";
  const pendingCount = pendingRequests ? pendingRequests.length : pending.length;
  const selectedRequestAvailable = resource?.requests.some((request) => request.pendingId === selectedPendingId) === true;
  React.useEffect(() => { if (selectedPendingId) { setSelectedId(selectedPendingId); setTab("pending"); } }, [selectedPendingId]);
  React.useEffect(() => {
    if (tab !== "pending" || !selectedPendingId || !selectedRequestAvailable) return;
    const selected = Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-approval-id]") || [])
      .find((element) => element.dataset.approvalId === selectedPendingId);
    selected?.scrollIntoView?.({ block: "nearest" });
    selected?.focus({ preventScroll: true });
  }, [selectedPendingId, selectedRequestAvailable, tab]);

  const auditEntries = audit.map((raw) => ({ raw: recordOf(raw), record: normalizeGatingAuditEntry(raw) }));
  const automaticallyAllowed = auditEntries.filter((entry) => entry.record.automaticAllowance);
  const automaticCount = automaticallyAllowed.length;

  const toggle = (key: string) => setExpandedId((current) => (current === key ? null : key));

  // Host-provided attention items follow the approvals on Pending: link cards
  // that the host decides on its own page, then the sources that could not be
  // read. Neither hides an approval. While they show, an empty approval list
  // says so in one line, so the items stay in view.
  const hostItems = tab === "pending" ? hostAttention?.items ?? [] : [];
  const hostFailedSources = tab === "pending" ? hostAttention?.failedSources ?? [] : [];
  const hostShown = hostItems.length > 0 || hostFailedSources.length > 0;
  const hostList = hostShown ? (
    <div className="gating-inbox__cards gating-inbox__host" data-testid="host-attention-list">
      {hostItems.map((item) => <HostAttentionCard key={`${item.sourceId}:${item.id}`} item={item} />)}
      {hostFailedSources.length ? (
        <Notice tone="warn" role="status" className="gating-inbox__host-note">
          {`Could not read: ${hostFailedSources.join(", ")}`}
        </Notice>
      ) : null}
    </div>
  ) : null;

  let content: React.ReactNode;
  if (tab === "pending" && resource) {
    const state = resource.status === "ready" ? null : RESOURCE_STATE[resource.status];
    content = (
      <>
        {state && resource.requests.length === 0 ? (
          resource.status === "loading"
            ? <LoadingState variant="cards" rows={2} label={state.text} />
            : <EmptyState kind={state.empty} role="status" title={state.text} inline={hostShown} />
        ) : state ? (
          <Notice tone={state.tone} role="status">{state.text}</Notice>
        ) : null}
        {resource.status === "ready" && pendingRequests?.length === 0 ? (
          <EmptyState title="No pending approvals." body="Requests that need a decision appear here." inline={hostShown} />
        ) : null}
        {resource.requests.length ? (
          <div className="gating-inbox__cards">
            {resource.requests.map((request) => (
              <div
                key={request.pendingId}
                tabIndex={-1}
                data-approval-id={request.pendingId}
                data-selected={selectedId === request.pendingId}
                className={selectedId === request.pendingId ? "is-selected" : undefined}
              >
                <ApprovalCard
                  request={request}
                  resourceStatus={resource.status}
                  decision={resource.decisions[request.pendingId]}
                  readOnly={readOnly || resource.readOnly}
                  onDecide={onDecide}
                />
              </div>
            ))}
          </div>
        ) : null}
        {hostList}
      </>
    );
  } else if (tab === "policies") {
    content = <EmptyState kind="unavailable" role="status" title="Policy details are not available in this console." />;
  } else {
    const rows = tab === "pending"
      ? pending.map((raw) => ({ raw: recordOf(raw), record: normalizeGatingAuditEntry(raw) }))
      : tab === "auto" ? automaticallyAllowed : auditEntries;
    const showActions = tab === "pending" && !readOnly;
    content = (
      <>
        {tab !== "pending" ? (
          <Notice className="gating-inbox__note">Approval records do not confirm execution.</Notice>
        ) : null}
        {rows.length === 0 ? (
          <EmptyState title={`No ${tab} items.`} inline={hostShown} />
        ) : (
          <RowList>
            {rows.map(({ raw, record }, index) => {
              const pid = (tab === "pending" ? record.pendingId : record.auditId) || `item-${index}`;
              const key = `${tab}:${pid}`;
              return (
                <GatingRecordRow
                  key={pid}
                  pid={pid}
                  raw={raw}
                  record={record}
                  kind={tab === "pending" ? "pending" : "audit"}
                  expanded={expandedId === key}
                  onToggle={() => toggle(key)}
                  actions={showActions ? (
                    <>
                      <Button
                        size="sm"
                        variant="primary"
                        data-testid={`gating-action:${pid}:approve`}
                        onClick={() => onDecide(pid, "approve")}
                      >
                        Approve
                      </Button>
                      <Button
                        size="sm"
                        variant="danger"
                        data-testid={`gating-action:${pid}:reject`}
                        onClick={() => onDecide(pid, "reject")}
                      >
                        Reject
                      </Button>
                      <Button
                        size="sm"
                        data-testid={`gating-action:${pid}:escalate`}
                        onClick={() => onDecide(pid, "escalate")}
                      >
                        Escalate
                      </Button>
                    </>
                  ) : undefined}
                />
              );
            })}
          </RowList>
        )}
        {hostList}
      </>
    );
  }

  // Refresh and freshness belong to the list on screen: the pending queue on
  // Pending, the recorded approvals on Audit and Auto, nothing on Policies.
  const refreshShown = tab === "pending" ? onRefresh : tab === "policies" ? undefined : onRefreshAudit;
  const updatedShown = tab === "pending" ? resource?.updatedAtMs : tab === "policies" ? undefined : auditUpdatedAtMs ?? undefined;
  // A failed refresh keeps the records already loaded and says so.
  const errorShown = (tab === "auto" || tab === "audit") && auditError
    ? `Couldn't refresh the approval records (${auditError}). Showing the ones loaded before.`
    : null;

  return (
    <PanelShell className="gating-inbox" testId="gating-panel">
      <PanelHeader
        className="gating__head"
        title="Approvals"
        subtitle={approvalsUnsupported
          ? "Not available on this connection"
          : pendingKnown
            ? `${pendingCount} pending · ${automaticCount} allowed automatically`
            : `${automaticCount} allowed automatically`}
        divider={false}
        actions={refreshShown ? (
          <Button icon={<KitGlyph name="refresh" />} onClick={refreshShown}>Refresh approvals</Button>
        ) : undefined}
      />
      <PanelTabs<Tab>
        ariaLabel="Approval views"
        idBase={tabIds}
        active={tab}
        onSelect={setTab}
        tabs={[
          { id: "pending", label: "Pending", count: pendingKnown ? pendingCount : null, testId: "gating-tab:pending" },
          { id: "auto", label: "Auto", count: automaticCount, testId: "gating-tab:auto" },
          { id: "audit", label: "Audit", count: audit.length, testId: "gating-tab:audit" },
          { id: "policies", label: "Policies", testId: "gating-tab:policies" },
        ]}
      />
      <StatusRow updatedAt={updatedShown} error={errorShown} errorTestId="gating-audit-error" />
      <PanelBody className="gating-inbox__body">
        <PanelTabPanel className="gating-inbox__content" ref={listRef} idBase={tabIds} tab={tab}>{content}</PanelTabPanel>
      </PanelBody>
    </PanelShell>
  );
}
