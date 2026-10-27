import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import { createHttpConsoleTransport, type MetaConsoleTransport } from "./headless";
import type { ConsoleAccessConfig, ConsoleFrame } from "../types";
import { createConsoleSendAttempt, beginConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, saveConsoleSendAttempts } from "./send-attempt-storage";
import { createConsoleContextRecord } from "../../../packages/console-core/src/context-record";
import { ACCEPTANCE_NOTICE_GRACE_MS, PendingStack, type PendingItem } from "../panels/PendingStack";
import { idleSettleClock } from "./idle-reconcile";

const identity = "identity:queue-agent";
function seed(twoPanes = false) {
  const target = { id: `chat:${identity}`, kind: "agent-chat", title: "Queue agent", identity, memberId: identity };
  window.localStorage.setItem("meta-console-dock-state:runtime/realm/principal", JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: twoPanes ? { kind: "split", id: "split-1", direction: "horizontal", ratio: 0.5, first: { kind: "panel", panelId: "panel-1" }, second: { kind: "panel", panelId: "panel-2" } } : { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", mode: "console", target }, ...(twoPanes ? [{ id: "panel-2", mode: "console", target }] : [])],
    activeTabId: "tab-1", focusedPanelId: "panel-1",
  }));
}
function transport(send: MetaConsoleTransport["send"]): MetaConsoleTransport {
  return {
    loadExperience: async () => ({ runtime_id: "queue-test", console_config: { layout: { initial_agent: identity } }, agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Queue agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } }, activity_feed: { filter_presets: [], active_preset_id: "all" } }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline"] }) as never,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {}, send,
    executeCommand: async () => ({ result: {} }) as never,
    upload: async () => ({ blob_id: "blob" }) as never, blobUrl: (id) => `/blobs/${id}`,
  };
}
async function compose(text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
  return textarea;
}
beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear(); seed();
  window.sessionStorage.clear();
  // One browser lock queue provides the same mutual exclusion boundary as Web Locks.
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("owner activity refresh", () => {
  it.each(["interaction_started", "run_started"])("refreshes Working from the owner before the fallback poll on %s", async (event) => {
    const fake = transport(vi.fn());
    const quiet = await fake.loadExperience();
    const quietAgents = quiet.agent_sidebar!.live_snapshot!.agents!;
    quietAgents[0].response_phase = null;
    let finishRefresh!: (value: typeof quiet) => void;
    fake.loadExperience = vi.fn()
      .mockResolvedValueOnce(quiet)
      .mockImplementation(() => new Promise(resolve => { finishRefresh = resolve; }));
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`sidebar-agent:${identity}`);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    expect(fake.loadExperience).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Working", exact: true }));
    expect(screen.queryByTestId(`sidebar-agent:${identity}`)).toBeNull();
    await act(async () => {
      for (const id of ["text-run-start", "text-run-start-repeat"]) {
        receive?.({ id, event, identity, interactionId: "text-only-run", timestampMs: Date.now(), data: { content: "A text-only request", prompt: "A text-only request" } } as never);
      }
      receive?.({ id: "text-run-delta", event: "text_delta", identity, interactionId: "text-only-run", timestampMs: Date.now(), data: { delta: "Working on the answer" } } as never);
    });
    await waitFor(() => expect(fake.loadExperience).toHaveBeenCalledTimes(2), { timeout: 1_000 });
    // Live chat activity is only a freshness signal; the pending owner reply
    // still says quiet, so it must not fabricate a Working roster row.
    expect(screen.queryByTestId(`sidebar-agent:${identity}`)).toBeNull();
    const working = {
      ...quiet,
      agent_sidebar: { ...quiet.agent_sidebar, live_snapshot: {
        ...quiet.agent_sidebar?.live_snapshot,
        agents: quietAgents.map(agent => ({ ...agent, response_phase: "waiting" })),
      } },
    };
    await act(async () => { finishRefresh(working as typeof quiet); });
    await screen.findByTestId(`sidebar-agent:${identity}`);
    expect(fake.loadExperience).toHaveBeenCalledTimes(2);
  });
});

