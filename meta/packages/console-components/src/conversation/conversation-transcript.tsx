import { approvalInteractionIdsByTurn, ConversationApprovals, type ConversationApprovalProps } from "./conversation-approvals";
import type { MarkdownUrlPolicy } from "./conversation-markdown";
import clsx from "clsx";
import { Fragment } from "react";

import {
  transcriptDayKey,
  transcriptDayLabel,
  type ConversationTimelineGroup,
  type ConversationViewState,
} from "@console-core";

import { ConversationMessageGroup } from "./conversation-message-group";
import type { FlowRunRestoreHandler } from "./flow-run-card";
import type { WorkGraphCardActions } from "./work-graph-card";
import { groupConversationTranscriptTurns } from "./conversation-turns";
import { TurnDiffCard } from "./turn-diff-card";
import type { IconRenderer } from "../shared";

export type ConversationTranscriptProps = ConversationApprovalProps & {
  viewState: ConversationViewState;
  compact?: boolean;
  maxGroups?: number | null;
  showTurnDiff?: boolean;
  expandedDiffFile?: string | null;
  onToggleDiffFile?: ((filePath: string) => void) | null;
  Icon?: IconRenderer | null;
  markdownUrlPolicy?: MarkdownUrlPolicy;
  className?: string;
  onFlowRunMessageMember?: ((memberKey: string) => void) | null;
  onFlowRunRestore?: FlowRunRestoreHandler | null;
  workGraphActions?: WorkGraphCardActions | null;
};

function groupDayKey(group: ConversationTimelineGroup): string | null {
  for (const entry of group.entries) {
    const key = transcriptDayKey(entry.createdAt);
    if (key) return key;
  }
  return null;
}

export function ConversationTranscript({
  viewState,
  approvalSnapshot,
  approvalIdentity,
  onApprovalDecision,
  compact = false,
  maxGroups = null,
  showTurnDiff = true,
  expandedDiffFile = null,
  onToggleDiffFile = null,
  Icon,
  markdownUrlPolicy,
  className,
  onFlowRunMessageMember = null,
  onFlowRunRestore = null,
  workGraphActions = null,
}: ConversationTranscriptProps) {
  const canRenderTurnDiff = Boolean(showTurnDiff && viewState.turnDiff && onToggleDiffFile);
  const renderableTurnDiff = canRenderTurnDiff ? viewState.turnDiff : null;
  const groups = typeof maxGroups === "number" && maxGroups > 0
    ? viewState.groups.slice(-maxGroups)
    : viewState.groups;
  const turns = groupConversationTranscriptTurns(groups);
  const approvalInteractions = approvalInteractionIdsByTurn(turns.map((turn) =>
    turn.groups.flatMap((group) => group.entries.flatMap((entry) => entry.interactionId ? [entry.interactionId] : []))));

  if (!groups.length && !renderableTurnDiff && !approvalSnapshot?.requests.length) {
    return null;
  }

  // Day separators before the first dated group and at each local
  // calendar-day change, so row times are never ambiguous.
  const now = new Date();
  let previousDay: string | null = null;
  const daySeparator = (group: ConversationTimelineGroup) => {
    if (compact) return null;
    const day = groupDayKey(group);
    if (!day || day === previousDay) return null;
    previousDay = day;
    const label = transcriptDayLabel(day, now);
    return (
      <div aria-label={label} className="cc-conversation-day" data-testid={`conversation-day:${day}`} role="separator">
        <span>{label}</span>
      </div>
    );
  };

  return (
    <div className={clsx("cc-theme-scope", "cc-conversation-transcript", compact && "is-compact", className)}>
      {turns.map((turn, turnIndex) => {
        const isLastTurn = turnIndex === turns.length - 1;
        return (
          <section
            aria-label={`Turn ${turnIndex + 1}`}
            className="cc-conversation-turn"
            data-cc-conversation-turn-index={turnIndex}
            data-conversation-turn-id={turn.id}
            data-testid={`conversation-turn:${turnIndex}`}
            key={turn.id}
          >
            {turn.groups.map((group) => (
              <Fragment key={group.id}>
                {daySeparator(group)}
                <ConversationMessageGroup
                  compact={compact}
                  group={group}
                  Icon={Icon}
                  onFlowRunMessageMember={onFlowRunMessageMember}
                  onFlowRunRestore={onFlowRunRestore}
                  workGraphActions={workGraphActions}
            markdownUrlPolicy={markdownUrlPolicy}
                />
              </Fragment>
            ))}
            <ConversationApprovals approvalSnapshot={approvalSnapshot} approvalIdentity={approvalIdentity} onApprovalDecision={onApprovalDecision} conversationId={viewState.conversationId} interactionIds={approvalInteractions[turnIndex]} />
            {isLastTurn && renderableTurnDiff && onToggleDiffFile ? (
              <TurnDiffCard
                expandedFile={expandedDiffFile}
                onToggleFile={onToggleDiffFile}
                turnDiff={renderableTurnDiff}
              />
            ) : null}
          </section>
        );
      })}
      <ConversationApprovals approvalSnapshot={approvalSnapshot} approvalIdentity={approvalIdentity} onApprovalDecision={onApprovalDecision} conversationId={viewState.conversationId} />
      {!turns.length && renderableTurnDiff && onToggleDiffFile ? (
        <section
          aria-label="Turn 1"
          className="cc-conversation-turn"
          data-cc-conversation-turn-index={0}
          data-testid="conversation-turn:0"
        >
          <TurnDiffCard
            expandedFile={expandedDiffFile}
            onToggleFile={onToggleDiffFile}
            turnDiff={renderableTurnDiff}
          />
        </section>
      ) : null}
    </div>
  );
}
