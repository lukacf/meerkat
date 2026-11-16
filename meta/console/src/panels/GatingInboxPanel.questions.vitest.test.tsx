import React from "react";
import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GatingInboxPanel } from "./GatingInboxPanel";
import { normalizePendingApproval, type PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import { normalizeQuestionSet, type QuestionSetSnapshot } from "../../../packages/console-core/src/pending-questions";

afterEach(cleanup);

const PENDING_ID = `decision:${"a".repeat(64)}`;
const questions = normalizeQuestionSet({
  pending_id: PENDING_ID,
  origin: { identity: "identity:release-planner" },
  title: "Release plan",
  questions: [{ id: "q1", prompt: "Which region goes first?", choices: [{ id: "eu", label: "Europe" }, { id: "us", label: "United States" }] }],
  created_at_ms: Date.now() - 60_000,
  deadline_at_ms: Date.now() + 3_600_000,
  subject_digest: `sha256:${"b".repeat(64)}`,
})!;
const request = normalizePendingApproval({ pending_id: "p1", action_id: "a1", action: "Deploy release", rationale: "Publish reviewed changes" })!;
const approvals = (requests = [request]): PendingApprovalSnapshot => ({ scopeKey: "scope", status: "ready", requests, decisions: {}, readOnly: false });
const sets = (extra: Partial<QuestionSetSnapshot> = {}): QuestionSetSnapshot => ({
  scopeKey: "scope", status: "ready", sets: [questions], closed: [], decisions: {}, notices: [], slots: { [PENDING_ID]: 1 }, readOnly: false, ...extra,
});

describe("GatingInboxPanel question sets", () => {
  it("shows the question sets on Pending, above the approvals it still shows", () => {
    const onAnswer = vi.fn(), onDismiss = vi.fn(), onDecide = vi.fn();
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={onDecide} resource={approvals()}
        questions={{ snapshot: sets(), onAnswer, onDismiss }} />,
    );
    const card = view.getByTestId(`question-set:${PENDING_ID}`);
    const approval = view.getByTestId("gating-pending:p1");
    expect(within(card).getByText("Release plan")).toBeTruthy();
    expect(within(approval).getByText("Deploy release")).toBeTruthy();
    expect(card.compareDocumentPosition(approval) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(view.getByTestId("question-set-list").contains(card)).toBe(true);
    // The Pending tab counts what waits: the approval and the open question set.
    expect(view.getByTestId("gating-tab:pending").textContent).toContain("2");
    fireEvent.click(within(card).getByRole("radio", { name: "Europe" }));
    fireEvent.click(within(card).getByRole("button", { name: "Send answers" }));
    expect(onAnswer).toHaveBeenCalledWith(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    fireEvent.click(within(card).getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalledWith(PENDING_ID);
    fireEvent.click(within(approval).getByRole("button", { name: "Approve" }));
    expect(onDecide).toHaveBeenCalledWith("p1", "approve");
    // The other tabs list recorded approvals only.
    fireEvent.click(view.getByTestId("gating-tab:audit"));
    expect(view.queryByTestId(`question-set:${PENDING_ID}`)).toBeNull();
  });

  it("keeps the questions in view when no approval waits", () => {
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])}
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    expect(view.getByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    expect(view.getByText("No pending approvals.")).toBeTruthy();
  });

  it("shows the questions in an inbox that lists its requests without the shared approval resource", () => {
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()}
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    expect(view.getByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    expect(view.getByText("No pending items.")).toBeTruthy();
  });

  it("shows a read-only caller the questions with nothing to send", () => {
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} readOnly
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    const card = view.getByTestId(`question-set:${PENDING_ID}`);
    expect(card.dataset.state).toBe("read_only");
    expect((within(card).getByRole("button", { name: "Send answers" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it.each(["unsupported", "forbidden", "loading"] as const)("shows nothing and no error for a %s question read", (status) => {
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()}
        questions={{ snapshot: sets({ status, sets: [], readOnly: true }), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    expect(view.queryByTestId("question-set-list")).toBeNull();
    expect(view.queryByRole("alert")).toBeNull();
    expect(view.queryByText(/question/i)).toBeNull();
    expect(view.getByTestId("gating-pending:p1")).toBeTruthy();
    expect(view.getByTestId("gating-tab:pending").textContent).toContain("1");
  });

  it("leaves one neutral line in the inbox when a set it showed is no longer listed", () => {
    const value = (snapshot: QuestionSetSnapshot) => ({ snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() });
    const panel = (snapshot: QuestionSetSnapshot) => (
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()} questions={value(snapshot)} />
    );
    const view = render(panel(sets()));
    expect(view.getByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    view.rerender(panel(sets({ sets: [], slots: {}, notices: [{ slot: 1, kind: "gone", atMs: 5 }] })));
    expect(view.queryByTestId(`question-set:${PENDING_ID}`)).toBeNull();
    expect(view.getByTestId("question-set-notice").textContent).toBe("This question is no longer listed here.");
    expect(view.queryByText("Release plan")).toBeNull();
    expect(view.queryByText("Which region goes first?")).toBeNull();
    // The set no longer counts with what waits; the approval still does.
    expect(view.getByTestId("gating-tab:pending").textContent).toContain("1");
    expect(view.getByTestId("gating-pending:p1")).toBeTruthy();
  });

  it("says that the questions could not be loaded, with a way to try again, and still shows the approvals", () => {
    const onRefresh = vi.fn();
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()}
        questions={{ snapshot: sets({ status: "unavailable", sets: [], slots: {}, error: "gateway timeout" }), onAnswer: vi.fn(), onDismiss: vi.fn(), onRefresh }} />,
    );
    expect(view.getByTestId("question-set-unavailable").textContent)
      .toContain("The questions agents asked could not be loaded. The Console keeps trying; you can also try now.");
    fireEvent.click(within(view.getByTestId("question-set-unavailable")).getByRole("button", { name: "Try again" }));
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(view.getByTestId("gating-pending:p1")).toBeTruthy();
    expect(view.getByTestId("gating-tab:pending").textContent).toContain("1");
    cleanup();
    // With no approval waiting, "No pending approvals." sits small beside the
    // notice: the tab is not the clear inbox's large picture.
    const empty = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])}
        questions={{ snapshot: sets({ status: "unavailable", sets: [], slots: {}, error: "gateway timeout" }), onAnswer: vi.fn(), onDismiss: vi.fn(), onRefresh }} />,
    );
    expect(empty.getByTestId("question-set-unavailable")).toBeTruthy();
    const none = empty.getByText("No pending approvals.").closest(".cc-panel-empty");
    expect(none?.classList.contains("cc-panel-empty--inline")).toBe(true);
    expect(none?.classList.contains("cc-panel-empty--art")).toBe(false);
  });

  it("keeps the empty inbox small beside a line it shows, and large when the line is of a card it never showed", () => {
    const value = (snapshot: QuestionSetSnapshot) => ({ snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() });
    const panel = (snapshot: QuestionSetSnapshot) => (
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} questions={value(snapshot)} />
    );
    const left = sets({ sets: [], slots: {}, notices: [{ slot: 1, kind: "gone", atMs: 5 }] });
    const empty = (view: ReturnType<typeof render>) => view.getByText("No pending approvals.").closest(".cc-panel-empty");
    // This inbox showed the card: the line is there and the empty state sits small beside it.
    const view = render(panel(sets()));
    view.rerender(panel(left));
    expect(view.getByTestId("question-set-notice")).toBeTruthy();
    expect(empty(view)?.classList.contains("cc-panel-empty--inline")).toBe(true);
    cleanup();
    // An inbox opened afterwards never showed the card: no line, and the clear
    // inbox's own large picture.
    const late = render(panel(left));
    expect(late.queryByTestId("question-set-notice")).toBeNull();
    expect(late.queryByTestId("question-set-list")).toBeNull();
    expect(empty(late)?.classList.contains("cc-panel-empty--inline")).toBe(false);
    expect(empty(late)?.classList.contains("cc-panel-empty--art")).toBe(true);
  });

  it("counts on the Pending tab only the sets of a list that was read, as the sidebar's badge does, and keeps the approvals' count", () => {
    const panel = (snapshot: QuestionSetSnapshot) => (
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()}
        questions={{ snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() }} />
    );
    const tabCount = (view: ReturnType<typeof render>) => view.getByTestId("gating-tab:pending").textContent?.replace(/\D/g, "");
    const view = render(panel(sets()));
    expect(tabCount(view)).toBe("2");
    expect(view.getByTestId("gating-summary-questions").textContent).toBe("1 question set");
    // The list may be out of date: its card stays and says so, and its set
    // is not counted until the list is read again. The approval still is.
    view.rerender(panel(sets({ status: "stale" })));
    expect(view.getByTestId(`question-set:${PENDING_ID}`).dataset.state).toBe("stale");
    expect(tabCount(view)).toBe("1");
    expect(view.queryByTestId("gating-summary-questions")).toBeNull();
    view.rerender(panel(sets()));
    expect(tabCount(view)).toBe("2");
  });

  it("shows a question set once on Pending: the card a person answers, with no decision card for it beside", () => {
    // The durable decision of the set, as the decisions' read lists it, and
    // a decision of another kind.
    const ofTheSet = {
      decisionId: PENDING_ID, kind: "ask_user.questions", phase: "open", approverId: "ask_user", approverName: "Ask user",
      deadlineAtMs: Date.now() + 3_600_000, rendering: "Release plan", statusOnly: false,
    };
    const other = { decisionId: `decision:${"c".repeat(64)}`, kind: "route.release", phase: "open", rendering: "Release build 7.", statusOnly: false };
    const panel = (snapshot?: QuestionSetSnapshot) => (
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} decisions={[ofTheSet, other]}
        questions={snapshot ? { snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() } : undefined} />
    );
    const view = render(panel(sets()));
    expect(view.getByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    expect(view.queryByTestId(`decision-card:${PENDING_ID}`)).toBeNull();
    // A decision of another kind keeps its card.
    expect(view.getByTestId(`decision-card:${other.decisionId}`)).toBeTruthy();
    // While the sets are read, or their list may be out of date, the set is
    // the question list's to show.
    for (const status of ["loading", "stale"] as const) {
      view.rerender(panel(sets({ status })));
      expect(view.queryByTestId(`decision-card:${PENDING_ID}`), status).toBeNull();
      expect(view.getByTestId(`decision-card:${other.decisionId}`)).toBeTruthy();
    }
    // Where the question sets are not read, the decision card is the one
    // place that shows the set.
    for (const status of ["unavailable", "forbidden", "unsupported"] as const) {
      view.rerender(panel(sets({ status, sets: [] })));
      expect(view.getByTestId(`decision-card:${PENDING_ID}`), status).toBeTruthy();
    }
    view.rerender(panel());
    expect(view.getByTestId(`decision-card:${PENDING_ID}`)).toBeTruthy();
  });

  it("shows no decision list when the question sets are the only decisions", () => {
    const ofTheSet = { decisionId: PENDING_ID, kind: "ask_user.questions", phase: "open", statusOnly: false };
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} decisions={[ofTheSet]}
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    expect(view.getByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    expect(view.queryByTestId("decision-card-list")).toBeNull();
  });

  it("keeps the decision card of a question set the question list was read without", () => {
    // Two question sets among the durable decisions: the question list
    // holds the first, and was read without the second.
    const OTHER_ID = `decision:${"d".repeat(64)}`;
    const held = { decisionId: PENDING_ID, kind: "ask_user.questions", phase: "open", rendering: "Release plan", statusOnly: false };
    const unlisted = { decisionId: OTHER_ID, kind: "ask_user.questions", phase: "open", rendering: "Another set", statusOnly: false };
    const panel = (snapshot: QuestionSetSnapshot) => (
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} decisions={[held, unlisted]}
        questions={{ snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() }} />
    );
    const view = render(panel(sets()));
    for (const status of ["ready", "stale"] as const) {
      view.rerender(panel(sets({ status })));
      // The set the list holds is shown once, as the card a person answers.
      expect(view.getByTestId(`question-set:${PENDING_ID}`), status).toBeTruthy();
      expect(view.queryByTestId(`decision-card:${PENDING_ID}`), status).toBeNull();
      // The set the list does not hold is shown too: as its decision card.
      expect(view.getByTestId(`decision-card:${OTHER_ID}`), status).toBeTruthy();
    }
    // Before the first read of the question list nothing is known of the
    // sets: no decision card of a question set is shown yet.
    view.rerender(panel(sets({ status: "loading", sets: [], slots: {} })));
    expect(view.queryByTestId(`decision-card:${PENDING_ID}`)).toBeNull();
    expect(view.queryByTestId(`decision-card:${OTHER_ID}`)).toBeNull();
    // A set this console answered is the question list's too: its closed
    // card, with no decision card beside it while the decisions' read still
    // lists it.
    view.rerender(panel(sets({ sets: [], closed: [{ set: questions, outcome: "answered", closedAtMs: Date.now() }] })));
    expect(view.queryByTestId(`decision-card:${PENDING_ID}`)).toBeNull();
    expect(view.getByTestId(`decision-card:${OTHER_ID}`)).toBeTruthy();
  });

  it("says on the decision card of a set the question list does not hold why it has no answer form, and where the set is answered", () => {
    // A question set among the durable decisions that the question list was
    // read without: its decision card is the one place that shows it, as
    // text, and nobody can answer it from there.
    const OTHER_ID = `decision:${"d".repeat(64)}`;
    const unlisted = {
      decisionId: OTHER_ID, kind: "ask_user.questions", phase: "open", approverId: "ask_user", approverName: "Ask user",
      requester: "coordinator", requesterName: "coordinator", rendering: "Plan for the week", statusOnly: false,
    };
    const other = { decisionId: `decision:${"c".repeat(64)}`, kind: "route.release", phase: "open", rendering: "Release build 7.", statusOnly: false };
    const panel = (snapshot?: QuestionSetSnapshot) => (
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} decisions={[unlisted, other]}
        questions={snapshot ? { snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() } : undefined} />
    );
    const hintOf = (view: ReturnType<typeof render>, id: string) =>
      view.getByTestId(`decision-card:${id}`).querySelector(".cc-decision__hint")?.textContent;
    const NOT_LISTED = "This card has no answer form: the list of questions agents asked does not hold this set. "
      + "It is answered in coordinator's conversation, or here once the list holds it.";
    const NOT_READ = "This card has no answer form: the questions agents asked could not be read here. "
      + "This set is answered in coordinator's conversation, or here once they can be read.";

    const view = render(panel(sets()));
    // The list is read and does not hold the set.
    for (const status of ["ready", "stale"] as const) {
      view.rerender(panel(sets({ status })));
      expect(hintOf(view, OTHER_ID), status).toBe(NOT_LISTED);
    }
    // The question sets are not read at all.
    for (const status of ["unavailable", "forbidden", "unsupported"] as const) {
      view.rerender(panel(sets({ status, sets: [], slots: {} })));
      expect(hintOf(view, OTHER_ID), status).toBe(NOT_READ);
    }
    view.rerender(panel());
    expect(hintOf(view, OTHER_ID)).toBe(NOT_READ);
    // Never the words that name no place for a set that was presented nowhere.
    expect(view.getByTestId(`decision-card:${OTHER_ID}`).textContent).not.toContain("Decide where it was presented.");
    // A decision of another kind is not a question set: its hint is as it was.
    expect(hintOf(view, other.decisionId)).toBe("Decide where it was presented.");
  });

  it("names the asking agent in general words where the decision does not say who asked", () => {
    const OTHER_ID = `decision:${"d".repeat(64)}`;
    const unlisted = { decisionId: OTHER_ID, kind: "ask_user.questions", phase: "open", rendering: "Plan for the week", statusOnly: false };
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} decisions={[unlisted]}
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    expect(view.getByTestId(`decision-card:${OTHER_ID}`).querySelector(".cc-decision__hint")?.textContent)
      .toBe("This card has no answer form: the list of questions agents asked does not hold this set. "
        + "It is answered in the asking agent's conversation, or here once the list holds it.");
  });

  it("draws a question set's rendering without the closing note the gateway names beside it", async () => {
    // The gateway ends the rendering of an open question set with a note for
    // a page that cannot answer the set, and names what it added in
    // `rendering_suffix`. This page says in its own words where the set is
    // answered, so it draws the rendering as it is held, without the note.
    const { decisionCardsOf, renderingOf } = await import("./decision-cards");
    const OTHER_ID = `decision:${"d".repeat(64)}`;
    const HELD = "Plan for the week\n\n1. Which day?\n   - Monday\n   - Friday";
    const NOTE = "\n\nAnswer these questions in the Console, in the agent's chat or under Approvals. "
      + "If this page offers no way to answer them, it runs an earlier Console: reload the page, or, "
      + "in an application that builds the Console in, update its Console.";
    const answer = {
      decision_id: OTHER_ID, rendering: HELD + NOTE, renderer: "ask_user.v1",
      subject_digest: `sha256:${"e".repeat(64)}`, parts: [], rendering_suffix: NOTE,
    };
    const page = {
      decisions: [{ decision_id: OTHER_ID, kind: "ask_user.questions", phase: "open", approver_id: "ask_user", requester: "coordinator" }],
      statuses: [],
    };
    const rendering = renderingOf(answer);
    const cards = decisionCardsOf(page, new Map(rendering === undefined ? [] : [[OTHER_ID, rendering]]));
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} decisions={cards}
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    const card = view.getByTestId(`decision-card:${OTHER_ID}`);
    expect(within(card).getByTestId("decision-card-rendering").textContent).toBe(HELD);
    expect(card.textContent).not.toContain("it runs an earlier Console");
    expect(card.querySelector(".cc-decision__hint")?.textContent)
      .toBe("This card has no answer form: the list of questions agents asked does not hold this set. "
        + "It is answered in coordinator's conversation, or here once the list holds it.");
    // An answer that names no added text is drawn whole, whatever it ends with.
    expect(renderingOf({ rendering: HELD + NOTE })).toBe(HELD + NOTE);
    // Only the text the answer ends with is cut.
    expect(renderingOf({ rendering: HELD, rendering_suffix: NOTE })).toBe(HELD);
    expect(renderingOf({ rendering: HELD, rendering_suffix: "" })).toBe(HELD);
  });

  it("is unchanged without the question sets prop", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()} />);
    expect(view.queryByTestId("question-set-list")).toBeNull();
    expect(view.getByTestId("gating-pending:p1")).toBeTruthy();
  });
});