describe("stock durable queue integration", () => {
  it.each(["interaction_complete", "interaction_failed"])("holds a queued send through an older run's %s and releases on the current run", async event => {
    const send = vi.fn(async input => ({ interaction_id: "new-work", identity: input.identity }));
    const fake = transport(send);
    const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session", interactionId: "input", sourceKind: "console_event" as const };
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    fake.queryTimeline = async () => ({ available: true, frames: [
      { ...scope, id: "old-start", event: "run_started", runId: "old-run", timestampMs: 1, data: {} },
      { ...scope, id: "current-start", event: "run_started", runId: "current-run", timestampMs: 2, data: {} },
      { ...scope, id: "current-tool", event: "tool_execution_started", runId: "current-run", timestampMs: 3, data: { id: "tool", name: "working" } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await compose("wait for current run");
    await screen.findByTestId("pending-stack");
    await act(async () => {
      receive?.({ ...scope, id: "old-terminal", event, runId: "old-run", timestampMs: 4, data: {} } as never);
      await new Promise(resolve => window.setTimeout(resolve, 20));
    });
    expect(send).not.toHaveBeenCalled();
    expect(within(screen.getByTestId("pending-stack")).getByText("Agent busy")).toBeVisible();
    await act(async () => {
      receive?.({ ...scope, id: "current-terminal", event, runId: "current-run", timestampMs: 5, data: {} } as never);
    });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].content).toBe("wait for current run");
  });

  it.each([
    ["live run", false], ["live run", true],
    ["server phase", false], ["server phase", true],
  ] as const)("keeps a send queued after history reload with %s and typed history IDs=%s", async (evidence, historyIds) => {
    const send = vi.fn(async input => ({ interaction_id: "new-work", identity: input.identity }));
    const fake = transport(send);
    const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session" };
    const experience = await fake.loadExperience();
    if (evidence === "server phase") experience.agent_sidebar!.live_snapshot!.agents![0].response_phase = "waiting";
    fake.loadExperience = async () => experience;
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    fake.queryTimeline = async () => ({ available: true, frames: [
      { ...scope, id: "old-terminal", event: "interaction_complete", sourceKind: "console_event",
        interactionId: "old-input", runId: "old-run", timestampMs: 1, data: { text: "Saved work finished" } },
      ...(evidence === "live run" ? [{ ...scope, id: "current-run", event: "run_started", sourceKind: "console_event" as const,
        runId: "current-run", timestampMs: 2, data: {} }] : []),
      { ...scope, id: "saved-tool-result", event: "tool_execution_completed", sourceKind: "session_history",
        ...(historyIds ? { runId: evidence === "live run" ? "current-run" : "old-run",
          interactionId: evidence === "live run" ? "current-input" : "old-input" } : {}),
        timestampMs: 3, data: { id: "old-tool", result: "Saved tool output" } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await screen.findByText("Saved work finished", { selector: "p" });
    await compose("queued work waits for the current owner");
    const stack = await screen.findByTestId("pending-stack");
    // Let render effects and the asynchronous storage lock queue attempt to drain.
    await act(async () => { await new Promise(resolve => window.setTimeout(resolve, 20)); });
    expect(within(stack).getByText("queued work waits for the current owner")).toBeVisible();
    if (evidence === "live run") expect(within(stack).getByText("Agent busy")).toBeVisible();
    expect(send).not.toHaveBeenCalled();
    if (evidence === "live run") {
      await act(async () => {
        receive?.({ ...scope, id: "current-complete", event: "interaction_complete", sourceKind: "console_event",
          ...(historyIds ? { interactionId: "current-input" } : { runId: "current-run" }),
          timestampMs: 4, data: { text: "Current work finished" } } as never);
      });
      await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    }
  });

  it("sends new work after loading saved tool results beyond the snapshot observation", async () => {
    const send = vi.fn(async input => ({ interaction_id: "new-work", identity: input.identity }));
    const fake = transport(send);
    const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session" };
    fake.queryTimeline = async () => ({ available: true, frames: [
      { ...scope, id: "old-terminal", cursor: "console:39", event: "interaction_complete", sourceKind: "console_event",
        interactionId: "old-input", runId: "old-run", timestampMs: 1, data: { text: "Saved work finished" } },
      { ...scope, id: "saved-tool-result", cursor: "console:40", event: "tool_execution_completed", sourceKind: "session_history",
        timestampMs: 2, data: { id: "old-tool", result: "Saved tool output" } },
      { ...scope, id: "history-snapshot", cursor: "console:43", event: "assistant_history_snapshot", sourceKind: "session_history",
        timestampMs: 3, data: { session_id: scope.sessionId, complete: true, observed_through: "console:39", assistant_message_ids: ["saved-answer"] } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await screen.findByText("Saved work finished", { selector: "p" });
    await compose("new work after history reload");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].content).toBe("new work after history reload");
    expect(screen.queryByText("Agent busy")).toBeNull();
  });

  it("persists default embedded drafts and legacy queues in the server-owned scope", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "accepted", identity: input.identity }));
    const fake = transport(send);
    const experience = await fake.loadExperience();
    fake.loadExperience = async () => ({ ...experience, storage_scope: "console-storage:v1:owner-a" });
    fake.queryTimeline = async () => ({ available: true, frames: [{ id: "busy", event: "interaction_started", identity, interactionId: "busy", timestampMs: 1, data: { content: "working" } }] });
    const legacy = JSON.stringify([{ id: "pre-upgrade", text: "Preserve old queued instruction", addedAt: 1 }]);
    window.localStorage.setItem(`mobkit-pending-stack:${identity}`, legacy);
    const props = { baseUrl: "", transport: fake };
    const view = render(<ConsoleApp {...props} />);
    const composer = await screen.findByTestId(`chat-composer:${identity}`);
    fireEvent.change(composer, { target: { value: "Unsent default embedded draft" } });
    const importNotice = await screen.findByRole("group", { name: "Older queued messages" });
    const importButton = within(importNotice).getByRole("button", { name: "Import and send older queued messages" });
    expect(importButton).toHaveTextContent(/^Import and send$/);
    expect(within(importNotice).getByText("Resume them with this agent in this account.")).toBeVisible();
    expect(screen.queryByTestId("pending-stack")).toBeNull();
    expect(window.localStorage.getItem(`mobkit-pending-stack:${identity}`)).toBe(legacy);
    expect(send).not.toHaveBeenCalled();
    fireEvent.click(importButton);
    expect(within(await screen.findByTestId("pending-stack")).getByText("Preserve old queued instruction")).toBeVisible();
    expect(composer).toHaveValue("Unsent default embedded draft");
    expect(screen.queryByRole("group", { name: "Older queued messages" })).toBeNull();
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 160)); });
    view.unmount();
    render(<ConsoleApp {...props} />);
    await waitFor(() => expect((screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe("Unsent default embedded draft"));
    await screen.findByTestId("pending-stack");
    expect(screen.queryByRole("button", { name: "Import and send older queued messages" })).toBeNull();
    expect(window.localStorage.getItem(`mobkit-pending-stack:${identity}`)).toBe(legacy);
    expect(send).not.toHaveBeenCalled();
  });

  it("clears old authorized content when the server principal scope changes at the same URL", async () => {
    const fake = transport(vi.fn());
    const initial = await fake.loadExperience();
    let serverScope = "principal-a";
    fake.loadExperience = vi.fn(async () => ({ ...initial, storage_scope: serverScope }));
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const composer = await screen.findByTestId(`chat-composer:${identity}`);
    fireEvent.change(composer, { target: { value: "Principal A private draft" } });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 160)); });
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    serverScope = "principal-b";
    await act(async () => { receive?.({ id: "scope-refresh", event: "interaction_started", identity, interactionId: "refresh", timestampMs: 2, data: { content: "Trigger scope refresh" } } as never); });
    await waitFor(() => expect(fake.loadExperience).toHaveBeenCalledTimes(3));
    await waitFor(() => expect((screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe(""));
    expect(screen.queryByText("Trigger scope refresh")).toBeNull();
  });

  it("reconciles a lost alias send receipt only after fresh owner resolution and exact canonical query", async () => {
    const canonical = "canonical/queue-agent";
    const scope = "alias-recovery-scope";
    const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "lost-alias", scope, destination: identity,
      origin: "console:old-pane", idempotencyKey: "lost-key", text: "Keep exact alias intent", now: 1 }), { owner: "old-tab", now: 2, handlingMode: "steer" });
    const unknown = { ...attempted, state: "outcome-unknown" as const, lease: undefined };
    saveConsoleSendAttempts(window.localStorage, scope, identity, [unknown]);
    const send = vi.fn();
    const fake = transport(send);
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] }) as never;
    const calls: string[] = [];
    const receipt = { id: "canonical-receipt", event: "user_input", identity: canonical, interactionId: "canonical-turn", timestampMs: 2, data: { ...JSON.parse(attempted.envelopeJson!), identity: canonical } };
    fake.executeCommand = vi.fn(async input => {
      if (input.command !== "inspectIdentity") throw new Error("Unexpected command");
      if (!("identity" in input.target)) throw new Error("Inspection needs an identity target");
      calls.push(`inspect:${input.target.identity}`);
      return { command: input.command, accepted: true, result: { identity: { identity: canonical, runtime_key: "runtime", runtime_member_id: identity, addressable: true }, peers: [] } };
    });
    fake.queryTimeline = vi.fn(async input => {
      calls.push(`query:${input.identity}`);
      // Real store filtering is exact: querying the alias cannot find C's row.
      return { available: true, frames: input.identity === canonical ? [receipt] : [] };
    });
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    fireEvent.click(await screen.findByRole("button", { name: "Check" }));
    await waitFor(() => { expect(screen.queryByTestId(/^pending-item:/)).toBeNull(); expect(screen.getByTestId(/^pending-delivered:/)).toHaveTextContent(/^Delivered/); });
    expect(calls).toContain(`inspect:${identity}`);
    expect(calls.indexOf(`inspect:${identity}`)).toBeLessThan(calls.indexOf(`query:${canonical}`));
    expect(send).not.toHaveBeenCalled();
    expect(unknown.envelopeJson).toBe(attempted.envelopeJson);
    expect(JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts).toEqual([]);
  });

  it.each([true, false])("keeps the fresh acceptance page visible through a replay gap (exact receipt: %s)", async hasReceipt => {
    const scope = "acceptance-page-scope";
    const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "lost-receipt", scope, destination: identity,
      origin: "console:old-pane", idempotencyKey: "acceptance-page-key", text: "Check this saved instruction", now: 1 }),
    { owner: "old-tab", now: 2, handlingMode: "steer" });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [{ ...attempted, state: "outcome-unknown", lease: undefined }]);
    const fake = transport(vi.fn());
    fake.capabilities = async () => ({ methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] });
    let checkingAcceptance = false;
    fake.executeCommand = vi.fn(async input => {
      checkingAcceptance = true;
      return { command: input.command, accepted: true, result: { identity: { identity } } };
    });
    const before = { id: "before-check", event: "interaction_complete", identity, cursor: "console:1", timestampMs: 1,
      interactionId: "before-check", data: { text: "Already loaded reply" } };
    const queried = { id: "queried-reply", event: "interaction_complete", identity, cursor: "console:2", timestampMs: 2,
      interactionId: "queried-turn", data: { text: "Reply found by acceptance check" } };
    const receipt = { id: "queried-input", event: "user_input", identity, cursor: "console:3", timestampMs: 3,
      interactionId: "accepted-turn", data: { ...JSON.parse(attempted.envelopeJson!),
        ...(hasReceipt ? {} : { idempotency_key: "another-request" }) } };
    const later = { id: "after-gap", event: "interaction_complete", identity, cursor: "console:4", timestampMs: 4,
      interactionId: "after-gap", data: { text: "Reply after replay recovery" } };
    fake.queryTimeline = vi.fn(async input => {
      if (!input.identity) return { available: true, frames: [] };
      if (input.mode === "since") return { available: true, frames: [later], nextCursor: "console:4" };
      return checkingAcceptance
        ? { available: true, frames: [queried, receipt], latestCursor: "console:3", exhausted: false }
        : { available: true, frames: [before], latestCursor: "console:1", exhausted: false };
    });
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    const pane = within(await screen.findByTestId(`chat-pane:${identity}`));
    await pane.findByText("Already loaded reply", { selector: "p" });
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    fireEvent.click(await screen.findByRole("button", { name: "Check" }));
    if (hasReceipt) await waitFor(() => { expect(screen.queryByTestId(/^pending-item:/)).toBeNull(); expect(screen.getByTestId(/^pending-delivered:/)).toHaveTextContent(/^Delivered/); });
    else expect(await screen.findByTestId("pending-check:lost-receipt")).toHaveTextContent("Not found in Queue agent's recent messages.");
    await pane.findByText("Reply found by acceptance check", { selector: "p" });
    await act(async () => { receive?.({ event: "replay_unavailable", data: {} } as never); });
    await waitFor(() => expect(fake.queryTimeline).toHaveBeenCalledWith(expect.objectContaining({ identity, mode: "since", after: "console:3" })));
    await pane.findByText("Reply after replay recovery", { selector: "p" });
    // Recovery correctly begins after the checked page. Its reply must already
    // be retained locally, because the server will never return it in this gap.
    expect(pane.getAllByText("Reply found by acceptance check", { selector: "p" })).toHaveLength(1);
    expect(pane.getAllByText("Already loaded reply", { selector: "p" })).toHaveLength(1);
    expect(fake.send).not.toHaveBeenCalled();
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts;
    expect(saved).toHaveLength(hasReceipt ? 0 : 1);
    if (!hasReceipt) expect(saved[0].envelopeJson).toBe(attempted.envelopeJson);
  });

  it.each(["principal changed", "attempt removed"])("ignores a late acceptance page when the %s", async staleReason => {
    const scope = "late-acceptance-scope";
    const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "late-receipt", scope, destination: identity,
      origin: "console:old-pane", idempotencyKey: "late-acceptance-key", text: "Keep this attempt scoped", now: 1 }),
    { owner: "old-tab", now: 2, handlingMode: "steer" });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [{ ...attempted, state: "outcome-unknown", lease: undefined }]);
    const fake = transport(vi.fn());
    fake.capabilities = async () => ({ methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] });
    let checkingAcceptance = false;
    let finishCheck!: (page: Awaited<ReturnType<MetaConsoleTransport["queryTimeline"]>>) => void;
    fake.executeCommand = vi.fn(async input => {
      checkingAcceptance = true;
      return { command: input.command, accepted: true, result: { identity: { identity } } };
    });
    const before = { id: "before-late-check", event: "interaction_complete", identity, cursor: "console:1", timestampMs: 1,
      interactionId: "before-late-check", data: { text: "Current authorized reply" } };
    fake.queryTimeline = vi.fn(async input => {
      if (!input.identity) return { available: true, frames: [] };
      if (checkingAcceptance) {
        checkingAcceptance = false;
        return new Promise(resolve => { finishCheck = resolve; });
      }
      return { available: true, frames: [before], latestCursor: "console:1", exhausted: false };
    });
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    const view = render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    await within(await screen.findByTestId(`chat-pane:${identity}`)).findByText("Current authorized reply");
    fireEvent.click(await screen.findByRole("button", { name: "Check" }));
    await waitFor(() => expect(finishCheck).toBeTypeOf("function"));
    if (staleReason === "principal changed") {
      view.rerender(<ConsoleApp baseUrl="" storageNamespace="next-acceptance-principal" transport={fake} />);
      await within(await screen.findByTestId(`chat-pane:${identity}`)).findByText("Current authorized reply");
    } else {
      fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    }
    await waitFor(() => expect(screen.queryByTestId("pending-stack")).toBeNull());
    await act(async () => { finishCheck({ available: true, frames: [{ id: "stale-check-reply", event: "interaction_complete", identity,
      cursor: "console:50", timestampMs: 50, interactionId: "stale-check", data: { text: "Stale acceptance reply" } }], latestCursor: "console:50" }); });
    expect(screen.queryByText("Stale acceptance reply")).toBeNull();
    await act(async () => { receive?.({ event: "replay_unavailable", data: {} } as never); });
    await waitFor(() => expect(fake.queryTimeline).toHaveBeenCalledWith(expect.objectContaining({ identity, mode: "since", after: "console:1" })));
    expect(fake.queryTimeline).not.toHaveBeenCalledWith(expect.objectContaining({ after: "console:50" }));
    expect(screen.queryByText("Stale acceptance reply")).toBeNull();
    expect(fake.send).not.toHaveBeenCalled();
  });

  it.each(["denied", "missing identity"])("keeps the frozen alias attempt after %s owner resolution without resending", async failure => {
    const scope = "alias-resolution-failure";
    const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "unresolved-alias", scope, destination: identity,
      origin: "console:old-pane", idempotencyKey: "unresolved-key", text: "Preserve this attempt", now: 1 }), { owner: "old-tab", now: 2, handlingMode: "queue" });
    const unknown = { ...attempted, state: "outcome-unknown" as const, lease: undefined };
    saveConsoleSendAttempts(window.localStorage, scope, identity, [unknown]);
    const fake = transport(vi.fn());
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] }) as never;
    fake.executeCommand = vi.fn(async input => {
      if (failure === "denied") throw new Error("Owner resolution denied");
      return { command: input.command, accepted: true, result: { identity: { display_name: "Not canonical identity" } } };
    });
    fake.queryTimeline = vi.fn(async () => ({ available: true, frames: [] }));
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    const action = await screen.findByRole("button", { name: "Check" });
    const previousQueries = vi.mocked(fake.queryTimeline).mock.calls.length;
    fireEvent.click(action);
    await waitFor(() => expect(fake.executeCommand).toHaveBeenCalled());
    expect(screen.getByTestId("pending-stack")).toBeTruthy();
    expect(fake.send).not.toHaveBeenCalled();
    expect(vi.mocked(fake.queryTimeline).mock.calls.length).toBe(previousQueries);
    expect(JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts[0].envelopeJson).toBe(attempted.envelopeJson);
  });

  it.each(["exact receipt", "foreign identity", "different request", "fresh query denied"])(
    "checks acceptance beyond the recent 200 frames with %s", async evidence => {
      const canonical = "canonical/queue-agent";
      const scope = "older-receipt-scope";
      const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "older-receipt", scope, destination: identity,
        origin: "console:old-pane", idempotencyKey: "older-key", text: "Preserve older acceptance", now: 1 }),
      { owner: "old-tab", now: 2, handlingMode: "steer" });
      const unknown = { ...attempted, state: "outcome-unknown" as const, lease: undefined };
      saveConsoleSendAttempts(window.localStorage, scope, identity, [unknown]);
      const fake = transport(vi.fn());
      fake.capabilities = async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] }) as never;
      const calls: string[] = [];
      fake.executeCommand = vi.fn(async input => {
        if (input.command !== "inspectIdentity" || !("identity" in input.target)) throw new Error("Unexpected command");
        calls.push(`inspect:${input.target.identity}`);
        return { command: input.command, accepted: true, result: { identity: { identity: canonical }, peers: [] } };
      });
      const receipt = { id: "older-canonical-receipt", event: "user_input", cursor: "console:1", timestampMs: 1,
        identity: evidence === "foreign identity" ? "canonical/someone-else" : canonical,
        interactionId: "older-accepted-turn", data: { ...JSON.parse(attempted.envelopeJson!), identity: canonical,
          ...(evidence === "different request" ? { idempotency_key: "another-request" } : {}) } };
      const newer = Array.from({ length: 201 }, (_, index) => ({ id: `newer-${index}`, event: "text_delta",
        identity: canonical, cursor: `console:${index + 2}`, timestampMs: index + 2,
        interactionId: "later-turn", data: { delta: "." } }));
      fake.queryTimeline = vi.fn(async input => {
        if (input.identity !== canonical) return { available: true, frames: [] };
        calls.push(`query:${input.identity}`);
        if (evidence === "fresh query denied") throw new Error("Fresh canonical query denied");
        return { available: true, frames: newer.slice(-200), exhausted: false, latestCursor: "console:202" };
      });
      let receive: ((frame: never) => void) | undefined;
      fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
      render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
      const action = await screen.findByRole("button", { name: "Check" });
      await waitFor(() => expect(receive).toBeTypeOf("function"));
      // The current authorized stream loaded the receipt before 201 newer
      // events. The fresh server page truthfully omits that older receipt.
      await act(async () => { for (const frame of [receipt, ...newer]) receive?.(frame as never); });
      fireEvent.click(action);
      if (evidence === "exact receipt") {
        await waitFor(() => { expect(screen.queryByTestId(/^pending-item:/)).toBeNull(); expect(screen.getByTestId(/^pending-delivered:/)).toHaveTextContent(/^Delivered/); });
        expect(JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts).toEqual([]);
      } else {
        // The check answers on the row it checked, with a typed outcome.
        const check = await screen.findByTestId("pending-check:older-receipt");
        await waitFor(() => expect(check).toHaveTextContent(evidence === "fresh query denied"
          ? "Couldn't check: the server couldn't look up Queue agent's messages."
          : "Not found in Queue agent's recent messages."));
        expect(screen.getByTestId("pending-stack")).toBeTruthy();
        const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts;
        expect(saved).toHaveLength(1);
        expect(saved[0].state).toBe("outcome-unknown");
        expect(saved[0].envelopeJson).toBe(attempted.envelopeJson);
      }
      expect(calls).toEqual([`inspect:${identity}`, `query:${canonical}`]);
      expect(fake.send).not.toHaveBeenCalled();
    },
  );

  it("accepts canonical destination receipts for an alias without retrying", async () => {
    const send = vi.fn(async () => ({ interaction_id: "alias-accepted", identity: "canonical/queue-agent", input_frame_id: "canonical-input" }));
    render(<ConsoleApp baseUrl="" storageNamespace="alias-scope" transport={transport(send)} />);
    await compose("Address through an alias");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByTestId("pending-stack")).toBeNull());
    expect(screen.queryByText(/did not prove acceptance/)).toBeNull();
  });

  it.each(["namespace", "transport", "baseUrl"] as const)("clears the entire authorized console immediately when %s changes", async (change) => {
    const send = vi.fn(async (input) => ({ interaction_id: "unused", identity: input.identity }));
    const old = transport(send);
    old.queryTimeline = async () => ({ available: true, frames: [{ id: "private-frame", event: "interaction_complete", identity, interactionId: "private", timestampMs: 1, data: { text: "Previous principal private transcript" } }] });
    let oldReceive: ((frame: never) => void) | undefined;
    old.subscribeTimeline = (_input, receive) => { oldReceive = receive; return () => {}; };
    const view = render(<ConsoleApp baseUrl="old-url" storageNamespace="principal-one" transport={old} />);
    await screen.findAllByText("Previous principal private transcript");
    fireEvent.change(await screen.findByTestId(`chat-composer:${identity}`), { target: { value: "Previous private draft" } });
    const next = change === "transport" ? transport(send) : old;
    let finish!: (value: never) => void;
    next.loadExperience = vi.fn(() => new Promise(resolve => { finish = resolve; }));
    view.rerender(<ConsoleApp baseUrl={change === "baseUrl" ? "new-url" : "old-url"} storageNamespace={change === "namespace" ? "principal-two" : "principal-one"} transport={next} />);
    expect(screen.queryAllByText("Previous principal private transcript")).toHaveLength(0);
    expect(screen.queryByTestId(`sidebar-agent:${identity}`)).toBeNull();
    expect(screen.queryByDisplayValue("Previous private draft")).toBeNull();
    await act(async () => { oldReceive?.({ id: "late-old-frame", event: "interaction_complete", identity, interactionId: "late-private", timestampMs: 2, data: { text: "Late previous authority transcript" } } as never); });
    expect(screen.queryByText("Late previous authority transcript")).toBeNull();
    await waitFor(() => expect(next.loadExperience).toHaveBeenCalledTimes(1));
    await act(async () => { finish({ runtime_id: "queue-test", console_config: { layout: { initial_agent: identity } }, agent_sidebar: { live_snapshot: { agents: [] } }, activity_feed: { filter_presets: [], active_preset_id: "all" } } as never); });
    await waitFor(() => expect(screen.queryByTestId("console-loading")).toBeNull());
    expect(screen.queryByTestId(`sidebar-agent:${identity}`)).toBeNull();
    expect(screen.queryByTestId(`chat-composer:${identity}`)).toBeNull();
    expect(send).not.toHaveBeenCalled();
  });

  it("ignores an old experience promise after a same-URL transport replacement", async () => {
    const old = transport(vi.fn()); let resolveOld!: (value: never) => void;
    old.loadExperience = () => new Promise(resolve => { resolveOld = resolve; });
    const view = render(<ConsoleApp baseUrl="" transport={old} storageNamespace="scope" />);
    const next = transport(vi.fn());
    const nextExperience = await next.loadExperience();
    next.loadExperience = async () => ({ ...nextExperience, runtime_id: "new-runtime", agent_sidebar: { live_snapshot: { agents: [] } } }) as never;
    view.rerender(<ConsoleApp baseUrl="" transport={next} storageNamespace="scope" />);
    await waitFor(() => expect(screen.queryByTestId("console-loading")).toBeNull());
    await act(async () => { resolveOld(nextExperience as never); });
    expect(screen.queryByTestId(`sidebar-agent:${identity}`)).toBeNull();
    expect(screen.queryByTestId(`chat-composer:${identity}`)).toBeNull();
  });

  it("clears pending approval data before loading a new principal", async () => {
    const fake = transport(vi.fn());
    fake.capabilities = async () => ({ methods: ["meta/gating/pending", "meta/gating/decide", "meta/console/send"] });
    fake.executeCommand = async () => ({ result: { pending: [{ pending_id: "private-gate", action_id: "private-action", action: "Private principal approval", origin: { identity }, risk_tier: "r3" }] } }) as never;
    const view = render(<ConsoleApp baseUrl="" transport={fake} storageNamespace="old-principal" />);
    fireEvent.click(await screen.findByRole("button", { name: "Needs you, 1 pending approval" }));
    await screen.findAllByText("Private principal approval");
    fake.loadExperience = () => new Promise(() => {});
    view.rerender(<ConsoleApp baseUrl="" transport={fake} storageNamespace="new-principal" />);
    expect(screen.queryAllByText("Private principal approval")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
  });

  it("cannot dispatch a persisted intent from a lock callback after unmount", async () => {
    const scope = "scope";
    saveConsoleSendAttempts(window.localStorage, scope, identity, [createConsoleSendAttempt({ id: "unmount-lock", scope, destination: identity, origin: "console:old", idempotencyKey: "unmount-key", text: "old intent", now: 1 })]);
    const callbacks: Array<() => void> = [];
    Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => new Promise(resolve => callbacks.push(() => resolve(callback()))) } });
    const send = vi.fn(async input => ({ interaction_id: "unexpected", identity: input.identity }));
    const view = render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(send)} />);
    await waitFor(() => expect(callbacks.length).toBeGreaterThan(0));
    view.unmount();
    await act(async () => { callbacks.splice(0).forEach(callback => callback()); });
    expect(send).not.toHaveBeenCalled();
    expect(JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts[0].state).toBe("draft");
  });

  it("cannot dispatch after an in-flight capability check outlives the console", async () => {
    const send = vi.fn(async input => ({ interaction_id: "unexpected", identity: input.identity }));
    const fake = transport(send); let finish!: (value: never) => void;
    fake.capabilities = vi.fn(() => new Promise(resolve => { finish = resolve; }));
    const view = render(<ConsoleApp baseUrl="" transport={fake} storageNamespace="scope" />);
    await compose("waiting for capability");
    await waitFor(() => expect(fake.capabilities).toHaveBeenCalled());
    view.unmount();
    await act(async () => { finish({ version: "test", methods: ["meta/console/send"] } as never); });
    expect(send).not.toHaveBeenCalled();
  });

  it("loads and sends through the current lifetime in StrictMode", async () => {
    const send = vi.fn(async input => ({ interaction_id: "strict-accepted", identity: input.identity }));
    render(<React.StrictMode><ConsoleApp baseUrl="" transport={transport(send)} storageNamespace="scope" /></React.StrictMode>);
    await compose("strict lifetime");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
  });

  it("persists before sending and removes only after canonical acceptance", async () => {
    let finish!: (value: never) => void;
    const send = vi.fn(async () => new Promise<never>((resolve) => { finish = resolve; }));
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await compose("  byte exact\n🌳  ");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey("runtime/realm/principal", identity))!);
    expect(saved.attempts[0].state).toBe("attempting");
    expect(JSON.parse(saved.attempts[0].envelopeJson).content).toBe("  byte exact\n🌳  ");
    // Saved, but a send in flight stays out of the stack for the grace period.
    expect(screen.queryByTestId("pending-stack")).toBeNull();
    await act(async () => { finish({ interaction_id: "accepted", identity, input_frame_id: "canonical-frame" } as never); });
    await waitFor(() => expect(screen.queryByTestId("pending-stack")).toBeNull());
  });
  it("retains a lost response through reload without another dispatch", async () => {
    const send = vi.fn(async () => { throw new Error("response lost"); });
    const props = { baseUrl: "", storageNamespace: "runtime/realm/principal", transport: transport(send) };
    const view = render(<ConsoleApp {...props} />); await compose("saved unknown");
    await waitFor(() => expect(screen.getByText(/We couldn't confirm/)).toBeTruthy());
    expect(send).toHaveBeenCalledTimes(1); view.unmount();
    render(<ConsoleApp {...props} />);
    await waitFor(() => expect(screen.getByText(/We couldn't confirm/)).toBeTruthy());
    expect(send).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId(/^pending-steer:/)).toBeNull();
    expect(screen.queryByTestId(/^pending-edit:/)).toBeNull();
  });
  it("keeps a saved send failure in its recovery row without a duplicate global banner", async () => {
    const send = vi.fn(async () => { throw new Error("response lost after dispatch"); });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await compose("Keep this exact instruction");
    await screen.findByText(/We couldn't confirm/);
    expect(screen.queryByTestId("console-action-error")).toBeNull();
    expect(screen.getByRole("button", { name: "Check", exact: true })).toBeEnabled();
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey("runtime/realm/principal", identity))!).attempts;
    expect(saved).toHaveLength(1);
    expect(saved[0].state).toBe("outcome-unknown");
    expect(saved[0].error).toBe("The console could not confirm acceptance: response lost after dispatch. Check acceptance before retrying.");
    expect(saved[0].failureKind).toBe("unknown");
    expect(send).toHaveBeenCalledTimes(1);
  });
  // What the fetch layer raises for the gateway's typed 401 (see network.ts
  // and tests/console_route_auth.rs): no reservation exists server-side.
  const unauthenticated = () => Object.assign(new Error("meta/console/send request failed 401: code=-32600 unauthorized"), {
    httpStatus: 401,
    responseRpcError: { code: -32600, message: "unauthorized: console rpc requires a valid auth token", data: { kind: "unauthenticated", http_status: 401 } },
  });
  const savedAttempts = (scope = "runtime/realm/principal") =>
    JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts;

  it("shows a refused queue send as a typed 401, never as a pending acceptance, and clears the busy mark", async () => {
    const send = vi.fn(async () => { throw unauthenticated(); });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await compose("sent from off the home network");
    const row = await screen.findByTestId(/^pending-item:/);
    await waitFor(() => expect(within(row).getByText("Not sent")).toBeVisible());
    expect(within(row).getByTestId(/^pending-explanation:/)).toHaveTextContent(
      "Sign in again or check this connection's authentication. Then review your saved message before choosing Send again.",
    );
    expect(screen.queryByText("Sending")).toBeNull();
    expect(screen.queryByText(/Waiting for confirmation/)).toBeNull();
    expect(screen.queryByText("Agent busy")).toBeNull();
    expect(screen.getByText("Agent idle")).toBeVisible();
    // The saved message is kept for an explicit retry of the same envelope.
    expect(within(row).getByRole("button", { name: "Send again" })).toBeEnabled();
    expect(within(row).queryByRole("button", { name: "Check" })).toBeNull();
    const saved = savedAttempts();
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({ state: "definitely-rejected", failureKind: "unauthenticated", text: "sent from off the home network" });
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("shows a refused steer as a typed 401 with its frozen steer mode", async () => {
    const send = vi.fn(async () => { throw unauthenticated(); });
    const fake = transport(send);
    fake.queryTimeline = async () => ({ available: true, frames: [
      { id: "busy-frame", event: "interaction_started", identity, interactionId: "already-busy", timestampMs: Date.now(), data: { content: "long tool call" } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await compose("steer from off the home network");
    await waitFor(() => expect(screen.getByTestId("pending-stack")).toBeTruthy());
    fireEvent.click(screen.getByTestId(/^pending-steer:/));
    const row = await screen.findByTestId(/^pending-item:/);
    await waitFor(() => expect(within(row).getByText("Not sent")).toBeVisible());
    const saved = savedAttempts();
    expect(saved[0]).toMatchObject({ state: "definitely-rejected", failureKind: "unauthenticated" });
    expect(JSON.parse(saved[0].envelopeJson).handling_mode).toBe("steer");
  });

  it.each([
    ["message_delivery_failed", "delivery_failed", { reason: "host-human input refused", data: { kind: "host_human_input_unsupported" } }],
    ["interaction_complete", "completed", { reason: "steer_delivered", handling_mode: "steer" }],
  ] as const)("keeps owner A busy after input B emits %s", async (event, status, data) => {
    const send = vi.fn(async input => ({ interaction_id: "queued-turn-C", identity: input.identity }));
    const fake = transport(send);
    const startedAt = Date.now();
    const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session" };
    const history: ConsoleFrame[] = [
      { ...scope, id: "interaction-A-start", event: "interaction_started", interactionId: "interaction-A", timestampMs: startedAt, data: { content: "Active owner work A" } },
      { ...scope, id: "run-A-start", event: "run_started", interactionId: "interaction-A", runId: "run-A", timestampMs: startedAt + 1, data: {} },
    ];
    fake.queryTimeline = async () => ({ available: true, frames: [...history] });
    let receive: ((frame: ConsoleFrame) => void) | undefined;
    fake.subscribeTimeline = (_input, onFrame) => { receive = onFrame; return () => {}; };
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await compose("queued input C waits for owner A");
    const stack = await screen.findByTestId("pending-stack");
    expect(within(stack).getByText("Agent busy")).toBeVisible();
    expect(send).not.toHaveBeenCalled();
    const inputB: ConsoleFrame = { ...scope, id: "input-B", event: "user_input", sourceKind: "send",
      interactionId: "interaction-B", timestampMs: startedAt + 2, status: "queued", data: { content: "Steer B", handling_mode: "steer" } };
    const terminalB: ConsoleFrame = { ...scope, id: "input-B-terminal", event, sourceKind: "synthetic",
      interactionId: "interaction-B", timestampMs: startedAt + 3, status, data };
    history.push(inputB, terminalB);
    await act(async () => { receive?.(inputB); receive?.(terminalB); });
    expect(screen.getByText("Agent busy")).toBeVisible();
    expect(send).not.toHaveBeenCalled();
    const terminalA: ConsoleFrame = { ...scope, id: "owner-A-terminal", event: "interaction_complete",
      interactionId: "interaction-A", runId: "run-A", timestampMs: startedAt + 4, data: { text: "Owner A completed" } };
    history.push(terminalA);
    await act(async () => { receive?.(terminalA); });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].content).toBe("queued input C waits for owner A");
  });

  it("replays owner IDs corrected on the same terminal record before draining", async () => {
    const send = vi.fn(async input => ({ interaction_id: "queued-turn-C", identity: input.identity }));
    const fake = transport(send);
    const startedAt = Date.now();
    const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session" };
    const terminal: ConsoleFrame = { ...scope, id: "corrected-terminal", event: "interaction_complete",
      interactionId: "interaction-A", timestampMs: startedAt + 2, frameVersion: 1, data: { text: "Initial A terminal" } };
    const history: ConsoleFrame[] = [
      { ...scope, id: "interaction-A-start", event: "interaction_started", interactionId: "interaction-A", timestampMs: startedAt, data: {} },
      { ...scope, id: "run-A-start", event: "run_started", interactionId: "interaction-A", runId: "run-A", timestampMs: startedAt + 1, data: {} },
      terminal,
    ];
    fake.queryTimeline = async () => ({ available: true, frames: [...history] });
    let receive: ((frame: ConsoleFrame) => void) | undefined;
    fake.subscribeTimeline = (_input, onFrame) => { receive = onFrame; return () => {}; };
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await screen.findByText("Initial A terminal", { selector: "p" });
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    const corrected = { ...terminal, interactionId: "interaction-B", frameVersion: 2 };
    history[2] = corrected;
    await act(async () => { receive?.({ ...scope, id: "terminal-update", event: "frame_updated", data: { frame: corrected } }); });
    await compose("queued input C waits for corrected owner A");
    expect(screen.getByText("Agent busy")).toBeVisible();
    expect(send).not.toHaveBeenCalled();
    const terminalA: ConsoleFrame = { ...scope, id: "owner-A-terminal", event: "interaction_complete",
      interactionId: "interaction-A", timestampMs: startedAt + 3, data: { text: "Owner A completed" } };
    history.push(terminalA);
    await act(async () => { receive?.(terminalA); });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].content).toBe("queued input C waits for corrected owner A");
  });

  it("keeps tool-only owner work queued through text completion until a terminal turn", async () => {
    const send = vi.fn(async input => ({ interaction_id: "queued-turn-C", identity: input.identity }));
    const fake = transport(send);
    const startedAt = Date.now();
    const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session", interactionId: "interaction-A", runId: "run-A" };
    const history: ConsoleFrame[] = [
      { ...scope, id: "tool-A-result", event: "tool_execution_completed", timestampMs: startedAt, data: {} },
    ];
    fake.queryTimeline = async () => ({ available: true, frames: [...history] });
    let receive: ((frame: ConsoleFrame) => void) | undefined;
    fake.subscribeTimeline = (_input, onFrame) => { receive = onFrame; return () => {}; };
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await compose("queued input C waits for a terminal turn");
    expect(screen.getByText("Agent busy")).toBeVisible();
    const text: ConsoleFrame = { ...scope, id: "text-A-complete", event: "text_complete", timestampMs: startedAt + 1, data: { content: "Tool work text finished" } };
    history.push(text);
    await act(async () => { receive?.(text); });
    expect(screen.getByText("Agent busy")).toBeVisible();
    expect(send).not.toHaveBeenCalled();
    const terminal: ConsoleFrame = { ...scope, id: "turn-A-complete", event: "turn_completed", timestampMs: startedAt + 2, data: { stop_reason: "end_turn" } };
    history.push(terminal);
    await act(async () => { receive?.(terminal); });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
  });

  it("keeps the active owner run busy after a refused steer", async () => {
    const refused = Object.assign(new Error("send access denied"), {
      httpStatus: 403,
      responseRpcError: { code: -32030, message: "send access denied", data: { kind: "access_denied" } },
    });
    const send = vi.fn(async input => ({ interaction_id: "next-owner-turn", identity: input.identity }))
      .mockRejectedValueOnce(refused);
    const fake = transport(send);
    const startedAt = Date.now();
    fake.queryTimeline = async () => ({ available: true, frames: [
      { id: "owner-interaction", event: "interaction_started", identity, interactionId: "active-owner-turn", timestampMs: startedAt, data: { content: "Owner work is still running" } },
      { id: "owner-run", event: "run_started", identity, interactionId: "active-owner-turn", runId: "active-owner-run", timestampMs: startedAt + 1, data: {} },
    ] });
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, onFrame) => { receive = onFrame; return () => {}; };
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await compose("refused steer");
    expect(await screen.findByText("Agent busy")).toBeVisible();
    fireEvent.click(screen.getByTestId(/^pending-steer:/));
    const row = await screen.findByTestId(/^pending-item:/);
    await waitFor(() => expect(within(row).getByText("Not sent")).toBeVisible());
    expect(screen.getByText("Agent busy")).toBeVisible();
    expect(screen.queryByText("Agent idle")).toBeNull();
    fireEvent.click(within(row).getByRole("button", { name: "Discard", exact: true }));
    await compose("wait for the active owner turn");
    expect(await screen.findByTestId("pending-stack")).toHaveTextContent("wait for the active owner turn");
    expect(send).toHaveBeenCalledTimes(1);
    await act(async () => { receive?.({ id: "owner-terminal", event: "interaction_complete", identity,
      interactionId: "active-owner-turn", runId: "active-owner-run", timestampMs: startedAt + 2, data: { text: "Owner work completed" } } as never); });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    expect(send.mock.calls[1][0].content).toBe("wait for the active owner turn");
  });

  it("shows a connection failure (e.g. a lost acknowledgement) as acceptance unknown, never as not sent", async () => {
    const send = vi.fn(async () => { throw Object.assign(new TypeError("Failed to fetch"), { transportFailure: "connection_failed" }); });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await compose("sent while the tunnel was down");
    const row = await screen.findByTestId(/^pending-item:/);
    await waitFor(() => expect(within(row).getByText("Not confirmed")).toBeVisible());
    expect(within(row).getByTestId(/^pending-explanation:/)).toHaveTextContent("Couldn't reach the server (offline or signed out).");
    expect(within(row).getByRole("button", { name: "Check" })).toBeEnabled();
    // Uncertain, and this gateway does not advertise durable send dedupe:
    // no same-key resend is offered.
    expect(within(row).queryByRole("button", { name: "Send again" })).toBeNull();
    expect(savedAttempts()[0]).toMatchObject({ state: "outcome-unknown", failureKind: "connection_failed" });
    expect(screen.getByText("Agent idle")).toBeVisible();
  });

  it("reports a failed acceptance check as a typed state on the row", async () => {
    const send = vi.fn(async () => { throw Object.assign(new TypeError("Failed to fetch"), { transportFailure: "connection_failed" }); });
    const fake = transport(send);
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] }) as never;
    fake.executeCommand = vi.fn(async () => { throw unauthenticated(); });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await compose("check me");
    const row = await screen.findByTestId(/^pending-item:/);
    fireEvent.click(await within(row).findByRole("button", { name: "Check" }));
    expect(await within(row).findByTestId(/^pending-check:/)).toHaveTextContent(
      "Couldn't check: you were signed out. Sign in and check again.",
    );
    expect(send).toHaveBeenCalledTimes(1);
    expect(savedAttempts()[0].state).toBe("outcome-unknown");
  });

  it("names a refused attachment send on the multipart door and keeps the draft and files", async () => {
    vi.stubGlobal("URL", class extends URL {
      static createObjectURL = vi.fn(() => `blob:preview-${Math.random()}`);
      static revokeObjectURL = vi.fn();
    });
    const send = vi.fn(async (_input: Parameters<MetaConsoleTransport["send"]>[0]) => { throw unauthenticated(); });
    const fake = transport(send); const experience = await fake.loadExperience();
    fake.loadExperience = async () => ({ ...experience, agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Queue agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: true } }] } } }) as never;
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
    const file = new File([new Uint8Array([1, 2, 3])], "photo.png", { type: "image/png" });
    fireEvent.drop(document.querySelector(".composer__shell")!, { dataTransfer: { files: [file], items: [], types: ["Files"], getData: () => "" } });
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Remove attachment" })).toHaveLength(1));
    await compose("look at this from off the home network");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].attachments).toEqual([file]);
    expect(await screen.findByTestId("console-action-error")).toHaveTextContent(
      "Sign-in required (401). Nothing was sent.",
    );
    expect(textarea.value).toBe("look at this from off the home network");
    expect(screen.getAllByRole("button", { name: "Remove attachment" })).toHaveLength(1);
    expect(screen.queryByText("Sending")).toBeNull();
  });

  it.each(["refused", "accepted"] as const)("settles a send answered (%s) after the console was remounted instead of leaving it awaiting acceptance", async (outcome) => {
    const scope = "runtime/realm/principal";
    let settleOld!: { resolve: (value: never) => void; reject: (reason: unknown) => void };
    const oldSend = vi.fn(() => new Promise<never>((resolve, reject) => { settleOld = { resolve, reject }; }));
    const view = render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(oldSend)} />);
    await compose("in flight across a transport replacement");
    await waitFor(() => expect(oldSend).toHaveBeenCalledTimes(1));
    expect(savedAttempts(scope)[0].state).toBe("attempting");
    // A new transport remounts the console lifetime (and its controller).
    const newSend = vi.fn(async (input) => ({ interaction_id: "new", identity: input.identity }));
    view.rerender(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(newSend)} />);
    // The remount clears the old lifetime's stack at once; the queue note
    // returns only when the new lifetime has loaded the in-flight row. Wait
    // on that transition, not on the display-only acceptance grace timer
    // (which restarts with every stack mount; PendingStack.test.tsx covers
    // it with fake timers).
    expect(screen.queryByText("Saved in this browser until sent")).toBeNull();
    await screen.findByText("Saved in this browser until sent");
    expect(savedAttempts(scope)[0].state).toBe("attempting");
    expect(screen.queryByText("Not sent")).toBeNull();
    expect(screen.queryByText("Not confirmed")).toBeNull();
    await act(async () => {
      if (outcome === "refused") settleOld.reject(unauthenticated());
      else settleOld.resolve({ interaction_id: "late-receipt", identity, input_frame_id: "late-frame" } as never);
    });
    if (outcome === "refused") {
      await waitFor(() => expect(savedAttempts(scope)[0]).toMatchObject({ state: "definitely-rejected", failureKind: "unauthenticated" }));
      expect(await screen.findByText("Not sent")).toBeVisible();
    } else {
      // The ended lifetime discards the late answer (its transport scope is
      // gone), so the attempt settles as a typed, reconcilable interruption.
      await waitFor(() => expect(savedAttempts(scope)[0]).toMatchObject({ state: "outcome-unknown", failureKind: "interrupted" }));
      expect(await screen.findByText("Not confirmed")).toBeVisible();
      expect(screen.getByText("The page was reloaded while sending.")).toBeVisible();
      expect(screen.getByRole("button", { name: "Check" })).toBeEnabled();
    }
    expect(screen.queryByText("Sending")).toBeNull();
    expect(newSend).not.toHaveBeenCalled();
  });

  it("keeps the storage failure banner when saving a failed dispatched attempt fails", async () => {
    let rejectSend!: (reason: Error) => void;
    const send = vi.fn(() => new Promise<never>((_resolve, reject) => { rejectSend = reject; }));
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await compose("Already dispatched before storage fails");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const key = consoleSendStorageKey("runtime/realm/principal", identity);
    const frozen = JSON.parse(window.localStorage.getItem(key)!).attempts[0];
    const original = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (storageKey, value) {
      if (storageKey === key && JSON.parse(value).attempts.some((attempt: PendingItem) => attempt.state === "outcome-unknown")) {
        throw new Error("post-dispatch quota exhausted");
      }
      original.call(this, storageKey, value);
    });
    await act(async () => { rejectSend(new Error("response lost after dispatch")); });
    await waitFor(() => expect(screen.getByTestId("console-action-error")).toHaveTextContent("post-dispatch quota exhausted"));
    expect(screen.getByTestId("console-action-error")).not.toHaveTextContent("not queued or dispatched");
    const retained = JSON.parse(window.localStorage.getItem(key)!).attempts[0];
    expect(retained.envelopeJson).toBe(frozen.envelopeJson);
    expect(retained.idempotencyKey).toBe(frozen.idempotencyKey);
    // The stack still holds the row (its queue note is derived from the
    // stack itself); the row's own card may still be inside the display-only
    // acceptance grace window, so do not wait on that timer.
    expect(screen.getByText("Saved in this browser until sent")).toBeInTheDocument();
    expect(send).toHaveBeenCalledTimes(1);
  });
  it("clears a recovered queue storage failure before showing an unknown send outcome", async () => {
    const send = vi.fn(async () => { throw new Error("response lost after dispatch"); });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    const key = consoleSendStorageKey("runtime/realm/principal", identity);
    const original = Storage.prototype.setItem;
    let storageBlocked = true;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (storageKey, value) {
      if (storageKey === key && storageBlocked) throw new Error("temporary queue quota failure");
      original.call(this, storageKey, value);
    });
    const textarea = await compose("Keep the instruction through storage recovery");
    await waitFor(() => expect(screen.getByTestId("console-action-error")).toHaveTextContent("temporary queue quota failure"));
    expect(send).not.toHaveBeenCalled();
    expect(textarea.value).toBe("Keep the instruction through storage recovery");
    storageBlocked = false;
    await compose(textarea.value);
    await screen.findByText(/We couldn't confirm/);
    expect(screen.queryByTestId("console-action-error")).toBeNull();
    const attempts = JSON.parse(window.localStorage.getItem(key)!).attempts;
    expect(attempts).toHaveLength(1);
    expect(attempts[0].state).toBe("outcome-unknown");
    expect(attempts[0].text).toBe("Keep the instruction through storage recovery");
    expect(send).toHaveBeenCalledTimes(1);
  });
  it("keeps composer visible when persistence fails before queue or dispatch", async () => {
    const send = vi.fn(async () => ({ interaction_id: "wrong", identity }));
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    const original = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (key, value) {
      if (key.startsWith("meta-send-attempts:")) throw new Error("quota exhausted");
      original.call(this, key, value);
    });
    const textarea = await compose("preserve in composer");
    await waitFor(() => expect(screen.getAllByText(/quota exhausted/).length).toBeGreaterThan(0));
    await waitFor(() => expect(textarea.value).toBe("preserve in composer"));
    expect(send).not.toHaveBeenCalled();
    expect(screen.queryByTestId("pending-stack")).toBeNull();
  });
  it("clears visible drafts on authenticated scope change", async () => {
    const send = vi.fn(async () => ({ interaction_id: "i", identity })); const fake = transport(send);
    const view = render(<ConsoleApp baseUrl="" storageNamespace="principal-one" transport={fake} />);
    const textarea = await screen.findByTestId(`chat-composer:${identity}`);
    fireEvent.change(textarea, { target: { value: "private draft" } });
    view.rerender(<ConsoleApp baseUrl="" storageNamespace="principal-two" transport={fake} />);
    await waitFor(() => expect((screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe(""));
    expect(send).not.toHaveBeenCalled();
  });
  it("freezes selected context separately from instruction and does not send on selection", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "with-context", identity: input.identity, input_frame_id: "submitted" }));
    const fake = transport(send);
    fake.queryTimeline = async () => ({ available: true, frames: [
      { id: "source-frame", event: "interaction_complete", identity, interactionId: "source-interaction", timestampMs: Date.now(), data: { text: "This is exact selected context." } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    const source = await waitFor(() => {
      const node = document.querySelector("[data-quote-message-id]");
      if (!node) throw new Error("Transcript quote source not yet rendered");
      return node;
    });
    const range = document.createRange(); range.selectNodeContents(source);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range); fireEvent(document, new Event("selectionchange"));
    fireEvent.click(screen.getByRole("button", { name: "Add to message" }));
    expect(send).not.toHaveBeenCalled();
    expect(screen.getByRole("region", { name: "Quoted context for Queue agent" })).toBeTruthy();
    await compose("Explain the quote");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const content = send.mock.calls[0][0].content;
    expect(Array.isArray(content)).toBe(true);
    expect(content[0]).toEqual({ type: "text", text: "Explain the quote" });
    expect(content[1].text).toContain("This is exact selected context.");
    const context = JSON.parse(content[1].text.split("\n")[2]);
    expect(context.messageId).toBe(source.getAttribute("data-quote-message-id"));
    expect(context.sourceRange).toBeUndefined();
  });
  it("keeps a queued steer attempt visible on failure with its original frozen mode", async () => {
    const send = vi.fn(async () => { throw new Error("steer response lost"); });
    const fake = transport(send);
    fake.queryTimeline = async () => ({ available: true, frames: [
      { id: "busy-frame", event: "interaction_started", identity, interactionId: "already-busy", timestampMs: Date.now(), data: { content: "existing work" } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await compose("interrupt with this");
    await waitFor(() => expect(screen.getByTestId("pending-stack")).toBeTruthy());
    expect(send).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId(/^pending-steer:/));
    await waitFor(() => expect(screen.getByText(/We couldn't confirm/)).toBeTruthy());
    expect(send).toHaveBeenCalledTimes(1);
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey("runtime/realm/principal", identity))!);
    expect(JSON.parse(saved.attempts[0].envelopeJson).handling_mode).toBe("steer");
    expect(saved.attempts[0].state).toBe("outcome-unknown");
  });

  it("resumes a persisted unattempted draft after loading idle history", async () => {
    const attempt = createConsoleSendAttempt({ id: "queued-before-reload", scope: "runtime/realm/principal", destination: identity, origin: "console:old-pane", idempotencyKey: "original-key", text: "resume this draft", now: 1 });
    saveConsoleSendAttempts(window.localStorage, attempt.scope, identity, [attempt]);
    const send = vi.fn(async (input) => ({ interaction_id: "resumed", identity: input.identity }));
    render(<ConsoleApp baseUrl="" storageNamespace={attempt.scope} transport={transport(send)} />);
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0]).toMatchObject({ origin: "console:old-pane", idempotencyKey: "original-key", content: "resume this draft" });
  });
  it("never replays a stale attempted record after its browser lease expires", async () => {
    const attempt = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "in-flight", scope: "runtime/realm/principal", destination: identity, origin: "console:old-pane", idempotencyKey: "attempted-key", text: "may be accepted", now: 1 }), { owner: "closed-tab", now: 2, handlingMode: "queue" });
    saveConsoleSendAttempts(window.localStorage, attempt.scope, identity, [attempt]);
    const send = vi.fn(async (input) => ({ interaction_id: "duplicate", identity: input.identity }));
    render(<ConsoleApp baseUrl="" storageNamespace={attempt.scope} transport={transport(send)} />);
    await waitFor(() => expect(screen.getByText(/We couldn't confirm/)).toBeTruthy());
    expect(send).not.toHaveBeenCalled();
  });

  it("resumes an unattempted draft once storage recovers on window focus", async () => {
    const scope = "runtime/realm/principal";
    const attempt = createConsoleSendAttempt({ id: "waiting-storage", scope, destination: identity, origin: "console:old", idempotencyKey: "recover-key", text: "after recovery", now: 1 });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [attempt]);
    let quota = true;
    const original = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (key, value) {
      if (quota && key.startsWith("meta-send-attempts:")) throw new Error("frozen-write quota");
      original.call(this, key, value);
    });
    const send = vi.fn(async (input) => ({ interaction_id: "recovered", identity: input.identity }));
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(send)} />);
    await waitFor(() => expect(screen.getAllByText(/frozen-write quota/).length).toBeGreaterThan(0));
    expect(send).not.toHaveBeenCalled();
    quota = false; fireEvent(window, new Event("focus"));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].idempotencyKey).toBe("recover-key");
    await waitFor(() => expect(screen.queryByTestId("pending-stack")).toBeNull());
  });

  it("reopens a queued target after it becomes idle while its pane is closed", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "reopened", identity: input.identity }));
    const fake = transport(send); let receive: ((frame: never) => void) | undefined;
    fake.queryTimeline = async () => ({ available: true, frames: [{ id: "busy", event: "interaction_started", identity, interactionId: "busy", timestampMs: 1, data: { content: "working" } }] });
    fake.subscribeTimeline = (_input, onFrame) => { receive = onFrame; return () => {}; };
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await compose("queued while busy");
    await screen.findByTestId("pending-stack");
    fireEvent.click(screen.getByTestId("pane-close:panel-1"));
    await act(async () => { receive?.({ id: "idle", event: "interaction_complete", identity, interactionId: "busy", timestampMs: 2, data: { text: "finished" } } as never); });
    expect(send).not.toHaveBeenCalled();
    const row = await waitFor(() => {
      const node = document.querySelector('.agent[role="button"], .cc-sidebar-row');
      if (!node) throw new Error("Missing agent sidebar row"); return node;
    });
    fireEvent.click(row);
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
  });

  it("isolates two pane drafts and quotes through submission and reload", async () => {
    seed(true);
    const send = vi.fn(async (input) => ({ interaction_id: "only-a", identity: input.identity }));
    const fake = transport(send);
    fake.queryTimeline = async () => ({ available: true, frames: [{ id: "quote", event: "interaction_complete", identity, interactionId: "q", timestampMs: 1, data: { text: "Quoted source" } }] });
    const props = { baseUrl: "", storageNamespace: "runtime/realm/principal", transport: fake };
    const view = render(<ConsoleApp {...props} />);
    await waitFor(() => expect(screen.getAllByTestId(`chat-composer:${identity}`)).toHaveLength(2));
    for (const panelId of ["panel-1", "panel-2"]) {
      const pane = screen.getByTestId(`pane:${panelId}`);
      const source = await waitFor(() => { const node = pane.querySelector("[data-quote-message-id]"); if (!node) throw new Error("No quote source"); return node; });
      const range = document.createRange(); range.selectNodeContents(source);
      window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range); fireEvent(document, new Event("selectionchange"));
      fireEvent.click([...pane.querySelectorAll("button")].find((button) => button.getAttribute("aria-label") === "Add to message")!);
      fireEvent.change(pane.querySelector("textarea")!, { target: { value: `Unsent ${panelId}` } });
    }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 160)); });
    fireEvent.keyDown(screen.getByTestId("pane:panel-1").querySelector("textarea")!, { key: "Enter" });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    view.unmount(); render(<ConsoleApp {...props} />);
    await waitFor(() => expect((screen.getByTestId("pane:panel-2").querySelector("textarea") as HTMLTextAreaElement).value).toBe("Unsent panel-2"));
    expect(screen.getByTestId("pane:panel-2").querySelector(".cc-context-chip blockquote")?.textContent).toBe("Quoted source");
  });

  it("isolates independently opened tab drafts while preserving each reload identity", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "tab-a-send", identity: input.identity }));
    const props = { baseUrl: "", storageNamespace: "runtime/realm/principal", transport: transport(send) };
    window.sessionStorage.setItem("meta-composer-tab:v1", "browser-a");
    let view = render(<ConsoleApp {...props} />);
    fireEvent.change(await screen.findByTestId(`chat-composer:${identity}`), { target: { value: "Draft browser A" } });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 160)); }); view.unmount();
    window.sessionStorage.setItem("meta-composer-tab:v1", "browser-b");
    view = render(<ConsoleApp {...props} />);
    expect((await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe("");
    fireEvent.change(screen.getByTestId(`chat-composer:${identity}`), { target: { value: "Draft browser B" } });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 160)); }); view.unmount();
    window.sessionStorage.setItem("meta-composer-tab:v1", "browser-a");
    view = render(<ConsoleApp {...props} />);
    await waitFor(() => expect((screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe("Draft browser A"));
    fireEvent.keyDown(screen.getByTestId(`chat-composer:${identity}`), { key: "Enter" });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1)); view.unmount();
    window.sessionStorage.setItem("meta-composer-tab:v1", "browser-b"); render(<ConsoleApp {...props} />);
    await waitFor(() => expect((screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe("Draft browser B"));
    expect([...Array(window.sessionStorage.length)].map((_, index) => window.sessionStorage.key(index)).filter(key => key?.startsWith("meta-composer-draft:v2:"))).toHaveLength(2);
  });

  it("isolates transient intent when a replacement transport uses the same URL", async () => {
    const oldSend = vi.fn(async (input) => ({ interaction_id: "old", identity: input.identity }));
    const oldTransport = transport(oldSend);
    oldTransport.queryTimeline = async () => ({ available: true, frames: [{ id: "busy", event: "interaction_started", identity, interactionId: "busy", timestampMs: 1, data: { content: "working" } }] });
    const view = render(<ConsoleApp baseUrl="" transport={oldTransport} />);
    await compose("belongs to old controller"); await screen.findByTestId("pending-stack");
    fireEvent.change(screen.getByTestId(`chat-composer:${identity}`), { target: { value: "private unsent draft" } });
    const newSend = vi.fn(async (input) => ({ interaction_id: "new", identity: input.identity }));
    view.rerender(<ConsoleApp baseUrl="" transport={transport(newSend)} />);
    await waitFor(() => expect(screen.queryByTestId("pending-stack")).toBeNull());
    expect((await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe("");
    expect(oldSend).not.toHaveBeenCalled(); expect(newSend).not.toHaveBeenCalled();
  });

  it("does not dispatch a queued lock callback through a replacement controller", async () => {
    const scope = "runtime/realm/principal";
    const attempt = createConsoleSendAttempt({ id: "lock-wait", scope, destination: identity, origin: "console:old", idempotencyKey: "locked", text: "old intent", now: 1 });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [attempt]);
    const callbacks: Array<() => void> = [];
    Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => new Promise(resolve => callbacks.push(() => resolve(callback()))) } });
    const oldSend = vi.fn(async (input) => ({ interaction_id: "old", identity: input.identity }));
    const view = render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(oldSend)} />);
    await waitFor(() => expect(callbacks.length).toBeGreaterThan(0));
    const newSend = vi.fn(async (input) => ({ interaction_id: "new", identity: input.identity }));
    view.rerender(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(newSend)} />);
    await act(async () => { callbacks.shift()!(); });
    expect(oldSend).not.toHaveBeenCalled(); expect(newSend).not.toHaveBeenCalled();
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts[0];
    expect(saved.state).toBe("draft");
  });

  it("does not turn a failed capability lookup into a definite refusal", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "unexpected", identity: input.identity }));
    const fake = transport(send); fake.capabilities = async () => { throw new Error("capability connection lost"); };
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await compose("uncertain capability request");
    await screen.findByText(/We couldn't confirm/);
    expect(screen.queryByRole("button", { name: "Send again" })).toBeNull();
    expect(send).not.toHaveBeenCalled();
  });

  it("proves capability refusal before dispatch and retries the same frozen envelope explicitly", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "restored-capability", identity: input.identity }));
    const fake = transport(send); let canSend = false;
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/timeline", ...(canSend ? ["meta/console/send"] : [])] }) as never;
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await compose("retry after capability restoration");
    await screen.findByRole("button", { name: "Send again" });
    expect(send).not.toHaveBeenCalled();
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey("runtime/realm/principal", identity))!).attempts[0];
    expect(saved.state).toBe("definitely-rejected");
    canSend = true; fireEvent.click(screen.getByRole("button", { name: "Send again" }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0]).toMatchObject({ content: saved.text, idempotencyKey: saved.idempotencyKey, origin: saved.origin, handlingMode: "queue" });
  });

  it("preserves a multi-delta message reference without inventing a frame-relative source range", async () => {
    const send = vi.fn(async (input) => ({ interaction_id: "q", identity: input.identity }));
    const fake = transport(send);
    fake.queryTimeline = async () => ({ available: true, frames: [
      { id: "d", event: "interaction_complete", identity, interactionId: "stream", timestampMs: 1, data: { text: "Unrelated" } },
      { id: "d:1", event: "text_delta", identity, interactionId: "source", timestampMs: 2, data: { delta: "Hello " } },
      { id: "d:2", event: "text_delta", identity, interactionId: "source", timestampMs: 3, data: { delta: "world" } },
      { id: "d:3", event: "interaction_complete", identity, interactionId: "source", timestampMs: 4, data: { text: "Hello world" } },
    ] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    const source = await waitFor(() => { const node = [...document.querySelectorAll("[data-quote-message-id]")].find(item => item.textContent?.includes("Hello world")); if (!node) throw new Error("No assembled source"); return node; });
    const node = [...source.querySelectorAll("p")].find(item => item.textContent === "Hello world")!.firstChild!;
    const range = document.createRange(); range.setStart(node, 6); range.setEnd(node, 11);
    window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range); fireEvent(document, new Event("selectionchange"));
    fireEvent.click(screen.getByRole("button", { name: "Add to message" }));
    await compose("Explain this fragment");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const record = JSON.parse(send.mock.calls[0][0].content[1].text.split("\n")[2]);
    expect(record.quote).toBe("world");
    expect(record.messageId).toBe(source.getAttribute("data-quote-message-id"));
    expect(record.messageId).toBe("d:1");
    expect(record.sourceRange).toBeUndefined();
  });

  it.each([
    ["published different text", "Newer unsent intent", 450],
    ["published identical text", "First submitted intent", 450],
    ["unpublished different text", "Newer unsent intent", 0],
    ["unpublished identical text", "First submitted intent", 0],
  ] as const)("preserves %s while the previous enqueue waits for a Web Lock", async (_case, nextText, delay) => {
    const callbacks: Array<() => void> = [];
    Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => new Promise(resolve => callbacks.push(() => resolve(callback()))) } });
    const send = vi.fn(async input => ({ interaction_id: "accepted", identity: input.identity }));
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={transport(send)} />);
    const textarea = await compose("First submitted intent");
    await waitFor(() => expect(callbacks.length).toBeGreaterThan(0));
    expect(textarea.value).toBe("");
    fireEvent.change(textarea, { target: { value: nextText } });
    if (delay) await act(async () => { await new Promise(resolve => setTimeout(resolve, delay)); });
    await act(async () => { callbacks.shift()!(); await Promise.resolve(); });
    await waitFor(() => expect(textarea.value).toBe(nextText));
    const documents = [...Array(window.sessionStorage.length)].map((_, index) => window.sessionStorage.key(index)!).filter(key => key.startsWith("meta-composer-draft:v2:")).map(key => JSON.parse(window.sessionStorage.getItem(key)!));
    expect(documents.some(draft => draft.text === nextText)).toBe(true);
  });

  it("removes only submitted quotes when a newer quote is added during the enqueue lock wait", async () => {
    const callbacks: Array<() => void> = [];
    Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => new Promise(resolve => callbacks.push(() => resolve(callback()))) } });
    const fake = transport(vi.fn(async input => ({ interaction_id: "accepted", identity: input.identity })));
    fake.queryTimeline = async () => ({ available: true, frames: [{ id: "quote-race", event: "interaction_complete", identity, interactionId: "source", timestampMs: 1, data: { text: "Original quote. Newer quote." } }] });
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    const paragraph = await waitFor(() => { const element = [...document.querySelectorAll("[data-quote-message-id] p")].find(item => item.textContent === "Original quote. Newer quote."); if (!element) throw new Error("No source"); return element; });
    const select = (text: string) => {
      const node = paragraph.firstChild!; const start = node.textContent!.indexOf(text);
      const range = document.createRange(); range.setStart(node, start); range.setEnd(node, start + text.length);
      window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range); fireEvent(document, new Event("selectionchange"));
      fireEvent.click(screen.getByRole("button", { name: "Add to message" }));
    };
    select("Original quote.");
    const textarea = await compose("First instruction");
    await waitFor(() => expect(callbacks.length).toBeGreaterThan(0));
    select("Newer quote.");
    fireEvent.change(textarea, { target: { value: "Newer instruction" } });
    await act(async () => { callbacks.shift()!(); await Promise.resolve(); });
    await waitFor(() => expect([...document.querySelectorAll(".cc-context-chip blockquote")].map(node => node.textContent)).toEqual(["Newer quote."]));
    expect(textarea.value).toBe("Newer instruction");
    const documents = [...Array(window.sessionStorage.length)].map((_, index) => window.sessionStorage.key(index)!).filter(key => key.startsWith("meta-composer-draft:v2:")).map(key => JSON.parse(window.sessionStorage.getItem(key)!));
    expect(documents).toHaveLength(1);
    expect(documents[0].text).toBe("Newer instruction");
    expect(documents[0].contexts.map((context: { quote: string }) => context.quote)).toEqual(["Newer quote."]);
    const queued = JSON.parse(window.localStorage.getItem(consoleSendStorageKey("runtime/realm/principal", identity))!).attempts[0];
    expect(queued.contexts.map((context: { quote: string }) => context.quote)).toEqual(["Original quote."]);
  });

  it.each(["Newer attachment instruction", "Original attachment instruction"])("preserves new attachments and draft %s while an earlier attachment send awaits acceptance", async (nextText) => {
    vi.stubGlobal("URL", class extends URL {
      static createObjectURL = vi.fn(() => `blob:preview-${Math.random()}`);
      static revokeObjectURL = vi.fn();
    });
    let finish!: (value: never) => void;
    const send = vi.fn((_input: Parameters<MetaConsoleTransport["send"]>[0]) => new Promise<never>(resolve => { finish = resolve; }));
    const fake = transport(send); const experience = await fake.loadExperience();
    fake.loadExperience = async () => ({ ...experience, agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Queue agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: true } }] } } }) as never;
    render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
    const first = new File([new Uint8Array([1, 2, 3])], "first.png", { type: "image/png" });
    const second = new File([new Uint8Array([4, 5, 6])], "second.png", { type: "image/png" });
    const drop = (file: File) => fireEvent.drop(document.querySelector(".composer__shell")!, { dataTransfer: { files: [file], items: [], types: ["Files"], getData: () => "" } });
    drop(first); await waitFor(() => expect(screen.getAllByRole("button", { name: "Remove attachment" })).toHaveLength(1));
    await compose("Original attachment instruction");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].attachments).toEqual([first]);
    fireEvent.change(textarea, { target: { value: "" } });
    fireEvent.change(textarea, { target: { value: nextText } });
    drop(second); await waitFor(() => expect(screen.getAllByRole("button", { name: "Remove attachment" })).toHaveLength(2));
    await act(async () => { finish({ interaction_id: "accepted-image", identity, input_frame_id: "image-frame" } as never); });
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Remove attachment" })).toHaveLength(1));
    expect(textarea.value).toBe(nextText);
    expect(send).toHaveBeenCalledTimes(1);
    const documents = [...Array(window.sessionStorage.length)].map((_, index) => window.sessionStorage.key(index)!).filter(key => key.startsWith("meta-composer-draft:v2:")).map(key => JSON.parse(window.sessionStorage.getItem(key)!));
    expect(documents.some(draft => draft.text === nextText)).toBe(true);
  });

});

