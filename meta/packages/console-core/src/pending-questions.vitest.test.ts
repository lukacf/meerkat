import { describe, expect, it } from "vitest";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { ApprovalResourceEnvironment } from "./pending-approvals";
import {
  FREE_TEXT_MAX_BYTES,
  createQuestionSetResource,
  normalizeQuestionSet,
  questionSetAnswerParams,
  questionSetAnswersSendable,
  questionSetDismissParams,
  questionSetEntries,
  questionSetRefusal,
  utf8ByteLength,
  type QuestionSet,
} from "./pending-questions";

const tick = async () => { for (let i = 0; i < 15; i++) await Promise.resolve(); };
function clock() {
  let now = 0, visible = true, id = 0;
  const timers = new Map<number, { at: number; callback(): void }>();
  const visibility = new Set<() => void>();
  const environment: ApprovalResourceEnvironment = {
    now: () => now, visible: () => visible,
    setTimeout(callback, ms) { const key = ++id; timers.set(key, { at: now + ms, callback }); return key; },
    clearTimeout(key) { timers.delete(key as number); },
    onVisibilityChange(callback) { visibility.add(callback); return () => { visibility.delete(callback); }; },
  };
  return {
    environment, timers,
    advance(ms: number) {
      const until = now + ms;
      while (true) {
        const next = Array.from(timers.entries()).filter(([, timer]) => timer.at <= until).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) break;
        now = next[1].at; timers.delete(next[0]); next[1].callback();
      }
      now = until;
    },
    visible(value: boolean) { visible = value; for (const listener of visibility) listener(); },
  };
}

const PENDING_ID = `decision:${"a".repeat(64)}`;
const DIGEST = `sha256:${"b".repeat(64)}`;
const wire = (id = PENDING_ID, extra: Record<string, unknown> = {}) => ({
  pending_id: id,
  origin: { identity: "identity:release-planner" },
  title: "Release plan",
  questions: [
    { id: "q1", prompt: "Which region goes first?", choices: [{ id: "eu", label: "Europe" }, { id: "us", label: "United States" }], recommended: "eu" },
    { id: "q2", prompt: "When should it start?", choices: [{ id: "now", label: "Now" }, { id: "night", label: "Tonight" }], allow_free_text: true },
  ],
  created_at_ms: 1_000,
  deadline_at_ms: 3_600_000,
  subject_digest: DIGEST,
  ...extra,
});
const set = (): QuestionSet => normalizeQuestionSet(wire())!;
const rpcError = (code: number, data?: string | Record<string, unknown>, message = "refused by the gateway") => Object.assign(new Error(message), {
  rpcError: { code, message, ...(data ? { data: typeof data === "string" ? { kind: data } : data } : {}) },
});
/** The gateway's answer to a decide the set itself refuses: invalid params
 * with the typed refusal, an object tagged by `kind`. */
const refusedError = (token: string) => rpcError(
  -32602, { kind: "decision_refused", decision_refusal: { kind: token } }, `decision ${PENDING_ID} refused the input: ${token}`,
);
/** Whether a snapshot holds nothing of the wire set: no id, asking agent,
 * digest, title, prompt, label or question and choice id. */
const nothingKept = (snapshot: unknown) => {
  const held = JSON.stringify(snapshot);
  return ![
    PENDING_ID, `decision:${"c".repeat(64)}`, "identity:release-planner", DIGEST, "Release plan",
    "Which region goes first?", "When should it start?", "Europe", "United States", "Tonight", "\"q1\"", "\"q2\"", "\"eu\"", "\"night\"",
  ].some((word) => held.includes(word));
};

