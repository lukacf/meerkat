import React from "react";
import { ApprovalCard, HostAttentionCard, deadlineWords, useApprovalClock } from "../../../packages/console-components/src/conversation/approval-card";
import type { GatingAuditEntryView } from "../../../packages/console-core/src/control-plane";
import { hostDecideLinks, type HostAttentionSnapshot, type HostAttentionSourceConfig } from "../../../packages/console-core/src/host-attention";
import type { PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import {
  Button,
  Chip,
  Count,
  DotStrip,
  EmptyState,
  FigureTile,
  GlanceStrip,
  IconButton,
  KeyValueList,
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
  StatusMark,
  StatusRow,
  TechnicalId,
  agentDisplayName,
  formatAbsoluteTime,
  formatClockTime,
  formatDuration,
  humanizeEnum,
  joinMeta,
  plural,
  riskTierWord,
  riskTone,
  standardPanelMenu,
  toEpochMs,
  useAgentDirectory,
  usePanelDensity,
  useShowTechnicalIds,
} from "../../../packages/console-components/src/panel-kit";
import type { AgentDirectory, DotStripItem, EmptyStateKind, SummaryFact, Tone } from "../../../packages/console-components/src/panel-kit";
import {
  RISK_TIERS,
  actionTitles,
  auditRows,
  byDeadline,
  decidedSince,
  decisionEvents,
  groupByDay,
  recordOf,
  riskMix,
  startOfDay,
  type AuditRow,
  type DecisionEvent,
  type DecisionKind,
  type RawRecord,
} from "./gating-model";

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
  /** How many records the audit read asks for: a full read is a window of
   * the newest records, and its counts say so. */
  auditLimit?: number;
  selectedPendingId?: string;
  /** Host-provided attention items, listed on Pending after the approvals. */
  hostAttention?: HostAttentionSnapshot;
  /** Attention sources whose `decide_href` a read-only session links to. */
  hostDecideSources?: readonly HostAttentionSourceConfig[];
  /** Grows by one each time Needs you opens the inbox: each new value shows
   *  the Pending tab, also in an inbox that is already open on another tab. */
  showPending?: number;
}

type Tab = "pending" | "auto" | "audit" | "policies";

