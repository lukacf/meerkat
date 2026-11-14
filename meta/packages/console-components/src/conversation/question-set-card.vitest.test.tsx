import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentDirectoryProvider } from "../panel-kit";
import { ConversationPresentationProvider, ConversationRowStateScope } from "./presentation-policy";
import { createAgentDirectory } from "../panel-kit/agents";
import {
  QUESTION_SET_REFUSAL_WORDS,
  normalizeQuestionSet,
  questionSetAnswerParams,
  questionSetRefusal,
  type QuestionSet,
  type QuestionSetDecisionState,
  type QuestionSetSnapshot,
} from "../../../console-core/src/pending-questions";
import { normalizePendingApproval, type PendingApprovalSnapshot } from "../../../console-core/src/pending-approvals";
import { ApprovalAttention } from "./approval-card";
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
    // What to do, where the answers go, what happens next, and by when.
    const hint = view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent ?? "";
    expect(hint.startsWith("Pick one choice per question, then send. Your answers go to Release Planner, which continues with them; "
      + "a question you leave out reaches it as unanswered. If nobody answers by ")).toBe(true);
    expect(hint.endsWith(", Release Planner is told that the questions timed out.")).toBe(true);
    const [region, start] = groups(view);
    expect(groups(view).length).toBe(2);
    expect(view.getByRole("group", { name: "Which region goes first?" })).toBe(region);
    expect(region.querySelector("legend")?.textContent).toBe("Which region goes first?");
    expect(within(region).getAllByRole("radio").length).toBe(2);
    expect(within(start).getAllByRole("radio").length).toBe(3);
    cleanup();
    const undated = show({ set: questions({ deadline_at_ms: undefined }) });
    expect(undated.view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("Pick one choice per question, then send. Your answers go to Release Planner, which continues with them; a question you leave out reaches it as unanswered.");
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
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Sending your answers to Release Planner.");
    view.rerender(
      <AgentDirectoryProvider directory={directory}>
        <QuestionSetCard set={questions()} resourceStatus="ready" onAnswer={onAnswer} onDismiss={onDismiss}
          decision={{ phase: "submitting", action: "dismiss" }} />
      </AgentDirectoryProvider>,
    );
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Telling Release Planner that no answer is coming.");
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
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Your answers were sent to Release Planner, which continues with them.");
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
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe("Dismissed: no answer is coming, and Release Planner was told so.");
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
    expect(card.dataset.state).toBe("read_only");
    expect(view.getByRole("status").textContent).toBe("Sign in to answer");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("Sign in as yourself to answer. Only a signed-in person can answer an agent's questions.");
    expect(view.getAllByRole("radio").length).toBe(5);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
    fireEvent.click(send(view));
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("tells a caller with read-only access that the questions can be read and not answered", () => {
    const { view, card } = show({ readOnly: true });
    expect(card.dataset.state).toBe("read_only");
    expect(view.getByRole("status").textContent).toBe("Read only");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("You can read these questions but not answer them: your access to this Console is read-only.");
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
  });

  it("tells a person who lost the permission to decide what to ask for, on every card", () => {
    const { view, card } = show({ readOnly: true, readOnlyReason: "no_permission" });
    expect(card.dataset.state).toBe("read_only");
    expect(view.getByRole("status").textContent).toBe("No permission");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("You no longer have permission to answer questions here. Ask an administrator for the permission to decide.");
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
  });

  it.each([
    ["not_accepted", true],
    ["not_taken", true],
    ["subject_changed", false],
    ["not_eligible", false],
    ["approver_unauthenticated", false],
    ["access_denied", false],
    ["caller_changed", false],
    ["caller_not_current", false],
    ["not_sent", false],
  ] as const)("keeps the card after a refusal its set outlives (%s): not sent, why, what to do, and the person may send again", (kind, details) => {
    const sentence = QUESTION_SET_REFUSAL_WORDS[kind];
    expect(sentence.length).toBeGreaterThan(20);
    const { view, card, onDismiss } = show({ decision: {
      phase: "refused", action: "answer", error: sentence,
      refusal: { kind, words: sentence, ...(details ? { detail: "the gateway's own message" } : {}) },
    } });
    expect(card.dataset.state).toBe("refused");
    expect(view.getByRole("status").textContent).toBe("Not sent");
    expect(visibleText(card)).not.toMatch(/not permitted/i);
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe(sentence);
    expect(view.getByRole("alert").textContent).toContain(sentence);
    expect(groups(view).length).toBe(2);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => !input.disabled)).toBe(true);
    fireEvent.click(dismiss(view));
    expect(onDismiss).toHaveBeenCalledWith(PENDING_ID);
    const disclosure = view.queryByTestId(`question-set-refusal:${PENDING_ID}`);
    if (details) {
      expect(disclosure?.tagName).toBe("DETAILS");
      expect(disclosure?.hasAttribute("open")).toBe(false);
      expect(disclosure?.querySelector("summary")?.textContent).toBe("Details");
      expect(disclosure?.textContent).toContain("the gateway's own message");
    } else {
      expect(disclosure).toBeNull();
    }
  });

  it("says the list may be out of date before what an earlier reply said of the set", () => {
    const sentence = QUESTION_SET_REFUSAL_WORDS.not_accepted;
    const { view, card } = show({
      resourceStatus: "stale",
      decision: { phase: "refused", action: "answer", error: sentence, refusal: { kind: "not_accepted", words: sentence, detail: "gating decision failed" } },
    });
    expect(card.dataset.state).toBe("stale");
    expect(view.getByRole("status").textContent).toBe("May be out of date");
    expect(visibleText(card)).not.toContain(sentence);
    expect(view.queryByTestId(`question-set-refusal:${PENDING_ID}`)).toBeNull();
    expect(card.textContent).not.toContain("gating decision failed");
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
  });

  it("offers nothing to send on a refused card while the cards are read-only, and still says why it was not sent", () => {
    const sentence = QUESTION_SET_REFUSAL_WORDS.access_denied;
    const { view, card } = show({
      readOnly: true, readOnlyReason: "no_permission",
      decision: { phase: "refused", action: "answer", error: sentence, refusal: { kind: "access_denied", words: sentence } },
    });
    expect(card.dataset.state).toBe("refused");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toBe(sentence);
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
  });

  it("sends what it shows as chosen for any question id the gateway admits, also the names every object has", () => {
    const ids = ["__proto__", "constructor", "tostring"];
    const named = questions({ questions: ids.map((id, index) => ({
      id, prompt: `Question ${index + 1}?`, choices: [{ id: "yes", label: `Yes ${index + 1}` }, { id: "no", label: `No ${index + 1}` }],
    })) });
    const { view, onAnswer } = show({ set: named });
    // Nothing is chosen: no question reads as answered.
    expect((view.getAllByRole("radio") as HTMLInputElement[]).some((input) => input.checked)).toBe(false);
    expect(view.queryByRole("button", { name: /^Clear the answer/ })).toBeNull();
    expect(send(view).disabled).toBe(true);
    // The question whose id names an object's prototype, alone: Send is offered.
    fireEvent.click(radio(view, "Yes 1"));
    expect(radio(view, "Yes 1").checked).toBe(true);
    expect(send(view).disabled).toBe(false);
    fireEvent.click(radio(view, "No 2"));
    fireEvent.click(radio(view, "Yes 3"));
    expect(view.getAllByRole("button", { name: /^Clear the answer/ }).length).toBe(3);
    fireEvent.click(send(view));
    expect(onAnswer).toHaveBeenCalledTimes(1);
    // What the card showed as chosen is what the request carries.
    const chosen = onAnswer.mock.calls[0][1];
    expect(questionSetAnswerParams(named, chosen).answers).toEqual([
      { question_id: "__proto__", answer: { kind: "choice", choice_id: "yes" } },
      { question_id: "constructor", answer: { kind: "choice", choice_id: "no" } },
      { question_id: "tostring", answer: { kind: "choice", choice_id: "yes" } },
    ]);
    fireEvent.click(view.getByRole("button", { name: "Clear the answer to: Question 1?" }));
    expect(radio(view, "Yes 1").checked).toBe(false);
  });

  it("drops from the draft what a set that changed no longer offers", () => {
    const { view, onAnswer, onDismiss } = show();
    fireEvent.click(radio(view, /^Europe/));
    fireEvent.click(radio(view, "Tonight"));
    const changed = questions({
      subject_digest: `sha256:${"e".repeat(64)}`,
      questions: [
        { id: "q-region", prompt: "Which region goes first?", choices: [{ id: "c-us", label: "United States" }, { id: "c-asia", label: "Asia" }] },
        { id: "q-start", prompt: "When should it start?", choices: [{ id: "c-now", label: "Now" }, { id: "c-night", label: "Tonight" }], allow_free_text: true },
      ],
    });
    view.rerender(
      <AgentDirectoryProvider directory={directory}>
        <QuestionSetCard set={changed} resourceStatus="ready" onAnswer={onAnswer} onDismiss={onDismiss} />
      </AgentDirectoryProvider>,
    );
    const [region] = groups(view);
    expect((within(region).getAllByRole("radio") as HTMLInputElement[]).some((input) => input.checked)).toBe(false);
    expect(radio(view, "Tonight").checked).toBe(true);
    fireEvent.click(send(view));
    expect(onAnswer).toHaveBeenCalledWith(PENDING_ID, { "q-start": { kind: "choice", choiceId: "c-night" } });
  });

  it("says what could not be confirmed in the words of what was sent, and offers a resend only where one can be sent", () => {
    const sentence = (view: ReturnType<typeof render>) => view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent;
    const failed = show({ decision: { phase: "failed", action: "answer", error: "network" } });
    expect(failed.card.dataset.state).toBe("failed");
    expect(sentence(failed.view))
      .toBe("The Console could not confirm that your answer arrived. If this question is still listed after the next refresh, send your answer again.");
    cleanup();
    // A dismissal is not an answer.
    const dismissed = show({ decision: { phase: "failed", action: "dismiss", error: "network" } });
    expect(sentence(dismissed.view))
      .toBe("The Console could not confirm that the dismissal arrived. If this question is still listed after the next refresh, dismiss it again.");
    expect(sentence(dismissed.view)).not.toContain("your answer");
    cleanup();
    // An uncertain send stays uncertain when the caller's access then became
    // read-only: the card keeps saying so, offers nothing to send and does
    // not tell the person to send again.
    const locked = show({ decision: { phase: "failed", action: "answer", error: "network" }, readOnly: true, readOnlyReason: "sign_in" });
    expect(locked.card.dataset.state).toBe("failed");
    expect(sentence(locked.view))
      .toBe("The Console could not confirm that your answer arrived. Sign in as yourself to answer. Only a signed-in person can answer an agent's questions.");
    expect(dismiss(locked.view).disabled).toBe(true);
    expect((locked.view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
    cleanup();
    // The same for a set that is now larger than the Console shows.
    const cut = show({ set: questions({ title: "T".repeat(201) }), decision: { phase: "failed", action: "answer", error: "network" } });
    expect(cut.card.dataset.state).toBe("failed");
    expect(sentence(cut.view))
      .toBe("The Console could not confirm that your answer arrived. This set of questions is larger than the Console shows, so it cannot be answered here. Reply to Release Planner in its chat instead.");
    expect(sentence(cut.view)).not.toContain("send your answer again");
    expect(send(cut.view).disabled).toBe(true);
  });

  it("says why the list may be out of date: a read that failed, or a list not read for a while, and what a person who can answer may do", () => {
    const sentence = (view: ReturnType<typeof render>) => view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent;
    // A read failed, and this person can answer.
    const failed = show({ resourceStatus: "stale", readFailed: true });
    expect(failed.card.dataset.state).toBe("stale");
    expect(sentence(failed.view))
      .toBe("The Console could not read the questions again just now, so this card may be out of date. It keeps trying; you can answer once it has read them.");
    expect(send(failed.view).disabled).toBe(true);
    expect(dismiss(failed.view).disabled).toBe(true);
    cleanup();
    // A read failed, and this caller cannot answer anyway: no promise of it.
    const reader = show({ resourceStatus: "stale", readFailed: true, readOnly: true });
    expect(sentence(reader.view)).toBe("The Console could not read the questions again just now, so this card may be out of date. It keeps trying.");
    cleanup();
    const cut = show({ resourceStatus: "stale", readFailed: true, set: questions({ title: "T".repeat(201) }) });
    expect(sentence(cut.view)).toBe("The Console could not read the questions again just now, so this card may be out of date. It keeps trying.");
    cleanup();
    // No read failed (a reply is on its way, or the tab just came back): the
    // card does not say that one did.
    const waiting = show({ resourceStatus: "stale" });
    expect(waiting.card.dataset.state).toBe("stale");
    expect(sentence(waiting.view))
      .toBe("The Console has not read the questions for a while, so this card may be out of date. It is reading them again; you can answer once it has.");
    cleanup();
    const waitingReader = show({ resourceStatus: "stale", readOnly: true, readOnlyReason: "sign_in" });
    expect(sentence(waitingReader.view))
      .toBe("The Console has not read the questions for a while, so this card may be out of date. It is reading them again.");
  });

  it("shows only the kind of a refusal it does not know by name under Details, never a message that carries an id", () => {
    const refused = questionSetRefusal(Object.assign(new Error("refused"), { rpcError: {
      code: -32602, message: `decision ${PENDING_ID} refused the input: presentation_incomplete`,
      data: { kind: "decision_refused", decision_refusal: { kind: "presentation_incomplete" } },
    } }))!;
    const { view, card } = show({ decision: { phase: "refused", action: "answer", error: refused.words, refusal: refused } });
    const disclosure = view.getByTestId(`question-set-refusal:${PENDING_ID}`);
    expect(disclosure.textContent).toBe("DetailsThe gateway's refusal: presentation_incomplete");
    // The set's id is under Technical details and nowhere else on the card.
    const outside = card.cloneNode(true) as HTMLElement;
    outside.querySelector(`[data-testid="question-set-details:${PENDING_ID}"]`)?.remove();
    expect(outside.textContent).not.toContain(PENDING_ID);
    expect(outside.textContent).not.toContain("decision:");
  });

  it("moves focus to the question's first choice after Clear, and to the card's sentence after Send", () => {
    const { view, onAnswer, onDismiss } = show();
    const focusedId = () => document.activeElement?.getAttribute("data-testid") ?? null;
    fireEvent.click(radio(view, "Tonight"));
    const clear = view.getByRole("button", { name: "Clear the answer to: When should it start?" });
    clear.focus();
    fireEvent.click(clear);
    // Clear is gone: focus is on the first choice of its question, not lost.
    expect((document.activeElement as HTMLInputElement).type).toBe("radio");
    expect((document.activeElement as HTMLInputElement).value).toBe("c-now");
    fireEvent.click(radio(view, /^Europe/));
    send(view).focus();
    fireEvent.click(send(view));
    const sending = (decision: QuestionSetDecisionState, closed?: "answered") => (
      <AgentDirectoryProvider directory={directory}>
        <QuestionSetCard set={questions()} resourceStatus="ready" onAnswer={onAnswer} onDismiss={onDismiss} decision={decision} closed={closed} />
      </AgentDirectoryProvider>
    );
    // Send disables itself: focus goes to the sentence that says what happens.
    view.rerender(sending({ phase: "submitting", action: "answer" }));
    expect(focusedId()).toBe(`question-set-sentence:${PENDING_ID}`);
    expect(document.activeElement?.textContent).toBe("Sending your answers to Release Planner.");
    view.rerender(sending({ phase: "answered", action: "answer", answers: { "q-region": { kind: "choice", choiceId: "c-eu" } } }, "answered"));
    expect(focusedId()).toBe(`question-set-sentence:${PENDING_ID}`);
    cleanup();
    // A card nobody acted on takes no focus when its state changes.
    const idle = show();
    idle.view.rerender(sending({ phase: "submitting", action: "answer" }));
    expect(focusedId()).toBeNull();
  });

  it("moves focus to the card's sentence after Dismiss", () => {
    const { view, onAnswer, onDismiss } = show();
    dismiss(view).focus();
    fireEvent.click(dismiss(view));
    view.rerender(
      <AgentDirectoryProvider directory={directory}>
        <QuestionSetCard set={questions()} resourceStatus="ready" onAnswer={onAnswer} onDismiss={onDismiss}
          decision={{ phase: "submitting", action: "dismiss" }} />
      </AgentDirectoryProvider>,
    );
    expect(document.activeElement?.getAttribute("data-testid")).toBe(`question-set-sentence:${PENDING_ID}`);
  });

  it("says what it does not show of a set larger than the reader's limits", () => {
    const choices = (count: number) => Array.from({ length: count }, (_, index) => ({ id: `c${index}`, label: `Choice ${index}` }));
    const { view, card } = show({ set: questions({
      title: "T".repeat(230),
      questions: [
        { id: "q0", prompt: "Which one?", choices: choices(10) },
        { id: "q1", prompt: "And here?", choices: choices(9) },
        ...Array.from({ length: 9 }, (_, index) => ({ id: `q${index + 2}`, prompt: `Question ${index + 2}?`, choices: choices(2) })),
      ],
    }) });
    expect(groups(view).length).toBe(10);
    const [first, second] = groups(view);
    expect(within(first).getAllByRole("radio").length).toBe(8);
    expect(within(first).getByText("2 more choices are not shown.")).toBeTruthy();
    expect(within(second).getByText("1 more choice is not shown.")).toBeTruthy();
    expect(within(card).getByTestId(`question-set-more:${PENDING_ID}`).textContent)
      .toBe("1 more question is not shown. Some text is too long and is shown shortened.");
    cleanup();
    const usual = show();
    expect(usual.view.queryByTestId(`question-set-more:${PENDING_ID}`)).toBeNull();
    expect(usual.view.queryByText(/not shown/)).toBeNull();
  });

  it.each([
    ["a question it does not show", { questions: Array.from({ length: 11 }, (_, index) => ({ id: `q${index}`, prompt: `Question ${index}?`, choices: [{ id: "y", label: "Yes" }, { id: "n", label: "No" }] })) }, "1 more question is not shown."],
    ["a choice it does not show", { questions: [{ id: "q0", prompt: "Which one?", choices: Array.from({ length: 9 }, (_, index) => ({ id: `c${index}`, label: `Choice ${index}` })) }] }, "1 more choice is not shown."],
    ["text it shows shortened", { title: "T".repeat(201) }, "Some text is too long and is shown shortened."],
    ["a label it shows shortened", { questions: [{ id: "q0", prompt: "Which one?", choices: [{ id: "y", label: "Y".repeat(257) }, { id: "n", label: "No" }] }] }, "Some text is too long and is shown shortened."],
    ["a prompt it shows shortened", { questions: [{ id: "q0", prompt: "P".repeat(1025), choices: [{ id: "y", label: "Yes" }, { id: "n", label: "No" }] }] }, "Some text is too long and is shown shortened."],
  ] as const)("lets nobody answer or dismiss a set with %s, and says why and what to do instead", (_what, extra, line) => {
    const { view, card, onAnswer, onDismiss } = show({ set: questions(extra as Record<string, unknown>) });
    // The digest a reply sends back covers the whole set: the card showed less.
    expect(card.dataset.state).toBe("read_only");
    expect(view.getByRole("status").textContent).toBe("Read only");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("This set of questions is larger than the Console shows, so it cannot be answered here. Reply to Release Planner in its chat instead.");
    expect(card.textContent).toContain(line);
    expect(view.getAllByRole("radio").length).toBeGreaterThan(0);
    expect((view.getAllByRole("radio") as HTMLInputElement[]).every((input) => input.disabled)).toBe(true);
    expect(send(view).disabled).toBe(true);
    expect(dismiss(view).disabled).toBe(true);
    fireEvent.click(send(view));
    fireEvent.click(dismiss(view));
    expect(onAnswer).not.toHaveBeenCalled();
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("says that a set is larger than the Console shows before anything about signing in", () => {
    // Signing in would not make this set answerable here.
    const { view, card } = show({ set: questions({ title: "T".repeat(201) }), readOnly: true, readOnlyReason: "sign_in" });
    expect(card.dataset.state).toBe("read_only");
    expect(view.getByRole("status").textContent).toBe("Read only");
    expect(view.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent)
      .toBe("This set of questions is larger than the Console shows, so it cannot be answered here. Reply to Release Planner in its chat instead.");
  });

  it("remembers whether its Technical details are open by the card's place on the page, not by the set's id", () => {
    // A pane keeps what its rows remember while they are off screen.
    const pane = (slot: number | null) => (
      <ConversationPresentationProvider viewportKey={{ authority: "question-set-test", identity: IDENTITY, conversation: "c", pane: "left" }}>
        <AgentDirectoryProvider directory={directory}>
          <ConversationRowStateScope rowId="questions">
            {slot === null ? null : <QuestionSetCard set={questions()} slot={slot} resourceStatus="ready" onAnswer={vi.fn()} onDismiss={vi.fn()} />}
          </ConversationRowStateScope>
        </AgentDirectoryProvider>
      </ConversationPresentationProvider>
    );
    const details = (view: ReturnType<typeof render>) => view.getByTestId(`question-set-details:${PENDING_ID}`) as HTMLDetailsElement;
    const view = render(pane(1));
    expect(details(view).open).toBe(false);
    details(view).open = true;
    fireEvent(details(view), new Event("toggle"));
    // The same card, off screen and back: still open.
    view.rerender(pane(null));
    view.rerender(pane(1));
    expect(details(view).open).toBe(true);
    // The set left and a set with the same id is listed again, at a new
    // place: the pane remembers nothing under the id.
    view.rerender(pane(null));
    view.rerender(pane(2));
    expect(details(view).open).toBe(false);
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
    expect(lines[0].textContent).toBe("This question is no longer listed here.");
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
      .toBe("This question is no longer listed here. The Console could not confirm whether your reply arrived.");
    for (const word of SET_WORDS) expect(view.container.innerHTML).not.toContain(word);
  });

  it("says that the first read of the questions failed, with a way to try again, in the inbox's list only", () => {
    const onRefresh = vi.fn();
    const failed = listSnapshot({ status: "unavailable", sets: [], slots: {}, error: "gateway timeout" });
    const view = render(<QuestionSetList view={{ snapshot: failed, onAnswer: vi.fn(), onDismiss: vi.fn(), onRefresh }} testId="list" />);
    const notice = view.getByTestId("question-set-unavailable");
    expect(notice.getAttribute("role")).toBe("status");
    expect(notice.textContent).toContain("The questions agents asked could not be loaded. The Console keeps trying; you can also try now.");
    fireEvent.click(view.getByRole("button", { name: "Try again" }));
    expect(onRefresh).toHaveBeenCalledTimes(1);
    cleanup();
    // One agent's conversation does not repeat it.
    const chat = render(<QuestionSetList view={{ snapshot: failed, onAnswer: vi.fn(), onDismiss: vi.fn(), onRefresh }} identity={IDENTITY} />);
    expect(chat.container.textContent).toBe("");
    cleanup();
    // A read that failed after a list was read keeps the cards: no such
    // notice, and each card says that a read failed.
    const stale = render(list(listSnapshot({ status: "stale", error: "network" })));
    expect(stale.queryByTestId("question-set-unavailable")).toBeNull();
    expect(stale.getByTestId(`question-set:${PENDING_ID}`).dataset.state).toBe("stale");
    expect(stale.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toMatch(/^The Console could not read the questions again just now/);
    cleanup();
    // A list that was only not read for a while does not say that one did.
    const waiting = render(list(listSnapshot({ status: "stale" })));
    expect(waiting.getByTestId(`question-set-sentence:${PENDING_ID}`).textContent).toMatch(/^The Console has not read the questions for a while/);
  });

  it("keeps the line for as long as the snapshot holds it, through every read in between", () => {
    const left = () => listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 }, notices: [{ slot: 1, kind: "gone", atMs: 5 }] });
    const view = render(list(listSnapshot()));
    view.rerender(list(left()));
    expect(view.getAllByTestId("question-set-notice").length).toBe(1);
    // Later reads publish new snapshots that still hold the line (it goes a minute after its card).
    for (let read = 0; read < 4; read++) {
      view.rerender(list(left()));
      expect(view.getAllByTestId("question-set-notice").length).toBe(1);
    }
    // The snapshot drops it: so does the list.
    view.rerender(list(listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 } })));
    expect(view.queryByTestId("question-set-notice")).toBeNull();
  });

  it.each(["not_open", "expired"] as const)("leaves the gateway's refusal (%s) as the one line of a card it removed", (kind) => {
    const view = render(list(listSnapshot()));
    view.rerender(list(listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 }, notices: [{ slot: 1, kind, atMs: 5 }] })));
    expect(view.queryByTestId(`question-set:${PENDING_ID}`)).toBeNull();
    expect(view.getByTestId("question-set-notice").textContent).toBe(QUESTION_SET_REFUSAL_WORDS[kind]);
    expect(QUESTION_SET_REFUSAL_WORDS[kind]).toMatch(/^This question /);
    for (const word of SET_WORDS) expect(view.container.innerHTML).not.toContain(word);
  });

  it("moves the focus to the line when the card that held it is removed, and takes none otherwise", () => {
    const focused = () => document.activeElement?.getAttribute("data-testid") ?? null;
    const gone = listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 }, notices: [{ slot: 1, kind: "not_open", atMs: 5 }] });
    const view = render(list(listSnapshot()));
    // After Send the focus is on the card's sentence; the gateway then says
    // the set is no longer open and the card is removed at once.
    view.getByTestId(`question-set-sentence:${PENDING_ID}`).focus();
    expect(focused()).toBe(`question-set-sentence:${PENDING_ID}`);
    view.rerender(list(gone));
    expect(focused()).toBe("question-set-line");
    expect(document.activeElement?.textContent).toBe(QUESTION_SET_REFUSAL_WORDS.not_open);
    expect(document.activeElement?.getAttribute("tabindex")).toBe("-1");
    cleanup();
    // The focus was elsewhere (another card, or nowhere): the line takes none.
    const other = render(list(listSnapshot()));
    other.getByTestId(`question-set-sentence:${BILLING_ID}`).focus();
    other.rerender(list(gone));
    expect(focused()).toBe(`question-set-sentence:${BILLING_ID}`);
    cleanup();
    const idle = render(list(listSnapshot()));
    idle.rerender(list(gone));
    expect(idle.getByTestId("question-set-notice")).toBeTruthy();
    expect(focused()).toBeNull();
  });

  it("moves the focus to the line that is shown when two cards leave in one read, and to the list when no line is shown", () => {
    // jsdom rows: what the DOM focuses, with no browser.
    const focused = () => document.activeElement?.getAttribute("data-testid") ?? null;
    const onBody = () => document.activeElement === null || document.activeElement.tagName === "BODY";
    // Both cards leave in one read. The list shows one line, the newest,
    // which is the first card's: whichever card held the focus, it goes there.
    const both = listSnapshot({ sets: [], slots: {}, notices: [{ slot: 1, kind: "gone", atMs: 5 }, { slot: 2, kind: "sent_unconfirmed", atMs: 5 }] });
    for (const id of [PENDING_ID, BILLING_ID]) {
      const view = render(list(listSnapshot()));
      view.getByTestId(`question-set-sentence:${id}`).focus();
      expect(focused()).toBe(`question-set-sentence:${id}`);
      view.rerender(list(both));
      expect(view.getAllByTestId("question-set-notice").map((line) => line.textContent)).toEqual(["This question is no longer listed here."]);
      expect(focused()).toBe("question-set-line");
      expect(onBody()).toBe(false);
      cleanup();
    }
    // The card that held the focus leaves and the list shows no line for
    // it, beside a card that stays: the focus goes to the list itself.
    const view = render(list(listSnapshot()));
    view.getByTestId(`question-set-sentence:${PENDING_ID}`).focus();
    view.rerender(list(listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 } })));
    expect(view.queryByTestId("question-set-notice")).toBeNull();
    expect(focused()).toBe("list");
    expect(document.activeElement?.getAttribute("tabindex")).toBe("-1");
    expect(onBody()).toBe(false);
    cleanup();
    // The list takes no focus that no card of its own held.
    const idle = render(list(listSnapshot()));
    idle.rerender(list(listSnapshot({ sets: [billing()], slots: { [BILLING_ID]: 2 } })));
    expect(onBody()).toBe(true);
  });

  it("shows the line only where the card was shown, and at most one", () => {
    const mine = render(list(listSnapshot(), IDENTITY, "mine"));
    const other = render(list(listSnapshot(), "identity:billing", "other"));
    const left = listSnapshot({ sets: [], slots: {}, notices: [{ slot: 2, kind: "gone", atMs: 9 }, { slot: 1, kind: "sent_unconfirmed", atMs: 5 }] });
    mine.rerender(list(left, IDENTITY, "mine"));
    other.rerender(list(left, "identity:billing", "other"));
    expect(within(mine.getByTestId("mine")).getAllByTestId("question-set-notice").map((line) => line.textContent))
      .toEqual(["This question is no longer listed here. The Console could not confirm whether your reply arrived."]);
    expect(within(other.getByTestId("other")).getAllByTestId("question-set-notice").map((line) => line.textContent))
      .toEqual(["This question is no longer listed here."]);
    cleanup();
    // The inbox showed both cards: it shows one line, the newest.
    const inbox = render(list(listSnapshot()));
    inbox.rerender(list(left));
    expect(inbox.getAllByTestId("question-set-notice").map((line) => line.textContent)).toEqual(["This question is no longer listed here."]);
    cleanup();
    // A list that never showed the card says nothing of it.
    const late = render(list(left));
    expect(late.container.textContent).toBe("");
  });
});

