import React, { useState } from "react";

import type {
  ConversationWorkGraphAttentionRow,
  ConversationWorkGraphEntry,
  ConversationWorkGraphItemRow,
  WorkGraphCardStatus,
} from "@console-core";

import type { IconRenderer } from "../shared";

const CARD_STATUS_LABEL: Record<WorkGraphCardStatus, string> = {
  active: "Active",
  blocked: "Blocked",
  completed: "Done",
  failed: "Failed",
  mixed: "Mixed",
};

const ITEM_STATUS_LABEL: Record<string, string> = {
  open: "Open",
  in_progress: "In progress",
  blocked: "Blocked",
  completed: "Done",
  cancelled: "Cancelled",
  failed: "Failed",
};

// Operator action callbacks. Buttons render only for callbacks that are
// provided (the undefined-handler convention) — a read-only console passes
// no actions and gets a purely observational card. Every payload carries the
// latest observed revision: mutations are CAS-guarded upstream. Two CAS
// classes: goal actions (confirm / request-close) CAS against the goal WORK
// ITEM's revision; attention actions (pause / resume / reassign) CAS against
// the binding's machine revision. An absent revision means the card never
// observed one — the handler must resolve the live revision before sending
// (never substitute 0).
export interface WorkGraphCardActions {
  onClaim?: (input: { itemId: string; revision?: number }) => void;
  onClose?: (input: { itemId: string; revision?: number }) => void;
  onGoalConfirm?: (input: { bindingId: string; revision?: number }) => void;
  onGoalRequestClose?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionPause?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionResume?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionReassign?: (input: { bindingId: string; revision?: number }) => void;
}

// UI state (item detail expansion, card collapse) must survive the card's
// entry-id migration: when a loose item grows a hierarchy the timeline entry
// rekeys (catch-all → rooted) and React remounts the subtree, resetting
// component-local state. Item ids are stable ULIDs and the entry carries a
// stable `uiStateKey`, so a module-level registry keyed by them carries the
// state across the remount. Only `true` values are stored, so the maps stay
// tiny and self-pruning.
const expandedWorkGraphItems = new Set<string>();
const collapsedWorkGraphCards = new Set<string>();

function rememberFlag(registry: Set<string>, key: string, value: boolean): void {
  if (value) registry.add(key);
  else registry.delete(key);
}

export const __workGraphCardUiState = {
  reset(): void {
    expandedWorkGraphItems.clear();
    collapsedWorkGraphCards.clear();
  },
};

function itemStatusLabel(status: string): string {
  return ITEM_STATUS_LABEL[status] || status.replace(/_/g, " ");
}

function formatDay(iso: string | null | undefined): string {
  return typeof iso === "string" && iso.length >= 10 ? iso.slice(0, 10) : "";
}

function formatClock(iso: string | null | undefined): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const hh = String(date.getHours()).padStart(2, "0");
  const mm = String(date.getMinutes()).padStart(2, "0");
  return `${hh}:${mm}`;
}

function itemRowHasDetail(row: ConversationWorkGraphItemRow): boolean {
  return Boolean(
    row.description
    || (row.evidence && row.evidence.length > 0)
    || (row.labels && row.labels.length > 0)
    || (row.alsoUnder && row.alsoUnder.length > 0)
    || row.createdAt
    || row.updatedAt,
  );
}

