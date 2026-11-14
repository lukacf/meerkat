import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentDirectoryProvider } from "../panel-kit";
import { createAgentDirectory } from "../panel-kit/agents";
import {
  normalizeQuestionSet,
  type QuestionSet,
  type QuestionSetDecisionState,
  type QuestionSetSnapshot,
} from "../../../console-core/src/pending-questions";
import { ConversationApprovals } from "./conversation-approvals";
import { ConversationQuestionSets, QuestionSetCard, QuestionSetList, QuestionSetsProvider, type QuestionSetCardProps } from "./question-set-card";

afterEach(cleanup);

const PENDING_ID = `decision:${"a".repeat(64)}`;
const DIGEST = `sha256:${"b".repeat(64)}`;
const IDENTITY = "identity:release-planner";
const wire = (extra: Record<string, unknown> = {}) => ({
  pending_id: PENDING_ID,
  origin: { identity: IDENTITY },
  title: "Release plan",
  questions: [
    { id: "q-region", prompt: "Which region goes first?", choices: [{ id: "c-eu", label: "Europe" }, { id: "c-us", label: "United States" }], recommended: "c-eu" },
    { id: "q-start", prompt: "When should it start?", choices: [{ id: "c-now", label: "Now" }, { id: "c-night", label: "Tonight" }], allow_free_text: true },
  ],
  created_at_ms: Date.now() - 5 * 60_000,
  deadline_at_ms: Date.now() + 55 * 60_000,
  subject_digest: DIGEST,
  ...extra,
});
const questions = (extra: Record<string, unknown> = {}): QuestionSet => normalizeQuestionSet(wire(extra))!;
const directory = createAgentDirectory([{ id: IDENTITY, label: "Release Planner" }]);

function show(props: Partial<QuestionSetCardProps> = {}) {
  const onAnswer = vi.fn(), onDismiss = vi.fn();
  const view = render(
    <AgentDirectoryProvider directory={directory}>
      <QuestionSetCard set={questions()} resourceStatus="ready" onAnswer={onAnswer} onDismiss={onDismiss} {...props} />
    </AgentDirectoryProvider>,
  );
  const card = view.getByTestId(`question-set:${PENDING_ID}`);
  return { view, card, onAnswer, onDismiss };
}
const send = (view: ReturnType<typeof render>) => view.getByRole("button", { name: "Send answers" }) as HTMLButtonElement;
const dismiss = (view: ReturnType<typeof render>) => view.getByRole("button", { name: "Dismiss" }) as HTMLButtonElement;
const radio = (view: ReturnType<typeof render>, name: string | RegExp) => view.getByRole("radio", { name }) as HTMLInputElement;
/** The question groups (a `details` element is a group too). */
const groups = (view: ReturnType<typeof render>) => view.getAllByRole("group").filter((group) => group.tagName === "FIELDSET");
/** What a person reads on the card with its Technical details closed. */
function visibleText(card: HTMLElement): string {
  const copy = card.cloneNode(true) as HTMLElement;
  copy.querySelectorAll("details").forEach((details) => details.remove());
  return `${copy.textContent} ${Array.from(copy.querySelectorAll("[title], [aria-label]"))
    .map((element) => `${element.getAttribute("title") ?? ""} ${element.getAttribute("aria-label") ?? ""}`).join(" ")}`;
}