describe("pending recovery actions", () => {
  function renderAttempt(state: PendingItem["state"]) {
    const draft = createConsoleSendAttempt({ id: "recovery-actions", scope: "recovery-actions-scope", destination: identity,
      origin: "console:test", idempotencyKey: "recovery-actions-key", text: "  Keep A\u030A and 🚀 exact.\nSecond line.  ", now: Date.now() });
    const attempted = beginConsoleSendAttempt(draft, { owner: "browser", now: Date.now(), handlingMode: "queue" });
    const item: PendingItem = state === "draft" ? draft : { ...attempted, state,
      ...(state !== "attempting" ? { lease: undefined } : {}), error: state === "outcome-unknown" ? "Response lost" : undefined };
    const props = { items: [item], agentBusy: true, onSteer: vi.fn(), onRetry: vi.fn(), onReconcile: vi.fn(),
      onRemoveContext: vi.fn(), onReorderContext: vi.fn(), onTrash: vi.fn(), onEdit: vi.fn(), onCommitEdit: vi.fn(),
      onCancelEdit: vi.fn(), onReorder: vi.fn(), onClearAll: vi.fn(), onToggleExpand: vi.fn() };
    // A frozen attempting row is past its acceptance grace period here.
    vi.useFakeTimers();
    render(<PendingStack {...props} />);
    act(() => { vi.advanceTimersByTime(ACCEPTANCE_NOTICE_GRACE_MS); });
    vi.useRealTimers();
    return { item, props };
  }

  it.each(["attempting", "outcome-unknown"] as const)("offers read-only recovery for a frozen %s attempt", state => {
    const { item, props } = renderAttempt(state);
    const row = screen.getByTestId(`pending-item:${item.id}`);
    expect(row).toHaveAttribute("draggable", "false");
    expect(row.querySelector(".stk-item__text")?.textContent).toBe(item.text);
    expect(screen.queryByTestId(`pending-steer:${item.id}`)).toBeNull();
    expect(screen.queryByTestId(`pending-edit:${item.id}`)).toBeNull();
    // No same-key resend without a durable dedupe store (resendUncertain
    // defaults to false), and never for a row still in flight.
    expect(screen.queryByRole("button", { name: "Send again" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Check", exact: true }));
    expect(props.onReconcile).toHaveBeenCalledExactlyOnceWith(item.id);
    expect(props.onSteer).not.toHaveBeenCalled();
    expect(props.onRetry).not.toHaveBeenCalled();
    expect(props.onEdit).not.toHaveBeenCalled();
    const expand = screen.getByRole("button", { name: "Show saved message", exact: true });
    expect(expand).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(expand);
    expect(props.onToggleExpand).toHaveBeenCalledExactlyOnceWith(item.id);
    fireEvent.click(screen.getByRole("button", { name: "Discard", exact: true }));
    expect(props.onTrash).toHaveBeenCalledExactlyOnceWith(item.id);
  });

  it("keeps draft editing and steering available", () => {
    const { item, props } = renderAttempt("draft");
    fireEvent.click(screen.getByRole("button", { name: "Steer - send now and interrupt at next cooperative pause" }));
    fireEvent.click(screen.getByRole("button", { name: "Edit message", exact: true }));
    fireEvent.click(screen.getByRole("button", { name: "Remove from queue", exact: true }));
    expect(props.onSteer).toHaveBeenCalledExactlyOnceWith(item.id);
    expect(props.onEdit).toHaveBeenCalledExactlyOnceWith(item.id);
    expect(props.onTrash).toHaveBeenCalledExactlyOnceWith(item.id);
    expect(screen.queryByRole("button", { name: "Check" })).toBeNull();
  });

  it("offers only the original retry mode after definite rejection", () => {
    const { item, props } = renderAttempt("definitely-rejected");
    fireEvent.click(screen.getByRole("button", { name: "Send again", exact: true }));
    expect(props.onRetry).toHaveBeenCalledExactlyOnceWith(item.id);
    expect(screen.queryByTestId(`pending-steer:${item.id}`)).toBeNull();
    expect(screen.queryByTestId(`pending-edit:${item.id}`)).toBeNull();
    expect(screen.queryByRole("button", { name: "Check" })).toBeNull();
    expect(screen.getByRole("button", { name: "Discard" })).toBeEnabled();
  });
});

describe("durable queued quote editing", () => {
  function prepared() {
    const scope = "quote-edit-scope";
    const context = createConsoleContextRecord({ id: "quote-edit-one", sourceScope: scope, sourceIdentity: identity, messageId: "source-message", label: "Review source", quote: "original", sourceText: "before original after" });
    const other = { ...context, id: "quote-edit-two", quote: "another", sourceRange: undefined };
    const attempt = createConsoleSendAttempt({ id: "quote-edit-draft", scope, destination: identity, origin: "console:test", idempotencyKey: "quote-edit-key", text: "Compare quoted evidence", contexts: [context, other], now: 1 });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [attempt]);
    const send = vi.fn(async () => { throw new Error("response lost after actual attempt"); });
    const fake = transport(send);
    fake.queryTimeline = async () => ({ available: true, frames: [{ id: "busy", event: "interaction_started", identity, interactionId: "busy", timestampMs: 1, data: { content: "existing work" } }] });
    return { scope, attempt, send, fake, read: () => JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity))!).attempts[0] };
  }
  it("persists an exact quote edit under the queue lock, then freezes those bytes for the attempted send", async () => {
    const fixture = prepared();
    render(<ConsoleApp baseUrl="" storageNamespace={fixture.scope} transport={fixture.fake} />);
    await screen.findByTestId("pending-item:quote-edit-draft");
    fireEvent.click(screen.getByText("Compare quoted evidence"));
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit quote from Review source" }))[0]);
    const exact = "  Edited A\u030A 🚀\nsecond line  ";
    const editor = screen.getByRole("textbox", { name: "Quote from Review source" });
    fireEvent.change(editor, { target: { value: exact } });
    fireEvent.keyDown(editor, { key: "Backspace", ctrlKey: true });
    expect(fixture.read().contexts[0].quote).toBe("original");
    fireEvent.click(screen.getByRole("button", { name: "Save quote" }));
    await waitFor(() => expect(fixture.read().contexts[0].quote).toBe(exact));
    expect(fixture.read().contexts.map((context: { id: string }) => context.id)).toEqual(["quote-edit-one", "quote-edit-two"]);
    expect(fixture.read().contexts[0].sourceRange).toBeUndefined();
    expect(fixture.read().contexts[1]).toEqual(fixture.attempt.contexts[1]);
    expect(fixture.send).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("pending-steer:quote-edit-draft"));
    await waitFor(() => expect(fixture.read().state).toBe("outcome-unknown"));
    const frozen = fixture.read().envelopeJson;
    expect(JSON.parse(JSON.parse(frozen).content[1].text.split("\n")[2]).quote).toBe(exact);
    expect(fixture.send).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: "Edit quote from Review source" })).toBeNull();
    expect(fixture.read().envelopeJson).toBe(frozen);
  });
  it("rejects saving a quote if another tab has already attempted that queued message", async () => {
    const fixture = prepared();
    render(<ConsoleApp baseUrl="" storageNamespace={fixture.scope} transport={fixture.fake} />);
    await screen.findByTestId("pending-item:quote-edit-draft");
    fireEvent.click(screen.getByText("Compare quoted evidence"));
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit quote from Review source" }))[0]);
    fireEvent.change(screen.getByRole("textbox", { name: "Quote from Review source" }), { target: { value: "stale local edit" } });
    const attempted = beginConsoleSendAttempt(fixture.attempt, { owner: "other-tab", now: Date.now(), handlingMode: "queue" });
    saveConsoleSendAttempts(window.localStorage, fixture.scope, identity, [attempted], [fixture.attempt]);
    fireEvent.click(screen.getByRole("button", { name: "Save quote" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no longer an editable queued draft");
    expect(fixture.read().envelopeJson).toBe(attempted.envelopeJson);
    expect(fixture.read().contexts[0].quote).toBe("original");
    expect(fixture.send).not.toHaveBeenCalled();
  });
});

describe("dock layout hydration", () => {
  // The saved layout, or with none the configured initial target, used to be
  // applied in a passive effect, after the commit that first drew the
  // experience-gated nav. A click in that gap was overwritten when it landed.
  // Click the nav entry the moment it appears (a MutationObserver callback
  // runs before any later task) and require the click to win.
  it("keeps a nav click made as soon as the experience renders", async () => {
    // The restore race needs a saved layout; the file's beforeEach seeds it.
    expect(window.localStorage.getItem("meta-console-dock-state:runtime/realm/principal")).not.toBeNull();
    const fake = transport(vi.fn());
    const initial = await fake.loadExperience();
    fake.loadExperience = async () => ({ ...initial, access: { available: true, enabled: true, can_administer: true, subject: "first-admin" } }) as never;
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/timeline", "meta/access/status", "meta/access/get"] }) as never;
    fake.executeCommand = vi.fn(async input => {
      if (input.command === "accessStatus") return { command: input.command, accepted: true, result: { available: true, enabled: true, can_administer: true, subject: "first-admin", revision: 1, actions: [] } } as never;
      if (input.command === "getAccessConfig") return { command: input.command, accepted: true, result: { config: { enabled: true, admins: ["first-admin"], rules: [], groups: {} }, revision: 1 } } as never;
      throw new Error(`unexpected ${input.command}`);
    });
    let clicked = false;
    const observer = new MutationObserver(() => {
      const nav = document.querySelector("[data-testid='nav:access']");
      if (nav && !clicked) {
        clicked = true;
        fireEvent.click(nav);
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    try {
      render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
      await waitFor(() => expect(clicked).toBe(true));
    } finally {
      observer.disconnect();
    }
    await screen.findByTestId("access-panel");
    await screen.findByText("first-admin", { exact: true });
  });

  it("keeps a nav click made as soon as the experience renders when no layout is saved", async () => {
    window.localStorage.clear();
    const fake = transport(vi.fn());
    const initial = await fake.loadExperience();
    fake.loadExperience = async () => ({ ...initial, console_config: { layout: { initial_control: "logs" } }, access: { available: true, enabled: true, can_administer: true, subject: "first-admin" } }) as never;
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/timeline", "meta/access/status", "meta/access/get"] }) as never;
    fake.executeCommand = vi.fn(async input => {
      if (input.command === "accessStatus") return { command: input.command, accepted: true, result: { available: true, enabled: true, can_administer: true, subject: "first-admin", revision: 1, actions: [] } } as never;
      if (input.command === "getAccessConfig") return { command: input.command, accepted: true, result: { config: { enabled: true, admins: ["first-admin"], rules: [], groups: {} }, revision: 1 } } as never;
      throw new Error(`unexpected ${input.command}`);
    });
    let clicked = false;
    const observer = new MutationObserver(() => {
      const nav = document.querySelector("[data-testid='nav:access']");
      if (nav && !clicked) {
        clicked = true;
        fireEvent.click(nav);
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    try {
      render(<ConsoleApp baseUrl="" transport={fake} />);
      await waitFor(() => expect(clicked).toBe(true));
    } finally {
      observer.disconnect();
    }
    await screen.findByTestId("access-panel");
    await screen.findByText("first-admin", { exact: true });
    expect(screen.queryByTestId("logs-panel")).toBeNull();
  });
});

describe("access async scope isolation", () => {
  it.each(["preview", "mutation"])("does not let a delayed old-scope %s overwrite refreshed owner data", async (operation) => {
    const fake = transport(vi.fn());
    const initial = await fake.loadExperience();
    let subject = "first-admin";
    let receive: ((frame: never) => void) | undefined;
    let finish!: () => void;
    const started = vi.fn();
    fake.loadExperience = async () => ({ ...initial, access: { available: true, enabled: true, can_administer: true, subject } }) as never;
    fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/timeline", "meta/access/status", "meta/access/get", "meta/access/preview", "meta/access/enable"] }) as never;
    fake.executeCommand = vi.fn(async input => {
      let result: unknown;
      if (input.command === "accessStatus") result = { available: true, enabled: true, can_administer: true, subject, revision: 1, owner_instance: "scope-owner", conditional_mutations: "checked_v1", actions: ["agent.view"] };
      else if (input.command === "getAccessConfig") result = { config: { enabled: true, admins: [subject], rules: [], groups: {} }, revision: 1, owner_instance: "scope-owner", conditional_mutations: "checked_v1" };
      else if (input.command === "previewAccess" || input.command === "enableAccess") {
        started();
        return new Promise((resolve, reject) => { finish = () => operation === "preview" ? reject(new Error("OLD_SCOPE_PRIVATE_ERROR")) : resolve({ command: input.command, accepted: true, result: {} } as never); });
      } else throw new Error(`unexpected ${input.command}`);
      return { command: input.command, accepted: true, result } as never;
    });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    fireEvent.click(await screen.findByText("Access", { exact: true }));
    await screen.findByText("first-admin", { exact: true });
    if (operation === "preview") {
      fireEvent.click(screen.getByTestId("access-tab:preview"));
      fireEvent.change(screen.getByTestId("access-preview-subject"), { target: { value: "reader" } });
      fireEvent.click(screen.getByTestId("access-preview-run"));
    } else fireEvent.click(screen.getByTestId("access-toggle-enabled"));
    await waitFor(() => expect(started).toHaveBeenCalledOnce());
    subject = "second-admin";
    await act(async () => { receive?.({ id: "scope-update", event: "interaction_started", identity, interactionId: "scope-update", timestampMs: 2, data: {} } as never); });
    await screen.findByText("second-admin", { exact: true });
    await act(async () => { finish(); });
    expect(screen.getByText("second-admin", { exact: true })).toBeVisible();
    expect(screen.queryByTestId("access-error")).toBeNull();
    expect(document.body).not.toHaveTextContent("OLD_SCOPE_PRIVATE_ERROR");
    expect(screen.getByTestId("access-toggle-enabled")).toBeEnabled();
  });
});

describe("access revalidation follows the owner's access revision", () => {
  // The Console reads Access again when the owner commits an access change
  // (the timeline stream's access_changed signal), not on activity events
  // that cannot change it.
  async function openAccess() {
    const fake = transport(vi.fn());
    const initial = await fake.loadExperience();
    const state = { revision: 1, receive: undefined as ((frame: never) => void) | undefined };
    const experienceLoads = vi.fn();
    const statusReads = vi.fn();
    fake.loadExperience = async () => {
      experienceLoads();
      return ({ ...initial, access: { available: true, enabled: true, can_administer: true, subject: "first-admin" } }) as never;
    };
    fake.subscribeTimeline = (_input, next) => { state.receive = next; return () => {}; };
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/timeline", "meta/access/status", "meta/access/get"] }) as never;
    fake.executeCommand = vi.fn(async input => {
      if (input.command === "accessStatus") {
        statusReads();
        return { command: input.command, accepted: true, result: { available: true, enabled: true, can_administer: true, subject: "first-admin", revision: state.revision, actions: [] } } as never;
      }
      if (input.command === "getAccessConfig") {
        return { command: input.command, accepted: true, result: { config: { enabled: true, admins: ["first-admin"], rules: [], groups: {} }, revision: state.revision } } as never;
      }
      throw new Error(`unexpected ${input.command}`);
    });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    fireEvent.click(await screen.findByText("Access", { exact: true }));
    await screen.findByText("first-admin", { exact: true });
    await waitFor(() => expect(state.receive).toBeTypeOf("function"));
    await settle(400);
    expect(statusReads).toHaveBeenCalled();
    const push = (frame: unknown) => act(async () => { state.receive?.(frame as never); });
    return { state, experienceLoads, statusReads, push };
  }
  const settle = (ms: number) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

  it("does not read Access again for activity that cannot change it", async () => {
    const { experienceLoads, statusReads, push } = await openAccess();
    const readsBefore = statusReads.mock.calls.length;
    const loadsBefore = experienceLoads.mock.calls.length;
    await push({ id: "unrelated-tool", event: "tool_execution_completed", identity, interactionId: "unrelated-turn", timestampMs: 2, data: {} });
    // The activity still refreshes the roster.
    await waitFor(() => expect(experienceLoads.mock.calls.length).toBeGreaterThan(loadsBefore));
    await settle(400);
    expect(statusReads).toHaveBeenCalledTimes(readsBefore);
  });

  it("reads Access once for each newer revision the owner signals", async () => {
    const { state, statusReads, push } = await openAccess();
    const readsBefore = statusReads.mock.calls.length;
    state.revision = 2;
    await push({ id: "", event: "access_changed", data: { type: "access_changed", revision: 2 } });
    await waitFor(() => expect(statusReads).toHaveBeenCalledTimes(readsBefore + 1));
    await settle(400);
    expect(statusReads).toHaveBeenCalledTimes(readsBefore + 1);
    // A revision the Console has already read changes nothing.
    await push({ id: "", event: "access_changed", data: { type: "access_changed", revision: 2 } });
    await push({ id: "", event: "access_changed", data: { type: "access_changed", revision: 1 } });
    await settle(400);
    expect(statusReads).toHaveBeenCalledTimes(readsBefore + 1);
    expect(screen.getByText("first-admin", { exact: true })).toBeVisible();
  });
});

// These are actual ConsoleApp/HTTP-adapter tests with mocked RPC responses.
// They do not substitute for the real AccessController mutex/TOML tests.
describe("checked access saves", () => {
  const originalConfig: ConsoleAccessConfig = {
    enabled: true, admins: ["root@example.test", "alice@example.test"], rules: [], groups: {},
  };
  const editedAdmins = ["root@example.test", "alice@example.test", "carol@example.test"];
  const draftText = editedAdmins.join(", ");
  const newerRule = { id: "b-newer-rule", effect: "allow" as const, subjects: ["reader@example.test"], actions: ["agent.view"], agents: ["worker"] };
  const mutationMethods = new Set([
    "meta/access/set", "meta/access/enable", "meta/access/rules/upsert",
    "meta/access/rules/delete", "meta/access/groups/set", "meta/access/groups/delete",
  ]);

  async function fixture(capability: string | undefined) {
    const fake = transport(vi.fn());
    const experience = await fake.loadExperience();
    const state = {
      capability, ownerInstance: "opaque-owner:one", revision: 10,
      config: { ...originalConfig }, legacy: false, applied: 0,
    };
    const writes: Array<{ url: string; method: string; params: Record<string, unknown> }> = [];
    const reads: string[] = [];
    fake.loadExperience = async () => ({ ...experience, access: { available: true, enabled: true, can_administer: true, subject: "root@example.test" } }) as never;
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/timeline", "meta/access/status", "meta/access/get", ...mutationMethods] }) as never;
    fake.executeCommand = createHttpConsoleTransport({ baseUrl: "http://console.test" }).executeCommand;
    vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body || "{}")) as { id: string; method: string; params: Record<string, unknown> };
      const reply = (payload: Record<string, unknown>) => new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, ...payload }), { status: 200 });
      const metadata = {
        revision: state.revision,
        ...(!state.legacy ? { owner_instance: state.ownerInstance, conditional_mutations: state.capability } : {}),
      };
      if (body.method === "meta/access/status") {
        reads.push(body.method);
        return reply({ result: { available: true, enabled: state.config.enabled, can_administer: true, subject: "root@example.test", actions: ["agent.view"], ...metadata } });
      }
      if (body.method === "meta/access/get") {
        reads.push(body.method);
        return reply({ result: { config: state.config, ...metadata } });
      }
      if (mutationMethods.has(body.method)) {
        writes.push({ url: String(url), method: body.method, params: body.params });
        // A legacy handler accepts its required top-level payload. This makes
        // an unsafe fallback observable rather than making every write fail.
        if (state.legacy && body.method === "meta/access/set" && body.params.config) {
          state.config = body.params.config as ConsoleAccessConfig;
          state.applied += 1; state.revision += 1;
          return reply({ result: { config: state.config, revision: state.revision } });
        }
        const checked = body.params.checked_v1 as { owner_instance?: string; expected_revision?: number; config?: ConsoleAccessConfig } | undefined;
        if (state.legacy || body.method !== "meta/access/set" || !checked?.config || Object.keys(body.params).length !== 1) {
          return reply({ error: { code: -32602, message: "PRIVATE_CHECKED_PAYLOAD_ERROR" } });
        }
        if (checked.owner_instance !== state.ownerInstance) {
          return reply({ error: { code: -32009, message: "PRIVATE_OWNER_MESSAGE", data: { kind: "access_owner_changed" } } });
        }
        if (checked.expected_revision !== state.revision) {
          return reply({ error: { code: -32009, message: "PRIVATE_CONFLICT_MESSAGE", data: { kind: "access_revision_conflict", expected_revision: checked.expected_revision, actual_revision: state.revision } } });
        }
        state.config = checked.config;
        state.applied += 1; state.revision += 1;
        return reply({ result: { config: state.config, ...metadata, revision: state.revision } });
      }
      throw new Error(`unexpected mock HTTP method ${body.method}`);
    }));
    return { fake, state, writes, reads };
  }

  // Restore a saved layout with Access already open, so no nav click can race
  // the dock layout hydration.
  async function openAccess(fake: MetaConsoleTransport) {
    window.localStorage.setItem("meta-console-dock-state:runtime/realm/principal", JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "access", kind: "access", title: "Access" } }],
      activeTabId: "tab-1", focusedPanelId: "panel-1",
    }));
    const view = render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
    await screen.findByText("alice@example.test", { exact: true });
    return view;
  }

  function unavailableWrite(testId: string) {
    const button = screen.queryByTestId(testId);
    if (button) {
      expect(button).toBeDisabled();
      fireEvent.click(button);
    }
  }

  it("keeps the original edit token through refresh and requires explicit conflict review before reapply", async () => {
    const { fake, state, writes, reads } = await fixture("checked_v1");
    await openAccess(fake);
    fireEvent.click(screen.getByTestId("access-edit-admins"));
    fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });

    state.revision = 11;
    state.config = { ...originalConfig, rules: [newerRule] };
    const beforeRefresh = reads.length;
    await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
    await waitFor(() => expect(reads.length).toBeGreaterThan(beforeRefresh));
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    expect(await screen.findByTestId("access-rule:b-newer-rule")).toBeVisible();
    fireEvent.click(screen.getByTestId("access-tab:overview"));
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    expect(writes).toHaveLength(0);

    await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toEqual({
      url: "http://console.test/console/rpc", method: "meta/access/set",
      params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, config: { ...originalConfig, admins: editedAdmins } } },
    });
    expect(await screen.findByTestId("access-error")).toHaveTextContent("Access configuration changed. Review the latest settings before saving again.");
    expect(document.body).not.toHaveTextContent("PRIVATE_CONFLICT_MESSAGE");
    expect(document.body).not.toHaveTextContent("PRIVATE_CHECKED_PAYLOAD_ERROR");
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    expect(state.config).toEqual({ ...originalConfig, rules: [newerRule] });
    expect(state.revision).toBe(11);
    expect(state.applied).toBe(0);
    expect(screen.getByTestId("access-save-admins")).toBeDisabled();
    fireEvent.click(screen.getByTestId("access-save-admins"));
    await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    expect(screen.getByTestId("access-save-admins")).toBeDisabled();
    expect(writes).toHaveLength(1);

    // Review binds the displayed current base without submitting a write.
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Review and reapply", exact: true })); });
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    expect(writes).toHaveLength(1);
    expect(screen.getByTestId("access-save-admins")).toBeEnabled();
    await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
    await waitFor(() => expect(screen.queryByTestId("access-admins-input")).toBeNull());
    expect(writes).toHaveLength(2);
    expect(writes[1]).toEqual({
      url: "http://console.test/console/rpc", method: "meta/access/set",
      params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 11, config: { ...originalConfig, rules: [newerRule], admins: editedAdmins } } },
    });
    expect(state.config).toEqual({ ...originalConfig, rules: [newerRule], admins: editedAdmins });
    expect(state.revision).toBe(12);
    expect(state.applied).toBe(1);
    expect(screen.getByText("carol@example.test", { exact: true })).toBeVisible();
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    expect(screen.getByTestId("access-rule:b-newer-rule")).toBeVisible();
  });

  it("keeps authorized reads usable but sends no mutation without a recognized checked capability", async () => {
    for (const capability of [undefined, "checked_v2"]) {
      const { fake, state, writes, reads } = await fixture(capability);
      const view = await openAccess(fake);
      expect(reads).toContain("meta/access/status");
      expect(reads).toContain("meta/access/get");
      expect(screen.getByText("alice@example.test", { exact: true })).toBeVisible();
      unavailableWrite("access-toggle-enabled");
      unavailableWrite("access-edit-admins");
      fireEvent.click(screen.getByTestId("access-tab:rules"));
      unavailableWrite("access-rule-new");
      fireEvent.click(screen.getByTestId("access-tab:groups"));
      unavailableWrite("access-group-save");
      await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
      expect(writes).toHaveLength(0);
      expect(state.applied).toBe(0);
      expect(state.revision).toBe(10);
      view.unmount();
    }
  });

  it("retains the draft when a cached capability meets an old backend and never falls back to a legacy write", async () => {
    const { fake, state, writes } = await fixture("checked_v1");
    await openAccess(fake);
    fireEvent.click(screen.getByTestId("access-edit-admins"));
    fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });
    state.legacy = true;
    await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toEqual({
      url: "http://console.test/console/rpc", method: "meta/access/set",
      params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, config: { ...originalConfig, admins: editedAdmins } } },
    });
    expect(await screen.findByTestId("access-error")).toHaveTextContent("Changes were not saved. Checked access saves are unavailable; your draft is retained.");
    expect(document.body).not.toHaveTextContent("PRIVATE_CHECKED_PAYLOAD_ERROR");
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    unavailableWrite("access-save-admins");
    await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    unavailableWrite("access-save-admins");
    expect(screen.queryByRole("button", { name: "Review and reapply", exact: true })).toBeNull();
    expect(writes).toHaveLength(1);
    expect(state.applied).toBe(0);
    expect(state.revision).toBe(10);
    expect(state.config).toEqual(originalConfig);
  });

  it("routes all six access writes through the complete checked envelope", async () => {
    const { fake, state, writes } = await fixture("checked_v1");
    const originalFetch = globalThis.fetch;
    // Reuse the same real HTTP adapter and protected-read fixture for every
    // control. The mock checks only wire shape/sequence, not owner atomicity.
    vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body || "{}")) as { id: string; method: string; params: Record<string, unknown> };
      if (!mutationMethods.has(body.method) || body.method === "meta/access/set") return originalFetch(url, init);
      writes.push({ url: String(url), method: body.method, params: body.params });
      const reply = (payload: Record<string, unknown>) => new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, ...payload }), { status: 200 });
      const checked = body.params.checked_v1 as Record<string, unknown> | undefined;
      if (Object.keys(body.params).length !== 1 || !checked
          || checked.owner_instance !== state.ownerInstance || checked.expected_revision !== state.revision) {
        return reply({ error: { code: -32602, message: "PRIVATE_ROUTING_PAYLOAD_ERROR" } });
      }
      switch (body.method) {
        case "meta/access/enable": state.config = { ...state.config, enabled: checked.enabled as boolean }; break;
        case "meta/access/rules/upsert": state.config = { ...state.config, rules: [...(state.config.rules ?? []), checked.rule as NonNullable<ConsoleAccessConfig["rules"]>[number]] }; break;
        case "meta/access/rules/delete": state.config = { ...state.config, rules: state.config.rules?.filter(rule => rule.id !== checked.id) }; break;
        case "meta/access/groups/set": state.config = { ...state.config, groups: { ...state.config.groups, [checked.name as string]: checked.group as { members: string[] } } }; break;
        case "meta/access/groups/delete": {
          const groups = { ...state.config.groups };
          delete groups[checked.name as string];
          state.config = { ...state.config, groups };
          break;
        }
        default: throw new Error(`unhandled routing control ${body.method}`);
      }
      state.applied += 1; state.revision += 1;
      return reply({ result: { config: state.config, revision: state.revision, owner_instance: state.ownerInstance, conditional_mutations: "checked_v1" } });
    }));
    vi.spyOn(window, "confirm").mockReturnValue(true);
    await openAccess(fake);

    fireEvent.click(screen.getByTestId("access-edit-admins"));
    fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
    await waitFor(() => expect(screen.queryByTestId("access-admins-input")).toBeNull());
    await act(async () => { fireEvent.click(screen.getByTestId("access-toggle-enabled")); });
    await screen.findByRole("button", { name: "Enable enforcement", exact: true });

    fireEvent.click(screen.getByTestId("access-tab:rules"));
    fireEvent.click(screen.getByTestId("access-rule-new"));
    fireEvent.change(screen.getByTestId("access-rule-id"), { target: { value: "checked-rule" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    await screen.findByTestId("access-rule-delete:checked-rule");
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-delete:checked-rule")); });
    await waitFor(() => expect(screen.queryByTestId("access-rule:checked-rule")).toBeNull());
    await screen.findByTestId("access-rule-new");

    fireEvent.click(screen.getByTestId("access-tab:groups"));
    fireEvent.change(screen.getByTestId("access-group-name"), { target: { value: "checked-group" } });
    fireEvent.change(screen.getByTestId("access-group-members"), { target: { value: "alice@example.test" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-group-save")); });
    await screen.findByTestId("access-group-delete:checked-group");
    await act(async () => { fireEvent.click(screen.getByTestId("access-group-delete:checked-group")); });
    await waitFor(() => expect(screen.queryByTestId("access-group:checked-group")).toBeNull());
    expect(screen.queryByTestId("access-error")).toBeNull();

    const payloads = [
      ["meta/access/set", { config: { ...originalConfig, admins: editedAdmins } }],
      ["meta/access/enable", { enabled: false }],
      ["meta/access/rules/upsert", { rule: { id: "checked-rule", effect: "allow", actions: ["agent.view"] } }],
      ["meta/access/rules/delete", { id: "checked-rule" }],
      ["meta/access/groups/set", { name: "checked-group", group: { members: ["alice@example.test"] } }],
      ["meta/access/groups/delete", { name: "checked-group" }],
    ] as const;
    expect(writes).toEqual(payloads.map(([method, payload], index) => ({
      url: "http://console.test/console/rpc", method,
      params: { checked_v1: { ...payload, owner_instance: "opaque-owner:one", expected_revision: 10 + index } },
    })));
    expect(state.applied).toBe(6);
    expect(state.revision).toBe(16);
    expect(state.config).toEqual({ ...originalConfig, enabled: false, admins: editedAdmins });
    expect(window.confirm).toHaveBeenCalledTimes(2);
  });

  it("reapplies only the intended group members onto the reviewed description", async () => {
    const { fake, state, writes } = await fixture("checked_v1");
    const initialGroup = { description: "Original group description", members: ["alice@example.test"] };
    const newerGroup = { ...initialGroup, description: "Another admin's updated description" };
    const editedMembers = ["carol@example.test"];
    state.config = { ...originalConfig, groups: { ops: initialGroup } };
    const originalFetch = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body || "{}")) as { id: string; method: string; params: Record<string, unknown> };
      if (body.method !== "meta/access/groups/set") return originalFetch(url, init);
      writes.push({ url: String(url), method: body.method, params: body.params });
      const reply = (payload: Record<string, unknown>) => new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, ...payload }), { status: 200 });
      const checked = body.params.checked_v1 as { owner_instance?: string; expected_revision?: number; name?: string; group?: { description?: string; members: string[] } } | undefined;
      if (Object.keys(body.params).length !== 1 || !checked?.group || checked.name !== "ops" || checked.owner_instance !== state.ownerInstance) {
        return reply({ error: { code: -32602, message: "PRIVATE_GROUP_PAYLOAD_ERROR" } });
      }
      if (checked.expected_revision !== state.revision) {
        return reply({ error: { code: -32009, message: "PRIVATE_GROUP_CONFLICT", data: { kind: "access_revision_conflict", expected_revision: checked.expected_revision, actual_revision: state.revision } } });
      }
      // Like the real owner, a group save replaces the complete group.
      state.config = { ...state.config, groups: { ...state.config.groups, ops: checked.group } };
      state.applied += 1; state.revision += 1;
      return reply({ result: { revision: state.revision } });
    }));
    await openAccess(fake);
    fireEvent.click(screen.getByTestId("access-tab:groups"));
    fireEvent.click(screen.getByTestId("access-group-edit:ops"));
    fireEvent.change(screen.getByTestId("access-group-members"), { target: { value: editedMembers.join(", ") } });
    state.config = { ...state.config, groups: { ops: newerGroup } };
    state.revision = 11;
    await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
    expect(screen.getByTestId("access-group-members")).toHaveValue("carol@example.test");
    await act(async () => { fireEvent.click(screen.getByTestId("access-group-save")); });
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toEqual({
      url: "http://console.test/console/rpc", method: "meta/access/groups/set",
      params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, name: "ops", group: { ...initialGroup, members: editedMembers } } },
    });
    expect(await screen.findByTestId("access-error")).toHaveTextContent("Access configuration changed. Review the latest settings before saving again.");
    expect(document.body).not.toHaveTextContent("PRIVATE_GROUP_CONFLICT");
    expect(screen.getByTestId("access-group-members")).toHaveValue("carol@example.test");
    expect(screen.getByTestId("access-group-save")).toBeDisabled();
    expect(state.config.groups?.ops).toEqual(newerGroup);
    expect(state.applied).toBe(0);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Review and reapply", exact: true })); });
    expect(writes).toHaveLength(1);
    expect(screen.getByTestId("access-group-save")).toBeEnabled();
    await act(async () => { fireEvent.click(screen.getByTestId("access-group-save")); });
    await screen.findByTestId("access-group:ops");
    expect(writes).toEqual([
      writes[0],
      { url: "http://console.test/console/rpc", method: "meta/access/groups/set",
        params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 11, name: "ops", group: { ...newerGroup, members: editedMembers } } } },
    ]);
    expect(screen.getByText(newerGroup.description, { exact: true })).toBeVisible();
    expect(state.config.groups?.ops).toEqual({ ...newerGroup, members: editedMembers });
    expect(state.revision).toBe(12);
    expect(state.applied).toBe(1);
  });

  it("retains a typed unavailable draft and blocks repeated Save even when checked reads remain available", async () => {
    const { fake, state, writes, reads } = await fixture("checked_v1");
    const originalFetch = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body || "{}")) as { id: string; method: string; params: Record<string, unknown> };
      if (body.method !== "meta/access/set") return originalFetch(url, init);
      writes.push({ url: String(url), method: body.method, params: body.params });
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, error: {
        code: -32004, message: "PRIVATE_UNAVAILABLE_MESSAGE", data: { kind: "access_mutation_unavailable" },
      } }), { status: 200 });
    }));
    await openAccess(fake);
    fireEvent.click(screen.getByTestId("access-edit-admins"));
    fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
    expect(await screen.findByTestId("access-error")).toHaveTextContent("Changes were not saved. Checked access saves are unavailable; your draft is retained.");
    expect(document.body).not.toHaveTextContent("PRIVATE_UNAVAILABLE_MESSAGE");
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    unavailableWrite("access-save-admins");
    const beforeRefresh = reads.length;
    await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
    await waitFor(() => expect(reads.length).toBeGreaterThan(beforeRefresh));
    expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
    unavailableWrite("access-save-admins");
    expect(writes).toEqual([{ url: "http://console.test/console/rpc", method: "meta/access/set", params: {
      checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, config: { ...originalConfig, admins: editedAdmins } },
    } }]);
    expect(state.capability).toBe("checked_v1");
    expect(state.revision).toBe(10);
    expect(state.applied).toBe(0);
    expect(state.config).toEqual(originalConfig);
  });

  it("keeps an owner-rejected invalid rule draft editable on its own token, then saves the correction", async () => {
    const { fake, state, writes } = await fixture("checked_v1");
    const originalFetch = globalThis.fetch;
    vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body || "{}")) as { id: string; method: string; params: Record<string, unknown> };
      if (body.method !== "meta/access/rules/upsert") return originalFetch(url, init);
      writes.push({ url: String(url), method: body.method, params: body.params });
      const reply = (payload: Record<string, unknown>) => new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, ...payload }), { status: 200 });
      const checked = body.params.checked_v1 as { owner_instance?: string; expected_revision?: number; rule?: { groups?: string[] } } | undefined;
      if (Object.keys(body.params).length !== 1 || checked?.owner_instance !== state.ownerInstance || checked.expected_revision !== state.revision) {
        return reply({ error: { code: -32602, message: "PRIVATE_RULE_PAYLOAD_ERROR" } });
      }
      // Like the real owner: the rule names a group the configuration lacks.
      if (checked.rule?.groups?.length) {
        return reply({ error: { code: -32602, message: "Invalid access configuration.", data: { kind: "invalid_access_config" } } });
      }
      state.config = { ...state.config, rules: [...(state.config.rules ?? []), checked.rule as NonNullable<ConsoleAccessConfig["rules"]>[number]] };
      state.applied += 1; state.revision += 1;
      return reply({ result: { revision: state.revision } });
    }));
    await openAccess(fake);
    fireEvent.click(screen.getByTestId("access-tab:rules"));
    fireEvent.click(screen.getByTestId("access-rule-new"));
    fireEvent.change(screen.getByTestId("access-rule-id"), { target: { value: "ops-rule" } });
    fireEvent.change(screen.getByTestId("access-rule-groups"), { target: { value: "missing-group" } });
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    expect(await screen.findByTestId("access-error")).toHaveTextContent("Changes were not saved. The resulting access configuration is not valid; your draft is retained for correction.");
    expect(screen.queryByRole("button", { name: "Review and reapply", exact: true })).toBeNull();
    expect(screen.getByTestId("access-rule-groups")).toHaveValue("missing-group");
    expect(state.applied).toBe(0);
    fireEvent.change(screen.getByTestId("access-rule-groups"), { target: { value: "" } });
    expect(screen.getByTestId("access-rule-save")).toBeEnabled();
    await act(async () => { fireEvent.click(screen.getByTestId("access-rule-save")); });
    await screen.findByTestId("access-rule:ops-rule");
    expect(writes).toEqual([
      { url: "http://console.test/console/rpc", method: "meta/access/rules/upsert",
        params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, rule: { id: "ops-rule", effect: "allow", actions: ["agent.view"], groups: ["missing-group"] } } } },
      { url: "http://console.test/console/rpc", method: "meta/access/rules/upsert",
        params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, rule: { id: "ops-rule", effect: "allow", actions: ["agent.view"] } } } },
    ]);
    expect(state.applied).toBe(1);
    expect(state.revision).toBe(11);
    expect(screen.queryByTestId("access-error")).toBeNull();
    expect(document.body).not.toHaveTextContent("PRIVATE_RULE_PAYLOAD_ERROR");
  });

  // Reads the console starts on its own (stream events, layout changes)
  // revalidate a settled Access without taking it away from the user; the
  // owner's checked_v1 token, not a disabled form, protects a save.
  describe("background revalidation", () => {
    type HeldRead = { method: string; release: () => void; fail: (response: Response) => void };
    /// Each matching protected read is answered from the owner state when it
    /// was sent, then held until the test releases or fails it.
    function holdReads(methods: string[]) {
      const originalFetch = globalThis.fetch;
      const control = { holding: true, held: [] as HeldRead[] };
      vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
        const body = JSON.parse(String(init?.body || "{}")) as { method: string };
        const answer = await originalFetch(url, init);
        if (!control.holding || !methods.includes(body.method)) return answer;
        return new Promise<Response>(resolve => {
          control.held.push({ method: body.method, release: () => resolve(answer), fail: response => resolve(response) });
        });
      }));
      return control;
    }
    /// The owner's access_changed signal on the timeline, which schedules a
    /// background read. Each one names a revision newer than any the
    /// fixture's owner reports, so each one reads Access.
    function accessChanges(fake: MetaConsoleTransport) {
      let receive: ((frame: never) => void) | undefined;
      fake.subscribeTimeline = (_input, next) => { receive = next; return () => {}; };
      let sequence = 0;
      return async () => {
        sequence += 1;
        await act(async () => {
          receive?.({ id: "", event: "access_changed", data: { type: "access_changed", revision: 1_000 + sequence } } as never);
        });
      };
    }
    const refreshNotice = () => screen.queryByText("Refreshing owner state. Changes are temporarily unavailable.");
    const markerRule = (id: string) => ({ id, effect: "allow" as const, subjects: ["marker@example.test"], actions: ["agent.view"], agents: ["worker"] });

    it("keeps a settled Access editable while a background read is in flight", async () => {
      const { fake } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      await openAccess(fake);
      const reads = holdReads(["meta/access/get"]);
      await accessChange();
      await waitFor(() => expect(reads.held).toHaveLength(1));
      expect(refreshNotice()).toBeNull();
      expect(screen.getByTestId("access-edit-admins")).toBeEnabled();
      fireEvent.click(screen.getByTestId("access-edit-admins"));
      expect(screen.getByTestId("access-admins-input")).toBeEnabled();
      await act(async () => { reads.held.forEach(read => read.release()); });
    });

    it("saves during a background read, whose older answer never lands after the save", async () => {
      const { fake, state, writes } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      await openAccess(fake);
      fireEvent.click(screen.getByTestId("access-edit-admins"));
      fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });
      // The background read answers with a marker no later read carries.
      state.config = { ...originalConfig, rules: [markerRule("held-answer")] };
      const reads = holdReads(["meta/access/get"]);
      await accessChange();
      await waitFor(() => expect(reads.held).toHaveLength(1));
      state.config = { ...originalConfig };
      expect(screen.getByTestId("access-save-admins")).toBeEnabled();
      await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
      await waitFor(() => expect(writes).toHaveLength(1));
      expect(writes[0]).toEqual({
        url: "http://console.test/console/rpc", method: "meta/access/set",
        params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, config: { ...originalConfig, admins: editedAdmins } } },
      });
      // The read after the save waits behind the held one; the save's
      // refresh notice shows meanwhile.
      await waitFor(() => expect(refreshNotice()).not.toBeNull());
      await act(async () => { reads.held[0].release(); });
      await waitFor(() => expect(reads.held).toHaveLength(2));
      expect(refreshNotice()).not.toBeNull();
      fireEvent.click(screen.getByTestId("access-tab:rules"));
      expect(screen.queryByTestId("access-rule:held-answer")).toBeNull();
      await act(async () => { reads.held[1].release(); });
      await waitFor(() => expect(refreshNotice()).toBeNull());
      expect(screen.queryByTestId("access-rule:held-answer")).toBeNull();
      fireEvent.click(screen.getByTestId("access-tab:overview"));
      expect(screen.getByText("carol@example.test", { exact: true })).toBeVisible();
      expect(state.revision).toBe(11);
      expect(state.applied).toBe(1);
    });

    it("keeps the draft's own base when a background read brings a newer revision", async () => {
      const { fake, state, writes } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      await openAccess(fake);
      fireEvent.click(screen.getByTestId("access-edit-admins"));
      fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });
      state.revision = 11;
      state.config = { ...originalConfig, rules: [newerRule] };
      await accessChange();
      fireEvent.click(screen.getByTestId("access-tab:rules"));
      expect(await screen.findByTestId("access-rule:b-newer-rule")).toBeVisible();
      fireEvent.click(screen.getByTestId("access-tab:overview"));
      expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
      await act(async () => { fireEvent.click(screen.getByTestId("access-save-admins")); });
      await waitFor(() => expect(writes).toHaveLength(1));
      expect(writes[0]).toEqual({
        url: "http://console.test/console/rpc", method: "meta/access/set",
        params: { checked_v1: { owner_instance: state.ownerInstance, expected_revision: 10, config: { ...originalConfig, admins: editedAdmins } } },
      });
      expect(await screen.findByTestId("access-error")).toHaveTextContent("Access configuration changed. Review the latest settings before saving again.");
      expect(screen.getByRole("button", { name: "Review and reapply", exact: true })).toBeVisible();
      expect(screen.getByTestId("access-admins-input")).toHaveValue(draftText);
      expect(state.applied).toBe(0);
    });

    it("drops the view and the draft when a background read is refused", async () => {
      const { fake } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      await openAccess(fake);
      fireEvent.click(screen.getByTestId("access-edit-admins"));
      fireEvent.change(screen.getByTestId("access-admins-input"), { target: { value: draftText } });
      const originalFetch = globalThis.fetch;
      vi.stubGlobal("fetch", vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
        const body = JSON.parse(String(init?.body || "{}")) as { id: string; method: string };
        if (body.method !== "meta/access/status") return originalFetch(url, init);
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, error: { code: -32030, message: "PRIVATE_DENIED", data: { kind: "access_denied" } } }), { status: 200 });
      }));
      await accessChange();
      expect(await screen.findByText("Current administrator access is required to view this configuration.")).toBeVisible();
      expect(screen.queryByText("alice@example.test", { exact: true })).toBeNull();
      expect(screen.queryByTestId("access-admins-input")).toBeNull();
    });

    it("fails closed on a background failure without dropping the alert, until a read succeeds", async () => {
      const { fake } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      await openAccess(fake);
      const reads = holdReads(["meta/access/get"]);
      await accessChange();
      await waitFor(() => expect(reads.held).toHaveLength(1));
      await act(async () => { reads.held[0].fail(new Response("upstream unavailable", { status: 500 })); });
      expect(await screen.findByTestId("access-error")).toBeVisible();
      expect(screen.getByText("alice@example.test", { exact: true })).toBeVisible();
      expect(screen.queryByTestId("access-edit-admins")).toBeNull();
      await accessChange();
      await waitFor(() => expect(reads.held).toHaveLength(2));
      expect(screen.getByTestId("access-error")).toBeVisible();
      expect(screen.queryByTestId("access-edit-admins")).toBeNull();
      expect(refreshNotice()).toBeNull();
      await act(async () => { reads.held[1].release(); });
      await waitFor(() => expect(screen.queryByTestId("access-error")).toBeNull());
      expect(screen.getByTestId("access-edit-admins")).toBeEnabled();
    });

    it("blocks from an explicit Refresh until a read issued after it lands", async () => {
      const { fake, state } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      await openAccess(fake);
      state.config = { ...originalConfig, rules: [markerRule("held-answer")] };
      const reads = holdReads(["meta/access/get"]);
      await accessChange();
      await waitFor(() => expect(reads.held).toHaveLength(1));
      state.revision = 11;
      state.config = { ...originalConfig, rules: [markerRule("trailing-answer")] };
      fireEvent.click(screen.getByTestId("access-tab:rules"));
      await act(async () => { fireEvent.click(screen.getByTestId("access-refresh")); });
      expect(refreshNotice()).not.toBeNull();
      // A background trigger after the Refresh replaces its trailing run, so
      // the read that ends the notice is a background one. The trigger has
      // run once the experience load it starts with has been asked for.
      let experienceLoads = 0;
      const loadExperience = fake.loadExperience;
      fake.loadExperience = async () => { experienceLoads += 1; return loadExperience(); };
      await accessChange();
      await waitFor(() => expect(experienceLoads).toBeGreaterThan(0));
      await act(async () => { reads.held[0].release(); });
      await waitFor(() => expect(reads.held).toHaveLength(2));
      expect(refreshNotice()).not.toBeNull();
      expect(screen.queryByTestId("access-rule:held-answer")).toBeNull();
      await act(async () => { reads.held[1].release(); });
      await waitFor(() => expect(refreshNotice()).toBeNull());
      expect(screen.getByTestId("access-rule:trailing-answer")).toBeVisible();
      expect(screen.queryByTestId("access-rule:held-answer")).toBeNull();
    });

    it("still blocks edits during the first read of a scope", async () => {
      const { fake } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
      const accessNav = await screen.findByText("Access", { exact: true });
      // The first read after opening Access.
      const reads = holdReads(["meta/access/get"]);
      fireEvent.click(accessNav);
      await waitFor(() => expect(reads.held).toHaveLength(1));
      expect(screen.getByText("Loading Console access...")).toBeVisible();
      expect(screen.queryByTestId("access-edit-admins")).toBeNull();
      reads.holding = false;
      await act(async () => { reads.held[0].release(); });
      expect(await screen.findByText("alice@example.test", { exact: true })).toBeVisible();
      expect(screen.getByTestId("access-edit-admins")).toBeEnabled();
      // Another subject is another scope. Its first read blocks at once, even
      // while it waits behind the old scope's read, which is held here. The
      // new experience is held too, so the old scope's read is sent first.
      const statuses = holdReads(["meta/access/status"]);
      const experience = await fake.loadExperience();
      const nextExperience = { ...experience, access: { ...(experience as { access: object }).access, subject: "alice@example.test" } };
      const experienceLoads: Array<() => void> = [];
      fake.loadExperience = () => new Promise(resolve => { experienceLoads.push(() => resolve(nextExperience as never)); });
      await accessChange();
      await waitFor(() => expect(statuses.held).toHaveLength(1));
      await waitFor(() => expect(experienceLoads.length).toBeGreaterThan(0));
      await act(async () => { experienceLoads.forEach(load => load()); });
      expect(screen.getByText("Loading Console access...")).toBeVisible();
      expect(screen.queryByTestId("access-edit-admins")).toBeNull();
      statuses.holding = false;
      await act(async () => { statuses.held.forEach(read => read.release()); });
      expect(await screen.findByText("alice@example.test", { exact: true })).toBeVisible();
      expect(screen.getByTestId("access-edit-admins")).toBeEnabled();
    });

    it("lands the first read of a scope while stream events queue more reads behind it", async () => {
      const { fake } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      const loadExperience = fake.loadExperience.bind(fake);
      let experienceLoads = 0;
      fake.loadExperience = () => { experienceLoads += 1; return loadExperience(); };
      render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
      const accessNav = await screen.findByText("Access", { exact: true });
      const reads = holdReads(["meta/access/get"]);
      fireEvent.click(accessNav);
      await waitFor(() => expect(reads.held).toHaveLength(1));
      // The event's read is requested while the first one is in flight.
      const loadsBefore = experienceLoads;
      await accessChange();
      await waitFor(() => expect(experienceLoads).toBeGreaterThan(loadsBefore));
      expect(screen.getByText("Loading Console access...")).toBeVisible();
      await act(async () => { reads.held[0].release(); });
      await waitFor(() => expect(reads.held).toHaveLength(2));
      expect(screen.getByText("alice@example.test", { exact: true })).toBeVisible();
      expect(screen.getByTestId("access-edit-admins")).toBeEnabled();
      expect(refreshNotice()).toBeNull();
      await act(async () => { reads.held[1].release(); });
    });

    it("settles a scope on a failed first read, so stream events do not return it to loading", async () => {
      const { fake } = await fixture("checked_v1");
      const accessChange = accessChanges(fake);
      render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
      const accessNav = await screen.findByText("Access", { exact: true });
      const reads = holdReads(["meta/access/status"]);
      fireEvent.click(accessNav);
      await waitFor(() => expect(reads.held).toHaveLength(1));
      expect(screen.getByText("Loading Console access...")).toBeVisible();
      await act(async () => { reads.held[0].fail(new Response("upstream unavailable", { status: 500 })); });
      await waitFor(() => expect(screen.queryByText("Loading Console access...")).toBeNull());
      await accessChange();
      await waitFor(() => expect(reads.held).toHaveLength(2));
      expect(screen.queryByText("Loading Console access...")).toBeNull();
      await act(async () => { reads.held[1].release(); });
      expect(await screen.findByText("alice@example.test", { exact: true })).toBeVisible();
      expect(screen.getByTestId("access-edit-admins")).toBeEnabled();
    });
  });
});

