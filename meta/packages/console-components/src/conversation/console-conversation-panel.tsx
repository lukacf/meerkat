import type { MarkdownUrlPolicy } from "./conversation-markdown";
import type { ReactNode } from "react";

import type {
  ConsoleAgent,
  ConsoleComposerToolbarItem,
  ConsoleComposerViewState,
  ConversationTimelineEntry,
} from "@console-core";
import {
  buildConversationViewState,
} from "@console-core";

import { ConsoleComposer, type ConsoleComposerProps } from "../composer/console-composer";
import { ConversationPane, type ConversationPaneProps } from "./conversation-pane";
import type { IconRenderer } from "../shared";

export type ConsoleConversationPanelPhase = "waiting" | "tool-executing" | "generating" | string | null;

export type ConsoleConversationPanelProps = Pick<ConversationPaneProps, "viewportKey" | "submittedRowId" | "onRevealAnchor" | "displayLabels" | "header" | "contextSlot" | "onQuoteSelection" | "approvalSnapshot" | "onApprovalDecision"> & {
  agent: ConsoleAgent | null;
  agentLabel: string;
  identity: string;
  entries: ConversationTimelineEntry[];
  phase?: ConsoleConversationPanelPhase;
  draft: string;
  sending?: boolean;
  targetLabel?: string;
  modelLabel?: string | null;
  reasoningLabel?: string | null;
  executionModeLabel?: string | null;
  branchLabel?: string | null;
  permissionsLabel?: string | null;
  providerLabel?: string | null;
  stackSlot?: ReactNode;
  Icon?: IconRenderer | null;
  markdownUrlPolicy?: MarkdownUrlPolicy;
  className?: string;
  inputId?: string;
  submitButtonId?: string;
  mainRowItems?: ConsoleComposerToolbarItem[];
  footerLeftItems?: ConsoleComposerToolbarItem[];
  footerRightItems?: ConsoleComposerToolbarItem[];
  getToolbarButtonProps?: ConsoleComposerProps["getToolbarButtonProps"];
  onDraftChange: (value: string) => void;
  onSend: (content: string) => void | boolean | Promise<void | boolean>;
};

function defaultMainRowItems(args: {
  providerLabel: string;
  modelLabel: string;
  reasoningLabel: string;
}): ConsoleComposerToolbarItem[] {
  return [
    { id: "attach", kind: "pill-icon", iconName: "i-plus", label: "Add context" },
    { id: "provider", kind: "pill", label: args.providerLabel, hasMenu: true },
    { id: "model", kind: "pill", label: args.modelLabel, hasMenu: true },
    { id: "reasoning", kind: "pill", label: args.reasoningLabel, hasMenu: true },
  ];
}

function defaultFooterLeftItems(args: {
  targetLabel: string;
  executionModeLabel: string;
  permissionsLabel: string;
}): ConsoleComposerToolbarItem[] {
  return [
    { id: "target", kind: "sub-pill", iconName: "i-team", label: `To: ${args.targetLabel}` },
    { id: "execution", kind: "sub-pill", iconName: "i-terminal", label: args.executionModeLabel, hasMenu: true },
    { id: "permissions", kind: "sub-pill", iconName: "i-bolt", label: args.permissionsLabel, hasMenu: true },
  ];
}

function defaultFooterRightItems(args: {
  branchLabel: string;
  phase: ConsoleConversationPanelPhase;
}): ConsoleComposerToolbarItem[] {
  return [
    { id: "branch", kind: "sub-pill", iconName: "i-branch", label: args.branchLabel, hasMenu: true },
    { id: "phase", kind: "sub-pill", label: args.phase || "", hidden: !args.phase },
  ];
}

export function ConsoleConversationPanel({
  agent,
  agentLabel,
  identity,
  entries,
  phase = null,
  draft,
  sending = false,
  targetLabel = agentLabel,
  modelLabel = "Model",
  reasoningLabel = "Reasoning",
  executionModeLabel = "Local project",
  branchLabel = "branch",
  permissionsLabel = "Permissions",
  providerLabel = "OpenAI",
  stackSlot = null,
  Icon = null,
  markdownUrlPolicy,
  viewportKey,
  submittedRowId,
  onRevealAnchor,
  displayLabels,
  header,
  contextSlot,
  onQuoteSelection,
  approvalSnapshot,
  onApprovalDecision,
  className,
  inputId,
  submitButtonId,
  mainRowItems,
  footerLeftItems,
  footerRightItems,
  getToolbarButtonProps,
  onDraftChange,
  onSend,
}: ConsoleConversationPanelProps) {
  const modelBits = [modelLabel, reasoningLabel, executionModeLabel, branchLabel, permissionsLabel].filter(Boolean);
  const roleLabel = agent?.role || modelBits.join(" · ") || "agent";
  const chatAgent: ConsoleAgent = agent
    ? { ...agent, role: roleLabel }
    : {
        identity,
        agent_id: identity,
        member_id: identity,
        label: agentLabel,
        kind: "host",
        role: roleLabel,
        state: sending ? "loading" : "ready",
      };
  const conversation = buildConversationViewState({
    memberId: identity,
    agentLabel: targetLabel,
    agent: chatAgent,
    entries,
  });
  const composerViewState: ConsoleComposerViewState = {
    value: draft,
    placeholder: `Ask ${targetLabel} anything, @ to add files, / for commands`,
    disabled: sending,
    submitDisabled: sending || !draft.trim(),
    submitLabel: `Send to ${targetLabel}`,
    mainRowItems: mainRowItems ?? defaultMainRowItems({
      providerLabel: providerLabel || "Provider",
      modelLabel: modelLabel || "Model",
      reasoningLabel: reasoningLabel || "Reasoning",
    }),
    footerLeftItems: footerLeftItems ?? defaultFooterLeftItems({
      targetLabel,
      executionModeLabel: executionModeLabel || "Local project",
      permissionsLabel: permissionsLabel || "Permissions",
    }),
    footerRightItems: footerRightItems ?? defaultFooterRightItems({
      branchLabel: branchLabel || "branch",
      phase,
    }),
  };

  function submitComposer() {
    const content = draft.trim();
    if (!content) {
      return;
    }
    void onSend(content);
  }

  return (
    <div className={["cc-conversation-panel", className].filter(Boolean).join(" ")} data-testid={`conversation-pane:${identity}`}>
      <ConversationPane
        markdownUrlPolicy={markdownUrlPolicy}
        viewportKey={viewportKey}
        submittedRowId={submittedRowId}
        onRevealAnchor={onRevealAnchor}
        displayLabels={displayLabels}
        header={header}
        contextSlot={contextSlot}
        onQuoteSelection={onQuoteSelection}
        isWorking={phase !== null}
        approvalSnapshot={approvalSnapshot}
        approvalIdentity={identity}
        onApprovalDecision={onApprovalDecision}
        Icon={Icon}
        viewState={conversation}
        footer={(
          <>
            {stackSlot}
            {phase ? (
              <div className="cc-conversation-panel__phase" data-testid={`conversation-phase:${identity}`} aria-live="polite">
                <span className="cc-conversation-panel__phase-dot" aria-hidden="true" />
                <span>{targetLabel} is working</span>
              </div>
            ) : null}
            <ConsoleComposer
              Icon={Icon}
              inputId={inputId}
              submitButtonId={submitButtonId}
              viewState={composerViewState}
              getToolbarButtonProps={getToolbarButtonProps}
              onChange={onDraftChange}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  submitComposer();
                }
              }}
              onSubmit={submitComposer}
            />
          </>
        )}
        onApplySuggestion={(value) => onDraftChange(value)}
      />
    </div>
  );
}