function ItemRow({
  row,
  actions,
}: {
  row: ConversationWorkGraphItemRow;
  actions?: WorkGraphCardActions | null;
}) {
  const [expanded, setExpandedState] = useState(() => expandedWorkGraphItems.has(row.itemId));
  const setExpanded = (update: (value: boolean) => boolean) => {
    setExpandedState((value) => {
      const next = update(value);
      rememberFlag(expandedWorkGraphItems, row.itemId, next);
      return next;
    });
  };
  const hasDetail = itemRowHasDetail(row);
  const terminal = row.status === "completed" || row.status === "cancelled" || row.status === "failed";
  const canClaim = Boolean(actions?.onClaim) && row.status === "open" && !row.ownerLabel;
  const canClose = Boolean(actions?.onClose) && !terminal && row.status !== "blocked";
  const dueDay = formatDay(row.dueAt);

  return (
    <li
      className={`cc-work-graph__item is-${row.status}${expanded ? " is-expanded" : ""}`}
      data-workgraph-item={row.itemId}
      data-item-status={row.status}
      data-revision={row.revision}
    >
      <div className="cc-work-graph__item-line" style={{ paddingLeft: `${row.depth * 18}px` }}>
        <button
          type="button"
          className="cc-work-graph__item-row"
          disabled={!hasDetail}
          aria-expanded={hasDetail ? expanded : undefined}
          onClick={hasDetail ? () => setExpanded((value) => !value) : undefined}
          data-testid={`workgraph-item:${row.itemId}`}
        >
          <span className={`cc-work-graph__dot is-${row.status}`} aria-hidden="true" />
          <span className="cc-work-graph__item-title">{row.title}</span>
          {row.priority && row.priority !== "medium" ? (
            <span className={`cc-work-graph__chip is-priority-${row.priority}`}>{row.priority}</span>
          ) : null}
          {row.ownerLabel ? (
            <span className="cc-work-graph__chip is-owner" title={`Owned by ${row.ownerLabel}`}>{row.ownerLabel}</span>
          ) : null}
          {row.blocked ? <span className="cc-work-graph__chip is-blocked">blocked</span> : null}
          {dueDay ? <span className="cc-work-graph__chip is-due" title="Due date">{dueDay}</span> : null}
          {hasDetail ? (
            <span className="cc-work-graph__item-chevron" aria-hidden="true">{expanded ? "▾" : "▸"}</span>
          ) : (
            <span className="cc-work-graph__item-status">{itemStatusLabel(row.status)}</span>
          )}
        </button>
        {canClaim ? (
          <button
            type="button"
            className="cc-work-graph__action"
            title={`Claim ${row.title}`}
            data-testid={`workgraph-action:${row.itemId}:claim`}
            onClick={(event) => {
              event.stopPropagation();
              actions?.onClaim?.({ itemId: row.itemId, revision: row.revision });
            }}
          >
            Claim
          </button>
        ) : null}
        {canClose ? (
          <button
            type="button"
            className="cc-work-graph__action"
            title={`Close ${row.title} as completed`}
            data-testid={`workgraph-action:${row.itemId}:close`}
            onClick={(event) => {
              event.stopPropagation();
              actions?.onClose?.({ itemId: row.itemId, revision: row.revision });
            }}
          >
            Done
          </button>
        ) : null}
      </div>
      {hasDetail && expanded ? (
        <div className="cc-work-graph__item-detail" style={{ marginLeft: `${row.depth * 18 + 25}px` }}>
          {row.description ? <p className="cc-work-graph__item-description">{row.description}</p> : null}
          {row.alsoUnder && row.alsoUnder.length > 0 ? (
            <p
              className="cc-work-graph__item-also-under"
              data-testid={`workgraph-item:${row.itemId}:also-under`}
            >
              also under {row.alsoUnder.join(", ")}
            </p>
          ) : null}
          {row.labels && row.labels.length > 0 ? (
            <div className="cc-work-graph__item-labels">
              {row.labels.map((label) => (
                <span className="cc-work-graph__chip is-label" key={label}>{label}</span>
              ))}
            </div>
          ) : null}
          {row.evidence && row.evidence.length > 0 ? (
            <ul className="cc-work-graph__evidence">
              {row.evidence.map((line, index) => (
                <li key={`${line}-${index}`}>{line}</li>
              ))}
            </ul>
          ) : null}
          <div className="cc-work-graph__item-meta">
            <span>{itemStatusLabel(row.status)}</span>
            {typeof row.revision === "number" ? <span>rev {row.revision}</span> : null}
            {row.updatedAt ? <span>updated {formatDay(row.updatedAt)} {formatClock(row.updatedAt)}</span> : null}
          </div>
        </div>
      ) : null}
    </li>
  );
}