describe("Needs you with question sets", () => {
  const approvals: PendingApprovalSnapshot = {
    scopeKey: "a", status: "ready", decisions: {}, readOnly: false,
    requests: [normalizePendingApproval({ pending_id: "p1", action_id: "a1", action: "Deploy release", rationale: "Publish reviewed changes" })!],
  };
  const count = (view: ReturnType<typeof render>) => view.getByTestId("approval-attention").querySelector(".cc-approval-attention__count")?.textContent;

  it("counts the open question sets with the approvals and opens the inbox's list", () => {
    const onOpen = vi.fn();
    const view = render(<ApprovalAttention snapshot={approvals} questionSets={2} onOpen={onOpen} />);
    expect(count(view)).toBe("3");
    expect(view.getByRole("status").textContent).toBe("1 pending approval, 2 question sets");
    expect(view.getByTestId("approval-attention").getAttribute("data-pending")).toBe("true");
    fireEvent.click(view.getByRole("button", { name: "Needs you, 1 pending approval, 2 question sets" }));
    // The list, where the question sets head Pending: not the one approval.
    expect(onOpen).toHaveBeenCalledWith(undefined);
  });

  it("shows a question set that waits where no approval does, and nothing for none", () => {
    const view = render(<ApprovalAttention snapshot={{ ...approvals, requests: [] }} questionSets={1} onOpen={vi.fn()} />);
    expect(count(view)).toBe("1");
    expect(view.getByRole("button", { name: "Needs you, 0 pending approvals, 1 question set" })).toBeTruthy();
    cleanup();
    // A connection that shows no approvals still names the question set.
    const alone = render(<ApprovalAttention questionSets={1} onOpen={vi.fn()} />);
    expect(count(alone)).toBe("1");
    expect(alone.getByRole("button", { name: "Needs you, 1 question set" })).toBeTruthy();
    alone.rerender(<ApprovalAttention questionSets={0} onOpen={vi.fn()} />);
    expect(alone.queryByTestId("approval-attention") === null).toBe(true);
  });
  it("never hides a count it knows: the number is the count of what was read, and the words say what was not", () => {
    const row = (view: ReturnType<typeof render>) => view.getByTestId("approval-attention");
    const name = (view: ReturnType<typeof render>) => within(row(view)).getByRole("button").getAttribute("aria-label");
    // The question sets are still being read: the approvals' count stays.
    const loading = render(<ApprovalAttention snapshot={approvals} questionSets={0} questionStatus="loading" onOpen={vi.fn()} />);
    expect(count(loading)).toBe("1");
    expect(name(loading)).toBe("Needs you, 1 pending approval, Checking questions");
    expect(row(loading).getAttribute("data-pending")).toBe("true");
    cleanup();
    // The last list of question sets may be out of date: its sets are not
    // counted, and the words say so.
    const stale = render(<ApprovalAttention snapshot={approvals} questionSets={2} questionStatus="stale" onOpen={vi.fn()} />);
    expect(count(stale)).toBe("1");
    expect(name(stale)).toBe("Needs you, 1 pending approval, Questions may be out of date");
    cleanup();
    // The question sets could not be read: everything else that is known is counted.
    const failing = render(<ApprovalAttention snapshot={approvals} hostItems={1} replies={1} questionSets={0} questionStatus="unavailable" onOpen={vi.fn()} />);
    expect(count(failing)).toBe("3");
    expect(name(failing)).toBe("Needs you, 1 pending approval, Questions unavailable, 1 item from the host, 1 reply for you");
    cleanup();
    // A person with no approval waiting sees that count too while the question read fails.
    const none = render(<ApprovalAttention snapshot={{ ...approvals, requests: [] }} questionSets={0} questionStatus="unavailable" onOpen={vi.fn()} />);
    expect(count(none)).toBe("0");
    expect(name(none)).toBe("Needs you, 0 pending approvals, Questions unavailable");
    expect(row(none).getAttribute("data-pending")).toBe("false");
    cleanup();
    // The approvals are not read yet: the sets that were read are counted.
    const approving = render(<ApprovalAttention snapshot={{ ...approvals, status: "loading", requests: [] }} questionSets={2} questionStatus="ready" onOpen={vi.fn()} />);
    expect(count(approving)).toBe("2");
    expect(name(approving)).toBe("Needs you, Checking approvals, 2 question sets");
    expect(row(approving).getAttribute("data-pending")).toBe("true");
    cleanup();
    // Approvals that may be out of date are not counted; the sets that were read are.
    const older = render(<ApprovalAttention snapshot={{ ...approvals, status: "stale" }} questionSets={2} questionStatus="ready" onOpen={vi.fn()} />);
    expect(count(older)).toBe("2");
    expect(name(older)).toBe("Needs you, Approvals may be out of date, 2 question sets");
    cleanup();
    // What the host gives is known while the approvals are still being read.
    const hosted = render(<ApprovalAttention snapshot={{ ...approvals, status: "loading", requests: [] }} hostItems={1} onOpen={vi.fn()} />);
    expect(count(hosted)).toBe("1");
    expect(name(hosted)).toBe("Needs you, Checking approvals, 1 item from the host");
    cleanup();
    // Both read: the count.
    const ready = render(<ApprovalAttention snapshot={approvals} questionSets={2} questionStatus="ready" onOpen={vi.fn()} />);
    expect(count(ready)).toBe("3");
    expect(row(ready).getAttribute("data-pending")).toBe("true");
    cleanup();
    // A gateway without question sets: nothing of questions.
    const without = render(<ApprovalAttention snapshot={approvals} questionSets={0} questionStatus="unsupported" onOpen={vi.fn()} />);
    expect(count(without)).toBe("1");
    expect(name(without)).toBe("Needs you, 1 pending approval");
  });

  it("shows a placeholder alone only when nothing was read, and no zero beside approvals that were not read", () => {
    const row = (view: ReturnType<typeof render>) => view.getByTestId("approval-attention");
    const name = (view: ReturnType<typeof render>) => within(row(view)).getByRole("button").getAttribute("aria-label");
    const unread = (status: PendingApprovalSnapshot["status"]): PendingApprovalSnapshot => ({ ...approvals, status, requests: [] });
    // Both lists are still being read.
    const both = render(<ApprovalAttention snapshot={unread("loading")} questionSets={0} questionStatus="loading" onOpen={vi.fn()} />);
    expect(count(both)).toBe("...");
    expect(name(both)).toBe("Needs you, Checking approvals, Checking questions");
    expect(row(both).getAttribute("data-pending")).toBe("false");
    cleanup();
    // One is still being read and the other could not be read, either way round.
    const mixed = render(<ApprovalAttention snapshot={unread("loading")} questionSets={0} questionStatus="unavailable" onOpen={vi.fn()} />);
    expect(count(mixed)).toBe("!");
    cleanup();
    const other = render(<ApprovalAttention snapshot={unread("unavailable")} questionSets={0} questionStatus="loading" onOpen={vi.fn()} />);
    expect(count(other)).toBe("!");
    cleanup();
    // No approvals on this connection, and the question sets could not be
    // read or may be out of date: said, also where nothing else is shown.
    const failed = render(<ApprovalAttention questionSets={0} questionStatus="unavailable" onOpen={vi.fn()} />);
    expect(count(failed)).toBe("!");
    expect(name(failed)).toBe("Needs you, Questions unavailable");
    cleanup();
    const old = render(<ApprovalAttention questionSets={2} questionStatus="stale" onOpen={vi.fn()} />);
    expect(count(old)).toBe("!");
    expect(name(old)).toBe("Needs you, Questions may be out of date");
    cleanup();
    // The approvals are not read and the question sets were read as none: a
    // zero would read as "nothing needs you" while the approvals are unknown.
    const pendingRead = render(<ApprovalAttention snapshot={unread("loading")} questionSets={0} questionStatus="ready" onOpen={vi.fn()} />);
    expect(count(pendingRead)).toBe("...");
    expect(name(pendingRead)).toBe("Needs you, Checking approvals");
    cleanup();
    const noRead = render(<ApprovalAttention snapshot={unread("unavailable")} questionSets={0} questionStatus="ready" onOpen={vi.fn()} />);
    expect(count(noRead)).toBe("!");
    expect(name(noRead)).toBe("Needs you, Approvals unavailable");
  });

  it("opens the list, not the one approval, while the question sets are not read", () => {
    const onOpen = vi.fn();
    const view = render(<ApprovalAttention snapshot={approvals} questionSets={0} questionStatus="unavailable" onOpen={onOpen} />);
    fireEvent.click(view.getByRole("button", { name: "Needs you, 1 pending approval, Questions unavailable" }));
    expect(onOpen).toHaveBeenCalledWith(undefined);
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
