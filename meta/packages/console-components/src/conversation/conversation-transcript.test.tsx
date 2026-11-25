import { mapFramesToTimelineEntries as projectStockApprovalInputs } from "../../../../console/src/lib/adapters";
import { mapFramesToTimelineEntries as projectSharedApprovalInputs } from "../../../console-core/src/adapters";
import { groupConversationTimelineEntries, type ConversationTimelineEntry, type ConversationViewState } from "@console-core";
import { fireEvent, render, screen } from "@testing-library/react";

import { normalizePendingApproval } from "../../../console-core/src/pending-approvals";
import { normalizeQuestionSet, type QuestionSetSnapshot } from "../../../console-core/src/pending-questions";
import { ConversationTranscript } from "./conversation-transcript";
import { QuestionSetsProvider } from "./question-set-card";

function Icon({ name }: { name: string; className?: string }) {
  return <span>{name}</span>;
}

function buildViewState(): ConversationViewState {
  const entries: ConversationTimelineEntry[] = [
    {
      id: "user-1",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user" },
      text: "Ship the extraction.",
    },
    {
      id: "assistant-1",
      kind: "message",
      variant: "plain",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "I extracted the transcript and sidebar.",
    },
    {
      id: "assistant-2",
      kind: "summary",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      title: "1 file changed",
      plus: 12,
      minus: 2,
      files: [{ name: "desktop/renderer/src/app/App.tsx", plus: 12, minus: 2 }],
    },
  ];

  return {
    conversationId: "thread-1",
    entries,
    groups: groupConversationTimelineEntries(entries),
    turnDiff: {
      fileCount: 1,
      plus: 12,
      minus: 2,
      files: [{
        path: "desktop/renderer/src/app/App.tsx",
        plus: 12,
        minus: 2,
        hunks: [{
          oldStart: 1,
          oldLines: 1,
          newStart: 1,
          newLines: 2,
          lines: [
            { type: "context", text: "import App from \"@/app/App\";", oldLine: 1, newLine: 1 },
            { type: "add", text: "import { ConversationTranscript } from \"@console-components\";", oldLine: null, newLine: 2 },
          ],
        }],
      }],
    },
    emptyState: null,
  };
}