function attentionIsPaused(row: ConversationWorkGraphAttentionRow): boolean {
  return row.statusLabel.startsWith("paused");
}

function attentionIsActive(row: ConversationWorkGraphAttentionRow): boolean {
  return row.statusLabel === "active";
}

function AttentionRow({
  row,
  goalRevision,
  actions,
}: {
  row: ConversationWorkGraphAttentionRow;
  // Latest observed revision of the bound goal WORK ITEM — goal confirm /
  // request-close CAS against it, not against the binding revision.
  goalRevision?: number;
  actions?: WorkGraphCardActions | null;
}) {
  const bindingInput = { bindingId: row.bindingId, revision: row.revision };
  const goalInput = { bindingId: row.bindingId, revision: goalRevision };
  const live = attentionIsActive(row) || attentionIsPaused(row);
  const buttons: Array<{ key: string; label: string; title: string; onClick: () => void }> = [];
  if (actions?.onAttentionPause && attentionIsActive(row)) {
    buttons.push({
      key: "pause",
      label: "Pause",
      title: "Pause this attention binding",
      onClick: () => actions.onAttentionPause?.(bindingInput),
    });
  }
  if (actions?.onAttentionResume && attentionIsPaused(row)) {
    buttons.push({
      key: "resume",
      label: "Resume",
      title: "Resume this attention binding",
      onClick: () => actions.onAttentionResume?.(bindingInput),
    });
  }
  if (actions?.onGoalConfirm && live) {
    buttons.push({
      key: "confirm",
      label: "Confirm",
      title: "Confirm goal completion",
      onClick: () => actions.onGoalConfirm?.(goalInput),
    });
  }
  if (actions?.onGoalRequestClose && live) {
    buttons.push({
      key: "request-close",
      label: "Request close",
      title: "Request goal closure",
      onClick: () => actions.onGoalRequestClose?.(goalInput),
    });
  }
  // Reassign authority is machine-derived from the binding mode upstream:
  // only coordinate-mode bindings can reassign, so others get no affordance.
  if (actions?.onAttentionReassign && live && row.mode === "coordinate") {
    buttons.push({
      key: "reassign",
      label: "Reassign",
      title: "Reassign this attention binding",
      onClick: () => actions.onAttentionReassign?.(bindingInput),
    });
  }

  return (
    <li
      className={`cc-work-graph__attention-row${attentionIsPaused(row) ? " is-paused" : ""}`}
      data-workgraph-binding={row.bindingId}
    >
      <span className={`cc-work-graph__mode is-${row.mode}`}>{row.mode}</span>
      <span className="cc-work-graph__attention-status">{row.statusLabel}</span>
      {row.targetLabel ? (
        <span className="cc-work-graph__attention-target" title="Attention target">{row.targetLabel}</span>
      ) : null}
      <span className="cc-work-graph__attention-spacer" />
      {buttons.map((button) => (
        <button
          key={button.key}
          type="button"
          className="cc-work-graph__action"
          title={button.title}
          data-testid={`workgraph-attention:${row.bindingId}:${button.key}`}
          onClick={(event) => {
            event.stopPropagation();
            button.onClick();
          }}
        >
          {button.label}
        </button>
      ))}
    </li>
  );
}