describe("QuestionSetCard", () => {
  it("shows a pending set: its title, who asked, a status chip, the deadline and one group per question", () => {
    const { view, card } = show();
    expect(card.dataset.state).toBe("pending");
    expect(card.classList.contains("cc-approval")).toBe(true);
    expect(view.getByText("Release plan").classList.contains("cc-approval__title")).toBe(true);
    expect(view.getByTestId(`question-set-asked:${PENDING_ID}`).textContent).toContain("Asked by Release Planner");
    expect(view.getByRole("status").textContent).toBe("Answer needed");
    expect(view.getByTestId(`question-set-deadline:${PENDING_ID}`).textContent).toMatch(/^in \d+m/);
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("Pick one choice per question. A question you leave out reaches the agent as unanswered.");
    const [region, start] = groups(view);
    expect(groups(view).length).toBe(2);
    expect(view.getByRole("group", { name: "Which region goes first?" })).toBe(region);
    expect(region.querySelector("legend")?.textContent).toBe("Which region goes first?");
    expect(within(region).getAllByRole("radio").length).toBe(2);
    expect(within(start).getAllByRole("radio").length).toBe(3);
  });

  it("falls back to the asking agent's name for a set without a title", () => {
    const { view } = show({ set: questions({ title: undefined }) });
    expect(view.getByText("Questions from Release Planner").classList.contains("cc-approval__title")).toBe(true);
  });

  it("marks the recommended choice with a chip and preselects nothing", () => {
    const { view } = show();
    const chips = view.getAllByText("Recommended");
    expect(chips.length).toBe(1);
    expect(chips[0].classList.contains("cc-chip")).toBe(true);
    expect(chips[0].closest("label")?.textContent).toContain("Europe");
    expect((view.getAllByRole("radio") as HTMLInputElement[]).some((input) => input.checked)).toBe(false);
    expect(send(view).disabled).toBe(true);
  });

  it("offers free text only where the agent allowed it, behind a last choice that reveals the field", () => {
    const { view } = show();
    const [region, start] = groups(view);
    expect(within(region).queryByRole("radio", { name: "Something else" })).toBeNull();
    const radios = within(start).getAllByRole("radio");
    expect(radios[radios.length - 1]).toBe(within(start).getByRole("radio", { name: "Something else" }));
    expect(view.queryByRole("textbox")).toBeNull();
    fireEvent.click(radios[radios.length - 1]);
    expect(view.getByRole("textbox", { name: "Your answer to: When should it start?" })).toBeTruthy();
    fireEvent.click(radio(view, "Now"));
    expect(view.queryByRole("textbox")).toBeNull();
  });

  it("sends once a question has an answer, and not while a selected free text is empty or too long", () => {
    const { view, onAnswer } = show();
    expect(send(view).disabled).toBe(true);
    fireEvent.click(radio(view, /^Europe/));
    expect(send(view).disabled).toBe(false);
    fireEvent.click(radio(view, "Something else"));
    expect(send(view).disabled).toBe(true);
    const field = view.getByRole("textbox") as HTMLTextAreaElement;
    fireEvent.change(field, { target: { value: "   " } });
    expect(send(view).disabled).toBe(true);
    fireEvent.change(field, { target: { value: "After the freeze" } });
    expect(send(view).disabled).toBe(false);
    expect(view.queryByTestId(`question-free-count:${PENDING_ID}:q-start`)).toBeNull();
    // 141 two-byte letters are 282 bytes: over the 280 byte limit.
    fireEvent.change(field, { target: { value: "å".repeat(141) } });
    expect(send(view).disabled).toBe(true);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(view.getByTestId(`question-free-count:${PENDING_ID}:q-start`).textContent).toBe("Too long: 282 of 280 bytes");
    fireEvent.change(field, { target: { value: "å".repeat(130) } });
    expect(view.getByTestId(`question-free-count:${PENDING_ID}:q-start`).textContent).toBe("260 of 280 bytes");
    expect(send(view).disabled).toBe(false);
    fireEvent.click(send(view));
    expect(onAnswer).toHaveBeenCalledWith(PENDING_ID, {
      "q-region": { kind: "choice", choiceId: "c-eu" },
      "q-start": { kind: "free_text", text: "å".repeat(130) },
    });
  });

  it("lets a person clear a question's selection and leave questions out", () => {
    const { view, onAnswer } = show();
    expect(view.queryByRole("button", { name: /^Clear the answer/ })).toBeNull();
    fireEvent.click(radio(view, /^Europe/));
    fireEvent.click(radio(view, "Tonight"));
    fireEvent.click(view.getByRole("button", { name: "Clear the answer to: Which region goes first?" }));
    expect(radio(view, /^Europe/).checked).toBe(false);
    fireEvent.click(send(view));
    expect(onAnswer).toHaveBeenCalledWith(PENDING_ID, { "q-start": { kind: "choice", choiceId: "c-night" } });
    fireEvent.click(view.getByRole("button", { name: "Clear the answer to: When should it start?" }));
    expect(send(view).disabled).toBe(true);
  });

  it("dismisses the set and disables both actions while a decision is on its way", () => {
    const { view, onAnswer, onDismiss } = show();
    fireEvent.click(dismiss(view));
    expect(onDismiss).toHaveBeenCalledWith(PENDING_ID);
    fireEvent.click(radio(view, /^Europe/));
    view.rerender(
      <AgentDirectoryProvider directory={directory}>
        <QuestionSetCard set={questions()} resourceStatus="ready" onAnswer={onAnswer} onDismiss={onDismiss}
          decision={{ phase: "submitting", action: "answer" }} />
      </AgentDirectoryProvider>,
    );
    expect(view.getByTestId(`question-set:${PENDING_ID}`).dataset.state).toBe("submitting");
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
  });

  it("keeps radios native: one name per question, so arrow keys and Tab work as in any radio group", () => {
    const { view } = show();
    const [region, start] = groups(view);
    const names = (group: HTMLElement) => new Set(within(group).getAllByRole("radio").map((input) => (input as HTMLInputElement).name));
    expect(names(region).size).toBe(1);
    expect(names(start).size).toBe(1);
    expect([...names(region)][0]).not.toBe([...names(start)][0]);
    for (const input of view.getAllByRole("radio")) {
      expect(input.tagName).toBe("INPUT");
      expect(input.getAttribute("type")).toBe("radio");
      expect(input.getAttribute("tabindex")).toBeNull();
    }
  });

  it("shows what was sent once answered", () => {
    const decision: QuestionSetDecisionState = { phase: "answered", action: "answer", decidedAtMs: 5, answers: {
      "q-start": { kind: "free_text", text: "After the <b>freeze</b>" },
    } };
    const { view, card } = show({ decision, closed: "answered" });
    expect(card.dataset.state).toBe("answered");
    expect(view.getByRole("status").textContent).toBe("Answered");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Answers sent.");
    const summary = view.getByTestId(`question-set-summary:${PENDING_ID}`);
    expect(within(summary).getByText("Which region goes first?").nextElementSibling?.textContent).toBe("Not answered");
    expect(within(summary).getByText("When should it start?").nextElementSibling?.textContent).toBe("After the <b>freeze</b>");
    expect(summary.querySelector("b")).toBeNull();
    expect(view.queryByRole("radio")).toBeNull();
    expect(view.queryByRole("button", { name: "Send answers" })).toBeNull();
    expect(view.queryByRole("button", { name: "Dismiss" })).toBeNull();
  });

  it("says a dismissed set was dismissed", () => {
    const { view, card } = show({ decision: { phase: "dismissed", action: "dismiss" }, closed: "dismissed" });
    expect(card.dataset.state).toBe("dismissed");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Dismissed. The agent was told no answer is coming.");
    expect(view.queryByRole("button", { name: "Send answers" })).toBeNull();
  });

  it("keeps a listed set answerable when the browser clock is past its deadline, and never says it timed out", () => {
    const { view, card, onAnswer } = show({ set: questions({ deadline_at_ms: Date.now() - 60_000 }) });
    expect(card.dataset.state).toBe("pending");
    expect(view.getByRole("status").textContent).toBe("Answer needed");
    // The deadline is the gateway's: past it on this clock the chip names the
    // time and counts nothing down.
    expect(view.getByTestId(`question-set-deadline:${PENDING_ID}`).textContent).toMatch(/^Answer by /);
    expect(visibleText(card)).not.toMatch(/timed out|expired/i);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => !input.disabled)).toBe(true);
    fireEvent.click(radio(view, /^Europe/));
    expect(send(view).disabled).toBe(false);
    expect(dismiss(view).disabled).toBe(false);
    fireEvent.click(send(view));
    expect(onAnswer).toHaveBeenCalledWith(PENDING_ID, { "q-region": { kind: "choice", choiceId: "c-eu" } });
  });

  it("tells a caller who is not signed in as a person to sign in, with the questions shown and disabled", () => {
    const { view, card, onAnswer } = show({ readOnly: true, readOnlyReason: "sign_in" });
    expect(card.dataset.state).toBe("not_permitted");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Sign in as yourself to answer");
    expect(view.getAllByRole("radio").length).toBe(5);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
    fireEvent.click(send(view));
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("tells a read-only caller they cannot answer for the agent", () => {
    const { view, card } = show({ readOnly: true });
    expect(card.dataset.state).toBe("not_permitted");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("You cannot answer for this agent");
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
  });

  it.each([
    [-32030, "access_denied", "You cannot answer for this agent", true],
    [-32057, "approver_unauthenticated", "Sign in as yourself to answer", true],
    [-32602, "refused", "This question set is no longer open, or the answer was not accepted.", false],
  ] as const)("keeps the card and says why after a refused decision (%i)", (_code, kind, sentence, locked) => {
    const { view, card } = show({ decision: {
      phase: "refused", action: "answer", error: sentence,
      refusal: { kind, words: sentence, detail: "the gateway's own message" },
    } });
    expect(card.dataset.state).toBe("not_permitted");
    expect(view.getByRole("status").textContent).toBe("Not permitted");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe(sentence);
    expect(groups(view).length).toBe(2);
    expect(dismiss(view).disabled).toBe(locked);
    const details = view.queryByTestId(`question-set-refusal:${PENDING_ID}`);
    if (kind === "refused") {
      expect(details?.tagName).toBe("DETAILS");
      expect(details?.hasAttribute("open")).toBe(false);
      expect(details?.querySelector("summary")?.textContent).toBe("Details");
      expect(details?.textContent).toContain("the gateway's own message");
    } else {
      expect(details).toBeNull();
    }
  });

  it("says when an answer may not have arrived and when the list may be out of date", () => {
    const failed = show({ decision: { phase: "failed", action: "answer", error: "network" } });
    expect(failed.card.dataset.state).toBe("failed");
    expect(failed.view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("The Console could not confirm that your answer arrived. If this question is still listed after the next refresh, send your answer again.");
    cleanup();
    const stale = show({ resourceStatus: "stale" });
    expect(stale.card.dataset.state).toBe("stale");
    expect(send(stale.view).disabled).toBe(true);
    expect(dismiss(stale.view).disabled).toBe(true);
    cleanup();
    // An uncertain send stays uncertain when the caller's access then became
    // read-only: the card keeps saying so and offers nothing to send.
    const locked = show({ decision: { phase: "failed", action: "answer", error: "network" }, readOnly: true });
    expect(locked.card.dataset.state).toBe("failed");
    expect(dismiss(locked.view).disabled).toBe(true);
    expect((locked.view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
  });

  it("puts names first: the identity and every id only inside the collapsed Technical details", () => {
    const { view, card } = show();
    const details = view.getByTestId(`question-set-details:${PENDING_ID}`);
    expect(details.tagName).toBe("DETAILS");
    expect(details.hasAttribute("open")).toBe(false);
    expect(details.querySelector("summary")?.textContent).toBe("Technical details");
    for (const id of [IDENTITY, PENDING_ID, DIGEST, "q-region", "q-start", "c-eu", "c-us", "c-now", "c-night"]) {
      expect(details.textContent).toContain(id);
      expect(visibleText(card)).not.toContain(id);
    }
    expect(visibleText(card)).toContain("Release Planner");
  });

  it("renders every title, prompt and label as text", () => {
    const markup = questions({
      title: "<b>Plan</b> [link](https://example.test)",
      questions: [{ id: "q1", prompt: "<i>Which</i> one?", choices: [{ id: "a", label: "<b>Bold</b> choice" }, { id: "b", label: "**Plain** choice" }] }],
    });
    const { view, card } = show({ set: markup });
    expect(view.getByText("<b>Plan</b> [link](https://example.test)")).toBeTruthy();
    expect(view.getByRole("group", { name: "<i>Which</i> one?" })).toBeTruthy();
    expect(view.getByRole("radio", { name: "<b>Bold</b> choice" })).toBeTruthy();
    expect(view.getByRole("radio", { name: "**Plain** choice" })).toBeTruthy();
    expect(card.querySelector("b, i, a, strong:not(.cc-approval__title)")).toBeNull();
  });
});

const BILLING_ID = `decision:${"c".repeat(64)}`;
/** Another agent's set, with no word or id of the first one. */
const billing = () => questions({
  pending_id: BILLING_ID, origin: { identity: "identity:billing" }, title: "Billing check", subject_digest: `sha256:${"d".repeat(64)}`,
  questions: [{ id: "b-refund", prompt: "Refund the order?", choices: [{ id: "b-yes", label: "Yes" }, { id: "b-no", label: "No" }] }],
});
const listSnapshot = (extra: Partial<QuestionSetSnapshot> = {}): QuestionSetSnapshot => ({
  scopeKey: "a", status: "ready", readOnly: false, decisions: {}, closed: [], notices: [],
  sets: [questions(), billing()],
  slots: { [PENDING_ID]: 1, [BILLING_ID]: 2 },
  ...extra,
});

describe("QuestionSetList", () => {
  const list = (snapshot: QuestionSetSnapshot, identity?: string, testId = "list") => (
    <AgentDirectoryProvider directory={directory}>
      <QuestionSetList view={{ snapshot, onAnswer: vi.fn(), onDismiss: vi.fn() }} identity={identity} testId={testId} />
    </AgentDirectoryProvider>
  );
  const SET_WORDS = [
    PENDING_ID, IDENTITY, DIGEST, "Release plan", "Release Planner", "Which region goes first?", "When should it start?",
    "Europe", "United States", "Tonight", "q-region", "q-start", "c-eu", "c-night",
  ];

  it("takes the card of a set that left along and leaves one neutral line that names nothing of it", () => {
    const view = render(list(listSnapshot()));
    expect(view.getByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    expect(view.queryByTestId("question-set-notice")).toBeNull();
    view.rerender(list(listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 }, notices: [{ slot: 1, kind: "gone", atMs: 5 }] })));
    expect(view.queryByTestId(`question-set:${PENDING_ID}`)).toBeNull();
    expect(view.getByTestId(`question-set:${BILLING_ID}`)).toBeTruthy();
    const lines = view.getAllByTestId("question-set-notice");
    expect(lines.length).toBe(1);
    expect(lines[0].textContent).toBe("This question is no longer waiting for an answer.");
    expect(lines[0].getAttribute("role")).toBe("status");
    // Nothing of the set is left in the page: no name, text or id, shown or hidden.
    for (const word of SET_WORDS) expect(view.container.innerHTML).not.toContain(word);
    expect(view.container.querySelector("[data-state='not_permitted'], [data-state='timed_out']")).toBeNull();
  });

  it("says in the line when the set left after a send the Console could not confirm", () => {
    const view = render(list(listSnapshot({ decisions: { [PENDING_ID]: { phase: "failed", action: "answer", error: "network" } } })));
    view.rerender(list(listSnapshot({ sets: [], slots: {}, notices: [{ slot: 1, kind: "sent_unconfirmed", atMs: 5 }] })));
    expect(view.queryByTestId(`question-set:${PENDING_ID}`)).toBeNull();
    expect(view.getByTestId("question-set-notice").textContent)
      .toBe("This question is no longer waiting for an answer. The Console could not confirm whether your answer was the one that arrived.");
    for (const word of SET_WORDS) expect(view.container.innerHTML).not.toContain(word);
  });

  it("shows the line only where the card was shown, and at most one", () => {
    const mine = render(list(listSnapshot(), IDENTITY, "mine"));
    const other = render(list(listSnapshot(), "identity:billing", "other"));
    const left = listSnapshot({ sets: [], slots: {}, notices: [{ slot: 2, kind: "gone", atMs: 9 }, { slot: 1, kind: "sent_unconfirmed", atMs: 5 }] });
    mine.rerender(list(left, IDENTITY, "mine"));
    other.rerender(list(left, "identity:billing", "other"));
    expect(within(mine.getByTestId("mine")).getAllByTestId("question-set-notice").map((line) => line.textContent))
      .toEqual(["This question is no longer waiting for an answer. The Console could not confirm whether your answer was the one that arrived."]);
    expect(within(other.getByTestId("other")).getAllByTestId("question-set-notice").map((line) => line.textContent))
      .toEqual(["This question is no longer waiting for an answer."]);
    cleanup();
    // The inbox showed both cards: it shows one line, the newest.
    const inbox = render(list(listSnapshot()));
    inbox.rerender(list(left));
    expect(inbox.getAllByTestId("question-set-notice").map((line) => line.textContent)).toEqual(["This question is no longer waiting for an answer."]);
    cleanup();
    // A list that never showed the card says nothing of it.
    const late = render(list(left));
    expect(late.container.textContent).toBe("");
  });
});

describe("ConversationQuestionSets", () => {
  const snapshot = listSnapshot;

  it("shows the sets of the conversation's agent, and answers through the provider", () => {
    const onAnswer = vi.fn(), onDismiss = vi.fn();
    const view = render(
      <QuestionSetsProvider value={{ snapshot: snapshot(), onAnswer, onDismiss }}>
        <ConversationQuestionSets identity={IDENTITY} />
      </QuestionSetsProvider>,
    );
    expect(view.getByText("Release plan")).toBeTruthy();
    expect(view.queryByText("Billing check")).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalledWith(PENDING_ID);
  });

  it("sits with the conversation's own approvals, once: never beside a turn's approvals", () => {
    const value = { snapshot: snapshot(), onAnswer: vi.fn(), onDismiss: vi.fn() };
    const view = render(
      <QuestionSetsProvider value={value}>
        <ConversationApprovals approvalIdentity={IDENTITY} interactionIds={["interaction-1"]} />
        <ConversationApprovals approvalIdentity={IDENTITY} interactionIds={[]} />
        <ConversationApprovals approvalIdentity={IDENTITY} />
      </QuestionSetsProvider>,
    );
    expect(view.getAllByTestId(`question-set:${PENDING_ID}`).length).toBe(1);
  });

  it("shows nothing without a provider, for another agent, or on a gateway without question sets", () => {
    const bare = render(<ConversationQuestionSets identity={IDENTITY} />);
    expect(bare.container.textContent).toBe("");
    cleanup();
    const other = render(
      <QuestionSetsProvider value={{ snapshot: snapshot(), onAnswer: vi.fn(), onDismiss: vi.fn() }}>
        <ConversationQuestionSets identity="identity:nobody" />
      </QuestionSetsProvider>,
    );
    expect(other.container.textContent).toBe("");
    cleanup();
    const unsupported = render(
      <QuestionSetsProvider value={{ snapshot: snapshot({ status: "unsupported", sets: [], readOnly: true }), onAnswer: vi.fn(), onDismiss: vi.fn() }}>
        <ConversationQuestionSets identity={IDENTITY} />
      </QuestionSetsProvider>,
    );
    expect(unsupported.container.textContent).toBe("");
    expect(unsupported.queryByRole("alert")).toBeNull();
  });
});
