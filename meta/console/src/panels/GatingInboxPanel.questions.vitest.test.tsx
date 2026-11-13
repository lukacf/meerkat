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
  scopeKey: "scope", status: "ready", sets: [questions], closed: [], decisions: {}, readOnly: false, ...extra,
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

  it("shows a read-only caller the questions with nothing to send", () => {
    const view = render(
      <GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals([])} readOnly
        questions={{ snapshot: sets(), onAnswer: vi.fn(), onDismiss: vi.fn() }} />,
    );
    const card = view.getByTestId(`question-set:${PENDING_ID}`);
    expect(card.dataset.state).toBe("not_permitted");
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

  it("is unchanged without the question sets prop", () => {
    const view = render(<GatingInboxPanel pending={[]} audit={[]} onDecide={vi.fn()} resource={approvals()} />);
    expect(view.queryByTestId("question-set-list")).toBeNull();
    expect(view.getByTestId("gating-pending:p1")).toBeTruthy();
  });
});
