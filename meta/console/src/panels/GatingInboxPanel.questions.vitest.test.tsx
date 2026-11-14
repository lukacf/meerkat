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

  it("is unchanged without the question sets prop", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()} />);
    expect(view.queryByTestId("question-set-list")).toBeNull();
    expect(view.getByTestId("gating-pending:p1")).toBeTruthy();
  });
});