describe("ConversationTranscript", () => {
  test("renders grouped transcript entries and expandable turn diffs", () => {
    const onToggleDiffFile = vi.fn();

    render(
      <ConversationTranscript
        Icon={Icon}
        expandedDiffFile={null}
        onToggleDiffFile={onToggleDiffFile}
        viewState={buildViewState()}
      />,
    );

    expect(screen.getByText("Ship the extraction.")).toBeInTheDocument();
    expect(screen.getByText("I extracted the transcript and sidebar.")).toBeInTheDocument();
    expect(screen.getAllByText("1 file changed")).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", { name: /desktop\/renderer\/src\/app\/app\.tsx/i }));
    expect(onToggleDiffFile).toHaveBeenCalledWith("desktop/renderer/src/app/App.tsx");
  });

  test("renders labeled participant groups without app-specific transcript state", () => {
    const entries: ConversationTimelineEntry[] = [
      {
        id: "builder-1",
        kind: "message",
        variant: "plain",
        identity: {
          id: "builder",
          label: "Builder",
          role: "other",
          presentation: "participant",
          showLabel: true,
          meta: "implementation",
          avatarLabel: "BLD",
        },
        text: "I normalized the view model for Meta.",
      },
    ];

    render(
      <ConversationTranscript
        Icon={Icon}
        onToggleDiffFile={() => undefined}
        viewState={{
          conversationId: "meta-thread",
          entries,
          groups: groupConversationTimelineEntries(entries),
          turnDiff: null,
          emptyState: null,
        }}
      />,
    );

    expect(screen.getByText("Builder")).toBeInTheDocument();
    expect(screen.getByText("implementation")).toBeInTheDocument();
    expect(screen.getByText("I normalized the view model for Meta.")).toBeInTheDocument();
  });

  test("groups timeline entries into scrollable user turns", () => {
    const entries: ConversationTimelineEntry[] = [
      {
        id: "user-1",
        kind: "message",
        variant: "plain",
        identity: { id: "user", label: "You", role: "user" },
        text: "First request.",
      },
      {
        id: "assistant-1",
        kind: "message",
        variant: "plain",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        text: "First response.",
      },
      {
        id: "user-2",
        kind: "message",
        variant: "plain",
        identity: { id: "user", label: "You", role: "user" },
        text: "Second request.",
      },
      {
        id: "assistant-2",
        kind: "message",
        variant: "plain",
        identity: { id: "assistant", label: "Assistant", role: "assistant" },
        text: "Second response.",
      },
    ];

    render(
      <ConversationTranscript
        viewState={{
          conversationId: "thread-turns",
          entries,
          groups: groupConversationTimelineEntries(entries),
          turnDiff: null,
          emptyState: null,
        }}
      />,
    );

    const turns = screen.getAllByTestId(/conversation-turn:/);
    expect(turns).toHaveLength(2);
    expect(turns[0]).toHaveTextContent("First request.");
    expect(turns[0]).toHaveTextContent("First response.");
    expect(turns[1]).toHaveTextContent("Second request.");
    expect(turns[1]).toHaveTextContent("Second response.");
  });
  test("one interaction spanning a steer renders its approval only beside its latest turn", () => {
    const entries: ConversationTimelineEntry[] = [
      { id: "ask", kind: "message", variant: "plain", identity: { id: "operator", label: "You", role: "user" }, interactionId: "original", text: "Start review" },
      { id: "work", kind: "message", variant: "plain", identity: { id: "agent", label: "Agent", role: "assistant" }, interactionId: "original", text: "Reviewing" },
      { id: "steer", kind: "message", variant: "plain", identity: { id: "operator", label: "You", role: "user" }, interactionId: "steer", text: "Include deployment" },
      { id: "continue", kind: "message", variant: "plain", identity: { id: "agent", label: "Agent", role: "assistant" }, interactionId: "original", text: "Deployment needs approval" },
    ];
    const request = normalizePendingApproval({ pending_id: "deployment", action: "Deploy reviewed change", action_id: "deploy", origin: { identity: "agent", conversation_id: "review", interaction_id: "original" } })!;
    const viewState = { conversationId: "review", entries, groups: groupConversationTimelineEntries(entries), turnDiff: null, emptyState: null };
    const view = render(<ConversationTranscript viewState={viewState} approvalIdentity="agent" approvalSnapshot={{ scopeKey: "scope", status: "ready", requests: [request], decisions: {}, readOnly: false }} onApprovalDecision={() => {}} />);
    expect(screen.getAllByTestId("gating-pending:deployment")).toHaveLength(1);
    expect(screen.getByTestId("gating-pending:deployment").closest("section.cc-conversation-turn")).toBe(screen.getByTestId("conversation-turn:1"));
    view.rerender(<ConversationTranscript viewState={viewState} maxGroups={2} approvalIdentity="agent" approvalSnapshot={{ scopeKey: "scope", status: "ready", requests: [request], decisions: {}, readOnly: false }} onApprovalDecision={() => {}} />);
    expect(screen.getAllByTestId("gating-pending:deployment")).toHaveLength(1);
  });

  test("a transcript with nothing to draw renders nothing and reads nothing of the question sets", () => {
    // A host can render an empty transcript where the page's question sets
    // cannot be read: it must not ask for them.
    let reads = 0;
    const snapshot: QuestionSetSnapshot = { scopeKey: "scope", status: "ready", readOnly: false, sets: [], closed: [], decisions: {}, notices: [], slots: {} };
    const view = { get snapshot() { reads += 1; return snapshot; }, onAnswer: () => {}, onDismiss: () => {} };
    const empty: ConversationViewState = { conversationId: "empty", entries: [], groups: [], turnDiff: null, emptyState: null };
    const rendered = render(
      <QuestionSetsProvider value={view}>
        <div data-testid="host"><ConversationTranscript viewState={empty} approvalIdentity="agent" /></div>
      </QuestionSetsProvider>,
    );
    expect(rendered.getByTestId("host").innerHTML).toBe("");
    expect(reads).toBe(0);
    // A transcript that draws a turn does read them: it places their receipts.
    rendered.rerender(
      <QuestionSetsProvider value={view}>
        <div data-testid="host"><ConversationTranscript viewState={buildViewState()} approvalIdentity="agent" /></div>
      </QuestionSetsProvider>,
    );
    expect(rendered.getByTestId("conversation-turn:0")).toBeTruthy();
    expect(reads > 0).toBe(true);
  });

  test("a transcript that shows only its newest groups draws no line of a question set answered after a turn it does not show", () => {
    const at = (minute: number) => new Date(Date.UTC(2026, 0, 5, 10, minute)).toISOString();
    const entries: ConversationTimelineEntry[] = [
      { id: "ask-1", kind: "message", variant: "plain", identity: { id: "operator", label: "You", role: "user" }, text: "Plan the release.", createdAt: at(0) },
      { id: "work-1", kind: "message", variant: "plain", identity: { id: "agent", label: "Agent", role: "assistant" }, text: "Which region first?", createdAt: at(1) },
      { id: "ask-2", kind: "message", variant: "plain", identity: { id: "operator", label: "You", role: "user" }, text: "Go on.", createdAt: at(10) },
      { id: "work-2", kind: "message", variant: "plain", identity: { id: "agent", label: "Agent", role: "assistant" }, text: "Europe goes first.", createdAt: at(11) },
    ];
    const viewState: ConversationViewState = { conversationId: "plan", entries, groups: groupConversationTimelineEntries(entries), turnDiff: null, emptyState: null };
    const pendingId = `decision:${"a".repeat(64)}`;
    const set = normalizeQuestionSet({
      pending_id: pendingId, origin: { identity: "agent" }, title: "Release plan", subject_digest: `sha256:${"b".repeat(64)}`,
      questions: [{ id: "q-region", prompt: "Which region goes first?", choices: [{ id: "c-eu", label: "Europe" }, { id: "c-us", label: "United States" }] }],
      created_at_ms: Date.parse(at(1)),
    })!;
    // Answered between the first turn and the second, by the gateway's clock.
    const decidedAtMs = Date.parse(at(5));
    const snapshot: QuestionSetSnapshot = {
      scopeKey: "scope", status: "ready", readOnly: false, sets: [], notices: [], slots: { [pendingId]: 1 },
      closed: [{ set, outcome: "answered", closedAtMs: decidedAtMs }],
      decisions: { [pendingId]: { phase: "answered", action: "answer", answers: { "q-region": { kind: "choice", choiceId: "c-eu" } }, decidedAtMs } },
    };
    const transcript = (maxGroups: number | null) => (
      <QuestionSetsProvider value={{ snapshot, onAnswer: () => {}, onDismiss: () => {} }}>
        <ConversationTranscript viewState={viewState} maxGroups={maxGroups} approvalIdentity="agent" />
      </QuestionSetsProvider>
    );
    // The whole conversation: the line stands with the first turn.
    const view = render(transcript(null));
    expect(screen.getByTestId(`question-set-receipt:${pendingId}`).closest("section.cc-conversation-turn")).toBe(screen.getByTestId("conversation-turn:0"));
    // Only the newest turn is shown: the line goes with its turn, and is not
    // drawn with a turn it was not answered after.
    view.rerender(transcript(2));
    expect(screen.getAllByTestId(/^conversation-turn:/)).toHaveLength(1);
    expect(screen.getByTestId("conversation-turn:0")).toHaveTextContent("Europe goes first.");
    expect(screen.queryByTestId(`question-set-receipt:${pendingId}`)).toBeNull();
  });
});