export function WorkGraphCard({
  entry,
  Icon,
  actions = null,
}: {
  entry: ConversationWorkGraphEntry;
  Icon?: IconRenderer | null;
  actions?: WorkGraphCardActions | null;
}) {
  const uiStateKey = entry.uiStateKey || entry.id;
  const [collapsed, setCollapsedState] = useState(() => collapsedWorkGraphCards.has(uiStateKey));
  const setCollapsed = (update: (value: boolean) => boolean) => {
    setCollapsedState((value) => {
      const next = update(value);
      rememberFlag(collapsedWorkGraphCards, uiStateKey, next);
      return next;
    });
  };
  const { completed, total } = entry.progress;
  const percent = total > 0 ? Math.round((completed / total) * 100) : 0;
  const hasBody = entry.items.length > 0
    || entry.attention.length > 0
    || Boolean(entry.recentEvents && entry.recentEvents.length > 0);
  // Goal actions CAS against the goal work item, not the binding: resolve
  // each binding's bound item revision from the folded rows. When the bound
  // item was never folded (or the binding names no item) the revision stays
  // absent so the handler resolves the live one — substituting another
  // item's revision (e.g. the card root's) would CAS against the wrong item.
  const revisionByItemId = new Map(entry.items.map((row) => [row.itemId, row.revision]));
  const goalRevisionFor = (row: ConversationWorkGraphAttentionRow): number | undefined => (
    row.itemId != null ? revisionByItemId.get(row.itemId) : undefined
  );

  return (
    <section
      className={`cc-work-graph is-${entry.status}${collapsed ? " is-collapsed" : ""}`}
      data-work-graph-card=""
      data-root-id={entry.rootId}
      data-status={entry.status}
      data-testid={`workgraph-card:${entry.rootId}`}
    >
      <header className="cc-work-graph__header">
        <span className="cc-work-graph__mark" aria-hidden="true">
          {Icon ? <Icon name="i-cube" /> : "◈"}
        </span>
        <div className="cc-work-graph__heading">
          <span className="cc-work-graph__title">{entry.title}</span>
          {entry.objective ? <span className="cc-work-graph__objective">{entry.objective}</span> : null}
        </div>
        {total > 0 ? (
          <div
            className="cc-work-graph__progress"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={total}
            aria-valuenow={completed}
            aria-label={`${completed} of ${total} work items completed`}
          >
            <span className="cc-work-graph__progress-count">{completed}/{total}</span>
            <span className="cc-work-graph__progress-track">
              <span className="cc-work-graph__progress-fill" style={{ width: `${percent}%` }} />
            </span>
          </div>
        ) : null}
        <span className={`cc-work-graph__badge is-${entry.status}`}>
          {entry.status === "active" ? <span className="cc-work-graph__pulse" aria-hidden="true" /> : null}
          {CARD_STATUS_LABEL[entry.status]}
        </span>
        {entry.lastActionFailed ? (
          <span
            className="cc-work-graph__last-failed"
            title="The last WorkGraph action failed"
            data-testid={`workgraph-card:${entry.rootId}:last-action-failed`}
          >
            ✗
          </span>
        ) : null}
        {hasBody ? (
          <button
            type="button"
            className="cc-work-graph__collapse"
            aria-expanded={!collapsed}
            aria-label={collapsed ? "Expand work graph" : "Collapse work graph"}
            data-testid={`workgraph-card:${entry.rootId}:toggle`}
            onClick={() => setCollapsed((value) => !value)}
          >
            {collapsed ? "▸" : "▾"}
          </button>
        ) : null}
      </header>
      {!collapsed && entry.items.length > 0 ? (
        <ul className="cc-work-graph__items">
          {entry.items.map((row) => (
            <ItemRow key={row.itemId} row={row} actions={actions} />
          ))}
          {typeof entry.itemOverflowCount === "number" && entry.itemOverflowCount > 0 ? (
            <li
              className="cc-work-graph__overflow"
              data-testid={`workgraph-card:${entry.rootId}:overflow`}
            >
              +{entry.itemOverflowCount} more items
            </li>
          ) : null}
        </ul>
      ) : null}
      {!collapsed && entry.attention.length > 0 ? (
        <ul className="cc-work-graph__attention">
          {entry.attention.map((row) => (
            <AttentionRow
              key={row.bindingId}
              row={row}
              goalRevision={goalRevisionFor(row)}
              actions={actions}
            />
          ))}
        </ul>
      ) : null}
      {!collapsed && entry.recentEvents && entry.recentEvents.length > 0 ? (
        <div className="cc-work-graph__events">
          {entry.recentEvents.map((line, index) => (
            <div className="cc-work-graph__event" key={`${line}-${index}`}>{line}</div>
          ))}
        </div>
      ) : null}
    </section>
  );
}
