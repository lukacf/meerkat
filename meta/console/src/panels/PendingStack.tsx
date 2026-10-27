import React from "react";
import { QuoteContextChips } from "../../../packages/console-components/src/conversation/context-chips";
import { describeConsolePendingRow, type ConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { Icon } from "../icon";

/// One row in the per-identity pending-message stack. Lives entirely
/// in the browser until either auto-drain (busy → idle), explicit
/// Steer, or Trash. Persisted across reloads + tabs in localStorage.
export interface PendingItem extends ConsoleSendAttempt {
  expanded?: boolean;
  editing?: boolean;
  /** Written by consoles up to 0.8.44; never rendered (see PendingCheckView). */
  checkResult?: string;
  status?: "entering" | "promoting" | "trashing" | "draining" | null;
}

/// View state of an explicit Check on one row. It is not part of the saved
/// attempt: a check proves nothing about the send unless it finds it, and
/// then the row clears. Every check starts at `checking` and ends in a
/// `result` (or the row clears), so a click always answers visibly.
export type PendingCheckView = { phase: "checking" } | { phase: "result"; text: string };

/// "Delivered at <time>" for a row a Check found and cleared.
export interface PendingDeliveredNotice {
  id: string;
  text: string;
}

type DropWhere = "above" | "below";

/// How long a normal send may stay in flight before its row appears.
/// A send moves its row to `attempting` and, on the acceptance receipt, to
/// `accepted` and out of the stack, usually within a fraction of a second;
/// rendering the row in between flashed "Awaiting acceptance" on every
/// message. A row still `attempting` (or `accepted` but not yet removed)
/// this long after this view first saw it attempting shows the unchanged
/// needs-acceptance UI. Display only: it never decides an outcome, and
/// settled failures (`definitely-rejected`, `outcome-unknown`) render
/// immediately.
export const ACCEPTANCE_NOTICE_GRACE_MS = 2_000;

/// Rows of a send still inside its acceptance grace window, keyed by item
/// id and timed from when this view first saw the row `attempting`.
/// A normal send first saves its row as a `draft` and dispatches it right
/// after; `directSendIds` names those rows, so the saved draft does not
/// flash "Queued" either. A draft queued behind a busy agent is not in the
/// set and renders at once.
export function acceptanceGraceDeadlines(
  items: readonly PendingItem[],
  firstSeenAttempting: Map<string, number>,
  now: number,
  directSendIds: ReadonlySet<string> = new Set(),
): Map<string, number> {
  const live = new Set(items.map((item) => item.id));
  for (const id of [...firstSeenAttempting.keys()]) {
    if (!live.has(id)) firstSeenAttempting.delete(id);
  }
  const deadlines = new Map<string, number>();
  for (const item of items) {
    const directDraft = item.state === "draft" && directSendIds.has(item.id);
    if ((item.state === "attempting" || directDraft) && !firstSeenAttempting.has(item.id)) {
      firstSeenAttempting.set(item.id, now);
    }
    if (item.state !== "attempting" && item.state !== "accepted" && !directDraft) continue;
    const seen = firstSeenAttempting.get(item.id);
    if (seen === undefined) continue;
    const deadline = seen + ACCEPTANCE_NOTICE_GRACE_MS;
    if (deadline > now) deadlines.set(item.id, deadline);
  }
  return deadlines;
}

interface DropTarget {
  id: string | null;
  where: DropWhere | null;
}

interface PendingStackProps {
  items: PendingItem[];
  /** Destination agent's display name, for the row wording. */
  agentLabel: string;
  /** Embedding host's display name, if the console has one. */
  hostLabel?: string;
  /**
   * The gateway keeps send dedupe records across restarts, so an uncertain
   * row may be sent again with its original idempotency key. False or
   * absent: uncertain rows offer Check and Discard only.
   */
  resendUncertain?: boolean;
  /** Explicit Check state per row id. */
  checks?: Readonly<Record<string, PendingCheckView>>;
  /** Rows a Check found (and cleared) since this view mounted. */
  delivered?: readonly PendingDeliveredNotice[];
  onDismissDelivered?: (id: string) => void;
  /** Rows the composer is sending right away (see acceptanceGraceDeadlines). */
  directSendIds?: ReadonlySet<string>;
  agentBusy: boolean;
  reducedMotion?: boolean;
  onSteer: (id: string) => void;
  onRetry: (id: string) => void;
  onReconcile: (id: string) => void;
  onRemoveContext: (id: string, contextId: string) => void;
  onEditContext?: (id: string, contextId: string, quote: string) => void | Promise<void>;
  onReorderContext: (id: string, contextId: string, direction: "up" | "down") => void;
  onTrash: (id: string) => void;
  onEdit: (id: string) => void;
  onCommitEdit: (id: string, text: string) => void;
  onCancelEdit: (id: string) => void;
  onReorder: (dragId: string, dropId: string, where: DropWhere) => void;
  onClearAll: () => void;
  onToggleExpand: (id: string) => void;
}

interface StackHeadProps {
  count: number;
  agentBusy: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onClear: () => void;
}

function StackHead({
  count,
  agentBusy,
  collapsed,
  onToggleCollapsed,
  onClear,
}: StackHeadProps): React.JSX.Element {
  return (
    <div className="stack__head">
      <button
        type="button"
        className="stack__head-btn"
        onClick={onToggleCollapsed}
        aria-expanded={!collapsed}
        aria-label={collapsed ? "Expand pending queue" : "Collapse pending queue"}
        title={collapsed ? "Expand queue" : "Collapse queue"}
      >
        <span className="stack__head-chev">{collapsed ? "▸" : "▾"}</span>
      </button>
      <span>Queue</span>
      <span className="stack__head-count">{count}</span>
      {!collapsed && count > 1 && (
        <span className="stack__head-hint">· drains top → bottom</span>
      )}
      <span className="stack__head-spacer" />
      <span className={`stack__head-phase ${agentBusy ? "" : "is-idle"}`}>
        <b />
        {agentBusy ? "Agent busy" : "Agent idle"}
      </span>
      {count > 0 && (
        <button
          type="button"
          className="stack__head-btn"
          onClick={onClear}
          aria-label="Clear all queued messages"
          title="Clear all"
        >
          Clear
        </button>
      )}
    </div>
  );
}

interface StackItemProps {
  item: PendingItem;
  agentLabel: string;
  hostLabel?: string;
  resendUncertain: boolean;
  check?: PendingCheckView;
  isHead: boolean;
  dragging: boolean;
  dropHint: DropWhere | null;
  onSteer: (id: string) => void;
  onRetry: (id: string) => void;
  onReconcile: (id: string) => void;
  onRemoveContext: (id: string, contextId: string) => void;
  onEditContext?: (id: string, contextId: string, quote: string) => void | Promise<void>;
  onReorderContext: (id: string, contextId: string, direction: "up" | "down") => void;
  onTrash: (id: string) => void;
  onEdit: (id: string) => void;
  onCommitEdit: (id: string, text: string) => void;
  onCancelEdit: (id: string) => void;
  onToggleExpand: (id: string) => void;
  onDragStart: (e: React.DragEvent<HTMLLIElement>, id: string) => void;
  onDragOver: (e: React.DragEvent<HTMLLIElement>, id: string) => void;
  onDragLeave: (e: React.DragEvent<HTMLLIElement>, id: string) => void;
  onDrop: (e: React.DragEvent<HTMLLIElement>, id: string) => void;
  onDragEnd: () => void;
}

function timeAgo(ts: number): string {
  const s = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (s < 5) return "just now";
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h`;
}

function StackItem({
  item,
  agentLabel,
  hostLabel,
  resendUncertain,
  check,
  isHead,
  dragging,
  dropHint,
  onSteer,
  onRetry,
  onReconcile,
  onRemoveContext,
  onEditContext,
  onReorderContext,
  onTrash,
  onEdit,
  onCommitEdit,
  onCancelEdit,
  onToggleExpand,
  onDragStart,
  onDragOver,
  onDragLeave,
  onDrop,
  onDragEnd,
}: StackItemProps): React.JSX.Element {
  const taRef = React.useRef<HTMLTextAreaElement | null>(null);
  const [draft, setDraft] = React.useState(item.text);

  React.useEffect(() => {
    if (item.editing && taRef.current) {
      taRef.current.focus();
      const len = taRef.current.value.length;
      taRef.current.setSelectionRange(len, len);
      taRef.current.style.height = "auto";
      taRef.current.style.height = taRef.current.scrollHeight + "px";
    }
  }, [item.editing]);

  React.useEffect(() => {
    setDraft(item.text);
  }, [item.text, item.editing]);

  const handleEditKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancelEdit(item.id);
    } else if (e.key === "Enter" && (!e.shiftKey || e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      onCommitEdit(item.id, draft);
    }
  };

  const isDraft = item.state === "draft";
  // Plain wording from the typed state and failure kind only; a stored
  // error string (including untyped ones saved by older consoles) is never
  // shown.
  const copy = describeConsolePendingRow(item, { agent: agentLabel, host: hostLabel });
  const needsAcceptance = item.state === "outcome-unknown" || item.state === "attempting";
  const settledFailure = item.state === "outcome-unknown" || item.state === "definitely-rejected";
  // A refused row was never admitted, so sending it again is always safe.
  // An uncertain row may already be admitted: resending it is safe only
  // when the gateway's dedupe record for its idempotency key is durable.
  const canResend = item.state === "definitely-rejected" || (item.state === "outcome-unknown" && resendUncertain);
  const checking = check?.phase === "checking";
  const previewId = React.useId();

  const cls = [
    "stk-item",
    !isDraft ? "is-frozen" : "",
    needsAcceptance ? "needs-acceptance" : "",
    isHead ? "is-head" : "",
    item.editing ? "is-editing" : "",
    item.status === "promoting" ? "is-promoting" : "",
    item.status === "trashing" ? "is-trashing" : "",
    item.status === "draining" ? "is-draining" : "",
    item.status === "entering" ? "is-entering" : "",
    dragging ? "is-dragging" : "",
    dropHint === "above" ? "drop-target drop-above" : "",
    dropHint === "below" ? "drop-target drop-below" : "",
  ].filter(Boolean).join(" ");

  const longText = item.contexts.length > 0 || item.text.length > 90 || /\n/.test(item.text);

  return (
    <li
      className={cls}
      role="listitem"
      tabIndex={0}
      data-id={item.id}
      data-testid={`pending-item:${item.id}`}
      draggable={!item.editing && item.state === "draft"}
      onDragStart={(e) => onDragStart(e, item.id)}
      onDragOver={(e) => onDragOver(e, item.id)}
      onDragLeave={(e) => onDragLeave(e, item.id)}
      onDrop={(e) => onDrop(e, item.id)}
      onDragEnd={onDragEnd}
      onKeyDown={(e) => {
        if (item.editing) return;
        if ((e.key === "Delete" || e.key === "Backspace") && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          onTrash(item.id);
        }
      }}
    >
      <div className="stk-item__lead">
        {isDraft && <span className="stk-item__grip" aria-label="Drag to reorder" title="Drag to reorder">
          <span /><span /><span /><span /><span /><span />
        </span>}
        <span className="stk-item__queue-glyph" aria-hidden="true"><Icon name={settledFailure ? "i-info" : "i-clock"} /></span>
      </div>

      {item.editing ? (
        <div className="stk-item__edit">
          <textarea
            ref={taRef}
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              const el = e.target;
              el.style.height = "auto";
              el.style.height = el.scrollHeight + "px";
            }}
            onKeyDown={handleEditKey}
            placeholder="Rewrite this message…"
            data-testid={`pending-item-edit:${item.id}`}
          />
          <div className="stk-item__edit-row">
            <span><span className="stk-kbd">Esc</span> cancel</span>
            <span><span className="stk-kbd">↵</span> save · <span className="stk-kbd">⇧↵</span> newline</span>
            <span className="stk-item__edit-spacer" />
            <button
              type="button"
              className="stk-btn"
              onClick={() => onCancelEdit(item.id)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="stk-btn stk-btn--save"
              onClick={() => onCommitEdit(item.id, draft)}
            >
              Save
            </button>
          </div>
        </div>
      ) : (
        <div className="stk-item__body">
          <div className="stk-item__meta">
            {isHead && isDraft && <span className="stk-item__head-tag">Next</span>}
            <span className="stk-item__state" role={needsAcceptance ? "status" : undefined}>{copy.label}</span>
            {item.contexts.length > 0 && <span>{item.contexts.length} {item.contexts.length === 1 ? "quote" : "quotes"}</span>}
            <span className="stk-item__age">{timeAgo(item.addedAt)}</span>
            {item.status === "promoting" && <span className="stk-item__sending">Sending...</span>}
          </div>
          <div
            id={previewId}
            className={`stk-item__text ${longText && !item.expanded ? "stk-item__text--preview" : "stk-item__text--expanded"}`}
            onClick={longText ? () => onToggleExpand(item.id) : undefined}
            style={longText ? { cursor: "pointer" } : undefined}
            title={longText && !item.expanded ? item.text : undefined}
          >
            {item.text}
          </div>
          {item.expanded && <QuoteContextChips records={item.contexts} destinationLabel={item.destination}
            onEdit={item.state === "draft" && !item.envelopeJson && onEditContext ? (contextId, quote) => onEditContext(item.id, contextId, quote) : undefined}
            onRemove={item.state === "draft" ? (contextId) => onRemoveContext(item.id, contextId) : undefined}
            onReorder={item.state === "draft" ? (contextId, direction) => onReorderContext(item.id, contextId, direction) : undefined} />}
          {copy.title && <p className="stk-item__title" data-testid={`pending-title:${item.id}`}>{copy.title}</p>}
          {copy.detail && <p className="stk-item__explanation" data-testid={`pending-explanation:${item.id}`}>{copy.detail}</p>}
          {check && <p className="stk-item__explanation stk-item__check" role="status" data-testid={`pending-check:${item.id}`}>{check.phase === "checking" ? "Checking..." : check.text}</p>}
        </div>
      )}

      {!item.editing && (
        <div className="stk-item__actions">
          {needsAcceptance && <button type="button" className="stk-btn stk-btn--primary" disabled={checking} onClick={() => onReconcile(item.id)}>Check</button>}
          {canResend && <button type="button" className="stk-btn" disabled={checking} onClick={() => onRetry(item.id)}>Send again</button>}
          {longText && <button type="button" className="stk-btn stk-btn--expand" aria-expanded={Boolean(item.expanded)} aria-controls={previewId} onClick={() => onToggleExpand(item.id)}>{item.expanded ? "Hide saved message" : "Show saved message"}</button>}
          {isDraft && <button
            type="button"
            className="stk-btn stk-btn--steer"
            onClick={() => onSteer(item.id)}
            aria-label="Steer - send now and interrupt at next cooperative pause"
            title="Send now and interrupt at the next cooperative pause"
            data-testid={`pending-steer:${item.id}`}
          >
            <span className="stk-btn__glyph" aria-hidden="true"><Icon name="i-bolt" /></span> Steer
          </button>}
          {isDraft && <button
            type="button"
            className="stk-btn stk-btn--icon"
            onClick={() => onEdit(item.id)}
            aria-label="Edit message"
            title="Edit message"
            data-testid={`pending-edit:${item.id}`}
          >
            <span className="stk-btn__glyph" aria-hidden="true"><Icon name="i-compose" /></span>
          </button>}
          {isDraft ? <button
            type="button"
            className="stk-btn stk-btn--icon stk-btn--trash"
            onClick={() => onTrash(item.id)}
            aria-label="Remove from queue"
            title="Remove from queue"
            data-testid={`pending-trash:${item.id}`}
          >
            <span className="stk-btn__glyph" aria-hidden="true"><Icon name="i-close" /></span>
          </button> : <button
            type="button"
            className="stk-btn stk-btn--trash"
            onClick={() => onTrash(item.id)}
            title="Discard this message"
            data-testid={`pending-trash:${item.id}`}
          >Discard</button>}
        </div>
      )}
    </li>
  );
}

export function PendingStack({
  items,
  agentLabel,
  hostLabel,
  resendUncertain = false,
  checks,
  delivered,
  onDismissDelivered,
  directSendIds,
  agentBusy,
  reducedMotion,
  onSteer,
  onRetry,
  onReconcile,
  onRemoveContext,
  onEditContext,
  onReorderContext,
  onTrash,
  onEdit,
  onCommitEdit,
  onCancelEdit,
  onReorder,
  onClearAll,
  onToggleExpand,
}: PendingStackProps): React.JSX.Element | null {
  // Tick once a minute so "X seconds ago" labels stay fresh without
  // forcing the parent to re-render on every tick.
  const [, setTick] = React.useState(0);
  React.useEffect(() => {
    const t = window.setInterval(() => setTick((n) => n + 1), 10_000);
    return () => window.clearInterval(t);
  }, []);

  const [dragId, setDragId] = React.useState<string | null>(null);
  const [dropTarget, setDropTarget] = React.useState<DropTarget>({ id: null, where: null });
  const [collapsed, setCollapsed] = React.useState(false);

  // A send in flight stays out of the stack for ACCEPTANCE_NOTICE_GRACE_MS
  // (see there); one timer re-renders at the earliest deadline and is
  // cleared on every change and on unmount.
  const firstSeenAttempting = React.useRef(new Map<string, number>());
  const [, revealTick] = React.useReducer((n: number) => n + 1, 0);
  const graceDeadlines = acceptanceGraceDeadlines(items, firstSeenAttempting.current, Date.now(), directSendIds);
  const nextReveal = graceDeadlines.size > 0 ? Math.min(...graceDeadlines.values()) : null;
  React.useEffect(() => {
    if (nextReveal === null) return undefined;
    const timer = window.setTimeout(revealTick, Math.max(0, nextReveal - Date.now()));
    return () => window.clearTimeout(timer);
  }, [nextReveal]);
  const visibleItems = graceDeadlines.size > 0 ? items.filter((item) => !graceDeadlines.has(item.id)) : items;

  // Auto-expand whenever the stack grows. Designer requirement: fresh
  // items pull the user's attention back to the queue.
  const lastCount = React.useRef(0);
  React.useEffect(() => {
    if (visibleItems.length > lastCount.current) setCollapsed(false);
    lastCount.current = visibleItems.length;
  }, [visibleItems.length]);

  const notices = delivered ?? [];
  if (visibleItems.length === 0 && notices.length === 0) return null;

  const onDragStart = (e: React.DragEvent<HTMLLIElement>, id: string) => {
    setDragId(id);
    try { e.dataTransfer.setData("text/plain", String(id)); } catch { /* ignore */ }
    e.dataTransfer.effectAllowed = "move";
  };
  const onDragOver = (e: React.DragEvent<HTMLLIElement>, id: string) => {
    if (dragId == null || dragId === id) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const rect = e.currentTarget.getBoundingClientRect();
    const where: DropWhere = (e.clientY - rect.top) < rect.height / 2 ? "above" : "below";
    setDropTarget((dt) => (dt.id === id && dt.where === where ? dt : { id, where }));
  };
  const onDragLeave = (e: React.DragEvent<HTMLLIElement>, id: string) => {
    if (dropTarget.id === id) {
      const related = e.relatedTarget as Node | null;
      if (!related || !e.currentTarget.contains(related)) {
        setDropTarget({ id: null, where: null });
      }
    }
  };
  const onDrop = (e: React.DragEvent<HTMLLIElement>, id: string) => {
    e.preventDefault();
    if (dragId == null || dragId === id) return;
    onReorder(dragId, id, dropTarget.where || "above");
    setDragId(null);
    setDropTarget({ id: null, where: null });
  };
  const onDragEnd = () => {
    setDragId(null);
    setDropTarget({ id: null, where: null });
  };

  return (
    <section
      className={`stack stack--console ${collapsed ? "is-collapsed" : ""} ${reducedMotion ? "reduced-motion" : ""}`}
      aria-label="Pending message queue"
      data-testid="pending-stack"
    >
      {visibleItems.length > 0 && <StackHead
        count={visibleItems.length}
        agentBusy={agentBusy}
        collapsed={collapsed}
        onToggleCollapsed={() => setCollapsed((c) => !c)}
        onClear={onClearAll}
      />}
      {notices.length > 0 && <ul className="stack__delivered" aria-label="Delivered messages">
        {notices.map((notice) => <li key={notice.id} className="stack__delivered-item" role="status" data-testid={`pending-delivered:${notice.id}`}>
          {notice.text}
          {onDismissDelivered && <button type="button" className="stk-btn stk-btn--icon" aria-label="Dismiss" onClick={() => onDismissDelivered(notice.id)}>
            <span className="stk-btn__glyph" aria-hidden="true"><Icon name="i-close" /></span>
          </button>}
        </li>)}
      </ul>}
      <ol className="stack__list" role="list">
        {visibleItems.map((item, i) => (
          <StackItem
            key={item.id}
            item={item}
            agentLabel={agentLabel}
            hostLabel={hostLabel}
            resendUncertain={resendUncertain}
            check={checks?.[item.id]}
            isHead={i === 0}
            dragging={dragId === item.id}
            dropHint={dropTarget.id === item.id ? dropTarget.where : null}
            onSteer={onSteer}
            onRetry={onRetry}
            onReconcile={onReconcile}
            onRemoveContext={onRemoveContext}
            onEditContext={onEditContext}
            onReorderContext={onReorderContext}
            onTrash={onTrash}
            onEdit={onEdit}
            onCommitEdit={onCommitEdit}
            onCancelEdit={onCancelEdit}
            onToggleExpand={onToggleExpand}
            onDragStart={onDragStart}
            onDragOver={onDragOver}
            onDragLeave={onDragLeave}
            onDrop={onDrop}
            onDragEnd={onDragEnd}
          />
        ))}
      </ol>
    </section>
  );
}