describe("normalizeQuestionSet", () => {
  it("reads a valid set", () => {
    expect(normalizeQuestionSet(wire())).toEqual({
      pendingId: PENDING_ID,
      origin: { identity: "identity:release-planner" },
      title: "Release plan",
      questions: [
        { id: "q1", prompt: "Which region goes first?", choices: [{ id: "eu", label: "Europe" }, { id: "us", label: "United States" }], allowFreeText: false, recommended: "eu" },
        { id: "q2", prompt: "When should it start?", choices: [{ id: "now", label: "Now" }, { id: "night", label: "Tonight" }], allowFreeText: true },
      ],
      createdAtMs: 1_000,
      deadlineAtMs: 3_600_000,
      subjectDigest: DIGEST,
    });
  });

  it("reads a set without its optional fields", () => {
    const bare = normalizeQuestionSet({
      pending_id: PENDING_ID, origin: { identity: "identity:a" }, subject_digest: DIGEST,
      questions: [{ id: "q1", prompt: "Proceed?", choices: [{ id: "y", label: "Yes" }, { id: "n", label: "No" }] }],
    })!;
    expect(bare.title).toBeUndefined();
    expect(bare.createdAtMs).toBeUndefined();
    expect(bare.deadlineAtMs).toBeUndefined();
    expect(bare.questions[0].allowFreeText).toBe(false);
    expect(bare.questions[0].recommended).toBeUndefined();
  });

  it("drops a malformed set and never throws", () => {
    const question = wire().questions[0];
    for (const malformed of [
      null, undefined, 3, "set", [], {},
      wire("  "),
      wire(PENDING_ID, { origin: undefined }),
      wire(PENDING_ID, { origin: { identity: 7 } }),
      wire(PENDING_ID, { subject_digest: "" }),
      wire(PENDING_ID, { questions: "q1" }),
      wire(PENDING_ID, { questions: [] }),
      wire(PENDING_ID, { questions: [{ ...question, id: "" }] }),
      wire(PENDING_ID, { questions: [{ ...question, prompt: 4 }] }),
      wire(PENDING_ID, { questions: [{ ...question, choices: "eu" }] }),
      wire(PENDING_ID, { questions: [{ ...question, choices: [] }] }),
      wire(PENDING_ID, { questions: [{ ...question, choices: [{ id: "eu" }] }] }),
      wire(PENDING_ID, { questions: [{ ...question, choices: [{ id: "eu", label: "Europe" }, { id: "eu", label: "Again" }] }] }),
      wire(PENDING_ID, { questions: [question, question] }),
      wire(PENDING_ID, { questions: [question, null] }),
    ]) {
      expect(normalizeQuestionSet(malformed)).toBeNull();
    }
  });

  it("ignores unknown fields and a recommendation that names no choice", () => {
    const base = wire();
    const read = normalizeQuestionSet({
      ...base,
      later_field: { nested: true },
      origin: { ...base.origin, conversation_id: "c1" },
      questions: [
        { ...base.questions[0], recommended: "asia", weight: 3, choices: base.questions[0].choices.map((choice) => ({ ...choice, hint: "later" })) },
        base.questions[1],
      ],
    })!;
    expect(read).toEqual({ ...set(), questions: [{ ...set().questions[0], recommended: undefined }, set().questions[1]] });
    expect(Object.keys(read.questions[0].choices[0])).toEqual(["id", "label"]);
    expect("recommended" in read.questions[0]).toBe(false);
  });
});

