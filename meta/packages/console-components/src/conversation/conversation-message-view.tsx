import { ConsoleMcpAppView } from "../mcp-apps";
import type { MarkdownUrlPolicy } from "./conversation-markdown";
import { Fragment } from "react";

import {
  UNTRUSTED_SOURCE_DESCRIPTION,
  conversationRichBlockCopyText,
  conversationRichBlocksToText,
  conversationIdentityPresentation,
  describeConversationEntrySource,
  type ConversationEntrySource,
  type ConversationTimelineEntry,
} from "@console-core";

import { ConversationRichContent } from "./conversation-rich-content";
import { DeliveredContextMessage } from "./delivered-context-message";
import { ConversationConnectionEventView } from "./conversation-connection-event";
import { FlowRunCard, type FlowRunRestoreHandler } from "./flow-run-card";
import { SummaryCard } from "./summary-card";
import { CouncilCard } from "./council-card";
import { WorkGraphCard, type WorkGraphCardActions } from "./work-graph-card";
import { CopyButton } from "../copy-button";
import type { IconRenderer } from "../shared";
import { RowDetails } from "./presentation-policy";

export function OperationFeedbackView({ feedback, createdAt }: {
  feedback: NonNullable<Extract<ConversationTimelineEntry, { kind: "message" }>["operationFeedback"]>;
  createdAt?: string;
}) {
  return (
    <article
      className="cc-operation-feedback"
      data-feedback-kind={feedback.kind}
      aria-label={feedback.title}
      role="status"
    >
      <div className="cc-operation-feedback__heading">
        <strong>{feedback.title}</strong>
        <EntryTime iso={createdAt} />
      </div>
      <p>{feedback.detail}</p>
      {feedback.operationId ? (
        <span className="cc-operation-feedback__reference">Operation {feedback.operationId}</span>
      ) : null}
      {feedback.toolCallId ? (
        <span className="cc-operation-feedback__reference">Call {feedback.toolCallId}</span>
      ) : null}
      {feedback.approvalId ? (
        <span className="cc-operation-feedback__reference">Approval reference {feedback.approvalId}</span>
      ) : null}
    </article>
  );
}

const SYSTEM_TASK_PROMPT_STYLE = { whiteSpace: "pre-wrap" } as const;

function renderMultilineText(text: string) {
  return text.split("\n").map((line, index) => (
    <Fragment key={`${line}-${index}`}>
      {index > 0 ? <br /> : null}
      {line}
    </Fragment>
  ));
}

function humanizeSystemTaskMetadata(value: string | null | undefined) {
  const normalized = String(value || "").trim().replace(/[_-]+/gu, " ");
  if (!normalized) {
    return "";
  }
  return `${normalized[0]?.toUpperCase() || ""}${normalized.slice(1)}`;
}