describe("idle status poll settles a fold the frames left open", () => {
  const scope = { identity, runtimeKey: "owner-runtime", sessionId: "owner-session", sourceKind: "console_event" as const };
  // The user's turn stops at a host callback and a runtime-started
  // continuation finishes it under its own lineage; or the user's turn
  // settles and a frame with no lifecycle ids arrives before the
  // continuation. Either way no terminal closes what the frames opened.
  const stuckFrames: Record<"paused" | "idless", ConsoleFrame[]> = {
    paused: [
      { ...scope, id: "q-input", event: "user_input", interactionId: "question", status: "delivered", timestampMs: 1000, data: { content: "Set it up and answer" } },
      { ...scope, id: "q-start", event: "run_started", interactionId: "question", runId: "question-run", timestampMs: 1001, data: {} },
      { ...scope, id: "q-tool", event: "tool_call_requested", interactionId: "question", runId: "question-run", timestampMs: 1002, data: { id: "setup", name: "setup_selected_install" } },
      { ...scope, id: "q-turn", event: "turn_completed", interactionId: "question", runId: "question-run", timestampMs: 1003, data: { stop_reason: "tool_use" } },
      { ...scope, id: "q-pause", event: "interaction_callback_pending", interactionId: "question", timestampMs: 1004, data: {} },
      { ...scope, id: "c-start", event: "run_started", runId: "continuation-run", timestampMs: 2000, data: {} },
      { ...scope, id: "c-text", event: "text_complete", runId: "continuation-run", timestampMs: 2001, data: { content: "Here is the answer" } },
      { ...scope, id: "c-turn", event: "turn_completed", runId: "continuation-run", timestampMs: 2002, data: { stop_reason: "end_turn" } },
      { ...scope, id: "c-end", event: "interaction_complete", runId: "continuation-run", timestampMs: 2003, data: {} },
    ],
    idless: [
      { ...scope, id: "q-input", event: "user_input", interactionId: "question", status: "delivered", timestampMs: 1000, data: { content: "Set it up and answer" } },
      { ...scope, id: "q-start", event: "run_started", interactionId: "question", runId: "question-run", timestampMs: 1001, data: {} },
      { ...scope, id: "q-end", event: "interaction_complete", interactionId: "question", runId: "question-run", timestampMs: 1004, data: {} },
      { identity, sourceKind: "console_event", id: "setup-done", event: "tool_execution_completed", timestampMs: 1500, data: { id: "setup", name: "setup_selected_install" } },
      { ...scope, id: "c-start", event: "run_started", runId: "continuation-run", timestampMs: 2000, data: {} },
      { ...scope, id: "c-text", event: "text_complete", runId: "continuation-run", timestampMs: 2001, data: { content: "Here is the answer" } },
      { ...scope, id: "c-end", event: "interaction_complete", runId: "continuation-run", timestampMs: 2003, data: {} },
    ],
  };
  const WORKING = "Queue agent is working";
  // The settlement's clock: each poll below runs a regular poll interval
  // after the previous one unless a test says otherwise.
  let nowMs = 1_000_000;
  const realNow = idleSettleClock.now;
  beforeEach(() => { nowMs = 1_000_000; idleSettleClock.now = () => nowMs; });
  afterEach(() => { idleSettleClock.now = realNow; });
  const BUSY = { response_phase: "waiting", run_state: "run_open" } as const;
  const IDLE = { response_phase: null, run_state: "idle" } as const;

  function harness(frames: ConsoleFrame[]) {
    const send = vi.fn(async (input: { identity: string }) => ({ interaction_id: "next-turn", identity: input.identity }));
    const fake = transport(send as never);
    let status: { response_phase: string | null; run_state: string } = { ...BUSY };
    const baseExperience = fake.loadExperience;
    const loadExperience = vi.fn(async () => {
      const base = await baseExperience();
      return {
        ...base,
        identity_status: { rows: [{ identity, state: "running", addressability: "addressable",
          progress: { run_state: status.run_state, in_flight_work: status.run_state === "idle" ? 0 : 1 } }] },
        agent_sidebar: { ...base.agent_sidebar, live_snapshot: { ...base.agent_sidebar?.live_snapshot,
          agents: base.agent_sidebar!.live_snapshot!.agents!.map(agent => ({ ...agent, response_phase: status.response_phase })) } },
      } as never;
    });
    fake.loadExperience = loadExperience;
    const live: { receive?: (frame: never) => void } = {};
    fake.subscribeTimeline = (_input, next) => { live.receive = next; return () => {}; };
    fake.queryTimeline = async () => ({ available: true, frames });
    let trigger = 0;
    const push = async (frame: Partial<ConsoleFrame>) => {
      await act(async () => { live.receive?.({ ...scope, data: {}, ...frame } as never); });
    };
    // One status poll, triggered by a frame that is not part of the
    // member's conversation (it does not enter the member's log).
    const poll = async (next: { response_phase: string | null; run_state: string }, gapMs = 15_000) => {
      nowMs += gapMs;
      status = { ...next };
      const before = loadExperience.mock.calls.length;
      trigger += 1;
      await act(async () => {
        live.receive?.({ id: `poll-${trigger}`, event: "state_changed", identity, timestampMs: 9_000_000 + trigger, data: {} } as never);
      });
      await waitFor(() => expect(loadExperience.mock.calls.length).toBeGreaterThan(before), { timeout: 2_000 });
      await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 50)); });
    };
    const start = async () => {
      render(<ConsoleApp baseUrl="" storageNamespace="runtime/realm/principal" transport={fake} />);
      await waitFor(() => expect(live.receive).toBeTypeOf("function"));
    };
    return { fake, send, loadExperience, push, poll, start };
  }
  const working = () => screen.queryByLabelText(WORKING);

  it.each(["paused", "idless"] as const)("Working clears on the second idle poll in a row, not the first (%s)", async (variant) => {
    const h = harness(stuckFrames[variant]);
    await h.start();
    await screen.findByLabelText(WORKING);
    await h.poll(BUSY);
    expect(working()).not.toBeNull();
    await h.poll(IDLE);
    expect(working()).not.toBeNull();
    await h.poll(IDLE);
    await waitFor(() => expect(working()).toBeNull());
  });

  it("two idle polls 150 ms apart do not settle; one a poll interval after the first does", async () => {
    const h = harness(stuckFrames.paused);
    await h.start();
    await screen.findByLabelText(WORKING);
    await h.poll(IDLE);
    // Another member's lifecycle event triggers a poll right away.
    await h.poll(IDLE, 150);
    expect(working()).not.toBeNull();
    await h.poll(IDLE, 150);
    expect(working()).not.toBeNull();
    await h.poll(IDLE, 15_000);
    await waitFor(() => expect(working()).toBeNull());
  });

  it.each(["paused", "idless"] as const)("a send queued behind the stale fold leaves once two polls report idle (%s)", async (variant) => {
    const h = harness(stuckFrames[variant]);
    await h.start();
    await screen.findByLabelText(WORKING);
    await compose("after the continuation");
    await screen.findByTestId("pending-stack");
    await h.poll(BUSY);
    await h.poll(IDLE);
    expect(h.send).not.toHaveBeenCalled();
    await h.poll(IDLE);
    await waitFor(() => expect(h.send).toHaveBeenCalledTimes(1), { timeout: 3_000 });
  });

  it("nothing closes while the server reports a live run (a response phase, or an open run)", async () => {
    const h = harness(stuckFrames.paused);
    await h.start();
    await screen.findByLabelText(WORKING);
    for (const live of [
      { response_phase: "waiting", run_state: "idle" },
      { response_phase: null, run_state: "run_open" },
      { response_phase: "generating", run_state: "run_open" },
    ]) {
      await h.poll(live);
      await h.poll(live);
      expect(working()).not.toBeNull();
    }
  });

  it("a same-id resume is unchanged: the paused turn stays Working and its own terminal settles it", async () => {
    const paused = stuckFrames.paused.slice(0, 5);
    const h = harness(paused);
    await h.start();
    await screen.findByLabelText(WORKING);
    // The server reports a paused interaction as not idle.
    for (let i = 0; i < 3; i++) await h.poll({ response_phase: "tool-executing", run_state: "idle" });
    expect(working()).not.toBeNull();
    // The host answered the callback; the SAME interaction resumes and ends.
    await h.push({ id: "r-start", event: "run_started", interactionId: "question", runId: "resumed-run", timestampMs: 3000, data: { input: { kind: "pending_tool_results" } } });
    await h.push({ id: "r-delta", event: "text_delta", interactionId: "question", runId: "resumed-run", timestampMs: 3001, data: { delta: "Done" } });
    expect(working()).not.toBeNull();
    await h.push({ id: "r-end", event: "interaction_complete", interactionId: "question", runId: "resumed-run", timestampMs: 3002, data: {} });
    await waitFor(() => expect(working()).toBeNull());
    await h.poll(IDLE);
    await h.poll(IDLE);
    await compose("a follow-up");
    await waitFor(() => expect(h.send).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId("pending-stack")).toBeNull();
  });

  it("a normal busy turn is unchanged: no idle poll settles it mid-turn", async () => {
    const h = harness([]);
    await h.start();
    await h.poll(IDLE);
    await compose("a normal question");
    await waitFor(() => expect(h.send).toHaveBeenCalledTimes(1));
    await h.push({ id: "t-input", event: "user_input", interactionId: "next-turn", status: "delivered", timestampMs: 5000, data: { content: "a normal question" } });
    await h.push({ id: "t-start", event: "run_started", interactionId: "next-turn", runId: "turn-run", timestampMs: 5001, data: {} });
    await screen.findByLabelText(WORKING);
    await h.poll(BUSY);
    await h.poll(BUSY);
    expect(working()).not.toBeNull();
    // A run before its first output can read idle once; a frame then
    // arrives before the next poll, so the count starts again.
    await h.poll(IDLE);
    await h.push({ id: "t-delta", event: "text_delta", interactionId: "next-turn", runId: "turn-run", timestampMs: 5002, data: { delta: "Thinking" } });
    await h.poll(IDLE);
    expect(working()).not.toBeNull();
    await h.push({ id: "t-end", event: "interaction_complete", interactionId: "next-turn", runId: "turn-run", timestampMs: 5003, data: {} });
    await waitFor(() => expect(working()).toBeNull());
  });

  it("a runtime-started run with no output yet is not settled by the first idle poll, only by the second", async () => {
    const h = harness([]);
    await h.start();
    await h.push({ id: "rt-start", event: "run_started", runId: "runtime-run", timestampMs: 6000, data: {} });
    await screen.findByLabelText(WORKING);
    await h.poll(IDLE);
    expect(working()).not.toBeNull();
    await h.poll(IDLE);
    await waitFor(() => expect(working()).toBeNull());
  });

  it("a rebuild of the fold after reconciliation stays idle", async () => {
    const h = harness(stuckFrames.paused);
    await h.start();
    await screen.findByLabelText(WORKING);
    await h.poll(IDLE);
    await h.poll(IDLE);
    await waitFor(() => expect(working()).toBeNull());
    // A history snapshot replays the whole fold; a late frame of the settled
    // turn arrives out of order.
    await h.push({ id: "late-tool", event: "tool_execution_completed", interactionId: "question", runId: "question-run", timestampMs: 1600, data: { id: "setup", name: "setup_selected_install" } });
    await h.push({ id: "history", event: "assistant_history_snapshot", timestampMs: 7000, data: { complete: true, session_id: "owner-session" } });
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 50)); });
    expect(working()).toBeNull();
    // The rebuilt busy fold stays idle too: a new message is sent, not queued.
    await compose("after the rebuild");
    await waitFor(() => expect(h.send).toHaveBeenCalledTimes(1));
    expect(screen.queryByTestId("pending-stack")).toBeNull();
  });

  it("a frame between two idle polls starts the count again", async () => {
    const h = harness(stuckFrames.idless);
    await h.start();
    await screen.findByLabelText(WORKING);
    await h.poll(IDLE);
    await h.push({ identity, sourceKind: "console_event", id: "late-idless", event: "tool_result_received", timestampMs: 2500, data: {} });
    await h.poll(IDLE);
    expect(working()).not.toBeNull();
    await h.poll(IDLE);
    await waitFor(() => expect(working()).toBeNull());
  });

  it("a frame that arrives while the idle poll is in flight keeps the fold", async () => {
    const h = harness(stuckFrames.paused);
    await h.start();
    await screen.findByLabelText(WORKING);
    await h.poll(IDLE);
    let answer!: (value: never) => void;
    const idle = await h.loadExperience();
    h.loadExperience.mockImplementationOnce(() => new Promise(resolve => { answer = resolve as never; }));
    const before = h.loadExperience.mock.calls.length;
    await h.push({ id: "trigger-idle", event: "state_changed", timestampMs: 9_500_000, data: {} });
    await waitFor(() => expect(h.loadExperience.mock.calls.length).toBeGreaterThan(before), { timeout: 2_000 });
    // A new turn's start arrives before the poll answers.
    await h.push({ id: "next-start", event: "run_started", interactionId: "next", runId: "next-run", timestampMs: 4000, data: {} });
    await act(async () => { answer(idle); await new Promise((resolve) => window.setTimeout(resolve, 50)); });
    expect(working()).not.toBeNull();
  });
});
