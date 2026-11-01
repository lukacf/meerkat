import * as React from "react";
import type { ApprovalAction, ApprovalDecisionState, PendingApproval, PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";
import type { HostAttentionItem, HostAttentionSourceConfig, HostDecideLink } from "../../../console-core/src/host-attention";
import {
  AgentAvatar,
  AgentName,
  Button,
  Chip,
  KeyValueList,
  KitGlyph,
  MiniBar,
  Notice,
  TechnicalId,
  agentDisplayName,
  countdownTone,
  cx,
  formatAbsoluteTime,
  formatCountdown,
  formatDuration,
  humanizeEnum,
  riskTierWord,
  riskTone,
  useAgentDirectory,
  useConsoleNavigation,
  useNow,
} from "../panel-kit";
import type { ButtonVariant, Tone } from "../panel-kit";
import { RowDetails } from "./presentation-policy";

const labels: Record<ApprovalAction, string> = { approve: "Approve", reject: "Reject", escalate: "Escalate" };
const variants: Record<ApprovalAction, ButtonVariant> = { approve: "positive", reject: "danger", escalate: "default" };

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

type ApprovalCardState = "pending" | "submitting" | "stale" | "failed" | "refused" | "unavailable" | "expired" | "settled";

const STATE_WORDS: Record<ApprovalCardState, { text: string; tone: Tone }> = {
  pending: { text: "Approval needed", tone: "warn" },
  submitting: { text: "Submitting decision", tone: "info" },
  stale: { text: "Approval state may be out of date", tone: "warn" },
  failed: { text: "Decision unconfirmed", tone: "crit" },
  refused: { text: "Decision refused", tone: "crit" },
  unavailable: { text: "Decision unavailable", tone: "neutral" },
  expired: { text: "Expired", tone: "neutral" },
  settled: { text: "Resolved", tone: "ok" },
};

const SECOND = 1000;
const MINUTE = 60 * SECOND;

/** The time now, from the shared 15 s clock, ticking every second while any
 * of `deadlines` is within two minutes (and the page is visible): a
 * deadline's last minutes count down second by second. */
export function useApprovalClock(deadlines: ReadonlyArray<number | undefined>): number {
  const shared = useNow();
  const near = deadlines.some((deadline) => deadline !== undefined && Math.abs(deadline - shared) < 2 * MINUTE + 15 * SECOND);
  const [fine, setFine] = React.useState<number | null>(null);
  React.useEffect(() => {
    if (!near || typeof window === "undefined") {
      setFine(null);
      return undefined;
    }
    let timer: ReturnType<typeof setInterval> | null = null;
    const start = () => {
      if (timer === null && !document.hidden) {
        setFine(Date.now());
        timer = setInterval(() => setFine(Date.now()), SECOND);
      }
    };
    const stop = () => {
      if (timer !== null) clearInterval(timer);
      timer = null;
    };
    const onVisibility = () => (document.hidden ? stop() : start());
    start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [near]);
  return near && fine !== null ? Math.max(fine, shared) : shared;
}

/** The deadline in words and its tone: "in 4m 12s" (by distance: neutral
 * over an hour, info, warn under 10 minutes), or, once it has passed while
 * the owner still lists the request, "Expired, waiting for refresh". */
export function deadlineWords(deadlineAtMs: number, now: number): { text: string; tone: Tone; past: boolean } {
  const remaining = deadlineAtMs - now;
  if (remaining <= 0) return { text: "Expired, waiting for refresh", tone: "crit", past: true };
  return { text: formatCountdown(remaining), tone: countdownTone(remaining), past: false };
}

/** The tone of the time-used bar: info, warn under 10 minutes left, crit
 * under 1 minute (and once past). */
function deadlineBarTone(remaining: number): Tone {
  if (remaining < MINUTE) return "crit";
  if (remaining < 10 * MINUTE) return "warn";
  return "info";
}

/** The deadline as a chip with the clock glyph (A3.5): its tone by distance,
 * the absolute time in the tooltip. */
export function DeadlineChip({ deadlineAtMs, now, testId, className }: {
  deadlineAtMs: number;
  now: number;
  testId?: string;
  className?: string;
}) {
  const words = deadlineWords(deadlineAtMs, now);
  return (
    <span className={cx("cc-countdown", "cc-countdown--chip", "cc-approval__deadline", className)} data-tone={words.tone} data-past={words.past ? "true" : undefined} data-testid={testId}>
      <KitGlyph name="clock" className="cc-countdown__glyph" />
      <time dateTime={new Date(deadlineAtMs).toISOString()} title={`Deadline ${formatAbsoluteTime(deadlineAtMs)}`}>
        {words.text}
      </time>
    </span>
  );
}

/** A risk tier for people: "High risk", with "Risk tier R3" as the tooltip.
 * A tier outside R0 to R3 reads as recorded. */
function riskChip(tier: string): { text: string; title: string } {
  const word = riskTierWord(tier);
  const code = /^r\d$/i.test(tier) ? tier.toUpperCase() : tier;
  return { text: word ? `${word} risk` : code, title: `Risk tier ${code}` };
}

/** Who asked and where: "Billing asked · in Billing's chat · Waiting 18 min".
 * The chat opens from here when the host can open agents. */
function AskedLine({ request, waiting, testId }: { request: PendingApproval; waiting: string; testId: string }) {
  const directory = useAgentDirectory();
  const navigation = useConsoleNavigation();
  const actor = request.actorId;
  const origin = request.origin?.identity;
  const originName = origin ? agentDisplayName(origin, directory) || origin : "";
  const openChat = origin && navigation?.openAgent ? () => navigation.openAgent?.(origin, "chat") : null;
  // A request from the actor's own chat names that chat once: "Open the
  // chat" when the host can open it, nothing more when it cannot.
  const ownChat = Boolean(origin && actor && origin === actor);
  if (!actor && !origin && !waiting) return null;
  return (
    <p className="cc-approval__asked" data-testid={testId}>
      {actor ? (
        <span className="cc-approval__asker">
          <AgentAvatar name={agentDisplayName(actor, directory) || actor} size={22} />
          <AgentName id={actor} link className="cc-approval__actor" />
          {" "}
          <span className="cc-approval__asked-verb">asked</span>
        </span>
      ) : null}
      {origin && ownChat && openChat ? (
        <span className="cc-approval__origin">
          <button type="button" className="cc-approval__origin-link" onClick={openChat} aria-label={`Open ${originName}'s chat`} title={`Open ${originName}'s chat`}>
            Open the chat
          </button>
        </span>
      ) : origin && !ownChat ? (
        <span className="cc-approval__origin">
          {openChat ? (
            <button type="button" className="cc-approval__origin-link" onClick={openChat} aria-label={`Open ${originName}'s chat`} title={`Open ${originName}'s chat`}>
              in <AgentName id={origin} />'s chat
            </button>
          ) : (
            <>in <AgentName id={origin} />'s chat</>
          )}
        </span>
      ) : null}
      {waiting ? <span className="cc-approval__waiting">{`Waiting ${waiting}`}</span> : null}
    </p>
  );
}

/** The same request and decision owner can feed an inbox and several panes. */
export function ApprovalCard({ request, resourceStatus, decision, readOnly = false, decideLinks, onDecide }: ApprovalCardProps) {
  const now = useApprovalClock([request.deadlineAtMs]);
  const pending = request.status === "pending" && decision?.phase !== "settled";
  const submitting = decision?.phase === "submitting";
  const stale = resourceStatus !== "ready";
  // A session that cannot decide shows the host's decide pages, never a dead Approve.
  const hostDecide = readOnly && pending && decideLinks?.length ? decideLinks : null;
  const state: ApprovalCardState = submitting ? "submitting"
    : decision?.phase === "failed" ? "failed"
      : request.status === "expired" ? "expired"
        : !pending ? "settled"
          : stale ? "stale"
            : decision?.phase === "refused" ? "refused"
              : decision?.phase === "unavailable" ? "unavailable"
                : "pending";
  const words = STATE_WORDS[state];
  const waiting = pending && request.createdAtMs !== undefined
    ? formatDuration(Math.max(0, now - request.createdAtMs))
    : "";
  // A request without its own label falls back to the action ID: read it as
  // words, with the exact ID in the tooltip (and in the request details).
  const titleIsId = request.action === request.actionId;
  const title = titleIsId ? humanizeEnum(request.actionId) || request.action : request.action;
  const risk = request.riskTier ? riskChip(request.riskTier) : null;
  // Escalation needs a route the owner knows; a request it says it cannot
  // escalate (or that it refused to) offers Escalate disabled, with the reason.
  const noEscalationRoute = request.escalation === "unavailable"
    || (decision?.phase === "refused" && decision.refusal?.kind === "no_escalation_route");
  const shownActions: ApprovalAction[] = noEscalationRoute && !request.actions.includes("escalate")
    ? [...request.actions, "escalate"]
    : [...request.actions];
  const deadline = pending && request.deadlineAtMs !== undefined ? request.deadlineAtMs : undefined;
  const span = deadline !== undefined && request.createdAtMs !== undefined && deadline > request.createdAtMs
    ? deadline - request.createdAtMs
    : null;
  const remaining = deadline !== undefined ? deadline - now : null;
  return (
    <article
      className="cc-approval"
      data-state={state}
      data-risk={request.riskTier || undefined}
      data-testid={`gating-pending:${request.pendingId}`}
    >
      <header className="cc-approval__header">
        <div className="cc-approval__heading">
          <strong className="cc-approval__title" title={titleIsId ? request.actionId : undefined}>
            {title}
          </strong>
          {risk ? (
            <Chip tone={riskTone(request.riskTier)} mark title={risk.title} className="cc-approval__risk">{risk.text}</Chip>
          ) : null}
        </div>
        {deadline !== undefined ? (
          <DeadlineChip deadlineAtMs={deadline} now={now} testId={`gating-countdown:${request.pendingId}`} />
        ) : null}
      </header>
      {span !== null && remaining !== null ? (
        <MiniBar
          className="cc-approval__time-used"
          value={Math.min(span, Math.max(0, now - request.createdAtMs!))}
          max={span}
          tone={deadlineBarTone(remaining)}
          label={remaining > 0
            ? `${formatDuration(remaining)} left of ${formatDuration(span)} to decide`
            : `The ${formatDuration(span)} to decide have passed`}
          testId={`gating-time-used:${request.pendingId}`}
        />
      ) : null}
      <AskedLine request={request} waiting={waiting} testId={`gating-asked:${request.pendingId}`} />
      {request.rationale ? <p className="cc-approval__rationale">{request.rationale}</p> : null}
      <KeyValueList
        className="cc-approval__scope"
        items={[
          { key: "approver", label: "Requested approver", value: request.requestedApprover || "Not specified" },
          request.approvalRecipient ? { key: "recipient", label: "Notification recipient", value: request.approvalRecipient } : null,
          request.approvalChannel ? { key: "channel", label: "Notification channel", value: request.approvalChannel } : null,
        ]}
      />
      {decision?.error ? (
        <Notice tone={decision.phase === "refused" ? "warn" : "crit"} role="alert">
          {decision.error}
          {decision.phase === "refused" && decision.refusal?.kind === "refused" && request.requestedApprover
            ? ` This request asks ${request.requestedApprover} to decide it.`
            : ""}
        </Notice>
      ) : null}
      {decision?.result?.next_pending_id ? (
        <Notice tone="info">
          Escalated to a new request <TechnicalId value={decision.result.next_pending_id} label="Request ID" />
        </Notice>
      ) : null}
      {readOnly && !hostDecide ? <Notice tone="locked">Read-only access</Notice> : null}
      {resourceStatus === "forbidden" ? <Notice tone="locked">Approval access denied</Notice> : null}
      <footer className="cc-approval__footer">
        <span className="cc-approval__status" role="status">
          <Chip tone={words.tone} mark variant={state === "pending" ? "quiet" : undefined}>{words.text}</Chip>
        </span>
        {hostDecide ? <div className="cc-approval__actions">{hostDecide.map((link) => (
          <a
            key={link.sourceId}
            className="cc-btn cc-host-attention__open"
            href={link.href}
            target="_top"
            aria-label={`Decide in ${link.label}: ${title}`}
            data-testid={`gating-decide-link:${request.pendingId}:${link.sourceId}`}
          >
            Decide in {link.label}
          </a>
        ))}</div> : pending ? <div className="cc-approval__actions">{shownActions.map((action) => {
          const noRoute = action === "escalate" && noEscalationRoute;
          return (
            <Button
              key={action}
              variant={variants[action]}
              disabled={readOnly || stale || submitting || noRoute}
              title={noRoute ? "No escalation route" : undefined}
              data-action={action}
              data-testid={`gating-action:${request.pendingId}:${action}`}
              onClick={() => { void onDecide(request.pendingId, action); }}
            >
              {labels[action]}
            </Button>
          );
        })}
          {noEscalationRoute ? <span className="cc-approval__hint" data-testid={`gating-no-escalation:${request.pendingId}`}>No escalation route</span> : null}
        </div> : null}
        <RowDetails part={`approval-details:${request.pendingId}`} className="cc-approval__details cc-tech">
          <summary className="cc-tech__summary">Complete request details</summary>
          <div className="cc-tech__body">
            <KeyValueList
              items={[
                { key: "request", label: "Request", value: <code>{request.pendingId}</code> },
                { key: "scope", label: "Action scope", value: <code>{request.actionId}</code> },
                request.riskTier ? { key: "risk", label: "Risk tier", value: <code>{request.riskTier}</code> } : null,
                request.deadlineAtMs !== undefined ? { key: "deadline", label: "Deadline", value: formatAbsoluteTime(request.deadlineAtMs) } : null,
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
      </footer>
    </article>
  );
}

/** A host-provided attention item, in the approval card's family: its title,
 * its source as a chip, the host's details and one Open link drawn as a
 * console button. It has no status and no decision: the console never
 * decides it, and Open loads the host's own page top-level, which does. */
export function HostAttentionCard({ item }: { item: HostAttentionItem }) {
  // The chip cuts a long source label short; its tooltip keeps the whole one.
  const source = item.sourceLabel ? `From ${item.sourceLabel}` : "";
  return (
    <article
      className="cc-approval cc-host-attention"
      data-kind="host"
      data-tone={item.tone || "attention"}
      data-testid={`host-attention:${item.sourceId}:${item.id}`}
    >
      <header className="cc-approval__header">
        <div className="cc-approval__heading">
          <strong className="cc-approval__title">{item.title}</strong>
        </div>
        {source ? <Chip className="cc-host-attention__source" title={source}>{source}</Chip> : null}
      </header>
      <KeyValueList
        className="cc-approval__scope"
        items={item.details.map((detail, index) => ({ key: `${detail.label}:${index}`, label: detail.label, value: detail.value }))}
      />
      <div className="cc-approval__actions">
        <a
          className="cc-btn cc-host-attention__open"
          href={item.href}
          target="_top"
          aria-label={`Open: ${item.title}`}
          data-testid={`host-attention-open:${item.sourceId}:${item.id}`}
        >
          Open
        </a>
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
  // Approvals count only where the connection can show them; host items
  // count on any connection, so an aggregate console still names them.
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
