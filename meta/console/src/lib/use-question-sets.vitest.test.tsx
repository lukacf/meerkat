import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useQuestionSets } from "./use-question-sets";

afterEach(cleanup);

const PENDING_ID = `decision:${"a".repeat(64)}`;
const DIGEST = `sha256:${"b".repeat(64)}`;
const wire = {
  pending_id: PENDING_ID,
  origin: { identity: "identity:release-planner" },
  questions: [{ id: "q1", prompt: "Which region goes first?", choices: [{ id: "eu", label: "Europe" }, { id: "us", label: "United States" }] }],
  subject_digest: DIGEST,
};

function mount(initial: { scopeKey?: string; enabled?: boolean; readOnly?: boolean; readOnlyReason?: "sign_in" } = {}) {
  const load = vi.fn(async () => ({ question_sets: [wire] }));
  const decide = vi.fn(async (_params: unknown) => ({ pending_id: PENDING_ID, kind: "question_set", outcome: "answered", decided_at_ms: 7 }));
  const owner = {};
  // Every render's scope beside the scope of the snapshot it returned.
  const renders: Array<[string, string | undefined]> = [];
  const hook = renderHook(
    (props: { scopeKey: string; enabled: boolean; readOnly: boolean; readOnlyReason?: "sign_in" }) => {
      const view = useQuestionSets({ ...props, owner, load, decide });
      renders.push([props.scopeKey, view?.snapshot.scopeKey]);
      return view;
    },
    { initialProps: { scopeKey: "scope-a", enabled: true, readOnly: false, ...initial } },
  );
  return { hook, load, decide, renders };
}

describe("useQuestionSets", () => {
  it("reads the open sets for its scope and answers through the decide call", async () => {
    const { hook, load, decide } = mount();
    await waitFor(() => expect(hook.result.current?.snapshot.status).toBe("ready"));
    expect(hook.result.current?.snapshot.scopeKey).toBe("scope-a");
    expect(hook.result.current?.snapshot.sets.map((set) => set.pendingId)).toEqual([PENDING_ID]);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => { await hook.result.current?.onAnswer(PENDING_ID, { q1: { kind: "choice", choiceId: "eu" } }); });
    expect(decide).toHaveBeenCalledTimes(1);
    expect(decide.mock.calls[0][0]).toEqual({
      pending_id: PENDING_ID, decision: "approve", presented_digest: DIGEST,
      answers: [{ question_id: "q1", answer: { kind: "choice", choice_id: "eu" } }],
    });
    expect(hook.result.current?.snapshot.decisions[PENDING_ID].phase).toBe("answered");
  });

  it("reads nothing while disabled", async () => {
    const { hook, load } = mount({ enabled: false });
    await act(async () => { await Promise.resolve(); });
    expect(hook.result.current).toBeUndefined();
    expect(load).not.toHaveBeenCalled();
  });

  it("starts over for a new scope and never shows the previous scope's sets under it", async () => {
    const { hook, load, renders } = mount();
    await waitFor(() => expect(hook.result.current?.snapshot.status).toBe("ready"));
    hook.rerender({ scopeKey: "scope-b", enabled: true, readOnly: false });
    await waitFor(() => expect(hook.result.current?.snapshot.status).toBe("ready"));
    expect(hook.result.current?.snapshot.scopeKey).toBe("scope-b");
    expect(load).toHaveBeenCalledTimes(2);
    // No render, not even the one before the effects ran, paired a scope
    // with another scope's snapshot.
    expect(renders.some(([scope]) => scope === "scope-b")).toBe(true);
    expect(renders.filter(([scope, shown]) => shown !== undefined && shown !== scope)).toEqual([]);
  });

  it("shows a caller who is no signed-in person the sets and sends nothing for them", async () => {
    const { hook, decide } = mount({ readOnly: true, readOnlyReason: "sign_in" });
    await waitFor(() => expect(hook.result.current?.snapshot.status).toBe("ready"));
    expect(hook.result.current?.snapshot).toMatchObject({ readOnly: true, readOnlyReason: "sign_in" });
    await act(async () => { await hook.result.current?.onDismiss(PENDING_ID); });
    expect(decide).not.toHaveBeenCalled();
  });
});
