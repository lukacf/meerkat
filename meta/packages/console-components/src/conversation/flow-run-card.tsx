import { useId, type CSSProperties } from "react";

import {
  parseConversationRichBlocks,
  type ConversationFlowRunEntry,
  type ConversationFlowRunMemberRow,
  type FlowRunStatus,
} from "@console-core";

import { ConversationRichContent } from "./conversation-rich-content";
import { ConversationTranscript } from "./conversation-transcript";
import { useRowState } from "./presentation-policy";
import type { IconRenderer } from "../shared";

export type FlowRunRestoreHandler = (
  helperId: string | null,
  entry: ConversationFlowRunEntry,
) => void;

const STATUS_LABEL: Record<FlowRunStatus, string> = {
  idle: "Idle",
  queued: "Queued",
  running: "Working",
  cancelling: "Stopping",
  completed: "Done",
  failed: "Failed",
  stopped: "Stopped",
};

function statusDotClass(status: FlowRunStatus): string {
  return `cc-flow-run__dot is-${status}`;
}

function isTerminalStatus(status: FlowRunStatus): boolean {
  return status === "completed" || status === "failed" || status === "stopped";
}

function MemberRow({
  row,
  Icon,
  onMessageMember,
}: {
  row: ConversationFlowRunMemberRow;
  Icon?: IconRenderer | null;
  onMessageMember?: ((memberKey: string) => void) | null;
}) {
  const detailId = useId();
  const [expanded, setExpanded] = useRowState(`flow-member:${row.memberKey}`, () => false);
  const hasDetail = Boolean(row.subView && row.subView.groups.length);
  const style = (row.tone?.variables || undefined) as CSSProperties | undefined;
  const rowContent = (
    <>
      <span className={statusDotClass(row.status)} aria-hidden="true" />
      <span className="cc-flow-run__member-label">{row.label}</span>
      <span className="cc-flow-run__member-caption">{row.caption}</span>
      <span className="cc-flow-run__member-status">{STATUS_LABEL[row.status]}</span>
      {hasDetail ? (
        <span className="cc-flow-run__member-chevron" aria-hidden="true">{expanded ? "▾" : "▸"}</span>
      ) : null}
    </>
  );

  return (
    <li
      className={`cc-flow-run__member is-${row.status}${expanded ? " is-expanded" : ""}`}
      data-flow-run-member={row.memberKey}
      style={style}
    >
      <div className="cc-flow-run__member-line">
        {hasDetail ? (
          <button
            type="button"
            className="cc-flow-run__member-row"
            aria-controls={detailId}
            aria-expanded={expanded}
            onClick={() => setExpanded(!expanded)}
          >
            {rowContent}
          </button>
        ) : (
          <div className="cc-flow-run__member-row">{rowContent}</div>
        )}
        {onMessageMember ? (
          <button
            type="button"
            className="cc-flow-run__member-message"
            aria-label={`Message ${row.label}`}
            title={`Message ${row.label}`}
            onClick={(event) => {
              event.stopPropagation();
              onMessageMember(row.memberKey);
            }}
          >
            Message
          </button>
        ) : null}
      </div>
      {hasDetail && expanded && row.subView ? (
        <div
          id={detailId}
          className="cc-flow-run__member-detail"
          role="region"
          aria-label={`${row.label} transcript`}
          tabIndex={0}
        >
          <ConversationTranscript
            viewState={row.subView}
            compact
            showTurnDiff={false}
            Icon={Icon}
          />
        </div>
      ) : null}
    </li>
  );
}

export function FlowRunCard({
  entry,
  Icon,
  onMessageMember,
  onRestore,
}: {
  entry: ConversationFlowRunEntry;
  Icon?: IconRenderer | null;
  onMessageMember?: ((memberKey: string) => void) | null;
  onRestore?: FlowRunRestoreHandler | null;
}) {
  // Message targeting needs a live member behind the row; paused (restorable)
  // crews only carry persisted job history, so the affordance is Resume.
  const memberMessageHandler = entry.restorable ? null : onMessageMember;
  const headingId = useId();
  const detailsId = useId();
  const terminal = isTerminalStatus(entry.status);
  const hasDetails = Boolean(entry.rows.length);
  // Active work must remain inspectable without another click. Conversely,
  // collapse a card when a live run COMPLETES so finished crews stop
  // dominating the transcript — but failed/stopped runs keep their details
  // open: per-member failure detail matters most at the moment of failure.
  // A user's choice is preserved for subsequent renders while the card
  // remains in the same state class, including when its row mounts again.
  const defaultExpanded = !terminal || entry.status !== "completed";
  const [details, setDetails] = useRowState(`flow-run-details:${entry.helperId ?? ""}`,
    () => ({ status: entry.status, expanded: defaultExpanded }));
  if (details.status !== entry.status) setDetails({ status: entry.status, expanded: defaultExpanded });
  const detailsExpanded = details.status === entry.status ? details.expanded : defaultExpanded;
  const setDetailsExpanded = (next: boolean | ((current: boolean) => boolean)) => setDetails({
    status: entry.status,
    expanded: typeof next === "function" ? next(detailsExpanded) : next,
  });

  return (
    <section
      className={`cc-flow-run is-${entry.status}${terminal && !detailsExpanded ? " is-compact" : " is-details-expanded"}`}
      data-flow-run-card=""
      data-helper-id={entry.helperId}
      data-status={entry.status}
      data-details-expanded={detailsExpanded ? "true" : "false"}
      aria-labelledby={headingId}
    >
      <header className="cc-flow-run__header">
        <span className="cc-flow-run__mark" aria-hidden="true">
          {Icon ? <Icon name="i-team" /> : "◇"}
        </span>
        <div className="cc-flow-run__heading">
          <span id={headingId} className="cc-flow-run__name">{entry.flowName}</span>
          {entry.objective ? <span className="cc-flow-run__objective">{entry.objective}</span> : null}
        </div>
        {entry.restorable && onRestore ? (
          <button
            type="button"
            className="cc-flow-run__restore"
            title="Resume this crew's helpers"
            onClick={(event) => {
              event.stopPropagation();
              onRestore(entry.helperId || null, entry);
            }}
          >
            Resume
          </button>
        ) : null}
        {terminal && hasDetails ? (
          <button
            type="button"
            className="cc-flow-run__disclosure"
            aria-controls={detailsId}
            aria-expanded={detailsExpanded}
            onClick={() => setDetailsExpanded((value) => !value)}
          >
            {detailsExpanded ? "Hide details" : "Show details"}
          </button>
        ) : null}
        <span className={`cc-flow-run__badge is-${entry.status}`}>{STATUS_LABEL[entry.status]}</span>
      </header>
      {hasDetails ? (
        <div
          id={detailsId}
          className="cc-flow-run__details"
          role="region"
          aria-label={`${entry.flowName} details`}
          hidden={!detailsExpanded}
        >
          {entry.rows.length ? (
            <ul className="cc-flow-run__members">
              {entry.rows.map((row) => (
                <MemberRow key={row.memberKey} row={row} Icon={Icon} onMessageMember={memberMessageHandler} />
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
      {entry.outcome ? (
        // The outcome is the conversation answer, not execution detail. Keep
        // it visible when a terminal card compacts so the transcript never
        // collapses into an empty status row.
        <div className="cc-flow-run__outcome">
          <ConversationRichContent blocks={parseConversationRichBlocks(entry.outcome)} Icon={Icon} />
        </div>
      ) : null}
    </section>
  );
}