describe("question set decision params", () => {
  it("builds the exact answer request, digest included", () => {
    expect(questionSetAnswerParams(set(), {
      q1: { kind: "choice", choiceId: "us" },
      q2: { kind: "free_text", text: "After the 16:00 freeze" },
    })).toEqual({
      pending_id: PENDING_ID,
      decision: "approve",
      presented_digest: DIGEST,
      answers: [
        { question_id: "q1", answer: { kind: "choice", choice_id: "us" } },
        { question_id: "q2", answer: { kind: "free_text", text: "After the 16:00 freeze" } },
      ],
    });
  });

  it("leaves out the questions a person did not answer and keeps the set's order", () => {
    expect(questionSetAnswerParams(set(), { q2: { kind: "choice", choiceId: "night" }, unknown: { kind: "choice", choiceId: "x" } }).answers)
      .toEqual([{ question_id: "q2", answer: { kind: "choice", choice_id: "night" } }]);
    const reversed = questionSetAnswerParams(set(), { q2: { kind: "choice", choiceId: "now" }, q1: { kind: "choice", choiceId: "eu" } });
    expect(reversed.answers.map((answer) => answer.question_id)).toEqual(["q1", "q2"]);
  });

  it("sends free text without the whitespace at its ends and names no approver", () => {
    const params = questionSetAnswerParams(set(), { q2: { kind: "free_text", text: "  next week \n" } });
    expect(params.answers).toEqual([{ question_id: "q2", answer: { kind: "free_text", text: "next week" } }]);
    expect("approver_id" in params).toBe(false);
  });

  it("builds the exact dismiss request", () => {
    expect(questionSetDismissParams(set())).toEqual({ pending_id: PENDING_ID, decision: "reject", presented_digest: DIGEST });
  });

  it("counts free text in UTF-8 bytes", () => {
    expect(FREE_TEXT_MAX_BYTES).toBe(280);
    expect(utf8ByteLength("abc")).toBe(3);
    expect(utf8ByteLength("å")).toBe(2);
    expect(utf8ByteLength("日本")).toBe(6);
  });

  it("says which answer sets can be sent", () => {
    const questions = set();
    expect(questionSetAnswersSendable(questions, {})).toBe(false);
    expect(questionSetAnswersSendable(questions, { q1: { kind: "choice", choiceId: "eu" } })).toBe(true);
    expect(questionSetAnswersSendable(questions, { q1: { kind: "choice", choiceId: "asia" } })).toBe(false);
    expect(questionSetAnswersSendable(questions, { q1: { kind: "choice", choiceId: "eu" }, q2: { kind: "free_text", text: "  " } })).toBe(false);
    expect(questionSetAnswersSendable(questions, { q2: { kind: "free_text", text: "x".repeat(280) } })).toBe(true);
    expect(questionSetAnswersSendable(questions, { q2: { kind: "free_text", text: "x".repeat(281) } })).toBe(false);
    expect(questionSetAnswersSendable(questions, { q2: { kind: "free_text", text: "å".repeat(141) } })).toBe(false);
    // Free text only where the agent allowed it.
    expect(questionSetAnswersSendable(questions, { q1: { kind: "free_text", text: "Asia" } })).toBe(false);
  });
});

describe("questionSetRefusal", () => {
  it("reads the decision's typed refusal, an object tagged by kind, each with its own words", () => {
    expect(questionSetRefusal(refusedError("not_open"))).toEqual({
      kind: "not_open", words: "This question was already answered or closed, so nothing was sent. Nothing more is needed from you.",
    });
    expect(questionSetRefusal(refusedError("expired"))).toEqual({
      kind: "expired", words: "This question timed out before the gateway took your reply, so nothing was sent. If the answer still matters, tell the agent in its chat.",
    });
    expect(questionSetRefusal(refusedError("subject_changed"))).toEqual({
      kind: "subject_changed", words: "These questions changed after you opened them, so nothing was sent. Read them again, then answer.",
    });
    expect(questionSetRefusal(refusedError("not_eligible"))).toEqual({
      kind: "not_eligible", words: "You are not one of the people who can answer this question, so nothing was sent. Ask an administrator if you should be.",
    });
    // Another refusal of the decision is not taken either; the gateway's own
    // message stays for the details.
    expect(questionSetRefusal(refusedError("presentation_incomplete"))).toEqual({
      kind: "not_taken", words: "The gateway did not take this reply, so nothing was sent. Try again; if it happens again, tell an administrator.",
      detail: `decision ${PENDING_ID} refused the input: presentation_incomplete`,
    });
    // The refusal is an object: a bare word is not read as one of the kinds.
    expect(questionSetRefusal(rpcError(-32602, { kind: "decision_refused", decision_refusal: "not_open" }))?.kind).toBe("not_taken");
    expect(questionSetRefusal(rpcError(-32602, { kind: "decision_refused" }))?.kind).toBe("not_taken");
  });

  it("reads the refusals of the person and of the caller as definite: nothing was sent", () => {
    expect(questionSetRefusal(rpcError(-32030, "access_denied", "access denied to gating decisions"))).toEqual({
      kind: "access_denied",
      words: "You no longer have permission to answer questions here, so nothing was sent. Ask an administrator for the permission to decide, then send again.",
    });
    expect(questionSetRefusal(rpcError(-32057, "approver_unauthenticated"))).toEqual({
      kind: "approver_unauthenticated", words: "Only a signed-in person can answer, so nothing was sent. Sign in as yourself, then send again.",
    });
    expect(questionSetRefusal(rpcError(-32056, "caller_changed"))).toEqual({
      kind: "caller_changed", words: "This browser is now signed in as someone else, so nothing was sent or changed. Answer again as the person signed in now.",
    });
    expect(questionSetRefusal(rpcError(-32056, "caller_not_current"))).toEqual({
      kind: "caller_not_current", words: "Your sign-in ended before the reply was sent, so nothing was changed. Sign in again, then send again.",
    });
  });

  it("reads invalid params without the typed refusal as a reply that was not accepted, and anything else as uncertain", () => {
    expect(questionSetRefusal(rpcError(-32602, undefined, "answers: unknown choice"))).toEqual({
      kind: "not_accepted", words: "The gateway did not accept this reply, so nothing was sent. Check your choices and send again.",
      detail: "answers: unknown choice",
    });
    expect(questionSetRefusal(new Error("network"))).toBeNull();
    expect(questionSetRefusal(rpcError(-32000))).toBeNull();
    expect(questionSetRefusal(rpcError(-32056))).toBeNull();
  });
});