function formatEntryTime(iso: string | undefined): { short: string; full: string } | null {
  if (!iso) return null;
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return null;
  const pad = (value: number) => String(value).padStart(2, "0");
  const short = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const full = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${short}:${pad(date.getSeconds())}`;
  return { short, full };
}

function EntryTime({ iso }: { iso: string | undefined }) {
  const time = formatEntryTime(iso);
  if (!time) return null;
  return (
    <time className="cc-message__time" dateTime={iso} title={time.full}>
      {time.short}
    </time>
  );
}

function UntrustedSourceBadge() {
  return (
    <span
      aria-label={`Untrusted source. ${UNTRUSTED_SOURCE_DESCRIPTION}`}
      className="cc-message__badge cc-message__badge--untrusted"
      role="note"
      tabIndex={0}
      title={UNTRUSTED_SOURCE_DESCRIPTION}
    >
      untrusted source
    </span>
  );
}

function payloadJson(payload: unknown): string {
  try {
    return JSON.stringify(payload, null, 2) ?? "";
  } catch {
    return String(payload);
  }
}

/// Source header for user-lane entries: what the message is, from the typed
/// origin, plus its time.
function EntrySourceHeader({ source, iso }: { source: ConversationEntrySource; iso: string | undefined }) {
  return (
    <div className="cc-message__source" data-source-kind={source.kind}>
      <span className="cc-message__source-label">{source.label}</span>
      {source.detail ? <span className="cc-message__source-detail">{source.detail}</span> : null}
      {source.untrusted ? <UntrustedSourceBadge /> : null}
      <EntryTime iso={iso} />
    </div>
  );
}

type ConversationMessageViewProps = {
  entry: ConversationTimelineEntry;
  compact?: boolean;
  Icon?: IconRenderer | null;
  markdownUrlPolicy?: MarkdownUrlPolicy;
  onFlowRunMessageMember?: ((memberKey: string) => void) | null;
  onFlowRunRestore?: FlowRunRestoreHandler | null;
  workGraphActions?: WorkGraphCardActions | null;
};

export function ConversationMessageView({
  entry,
  compact = false,
  Icon,
  markdownUrlPolicy,
  onFlowRunMessageMember = null,
  onFlowRunRestore = null,
  workGraphActions = null,
}: ConversationMessageViewProps) {
  const presentation = conversationIdentityPresentation(entry.identity);
  const assistantClassName = [
    "cc-message",
    "cc-message--assistant",
    presentation === "participant" ? "cc-message--participant" : "",
    presentation === "system" ? "cc-message--system" : "",
  ].filter(Boolean).join(" ");

  if (entry.kind === "flow_run") {
    return (
      <FlowRunCard
        entry={entry}
        Icon={Icon}
        onMessageMember={onFlowRunMessageMember}
        onRestore={onFlowRunRestore}
      />
    );
  }

  if (entry.kind === "council") {
    // The shared render path, not just ChatPane. Without this branch a council
    // entry falls through to the generic message render and shows NOTHING,
    // because it carries no text and no blocks - the card would appear in the
    // app that wires it explicitly and vanish in every other consumer of
    // ConversationPane.
    return <CouncilCard entry={entry} Icon={Icon} />;
  }

  if (entry.kind === "workgraph") {
    return (
      <WorkGraphCard
        entry={entry}
        Icon={Icon}
        actions={workGraphActions}
      />
    );
  }

  if (entry.kind === "summary") {
    return <SummaryCard entry={entry} />;
  }

  if (entry.mcpApp) return <ConsoleMcpAppView locator={{ ...entry.mcpApp, identity: entry.identity.id }} fallback={entry.text ?? "Tool result"} />;
  if (entry.operationFeedback) {
    return <OperationFeedbackView feedback={entry.operationFeedback} createdAt={entry.createdAt} />;
  }

  if (entry.taskKind || entry.taskLabel) {
    const taskLabel = entry.taskLabel?.trim() || "System task";
    const taskMetadata = [
      humanizeSystemTaskMetadata(entry.taskKind),
      humanizeSystemTaskMetadata(entry.taskStatus),
    ].filter(Boolean).join(" · ");
    const systemTaskClassName = [
      "cc-message",
      "cc-message--assistant",
      "cc-message--system",
      "cc-message--system-task",
      "cc-summary-card",
      "cc-rich-thinking",
    ].join(" ");
    return (
      <RowDetails
        part="system-task"
        aria-label={taskLabel}
        className={systemTaskClassName}
        data-task-kind={entry.taskKind}
        data-task-status={entry.taskStatus}
      >
        <summary className="cc-rich-thinking__label">
          <span className="cc-summary-card__title">{taskLabel}</span>
          {taskMetadata ? (
            <span className="cc-message-group__identity-meta"> · {taskMetadata}</span>
          ) : null}
        </summary>
        <div className="cc-rich-thinking__body">
          <p className="cc-rich-paragraph" style={SYSTEM_TASK_PROMPT_STYLE}>
            {entry.text || ""}
          </p>
        </div>
      </RowDetails>
    );
  }

  if (entry.variant === "meta") {
    if (entry.connectionEvent?.peers?.length) {
      return <ConversationConnectionEventView event={entry.connectionEvent} />;
    }
    if (entry.runtimeEvent) {
      // A runtime event reads as a sentence; the raw payload stays behind a
      // disclosure instead of being printed inline.
      const source = describeConversationEntrySource(entry);
      return (
        <article
          aria-label={source.label}
          className={`${assistantClassName} cc-message--meta cc-message--event`}
          data-source-kind={source.kind}
        >
          <p className="cc-message__event-line">
            <span>{source.sentence || entry.text}</span>
            {source.untrusted ? <UntrustedSourceBadge /> : null}
            <EntryTime iso={entry.createdAt} />
          </p>
          <RowDetails part="event-details" className="cc-message__event-details">
            <summary>Event details</summary>
            <pre>{payloadJson(entry.runtimeEvent.payload)}</pre>
          </RowDetails>
        </article>
      );
    }
    return <article className={`${assistantClassName} cc-message--meta`}><p>{entry.text}</p></article>;
  }

  if (presentation === "user") {
    const source = describeConversationEntrySource(entry);
    const visibleRichBlocks = entry.variant === "rich" && entry.blocks?.length
      ? entry.blocks.filter((block) => conversationRichBlockCopyText(block).trim().length > 0)
      : [];
    const copyText = entry.copyText || entry.text || conversationRichBlocksToText(visibleRichBlocks) || "";
    return (
      <article className={`cc-message cc-message--user${visibleRichBlocks.length ? " cc-message--rich" : ""}`}>
        {!compact ? (
          <CopyButton
            className="cc-message__copy"
            copiedLabel="Copied message"
            Icon={Icon}
            label="Copy message"
            text={copyText}
          />
        ) : null}
        {!compact ? <EntrySourceHeader iso={entry.createdAt} source={source} /> : null}
        <div data-quote-message-id={entry.id} data-quote-source={copyText}>
          {entry.contextMessage ? <DeliveredContextMessage message={entry.contextMessage} /> : visibleRichBlocks.length ? (
            <ConversationRichContent blocks={visibleRichBlocks} markdownUrlPolicy={markdownUrlPolicy} Icon={Icon} richStyle={entry.richStyle} />
          ) : (
            <p>{renderMultilineText(entry.text || "")}</p>
          )}
        </div>
      </article>
    );
  }

  const visibleRichBlocks = entry.variant === "rich" && entry.blocks?.length
    ? entry.blocks.filter((block) => conversationRichBlockCopyText(block).trim().length > 0)
    : [];

  if (entry.variant === "rich" && visibleRichBlocks.length) {
    return (
      <article className={`${assistantClassName} cc-message--rich`}>
        <div data-quote-message-id={entry.id} data-quote-source={entry.copyText || entry.text || conversationRichBlocksToText(visibleRichBlocks)}>
          <ConversationRichContent blocks={visibleRichBlocks} markdownUrlPolicy={markdownUrlPolicy} Icon={Icon} richStyle={entry.richStyle} />
        </div>
      </article>
    );
  }

  if (entry.variant === "rich") {
    return null;
  }

  return (
    <article className={assistantClassName}>
      <p data-quote-message-id={entry.id} data-quote-source={entry.text || ""}>{entry.text || ""}</p>
    </article>
  );
}