const DAY_MS = 24 * 3_600_000;

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
/// else the label of another record of the same action (a decision row
/// carries none), else the action ID in words. The raw ID stays in the
/// row's tooltip.
function actionTitle(raw: RawRecord, record: GatingAuditEntryView, titles?: ReadonlyMap<string, string>): string {
  const label = textOf(recordOf(raw.detail).action) || textOf(raw.action) || (record.actionId ? titles?.get(record.actionId) : "") || "";
  if (label) return label;
  // An owner-minted id ("gate-action-v2-6dd2...-000066") reads as nothing
  // in words; a named action ("send-message") reads as its words.
  if (record.actionId && /[0-9a-f]{12,}|\d{5,}/i.test(record.actionId)) return "Untitled request";
  return humanizeEnum(record.actionId) || humanizeEnum(record.eventType) || "Unknown action";
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
    // No one decided in time and the safe fallback applied: a quiet mark,
    // as on the decisions strip.
    case "timeout_fallback": return { label: "Timed out", tone: "neutral" };
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
  titles,
}: {
  /// The row's key in its test ID: the request or audit ID, or a position.
  pid: string;
  raw: RawRecord;
  record: GatingAuditEntryView;
  kind: "pending" | "audit";
  expanded: boolean;
  onToggle: () => void;
  actions?: React.ReactNode;
  titles?: ReadonlyMap<string, string>;
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
        title={actionTitle(raw, record, titles)}
        titleAttr={record.actionId || undefined}
        subtitle={(
          <span className="gitem__facts">
            {facts.map((fact) => <span key={fact} className="gitem__fact">{fact}</span>)}
          </span>
        )}
        meta={(
          <>
            {verdict ? <Chip tone={verdict.tone} mark variant="quiet">{verdict.label}</Chip> : null}
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

const DECISION_MARK: Record<DecisionKind, { shape: DotStripItem["shape"]; tone: Tone; word: string; plural: string }> = {
  approve: { shape: "dot", tone: "ok", word: "Approved", plural: "approved" },
  reject: { shape: "diamond", tone: "crit", word: "Rejected", plural: "rejected" },
  escalate: { shape: "triangle", tone: "warn", word: "Escalated", plural: "escalated" },
  timeout: { shape: "hollow", tone: "neutral", word: "Timed out", plural: "timed out" },
};

/** The width of an element, followed as it resizes (a fallback where the
 * browser cannot observe it). */
function useWidth(fallback: number): [React.RefCallback<HTMLElement>, number] {
  const [width, setWidth] = React.useState(fallback);
  const observer = React.useRef<ResizeObserver | null>(null);
  const ref = React.useCallback((element: HTMLElement | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (!element || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      const next = Math.floor(element.clientWidth);
      if (next > 0) setWidth((current) => (Math.abs(current - next) >= 2 ? next : current));
    };
    observer.current = new ResizeObserver(measure);
    observer.current.observe(element);
    measure();
  }, []);
  React.useEffect(() => () => observer.current?.disconnect(), []);
  return [ref, width];
}

/** Decisions (or automatic allowances) on the last day's axis: one mark per
 * decision, in its tone's shape, with the legend counted below. */
function DecisionsStrip({
  events,
  now,
  automatic,
}: {
  events: ReadonlyArray<{ id: string; at: number; kind: DecisionKind | "auto"; title: string; approverId?: string }>;
  now: number;
  automatic: boolean;
}) {
  const directory = useAgentDirectory();
  const [ref, width] = useWidth(560);
  // The axis spans the last day, or less when everything happened lately:
  // the last hour, 3, 6 or 12 hours, so recent marks do not pile up at "now".
  const recent = events.filter((event) => event.at >= now - DAY_MS && event.at <= now + 60_000);
  const oldest = recent.reduce((min, event) => Math.min(min, event.at), now);
  const hours = [1, 3, 6, 12, 24].find((span) => now - oldest <= span * 3_600_000 * 0.92) ?? 24;
  const start = now - hours * 3_600_000;
  const windowWords = hours === 1 ? "last hour" : `last ${hours} hours`;
  const shown = recent.filter((event) => event.at >= start);
  const counts = new Map<string, number>();
  for (const event of shown) counts.set(event.kind, (counts.get(event.kind) ?? 0) + 1);
  const items: DotStripItem[] = shown.map((event) => {
    const mark = event.kind === "auto" ? { shape: "dot" as const, tone: "ok" as Tone, word: "Allowed automatically" } : DECISION_MARK[event.kind];
    const by = event.approverId ? ` by ${agentDisplayName(event.approverId, directory)}` : "";
    return {
      id: event.id,
      t: event.at,
      shape: mark.shape,
      tone: mark.tone,
      label: `${mark.word}${by}: ${event.title}, ${formatClockTime(event.at)}`,
    };
  });
  const legend = automatic
    ? [{ key: "auto", tone: "ok" as Tone, text: `${counts.get("auto") ?? 0} allowed automatically` }]
    : (Object.keys(DECISION_MARK) as DecisionKind[]).map((kind) => ({ key: kind, tone: DECISION_MARK[kind].tone, text: `${counts.get(kind) ?? 0} ${DECISION_MARK[kind].plural}` }));
  const label = automatic
    ? `${plural(shown.length, "action")} allowed automatically in the ${windowWords}`
    : `${plural(shown.length, "decision")} in the ${windowWords}: ${legend.map((entry) => entry.text).join(", ")}`;
  return (
    <section className="gating-strip" aria-label={automatic ? `Allowed automatically, ${windowWords}` : `Decisions, ${windowWords}`}>
      <div className="gating-strip__head">
        <span className="gating-strip__label" data-testid="gating-strip-window">{automatic ? `Allowed automatically · ${windowWords}` : `Decisions · ${windowWords}`}</span>
        <span className="gating-strip__legend">
          {legend.map((entry) => (
            <span key={entry.key} className="gating-strip__legend-item" data-tone={entry.tone}>
              <StatusMark tone={entry.tone} />
              {entry.text}
            </span>
          ))}
        </span>
      </div>
      <div className="gating-strip__plot" ref={ref}>
        <DotStrip
          testId="gating-decisions-strip"
          testIdPrefix="gating-decision"
          items={items}
          start={start}
          end={now}
          width={Math.max(160, width)}
          height={20}
          markSize={8}
          axis
          now={now}
          label={label}
        />
      </div>
    </section>
  );
}

/** The risk mix of the recorded window as an ordinal ink ramp, R0 lightest
 * to R2 darkest, R3 in the negative tone; the words in the legend. */
function RiskMix({ mix }: { mix: ReturnType<typeof riskMix> }) {
  const known = mix.r0 + mix.r1 + mix.r2 + mix.r3;
  const words = RISK_TIERS.map((tier) => `${riskTierWord(tier)} ${mix[tier]}`).join(", ");
  return (
    <span className="gating-risk" data-testid="gating-risk-mix">
      <span className="gating-risk__bar" role="img" aria-label={`Risk of the recorded actions: ${words}`}>
        {known > 0 ? RISK_TIERS.map((tier) => (mix[tier] > 0 ? (
          <span
            key={tier}
            className="gating-risk__seg"
            data-tier={tier}
            style={{ flexGrow: mix[tier] }}
            title={`${riskTierWord(tier)} risk (${tier.toUpperCase()}): ${plural(mix[tier], "action")}`}
          />
        ) : null)) : null}
      </span>
    </span>
  );
}

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
  auditLimit,
  selectedPendingId,
  hostAttention,
  hostDecideSources,
  showPending,
}: GatingInboxPanelProps): React.JSX.Element {
  const [tab, setTab] = React.useState<Tab>("pending");
  const tabIds = React.useId();
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [expandedId, setExpandedId] = React.useState<string | null>(null);
  const [density, setDensity] = usePanelDensity("gating");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("gating");
  const listRef = React.useRef<HTMLDivElement>(null);
  const pendingRequests = resource?.requests.filter((request) => request.status === "pending" && resource.decisions[request.pendingId]?.phase !== "settled");
  const pendingKnown = !resource || resource.status === "ready";
  // A connection without approvals (an aggregate console) says so instead of
  // showing counts that cannot apply to it.
  const approvalsUnsupported = resource?.status === "unsupported";
  const pendingCount = pendingRequests ? pendingRequests.length : pending.length;
  const selectedRequestAvailable = resource?.requests.some((request) => request.pendingId === selectedPendingId) === true;
  React.useEffect(() => { if (selectedPendingId) { setSelectedId(selectedPendingId); setTab("pending"); } }, [selectedPendingId]);
  // Needs you opens the inbox on Pending, where its items are listed: a pane
  // that already shows the inbox would keep its tab otherwise.
  React.useEffect(() => { if (showPending) setTab("pending"); }, [showPending]);
  React.useEffect(() => {
    if (tab !== "pending" || !selectedPendingId || !selectedRequestAvailable) return;
    const selected = Array.from(listRef.current?.querySelectorAll<HTMLElement>("[data-approval-id]") || [])
      .find((element) => element.dataset.approvalId === selectedPendingId);
    selected?.scrollIntoView?.({ block: "nearest" });
    selected?.focus({ preventScroll: true });
  }, [selectedPendingId, selectedRequestAvailable, tab]);

  const auditEntries = React.useMemo(() => auditRows(audit), [audit]);
  const automaticallyAllowed = auditEntries.filter((entry) => entry.record.automaticAllowance);
  const automaticCount = automaticallyAllowed.length;
  const auditCapped = auditLimit !== undefined && audit.length >= auditLimit;
  // Titles seen in this session stay known: a request decided here leaves
  // the queue, and its own record may be older than the audit window.
  const titleMemory = React.useRef(new Map<string, string>());
  const titles = React.useMemo(() => {
    const read = actionTitles(auditEntries, resource?.requests ?? []);
    for (const [action, title] of read) titleMemory.current.set(action, title);
    return new Map(titleMemory.current);
  }, [auditEntries, resource?.requests]);
  const decisions = React.useMemo(() => decisionEvents(auditEntries, titles), [auditEntries, titles]);
  const queue = React.useMemo(() => (pendingRequests ? byDeadline(pendingRequests) : []), [pendingRequests]);
  const deadlines = queue.map((request) => request.deadlineAtMs).filter((deadline): deadline is number => deadline !== undefined);
  const now = useApprovalClock(deadlines);
  const next = queue.find((request) => request.deadlineAtMs !== undefined);
  const nextWords = next?.deadlineAtMs !== undefined ? deadlineWords(next.deadlineAtMs, now) : null;
  const decidedToday = decidedSince(decisions, startOfDay(now));
  const mix = riskMix(auditEntries);
  const highRisk = queue.filter((request) => request.riskTier === "r3" || request.riskTier === "r4").length;
  const oldest = queue.reduce<number | null>((min, request) => (request.createdAtMs !== undefined && (min === null || request.createdAtMs < min) ? request.createdAtMs : min), null);

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
    // Soonest deadline first; a request decided here stays in place until
    // the owner drops it.
    const cards = byDeadline(resource.requests);
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
          <EmptyState
            illustration={hostShown ? undefined : "approvals-clear"}
            title="No pending approvals."
            body="Requests that need a decision appear here, soonest deadline first."
            inline={hostShown}
          />
        ) : null}
        {cards.length ? (
          <div className="gating-inbox__cards">
            {cards.map((request) => (
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
                  decideLinks={readOnly || resource.readOnly ? hostDecideLinks(hostDecideSources, request.pendingId) : undefined}
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
  } else if (tab === "pending") {
    const rows = pending.map((raw, index) => ({ raw: recordOf(raw), record: auditRows([raw])[0].record, index }));
    const showActions = !readOnly;
    content = (
      <>
        {rows.length === 0 ? (
          <EmptyState title="No pending items." inline={hostShown} />
        ) : (
          <RowList>
            {rows.map(({ raw, record, index }) => {
              const pid = record.pendingId || `item-${index}`;
              const key = `pending:${pid}`;
              return (
                <GatingRecordRow
                  key={pid}
                  pid={pid}
                  raw={raw}
                  record={record}
                  kind="pending"
                  expanded={expandedId === key}
                  onToggle={() => toggle(key)}
                  titles={titles}
                  actions={showActions ? (
                    <>
                      <Button size="sm" variant="positive" data-testid={`gating-action:${pid}:approve`} onClick={() => onDecide(pid, "approve")}>
                        Approve
                      </Button>
                      <Button size="sm" variant="danger" data-testid={`gating-action:${pid}:reject`} onClick={() => onDecide(pid, "reject")}>
                        Reject
                      </Button>
                      <Button size="sm" data-testid={`gating-action:${pid}:escalate`} onClick={() => onDecide(pid, "escalate")}>
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
  } else {
    const rows: AuditRow[] = tab === "auto" ? automaticallyAllowed : auditEntries;
    const groups = groupByDay(rows, now);
    const stripEvents = tab === "auto"
      ? automaticallyAllowed.flatMap(({ record, at, raw }, index) => (at === null ? [] : [{
        id: record.auditId || `auto-${index}`,
        at,
        kind: "auto" as const,
        title: actionTitle(raw, record, titles),
      }]))
      : decisions;
    content = (
      <>
        <Notice className="gating-inbox__note">Approval records do not confirm execution.</Notice>
        {rows.length > 0 ? <DecisionsStrip events={stripEvents as ReadonlyArray<DecisionEvent | { id: string; at: number; kind: "auto"; title: string }>} now={now} automatic={tab === "auto"} /> : null}
        {auditCapped ? (
          <p className="gating-inbox__window" data-testid="gating-audit-window">
            {`The newest ${auditLimit} records. ${tab === "auto" ? `${automaticCount} of them were allowed automatically.` : "Older records are not loaded."}`}
          </p>
        ) : null}
        {rows.length === 0 ? (
          <EmptyState title={`No ${tab} items.`} inline={hostShown} />
        ) : (
          groups.map((group) => (
            <section key={group.key} className="gating-day" data-testid={`gating-day:${group.key}`}>
              <div className="gating-day__label">
                {group.label}
                <Count value={group.rows.length} />
              </div>
              <RowList>
                {group.rows.map(({ raw, record }, index) => {
                  const pid = record.auditId || `item-${group.key}-${index}`;
                  const key = `${tab}:${pid}`;
                  return (
                    <GatingRecordRow
                      key={pid}
                      pid={pid}
                      raw={raw}
                      record={record}
                      kind="audit"
                      expanded={expandedId === key}
                      onToggle={() => toggle(key)}
                      titles={titles}
                    />
                  );
                })}
              </RowList>
            </section>
          ))
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

  const automaticWords = `${automaticCount} allowed automatically${auditCapped ? ` in the last ${auditLimit} records` : ""}`;
  const summary: Array<SummaryFact | null> = approvalsUnsupported
    ? [{ key: "unsupported", text: "Not available on this connection", announce: false }]
    : [
      pendingKnown ? { key: "pending", text: `${pendingCount} pending`, tone: pendingCount > 0 ? "warn" : undefined, testId: "gating-summary-pending" } : null,
      { key: "auto", text: automaticWords },
      nextWords ? { key: "deadline", text: nextWords.past ? "a deadline has passed" : `next deadline ${nextWords.text}`, announce: false, testId: "gating-summary-deadline" } : null,
    ];
  const glance = approvalsUnsupported ? null : (
    <GlanceStrip ariaLabel="Approvals at a glance" testId="gating-glance" className="gating-glance">
      <FigureTile
        label="Waiting"
        value={pendingKnown ? pendingCount : 0}
        loading={!pendingKnown}
        tone={pendingKnown && pendingCount > 0 ? "warn" : undefined}
        foot={pendingKnown ? joinMeta(
          highRisk ? `${highRisk} high risk` : null,
          oldest !== null ? `oldest ${formatDuration(Math.max(0, now - oldest))}` : pendingCount === 0 ? "nothing needs you" : null,
        ) : undefined}
        testId="gating-glance:waiting"
      />
      <FigureTile
        label="Next deadline"
        value={nextWords ? (nextWords.past ? "Overdue" : nextWords.text) : "None"}
        tone={nextWords?.tone === "neutral" ? undefined : nextWords?.tone}
        loading={!pendingKnown}
        foot={next ? (nextWords?.past ? `waiting for refresh · ${next.action}` : next.action) : pendingKnown ? "no request has a deadline" : undefined}
        title={next?.deadlineAtMs !== undefined ? `Deadline ${formatAbsoluteTime(next.deadlineAtMs)}` : undefined}
        className="gating-glance__deadline"
        testId="gating-glance:deadline"
      />
      <FigureTile
        label="Decided today"
        value={decidedToday.total}
        foot={decidedToday.total
          ? joinMeta(
            decidedToday.approve ? `${decidedToday.approve} approved` : null,
            decidedToday.reject ? `${decidedToday.reject} rejected` : null,
            decidedToday.escalate ? `${decidedToday.escalate} escalated` : null,
            decidedToday.timeout ? `${decidedToday.timeout} timed out` : null,
          )
          : "no decisions yet today"}
        title={auditCapped ? `Counted in the newest ${auditLimit} records` : undefined}
        testId="gating-glance:decided"
      />
      <FigureTile
        label="Risk mix"
        value={mix.total}
        unit={mix.total === 1 ? "action" : "actions"}
        meter={<RiskMix mix={mix} />}
        foot={RISK_TIERS.map((tier) => `${riskTierWord(tier)} ${mix[tier]}`).join(" · ")}
        title={RISK_TIERS.map((tier) => `${riskTierWord(tier)} risk (${tier.toUpperCase()}): ${mix[tier]}`).join(", ")}
        className="gating-glance__risk"
        testId="gating-glance:risk"
      />
    </GlanceStrip>
  );

  return (
    <PanelShell className="gating-inbox" testId="gating-panel" density={density} showTechnicalIds={showTechnicalIds}>
      <PanelHeader
        className="gating__head"
        title="Approvals"
        summary={summary}
        summaryTestId="gating-summary"
        freshness={updatedShown !== undefined && updatedShown !== null ? { at: updatedShown } : undefined}
        divider={false}
        tools={refreshShown ? (
          <IconButton label="Refresh approvals" glyph="refresh" data-testid="gating-refresh" onClick={refreshShown} />
        ) : undefined}
        menu={{
          label: "More actions for Approvals",
          triggerTestId: "gating-menu",
          items: standardPanelMenu({
            density,
            onDensityChange: setDensity,
            showTechnicalIds,
            onShowTechnicalIdsChange: setShowTechnicalIds,
            getCopyData: () => ({ pending: resource ? resource.requests.map((request) => request.raw) : pending, audit }),
            onRefresh: refreshShown,
            testIdPrefix: "gating",
          }),
        }}
      />
      {glance}
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
      <StatusRow mode="toast" error={errorShown} errorTestId="gating-audit-error" updatedAt={updatedShown ?? undefined} onRetry={refreshShown} />
      <PanelBody className="gating-inbox__body">
        <PanelTabPanel className="gating-inbox__content" ref={listRef} idBase={tabIds} tab={tab}>{content}</PanelTabPanel>
      </PanelBody>
    </PanelShell>
  );
}
