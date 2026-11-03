import React from "react";

// One row in the per-identity pending-message queue. Lives entirely
// in the browser until either auto-drain (busy to idle), explicit
// Steer, or Trash. Persisted across reloads and tabs in localStorage.
export interface PendingItem {
  id: string;
  text: string;
  addedAt: number;
  expanded?: boolean;
  editing?: boolean;
  // Durable delivery failure. Hosts keep the row until an explicit retry;
  // the shared UI must distinguish it from an ordinary idle queue head.
  manualRetryRequired?: boolean;
  // `null` = static; the others are transient animation flags driven
  // by CSS keyframes. Items in `promoting`/`trashing`/`draining` are
  // non-interactive and on their way out.
  status?: "entering" | "promoting" | "trashing" | "draining" | null;
}

export type PendingDropWhere = "above" | "below";

interface DropTarget {
  id: string | null;
  where: PendingDropWhere | null;
}

export interface PendingStackProps {
  items: PendingItem[];
  agentBusy: boolean;
  reducedMotion?: boolean;
  onSteer: (id: string) => void;
  onRetry?: ((id: string) => void) | null;
  onTrash: (id: string) => void;
  onEdit: (id: string) => void;
  onCommitEdit: (id: string, text: string) => void;
  onCancelEdit: (id: string) => void;
  onReorder: (dragId: string, dropId: string, where: PendingDropWhere) => void;
  onClearAll: () => void;
  onToggleExpand: (id: string) => void;
}

interface StackHeadProps {
  count: number;
  agentBusy: boolean;
  needsRetry: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onClear: () => void;
}

