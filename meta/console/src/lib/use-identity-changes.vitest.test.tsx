/**
 * The console's requests that wait for a person's decision: kept for the
 * life of the page, read again by their request id, and gone once decided,
 * withdrawn or ended.
 */
import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { createScriptedIdentityFamily } from "../../fixtures/scripted-identity-family.cjs";
import { CONSOLE_COMMAND_NAMES, consoleCommandMethod } from "./headless";
import { useIdentityChanges, type IdentityCommand } from "./use-identity-changes";

const ALL = { available: true, canCreate: true, canChange: true, canRemove: true };

function setup(flags = ALL, readable = false) {
  const family = createScriptedIdentityFamily();
  const onChanged = vi.fn();
  const run = async (command: IdentityCommand, params: Record<string, unknown>) => {
    const answer = family.handle(consoleCommandMethod(CONSOLE_COMMAND_NAMES[command]), params);
    if (answer.lost) throw new Error("timed out");
    if (answer.error) throw Object.assign(new Error(answer.error.message), { rpcError: answer.error });
    return answer.result;
  };
  const hook = renderHook(
    (props: { flags: typeof ALL; readable: boolean }) => useIdentityChanges({ flags: props.flags, readable: props.readable, run, onChanged }),
    { initialProps: { flags, readable } },
  );
  return { family, onChanged, hook };
}

const NONE = { available: false, canCreate: false, canChange: false, canRemove: false };

