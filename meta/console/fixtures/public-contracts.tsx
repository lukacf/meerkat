// Compiled only: positive and negative examples of the public integration API.
import type { ComponentProps } from "react";
import {
  beginConsoleSendAttempt, buildConversationViewState, conversationRichBlockCopyText,
  createConsoleContextRecord, createConsoleSendAttempt, createHttpConsoleTransport,
  createMetaConsoleController, createPendingApprovalResource, finishConsoleSendAttempt,
  mapFramesToTimelineEntries, serializeConsoleContextMessage,
  type ConsoleFrame, type ConsoleTransportState, type ConversationRichMarkdownBlock,
} from "@console-core";
import {
  ApprovalCard, ConsoleConversationPanel, ConsoleTransportStatus, ConversationMarkdown,
  ConversationPane, QuoteContextChips,
  type ConversationScrollControllerOptions, type ConversationViewportKey, type MarkdownUrlPolicy,
} from "@console-components";
import { createConsoleApp } from "../src/index";

const viewportKey: ConversationViewportKey = { authority: "runtime/realm/principal", identity: "agent:one", conversation: "conversation:one", pane: "pane:one" };
const markdown: ConversationRichMarkdownBlock = { type: "markdown", id: "frame:one:segment:0", source: "# Result\n\n```ts\nconst x = 1;\n```", streaming: true };
const copied: string = conversationRichBlockCopyText(markdown);
const urlPolicy: MarkdownUrlPolicy = { resolveLink: value => value.startsWith("https:") ? value : null, resolveImage: () => null };
const quote = createConsoleContextRecord({ id: "quote:one", sourceScope: viewportKey.authority, sourceIdentity: viewportKey.identity, messageId: markdown.id, quote: "Result", label: "Source agent" });
const content = serializeConsoleContextMessage("Explain this result", [quote]);
const attempt = createConsoleSendAttempt({ id: "attempt:one", scope: viewportKey.authority, destination: viewportKey.identity, origin: "console:one", idempotencyKey: "key:one", text: "Explain this result", contexts: [quote], now: 1 });
const frozen = beginConsoleSendAttempt(attempt, { owner: "tab:one", now: 2, handlingMode: "queue" });
const accepted = finishConsoleSendAttempt(frozen, { state: "accepted", interactionId: "interaction:one", inputFrameId: "frame:accepted" });
const resource = createPendingApprovalResource({ scopeKey: viewportKey.authority, load: async signal => { signal.throwIfAborted(); return { pending: [] }; }, decide: async (_id, _action, signal) => { signal.throwIfAborted(); return {}; } });
const frames: ConsoleFrame[] = [];
const entries = mapFramesToTimelineEntries(null, frames, { textMode: "markdown", renderTextDeltas: true });
const viewState = buildConversationViewState({ memberId: viewportKey.identity, agentLabel: "One", entries });
const controller = createMetaConsoleController({ transport: createHttpConsoleTransport({ baseUrl: "/console" }) });
const transportState: ConsoleTransportState = { phase: "retrying", stale: true, freshness: "replaying", retryInMs: 1000 };
const legacyReveal: ConversationScrollControllerOptions["revealAnchor"] = _id => true;
const asyncReveal: ConversationScrollControllerOptions["revealAnchor"] = async (_id, signal) => !signal.aborted;
const props: ComponentProps<typeof ConversationPane> = {
  viewState, viewportKey, submittedRowId: accepted.accepted?.inputFrameId,
  onRevealAnchor: asyncReveal, markdownUrlPolicy: urlPolicy,
  header: { title: "One", identity: viewportKey.identity, variant: "compact" },
  displayLabels: { tools: new Map([["read_file", "Read file"]]), peers: new Map([["agent:two", "Two"]]) },
  approvalSnapshot: resource.getSnapshot(), approvalIdentity: viewportKey.identity,
  onApprovalDecision: (id, action) => resource.decide(id, action),
  onQuoteSelection: selected => { const text: string = selected.text; void text; },
};

export function PublicContractFixture() {
  const snapshot = resource.getSnapshot();
  return <>
    <ConversationMarkdown block={markdown} urlPolicy={urlPolicy} />
    <ConversationPane {...props} />
    <ConversationPane {...props} onRevealAnchor={legacyReveal} />
    <ConsoleConversationPanel agent={null} agentLabel="One" identity={viewportKey.identity} entries={entries} draft={copied}
      viewportKey={viewportKey} submittedRowId={accepted.accepted?.inputFrameId} onRevealAnchor={asyncReveal}
      approvalSnapshot={snapshot} onApprovalDecision={(id, action) => resource.decide(id, action)}
      markdownUrlPolicy={urlPolicy} header={props.header} displayLabels={props.displayLabels}
      onQuoteSelection={props.onQuoteSelection} onDraftChange={() => {}} onSend={async () => false} />
    <ConsoleTransportStatus state={transportState} onRetry={() => {}} />
    {snapshot.requests.map(request => <ApprovalCard key={request.pendingId} request={request} resourceStatus={snapshot.status} onDecide={(id, action) => resource.decide(id, action)} />)}
    <QuoteContextChips records={[quote]} destinationLabel="One" onRemove={() => {}} onReorder={() => {}} />
  </>;
}

// These must stay rejected by the compiler; the fixture cannot quietly become any.
// @ts-expect-error Markdown source is literal text.
const invalidMarkdown: ConversationRichMarkdownBlock = { ...markdown, source: 3 };
// @ts-expect-error Acceptance requires its canonical interaction ID.
finishConsoleSendAttempt(frozen, { state: "accepted" });
// @ts-expect-error A quote range declares UTF-16 units explicitly.
const invalidRange: typeof quote.sourceRange = { start: 0, end: 6, unit: "bytes" };
// @ts-expect-error Unknown transport states are not display truth.
const invalidTransport: ConsoleTransportState = { ...transportState, phase: "probably-live" };
void invalidMarkdown; void invalidRange; void invalidTransport;
void controller.transport.send({ identity: viewportKey.identity, content, origin: "console:fixture", idempotencyKey: "key:fixture", handlingMode: "steer" });
void createConsoleApp(document.createElement("div"), { baseUrl: "/console", storageNamespace: viewportKey.authority, markdownUrlPolicy: urlPolicy });

// Developers contribute sidebar panels independently of MCP Apps.
const panel: import("../src/index").ConsolePanelDefinition = {
  id: "example/overview", title: "Overview", mount(element, context, signal) {
    element.textContent = context.readOnly ? "View only" : "Ready";
    signal.throwIfAborted();
    return { update(next) { element.textContent = next.baseUrl; }, dispose() { element.replaceChildren(); } };
  },
};
createConsoleApp(document.createElement("div"), { customPanels: [panel],
  panelService: async (request, scope, signal) => {
    const original: string | undefined = scope.conversation?.identity;
    signal.throwIfAborted();
    return { original, method: request.method };
  },
});

// MCP Apps is a separate host adapter. App authors supply standard tools/resources.
const mcpAppsHost: import("../src/index").ConsoleMcpAppsHost = {
  sandboxProxyUrl: "https://sandbox.example.test/sandbox.html",
  async resolve(locator, signal) {
    signal.throwIfAborted();
    void locator.sessionId;
    return null;
  },
};
createConsoleApp(document.createElement("div"), { mcpAppsHost });