function StackHead({
  count,
  agentBusy,
  needsRetry,
  collapsed,
  onToggleCollapsed,
  onClear,
}: StackHeadProps): React.JSX.Element {
  return (
    <div className="cc-pending-stack__head">
      <button
        type="button"
        className="cc-pending-stack__head-btn"
        onClick={onToggleCollapsed}
        aria-expanded={!collapsed}
        aria-label={collapsed ? "Expand pending queue" : "Collapse pending queue"}
        title={collapsed ? "Expand queue" : "Collapse queue"}
      >
        <span className="cc-pending-stack__head-chev">{collapsed ? "▸" : "▾"}</span>
      </button>
      <span>Queue</span>
      <span className="cc-pending-stack__head-count">{String(count).padStart(2, "0")}</span>
      {!collapsed && count > 1 && (
        <span className="cc-pending-stack__head-hint">· drains top → bottom</span>
      )}
      <span className="cc-pending-stack__head-spacer" />
      <span className={`cc-pending-stack__head-phase ${needsRetry ? "is-needs-retry" : (agentBusy ? "" : "is-idle")}`}>
        <b />
        {needsRetry ? "Needs retry" : (agentBusy ? "Busy" : "Idle")}
      </span>
      {count > 0 && (
        <button
          type="button"
          className="cc-pending-stack__head-btn"
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
  isHead: boolean;
  dragging: boolean;
  dropHint: PendingDropWhere | null;
  onSteer: (id: string) => void;
  onRetry?: ((id: string) => void) | null;
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
  isHead,
  dragging,
  dropHint,
  onSteer,
  onRetry,
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

  const cls = [
    "cc-pending-item",
    isHead ? "is-head" : "",
    item.editing ? "is-editing" : "",
    item.status === "promoting" ? "is-promoting" : "",
    item.status === "trashing" ? "is-trashing" : "",
    item.status === "draining" ? "is-draining" : "",
    item.status === "entering" ? "is-entering" : "",
    item.manualRetryRequired ? "is-needs-retry" : "",
    dragging ? "is-dragging" : "",
    dropHint === "above" ? "drop-target drop-above" : "",
    dropHint === "below" ? "drop-target drop-below" : "",
  ].filter(Boolean).join(" ");

  const longText = item.text.length > 90 || /\n/.test(item.text);

  return (
    <li
      className={cls}
      role="listitem"
      tabIndex={0}
      data-id={item.id}
      data-testid={`pending-item:${item.id}`}
      draggable={!item.editing && item.status !== "promoting"}
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
      <div className="cc-pending-item__lead">
        <span className="cc-pending-item__grip" aria-label="Drag to reorder" title="Drag to reorder">
          <span /><span /><span /><span /><span /><span />
        </span>
        <span className="cc-pending-item__queue-glyph" aria-hidden="true">⤵</span>
      </div>

      {item.editing ? (
        <div className="cc-pending-item__edit">
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
          <div className="cc-pending-item__edit-row">
            <span><span className="cc-pending-kbd">Esc</span> cancel</span>
            <span><span className="cc-pending-kbd">↵</span> save · <span className="cc-pending-kbd">⇧↵</span> newline</span>
            <span className="cc-pending-item__edit-spacer" />
            <button
              type="button"
              className="cc-pending-btn"
              onClick={() => onCancelEdit(item.id)}
            >
              Cancel
            </button>
            <button
              type="button"
              className="cc-pending-btn cc-pending-btn--save"
              onClick={() => onCommitEdit(item.id, draft)}
            >
              Save
            </button>
          </div>
        </div>
      ) : (
        <div className="cc-pending-item__body">
          <div
            className={`cc-pending-item__text ${item.expanded ? "cc-pending-item__text--expanded" : ""}`}
            onClick={longText ? () => onToggleExpand(item.id) : undefined}
            style={longText ? { cursor: "pointer" } : undefined}
            title={longText && !item.expanded ? item.text : undefined}
          >
            {item.text}
          </div>
          <div className="cc-pending-item__meta">
            {item.manualRetryRequired ? (
              <span className="cc-pending-item__head-tag is-needs-retry">Needs retry</span>
            ) : (isHead ? <span className="cc-pending-item__head-tag">Next</span> : null)}
            <span>{timeAgo(item.addedAt)}</span>
            {item.status === "promoting" && (
              <span className="cc-pending-item__sending">SENDING…</span>
            )}
          </div>
        </div>
      )}

      {!item.editing && (
        <div className="cc-pending-item__actions">
          <button
            type="button"
            className="cc-pending-btn cc-pending-btn--steer"
            onClick={() => (item.manualRetryRequired ? (onRetry || onSteer)(item.id) : onSteer(item.id))}
            disabled={item.status === "promoting"}
            aria-label={item.manualRetryRequired
              ? "Retry delivery"
              : "Steer — send now and interrupt at next cooperative pause"}
            title={item.manualRetryRequired
              ? "Retry delivery"
              : "Send now and interrupt at the next cooperative pause"}
            data-testid={`pending-steer:${item.id}`}
          >
            <span className="cc-pending-btn__glyph">{item.manualRetryRequired ? "Retry" : "↪"}</span>
          </button>
          <button
            type="button"
            className="cc-pending-btn cc-pending-btn--icon"
            onClick={() => onEdit(item.id)}
            aria-label="Edit message"
            title="Edit message"
            data-testid={`pending-edit:${item.id}`}
          >
            <span className="cc-pending-btn__glyph">✎</span>
          </button>
          <button
            type="button"
            className="cc-pending-btn cc-pending-btn--icon cc-pending-btn--trash"
            onClick={() => onTrash(item.id)}
            aria-label="Remove from queue"
            title="Remove from queue"
            data-testid={`pending-trash:${item.id}`}
          >
            <span className="cc-pending-btn__glyph">×</span>
          </button>
        </div>
      )}
    </li>
  );
}

export function PendingStack({
  items,
  agentBusy,
  reducedMotion,
  onSteer,
  onRetry,
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

  // Auto-expand whenever the queue grows. Designer requirement: fresh
  // items pull the user's attention back to the queue.
  const lastCount = React.useRef(0);
  React.useEffect(() => {
    if (items.length > lastCount.current) setCollapsed(false);
    lastCount.current = items.length;
  }, [items.length]);

  if (items.length === 0) return null;

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
    const where: PendingDropWhere = (e.clientY - rect.top) < rect.height / 2 ? "above" : "below";
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
      className={`cc-pending-stack ${collapsed ? "is-collapsed" : ""} ${reducedMotion ? "reduced-motion" : ""}`}
      aria-label="Pending message queue"
      data-testid="pending-stack"
    >
      <StackHead
        count={items.length}
        agentBusy={agentBusy}
        needsRetry={items.some((item) => item.manualRetryRequired)}
        collapsed={collapsed}
        onToggleCollapsed={() => setCollapsed((c) => !c)}
        onClear={onClearAll}
      />
      <ol className="cc-pending-stack__list" role="list">
        {items.map((item, i) => (
          <StackItem
            key={item.id}
            item={item}
            isHead={i === 0}
            dragging={dragId === item.id}
            dropHint={dropTarget.id === item.id ? dropTarget.where : null}
            onSteer={onSteer}
            onRetry={onRetry}
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