describe("the console's changes of long-term agents", () => {
  it("is offered where the gateway serves the changes or the reads", () => {
    // Neither the changes nor the reads: nothing of the family.
    expect(setup(NONE).hook.result.current).toBeNull();
    const { hook } = setup();
    expect(hook.result.current?.flags).toEqual(ALL);
    // The same flags keep the same object: a panel reads again only on a change.
    const first = hook.result.current;
    hook.rerender({ flags: { ...ALL }, readable: false });
    expect(hook.result.current).toBe(first);
    // The reads without the changes: the archived agents can be listed, and
    // the flags offer no control.
    const reads = setup(NONE, true).hook.result.current;
    expect(reads?.flags).toEqual(NONE);
  });

  it("keeps a request that waits for a decision until it is decided", async () => {
    const { family, onChanged, hook } = setup();
    const api = () => hook.result.current!;
    const requestId = api().mintRequestId();
    let result: Awaited<ReturnType<ReturnType<typeof api>["change"]>> | undefined;
    await act(async () => {
      result = await api().change("create", { agents: [{ name: "auditor", profile: "reviewer" }] }, requestId, "The auditor");
    });
    expect(result).toMatchObject({ kind: "outcome", outcome: { outcome: "pending_decision", decisionId: "decision-1" } });
    expect(api().pending).toEqual([{ requestId, decisionId: "decision-1", change: "create", name: "The auditor", deadlineMs: expect.any(Number) }]);
    // Nothing changed yet, so nothing is read again.
    expect(onChanged).not.toHaveBeenCalled();

    // Still waiting: it stays.
    await act(async () => { await api().refreshPending(); });
    expect(api().pending).toHaveLength(1);

    // A person decides it: the request leaves the list and the roster is read again.
    family.decide("decision-1");
    const revision = api().revision;
    await act(async () => { await api().refreshPending(); });
    expect(api().pending).toEqual([]);
    expect(onChanged).toHaveBeenCalledTimes(1);
    expect(api().revision).toBe(revision + 1);
    expect(family.agents.has("auditor")).toBe(true);
  });

  it("drops a request that ended without a change, and reads nothing again for it", async () => {
    const { family, onChanged, hook } = setup();
    const api = () => hook.result.current!;
    const requestId = api().mintRequestId();
    await act(async () => {
      await api().change("create", { agents: [{ name: "auditor", profile: "reviewer" }] }, requestId, "The auditor");
    });
    family.decide("decision-1", false);
    await act(async () => { await api().refreshPending(); });
    expect(api().pending).toEqual([]);
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("withdraws its own request by its request id", async () => {
    const { family, hook } = setup();
    const api = () => hook.result.current!;
    const requestId = api().mintRequestId();
    await act(async () => {
      await api().change("create", { agents: [{ name: "auditor", profile: "reviewer" }] }, requestId, "The auditor");
    });
    let withdrawn: unknown;
    await act(async () => { withdrawn = await api().withdraw(requestId); });
    expect(withdrawn).toEqual({ kind: "withdrawn" });
    expect(api().pending).toEqual([]);
    expect(family.calls.at(-1)).toEqual({ method: "meta/identity/withdraw", params: { request_id: requestId } });
    // A request that is no longer held answers the gateway's refusal.
    await act(async () => { withdrawn = await api().withdraw(requestId); });
    expect(withdrawn).toMatchObject({ kind: "refused", refusal: { kind: "decision_not_found", detail: "No such decision." } });
  });

  it("tells a refusal from a lost answer, and reads the roster again only when a change may have been made", async () => {
    const { family, onChanged, hook } = setup();
    const api = () => hook.result.current!;
    let result: unknown;
    await act(async () => {
      result = await api().change("create", { agents: [{ name: "warden", profile: "worker" }] }, "r-1", "warden");
    });
    expect(result).toMatchObject({ kind: "refused", refusal: { kind: "name_taken", field: "agents[0].name" } });
    expect(onChanged).not.toHaveBeenCalled();
    family.scriptNext("meta/identity/create", { fail: "after" });
    await act(async () => {
      result = await api().change("create", { agents: [{ name: "scout", profile: "worker" }] }, "r-2", "scout");
    });
    expect(result).toEqual({ kind: "failed", message: "timed out" });
    expect(onChanged).toHaveBeenCalledTimes(1);
    await act(async () => {
      result = await api().change("create", { agents: [{ name: "scout", profile: "worker" }] }, "r-2", "scout");
    });
    expect(result).toMatchObject({ kind: "outcome", outcome: { outcome: "done" } });
    expect(onChanged).toHaveBeenCalledTimes(2);
  });

  it("reads every answer of the gateway as an answer: only a lost one is no answer", async () => {
    const { family, onChanged, hook } = setup();
    const api = () => hook.result.current!;
    let result: unknown;
    // The gateway's other refusals are answers too: nothing was made, and
    // nothing is read again.
    for (const error of [
      { code: -32030, message: "access denied", data: { kind: "access_denied", action: "agent.identity.manage" } },
      { code: -32010, message: "console is read-only", data: { kind: "read_only" } },
      { code: -32047, message: "a candidate launch changes nothing", data: { kind: "candidate_launch" } },
      { code: -32000, message: "internal error" },
    ]) {
      family.scriptNext("meta/identity/create", { error });
      await act(async () => {
        result = await api().change("create", { agents: [{ name: "scout", profile: "worker" }] }, `r-${error.code}`, "scout");
      });
      expect(result).toMatchObject({
        kind: "refused",
        refusal: { kind: error.data?.kind ?? "", retryable: false, detail: error.message, origin: "gateway" },
      });
    }
    expect(onChanged).not.toHaveBeenCalled();
    expect(family.agents.has("scout")).toBe(false);
    // The family's own refusal carries no such mark.
    await act(async () => {
      result = await api().change("create", { agents: [{ name: "warden", profile: "worker" }] }, "r-own", "warden");
    });
    expect((result as { refusal: { origin?: string } }).refusal.origin).toBeUndefined();
  });

  it("asks the gateway what a change would get, under a request id of its own, and takes only its judgment as one", async () => {
    const { family, onChanged, hook } = setup();
    const api = () => hook.result.current!;
    const digest = family.handle("meta/identity/get", { name: "warden" }).result.spec_digest;
    const request = (profile: string) => ({ name: "warden", expected_spec_digest: digest, set: { profile } });
    let result: unknown;
    await act(async () => { result = await api().plan("update", request("operator")); });
    expect(result).toEqual({ kind: "plan", plan: { outcome: "allow" } });
    family.setProfileChange("reviewer", "deny");
    await act(async () => { result = await api().plan("update", request("reviewer")); });
    expect(result).toMatchObject({ kind: "refused", refusal: { kind: "change_denied", field: "set.profile" } });
    // A plan that was refused as a read, lost or unreadable says nothing
    // about the change: the gateway judges it when it is sent.
    for (const script of [
      { error: { code: -32030, message: "access denied", data: { kind: "access_denied", action: "agent.view" } } },
      { fail: "before" },
    ]) {
      family.scriptNext("meta/identity/plan", script);
      await act(async () => { result = await api().plan("update", request("operator")); });
      expect(result).toEqual({ kind: "unknown" });
    }
    const plans = family.calls.filter((call: { method: string }) => call.method === "meta/identity/plan").map((call: { params: Record<string, unknown> }) => call.params);
    expect(plans[0]).toEqual({ change: "update", request_id: expect.stringMatching(/\S/), ...request("operator") });
    // Each question is a request of its own.
    expect(new Set(plans.map((params: Record<string, unknown>) => params.request_id)).size).toBe(plans.length);
    // Nothing changed, so nothing is read again and nothing waits.
    expect(onChanged).not.toHaveBeenCalled();
    expect(api().pending).toEqual([]);
    expect(family.agents.get("warden").profile).toBe("worker");
  });

  it("reads every page of the directory and of the archived agents", async () => {
    const family = createScriptedIdentityFamily({ pageSize: 1 });
    family.seed({ name: "oldhand", archived: true });
    family.seed({ name: "relic", archived: true });
    const run = async (command: IdentityCommand, params: Record<string, unknown>) =>
      family.handle(consoleCommandMethod(CONSOLE_COMMAND_NAMES[command]), params).result;
    const { result } = renderHook(() => useIdentityChanges({ flags: ALL, readable: true, run, onChanged: () => {} }));
    const directory = await result.current!.loadDirectory();
    expect(directory.profiles.map((entry) => entry.profile)).toEqual(["worker", "reviewer", "operator"]);
    expect(directory.peers.map((entry) => entry.name)).toEqual(["helper", "warden"]);
    expect((await result.current!.loadArchived()).map((agent) => agent.name)).toEqual(["oldhand", "relic"]);
  });
});