for (const [surface, project] of [["stock", projectStockApprovalInputs], ["shared", projectSharedApprovalInputs]] as const) {
  test(`${surface}: adapter-produced operator turn owns one approval before any assistant entry`, () => {
    const currentId = "94aa7544-fb65-4097-a82b-7e3172b76d06";
    const missingId = "ea5e8845-05ec-4602-a009-1e13c638e871";
    const agent = { agent_id: "reviewer", member_id: "reviewer", label: "Reviewer", kind: "identity" };
    const entries = project(agent, [
      { id: "current-input", event: "user_input", timestampMs: 2000, interactionId: currentId,
        data: { content: [{ type: "text", text: "Review this exact pending release." }, { type: "image", source: "blob", blob_id: "proof", media_type: "image/png" }] } },
    ], { renderInteractionStartsAsUser: true, textMode: "markdown" });
    expect(entries).toHaveLength(1);
    expect(entries.every(entry => entry.identity.role === "user")).toBe(true);
    const requests = [
      normalizePendingApproval({ pending_id: "exact", action: "Review release", origin: { identity: "reviewer", interaction_id: currentId } })!,
      normalizePendingApproval({ pending_id: "foreign-interaction", action: "Unseen review", origin: { identity: "reviewer", interaction_id: missingId } })!,
      normalizePendingApproval({ pending_id: "foreign-agent", action: "Other agent review", origin: { identity: "other-agent", interaction_id: currentId } })!,
    ];
    render(<ConversationTranscript
      viewState={{ conversationId: "reviewer", entries, groups: groupConversationTimelineEntries(entries), turnDiff: null, emptyState: null }}
      approvalIdentity="reviewer"
      approvalSnapshot={{ scopeKey: "reviewer", status: "ready", requests, decisions: {}, readOnly: false }}
      onApprovalDecision={() => {}}
    />);
    expect(screen.getAllByTestId("gating-pending:exact")).toHaveLength(1);
    const turn = screen.getByTestId("gating-pending:exact").closest("[data-conversation-turn-id]");
    expect(turn).toBe(screen.getByTestId("conversation-turn:0"));
    expect(turn).toHaveTextContent("Review this exact pending release.");
    expect(screen.queryByTestId("gating-pending:foreign-interaction")).toBeNull();
    expect(screen.queryByTestId("gating-pending:foreign-agent")).toBeNull();
  });
}