describe("createQuestionSetResource", () => {
  const accepted = (outcome: "answered" | "dismissed", id = PENDING_ID) => ({ pending_id: id, kind: "question_set", outcome, decided_at_ms: 42 });

  it("lists the open sets, drops a malformed one, and polls like the approvals", async () => {
    const c = clock(); let reads = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => { reads++; return { question_sets: [wire(), { pending_id: "broken" }, wire(`decision:${"c".repeat(64)}`)] }; },
      decide: async () => ({}),
    });
    expect(resource.getSnapshot().status).toBe("loading");
    await tick();
    expect(resource.getSnapshot().status).toBe("ready");
    expect(resource.getSnapshot().sets.map((entry) => entry.pendingId)).toEqual([PENDING_ID, `decision:${"c".repeat(64)}`]);
    c.advance(14_999); await tick(); expect(reads).toBe(1);
    c.advance(1); await tick(); expect(reads).toBe(2);
    c.visible(false); c.advance(60_000); await tick(); expect(reads).toBe(2);
    c.visible(true); await tick(); expect(reads).toBe(3);
    resource.dispose(); expect(c.timers.size).toBe(0);
  });

  it("sends the answers once, shows them as answered and keeps the set as a closed one", async () => {
    const c = clock(); const sent: unknown[] = []; let open = true;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: open ? [wire()] : [] }),
      decide: async (params) => { sent.push(params); open = false; return accepted("answered"); },
    });
    await tick();
    const answers = { q1: { kind: "choice", choiceId: "eu" } } as const;
    const first = resource.answer(PENDING_ID, answers), second = resource.answer(PENDING_ID, answers);
    expect(first).toBe(second);
    expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("submitting");
    await first;
    expect(sent).toEqual([questionSetAnswerParams(set(), answers)]);
    const snapshot = resource.getSnapshot();
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "answered", action: "answer", answers, decidedAtMs: 42 });
    expect(snapshot.sets).toEqual([]);
    expect(snapshot.closed.map((entry) => [entry.set.pendingId, entry.outcome])).toEqual([[PENDING_ID, "answered"]]);
    expect(questionSetEntries(snapshot).map((entry) => [entry.set.pendingId, entry.closed])).toEqual([[PENDING_ID, "answered"]]);
    resource.dispose();
  });

  it("dismisses a set", async () => {
    const c = clock(); const sent: unknown[] = [];
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: sent.length ? [] : [wire()] }),
      decide: async (params) => { sent.push(params); return accepted("dismissed"); },
    });
    await tick(); await resource.dismiss(PENDING_ID);
    expect(sent).toEqual([questionSetDismissParams(set())]);
    expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("dismissed");
    expect(resource.getSnapshot().closed[0].outcome).toBe("dismissed");
    resource.dispose();
  });

  it("sends nothing without an answer, for a read-only caller, or for a set it does not list", async () => {
    const c = clock(); let writes = 0;
    const input = { environment: c.environment, load: async () => ({ question_sets: [wire()] }), decide: async () => { writes++; return accepted("answered"); } };
    const resource = createQuestionSetResource({ scopeKey: "a", ...input });
    await tick();
    await resource.answer(PENDING_ID, {});
    await resource.answer(PENDING_ID, { q2: { kind: "free_text", text: " " } });
    await resource.answer("decision:unknown", { q1: { kind: "choice", choiceId: "eu" } });
    await resource.dismiss("decision:unknown");
    resource.dispose();
    const readOnly = createQuestionSetResource({ scopeKey: "b", readOnly: true, readOnlyReason: "sign_in", ...input });
    await tick();
    expect(readOnly.getSnapshot()).toMatchObject({ readOnly: true, readOnlyReason: "sign_in" });
    await readOnly.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    await readOnly.dismiss(PENDING_ID);
    readOnly.dispose();
    expect(writes).toBe(0);
  });

  it.each(["not_open", "expired"] as const)("removes a set whose decide the gateway refuses as %s, at once, with that refusal's own line", async (token) => {
    const c = clock(); let reads = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      // The gateway no longer lists a set it refuses this way.
      load: async () => ({ question_sets: reads++ === 0 ? [wire()] : [] }),
      decide: async () => { throw refusedError(token); },
    });
    await tick();
    const slot = questionSetEntries(resource.getSnapshot())[0].slot;
    const phases: string[] = [];
    resource.subscribe(() => { phases.push(resource.getSnapshot().decisions[PENDING_ID]?.phase ?? "none"); });
    await resource.answer(PENDING_ID, { q2: { kind: "free_text", text: "After the freeze" } });
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets).toEqual([]);
    expect(snapshot.closed).toEqual([]);
    expect(snapshot.decisions).toEqual({});
    expect(snapshot.notices).toEqual([{ slot, kind: token, atMs: 0 }]);
    expect(nothingKept(snapshot)).toBe(true);
    expect(JSON.stringify(snapshot)).not.toContain("After the freeze");
    expect(phases).not.toContain("refused");
    expect(phases).not.toContain("failed");
    resource.dispose();
  });

  it("reads a set that changed again and sends the next answer with the digest it then has", async () => {
    const c = clock(); const sent: Array<{ presented_digest: string }> = []; const changed = `sha256:${"e".repeat(64)}`;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: sent.length >= 2 ? [] : [wire(PENDING_ID, sent.length ? { subject_digest: changed } : {})] }),
      decide: async (params) => {
        sent.push(params);
        if (sent.length === 1) throw refusedError("subject_changed");
        return accepted("answered");
      },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    let snapshot = resource.getSnapshot();
    expect(snapshot.sets.map((entry) => entry.subjectDigest)).toEqual([changed]);
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind: "subject_changed" } });
    expect(snapshot.decisions[PENDING_ID].refusal?.detail).toBeUndefined();
    expect(snapshot.notices).toEqual([]);
    await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "us" } });
    snapshot = resource.getSnapshot();
    expect(sent.map((params) => params.presented_digest)).toEqual([DIGEST, changed]);
    expect(snapshot.decisions[PENDING_ID].phase).toBe("answered");
    resource.dispose();
  });

  it.each([
    ["not_eligible", "not_eligible"],
    ["presentation_incomplete", "not_taken"],
  ] as const)("keeps a listed set whose decide the gateway refuses as %s: not sent, and the person may send again", async (token, kind) => {
    const c = clock(); let decides = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: [wire()] }),
      decide: async () => { decides++; throw refusedError(token); },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets.map((entry) => entry.pendingId)).toEqual([PENDING_ID]);
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind } });
    expect(snapshot.readOnly).toBe(false);
    await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "us" } });
    expect(decides).toBe(2);
    resource.dispose();
  });

  it("offers no send on any card after the gateway says the person may no longer decide, until a later read", async () => {
    const other = `decision:${"c".repeat(64)}`;
    const c = clock(); let decides = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: [wire(), wire(other)] }),
      decide: async () => { decides++; throw rpcError(-32030, "access_denied", "access denied to gating decisions"); },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    let snapshot = resource.getSnapshot();
    expect(snapshot.status).toBe("ready");
    expect(snapshot.sets.length).toBe(2);
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind: "access_denied" } });
    expect(snapshot).toMatchObject({ readOnly: true, readOnlyReason: "no_permission" });
    // The other card sends nothing: it would be refused in turn.
    await resource.answer(other, { q1: { kind: "choice", choiceId: "eu" } });
    await resource.dismiss(other);
    expect(decides).toBe(1);
    // The next read of the list succeeds: the cards offer Send again.
    c.advance(15_000); await tick();
    snapshot = resource.getSnapshot();
    expect(snapshot.readOnly).toBe(false);
    expect(snapshot.readOnlyReason).toBeUndefined();
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind: "access_denied" } });
    await resource.answer(other, { q1: { kind: "choice", choiceId: "eu" } });
    expect(decides).toBe(2);
    resource.dispose();
  });

  it("recovers after the gateway asked for a signed-in person: a later read unlocks the card, and the answer goes through without a reload", async () => {
    const c = clock(); let decides = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: decides >= 2 ? [] : [wire()] }),
      decide: async () => {
        if (++decides === 1) throw rpcError(-32057, "approver_unauthenticated");
        return accepted("answered");
      },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    expect(resource.getSnapshot()).toMatchObject({ readOnly: true, readOnlyReason: "sign_in" });
    expect(resource.getSnapshot().decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind: "approver_unauthenticated" } });
    await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    expect(decides).toBe(1);
    c.advance(15_000); await tick();
    expect(resource.getSnapshot().readOnly).toBe(false);
    await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    expect(decides).toBe(2);
    expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("answered");
    resource.dispose();
  });

  it("keeps a host's own read-only mode through a refusal and the reads after it", async () => {
    const c = clock();
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment, readOnly: true, readOnlyReason: "sign_in",
      load: async () => ({ question_sets: [wire()] }),
      decide: async () => { throw rpcError(-32030, "access_denied"); },
    });
    await tick(); c.advance(15_000); await tick(); c.advance(15_000); await tick();
    expect(resource.getSnapshot()).toMatchObject({ status: "ready", readOnly: true, readOnlyReason: "sign_in" });
    resource.dispose();
  });

  it.each(["caller_changed", "caller_not_current"] as const)("reads a change refused for its caller (%s) as a definite answer: nothing was sent", async (kind) => {
    const c = clock();
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: [wire()] }),
      decide: async () => { throw rpcError(-32056, kind); },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    const snapshot = resource.getSnapshot();
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind } });
    expect(snapshot.sets.length).toBe(1);
    expect(snapshot.readOnly).toBe(false);
    resource.dispose();
  });

  it("drops a set withdrawn while the answer was on its way, with the gateway's refusal as its one line", async () => {
    const c = clock(); let withdrawn = false; let refuse: (error: unknown) => void = () => {};
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: withdrawn ? [] : [wire()] }),
      decide: () => new Promise((_resolve, reject) => { refuse = reject; }),
    });
    await tick();
    const job = resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    await tick();
    // The set is withdrawn and a poll reads the list while the answer waits.
    withdrawn = true; c.advance(15_000); await tick();
    expect(resource.getSnapshot().sets.map((entry) => entry.pendingId)).toEqual([PENDING_ID]);
    expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("submitting");
    expect(resource.getSnapshot().notices).toEqual([]);
    refuse(refusedError("not_open")); await job;
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets).toEqual([]);
    expect(snapshot.closed).toEqual([]);
    expect(snapshot.decisions).toEqual({});
    expect(snapshot.notices.map((notice) => notice.kind)).toEqual(["not_open"]);
    expect(nothingKept(snapshot)).toBe(true);
    resource.dispose();
  });

  it("shows an answer the gateway took as answered, with no line, when a read in between no longer listed the set", async () => {
    const c = clock(); let taken = false; let accept: (value: unknown) => void = () => {};
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: taken ? [] : [wire()] }),
      decide: () => new Promise((resolve) => { accept = resolve; }),
    });
    await tick();
    const job = resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    await tick();
    taken = true; c.advance(15_000); await tick();
    accept(accepted("answered")); await job;
    const snapshot = resource.getSnapshot();
    expect(snapshot.notices).toEqual([]);
    expect(snapshot.closed.map((entry) => [entry.set.pendingId, entry.outcome])).toEqual([[PENDING_ID, "answered"]]);
    expect(snapshot.decisions[PENDING_ID].phase).toBe("answered");
    resource.dispose();
  });

  it("never invents an answer from an unknown response or a lost one", async () => {
    const c = clock(); let stage = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: [wire()] }),
      decide: async () => { if (stage++ === 0) return { ok: true }; throw new Error("network"); },
    });
    await tick();
    await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("failed");
    await resource.dismiss(PENDING_ID);
    expect(resource.getSnapshot().decisions[PENDING_ID]).toMatchObject({ phase: "failed", action: "dismiss", error: "network" });
    expect(resource.getSnapshot().sets.length).toBe(1);
    resource.dispose();
  });

  it("takes only the question set's own result for its own outcome as confirmed", async () => {
    for (const [action, result] of [
      ["answer", { pending_id: PENDING_ID, kind: "approval", outcome: "answered", decided_at_ms: 42 }],
      ["answer", { pending_id: PENDING_ID, kind: "question_set", outcome: "dismissed", decided_at_ms: 42 }],
      ["answer", { pending_id: "decision:other", kind: "question_set", outcome: "answered", decided_at_ms: 42 }],
      ["dismiss", { pending_id: PENDING_ID, kind: "question_set", outcome: "answered", decided_at_ms: 42 }],
    ] as const) {
      const c = clock();
      const resource = createQuestionSetResource({
        scopeKey: "a", environment: c.environment,
        load: async () => ({ question_sets: [wire()] }),
        decide: async () => result,
      });
      await tick();
      if (action === "answer") await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
      else await resource.dismiss(PENDING_ID);
      expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("failed");
      expect(resource.getSnapshot().closed).toEqual([]);
      resource.dispose();
    }
  });

  it("reads an older gateway as unsupported: nothing listed, no error, no further reads", async () => {
    for (const failure of [
      Object.assign(new Error("Meta capability missing"), { kind: "console-capability-unavailable", method: CONSOLE_RPC_METHODS.gatingQuestions, availableMethods: [CONSOLE_RPC_METHODS.gatingPending] }),
      rpcError(-32601, undefined, "Method not found"),
    ]) {
      const c = clock(); let reads = 0;
      const resource = createQuestionSetResource({
        scopeKey: "a", environment: c.environment,
        load: async () => { reads++; throw failure; },
        decide: async () => ({}),
      });
      await tick();
      expect(resource.getSnapshot()).toMatchObject({ status: "unsupported", sets: [], closed: [], readOnly: true });
      expect(resource.getSnapshot().error).toBeUndefined();
      expect(questionSetEntries(resource.getSnapshot())).toEqual([]);
      c.advance(60_000); await tick(); expect(reads).toBe(1);
      resource.dispose();
    }
  });

  it("keeps the last list when a read fails and clears it when access is denied", async () => {
    const c = clock(); let stage = 0;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => {
        if (stage === 1) throw new Error("network");
        if (stage === 2) throw rpcError(-32030, "access_denied");
        return { question_sets: [wire()] };
      },
      decide: async () => ({}),
    });
    await tick(); stage = 1; await resource.refresh();
    expect(resource.getSnapshot()).toMatchObject({ status: "stale", error: "network" });
    expect(resource.getSnapshot().sets.length).toBe(1);
    stage = 2; await resource.refresh();
    expect(resource.getSnapshot()).toMatchObject({ status: "forbidden", sets: [], readOnly: true });
    resource.dispose();
  });

  it("drops a set the read no longer lists, before or after its deadline on the browser clock, and keeps nothing of it", async () => {
    const other = `decision:${"c".repeat(64)}`;
    const c = clock(); let listed: unknown[] = [wire(), wire(other, { deadline_at_ms: 9_000_000 })];
    const resource = createQuestionSetResource({ scopeKey: "a", environment: c.environment, load: async () => ({ question_sets: listed }), decide: async () => ({}) });
    await tick();
    // Past the first set's deadline on this clock, before the second's.
    c.advance(3_600_000); await tick();
    // The gateway still lists both: the browser clock ends neither.
    expect(resource.getSnapshot().sets.map((entry) => entry.pendingId)).toEqual([PENDING_ID, other]);
    listed = [];
    await resource.refresh();
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets).toEqual([]);
    expect(snapshot.closed).toEqual([]);
    expect(snapshot.decisions).toEqual({});
    expect(questionSetEntries(snapshot)).toEqual([]);
    // One neutral line per set that left: a kind, a time and the card's own
    // place on this page, nothing of the set.
    expect(snapshot.notices.map((notice) => notice.kind)).toEqual(["gone", "gone"]);
    expect(nothingKept(snapshot)).toBe(true);
    resource.dispose();
  });

  it("removes a set whose decide the gateway answered as an id that is not pending, once the read no longer lists it", async () => {
    const c = clock(); let hidden = false; const phases: string[] = [];
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: hidden ? [] : [wire()] }),
      decide: async () => { hidden = true; throw rpcError(-32602, undefined, "gating decision failed"); },
    });
    await tick();
    resource.subscribe(() => { phases.push(resource.getSnapshot().decisions[PENDING_ID]?.phase ?? "none"); });
    await resource.answer(PENDING_ID, { q2: { kind: "free_text", text: "After the freeze" } });
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets).toEqual([]);
    expect(snapshot.closed).toEqual([]);
    expect(snapshot.decisions).toEqual({});
    expect(snapshot.notices.map((notice) => notice.kind)).toEqual(["gone"]);
    expect(nothingKept(snapshot)).toBe(true);
    expect(JSON.stringify(snapshot)).not.toContain("After the freeze");
    expect(JSON.stringify(snapshot)).not.toContain("gating decision failed");
    // The set was never shown as refused: it read as sending, then it was gone.
    expect(new Set(phases)).toEqual(new Set(["submitting", "none"]));
    // It stays gone on the reads that follow.
    c.advance(15_000); await tick();
    expect(questionSetEntries(resource.getSnapshot())).toEqual([]);
    resource.dispose();
  });

  it("keeps a set the gateway still lists after it refused the answer, with the gateway's own message", async () => {
    const c = clock();
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: [wire()] }),
      decide: async () => { throw rpcError(-32602, undefined, "answers: unknown choice"); },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets.map((entry) => entry.pendingId)).toEqual([PENDING_ID]);
    expect(snapshot.decisions[PENDING_ID]).toMatchObject({ phase: "refused", refusal: { kind: "not_accepted", detail: "answers: unknown choice" } });
    expect(questionSetEntries(snapshot).map((entry) => entry.set.pendingId)).toEqual([PENDING_ID]);
    resource.dispose();
  });

  it("says so in one line when a set leaves after a send it could not confirm", async () => {
    const c = clock(); let listed = true;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: listed ? [wire()] : [] }),
      decide: async () => { throw new Error("network"); },
    });
    await tick(); await resource.answer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } });
    expect(resource.getSnapshot().decisions[PENDING_ID].phase).toBe("failed");
    listed = false; c.advance(15_000); await tick();
    const snapshot = resource.getSnapshot();
    expect(snapshot.sets).toEqual([]);
    expect(snapshot.closed).toEqual([]);
    expect(snapshot.decisions).toEqual({});
    expect(snapshot.notices.map((notice) => notice.kind)).toEqual(["sent_unconfirmed"]);
    expect(nothingKept(snapshot)).toBe(true);
    resource.dispose();
  });

  it("drops a set answered in another tab at the next poll, and its line a minute later", async () => {
    const c = clock(); let listed = true;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: listed ? [wire()] : [] }),
      decide: async () => ({}),
    });
    await tick();
    const slot = questionSetEntries(resource.getSnapshot())[0].slot;
    expect(typeof slot).toBe("number");
    listed = false; c.advance(15_000); await tick();
    expect(resource.getSnapshot().sets).toEqual([]);
    expect(resource.getSnapshot().notices).toEqual([{ slot, kind: "gone", atMs: 15_000 }]);
    c.advance(45_000); await tick();
    expect(resource.getSnapshot().notices.length).toBe(1);
    c.advance(15_000); await tick();
    expect(resource.getSnapshot().notices).toEqual([]);
    resource.dispose();
  });

  it("lists the entries of one asking agent, open sets before closed ones", async () => {
    const c = clock(); const other = `decision:${"c".repeat(64)}`;
    const resource = createQuestionSetResource({
      scopeKey: "a", environment: c.environment,
      load: async () => ({ question_sets: [wire(), wire(other, { origin: { identity: "identity:billing" } })] }),
      decide: async () => ({}),
    });
    await tick();
    const snapshot = resource.getSnapshot();
    expect(questionSetEntries(snapshot).map((entry) => entry.set.pendingId)).toEqual([PENDING_ID, other]);
    expect(questionSetEntries(snapshot, "identity:billing").map((entry) => entry.set.pendingId)).toEqual([other]);
    expect(questionSetEntries(snapshot, "identity:nobody")).toEqual([]);
    resource.dispose();
  });
});
