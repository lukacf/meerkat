import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleWhoami, MetaConsoleTransport } from "./headless";
import { createConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, saveConsoleSendAttempts } from "./send-attempt-storage";

// The browser's identity lifecycle: who the console acts as, a user switch
// noticed at once in every tab, and one sign-in state for an ended login.

const identity = "identity:switch-agent";
/** The record a tab writes when it learns who this gateway's console acts as
 * (base URL "" here, the console's own origin). */
const CALLER_KEY = "meta-console-caller:v1:";
type Caller = "a" | "b";

function experienceFor(caller: Caller, extra: Record<string, unknown> = {}) {
  return {
    runtime_id: "switch-test",
    storage_scope: `console-storage:v1:${caller}`,
    console_config: { layout: { initial_agent: identity } },
    agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Switch agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
    ...extra,
  };
}
const whoamiFor = (caller: Caller): ConsoleWhoami => ({
  actorKind: "person", principal: `${caller}@example.test`, approverIds: [], canAdminister: false, callerScope: `caller-${caller}`,
});
/** The default embedded scope the console keeps a caller's queue under. */
const namespaceOf = (caller: Caller) => JSON.stringify(["", `console-storage:v1:${caller}`]);
const busyHistory = { available: true, frames: [{ id: "busy", event: "interaction_started", identity, interactionId: "busy", timestampMs: 1, data: { content: "working" } }] };

function transport(state: { caller: Caller }, send: MetaConsoleTransport["send"] = vi.fn()) {
  let receive: ((frame: never) => void) | undefined;
  let streamState: ((state: unknown) => void) | undefined;
  const fake: MetaConsoleTransport = {
    loadExperience: vi.fn(async () => experienceFor(state.caller)) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline"] }) as never,
    whoami: vi.fn(async () => whoamiFor(state.caller)),
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: (input, next, options) => {
      receive = next as never;
      streamState = (value) => { input.onTransportState?.(value as never); options?.onTransportState?.(value as never); };
      return () => {};
    },
    send,
    executeCommand: async () => ({ result: {} }) as never,
  };
  return {
    fake,
    frame: (frame: Record<string, unknown>) => receive?.(frame as never),
    streamState: (value: Record<string, unknown>) => streamState?.(value),
    streaming: () => typeof receive === "function",
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

/** Another tab of the same gateway's console learned who it acts as. */
function announceFromAnotherTab(caller: string | null) {
  act(() => {
    window.dispatchEvent(new StorageEvent("storage", { key: CALLER_KEY, newValue: JSON.stringify({ caller, at: Date.now() }) }));
  });
}

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("user switch", () => {
  it("user_switch_clears_views_and_queue_now", async () => {
    const state: { caller: Caller } = { caller: "a" };
    const { fake } = transport(state);
    fake.queryTimeline = async () => busyHistory as never;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await compose("Queued while the agent works, by a");
    expect(within(await screen.findByTestId("pending-stack")).getByText("Queued while the agent works, by a")).toBeVisible();
    const composer = screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
    fireEvent.change(composer, { target: { value: "Unsent draft of a" } });
    await waitFor(() => expect(fake.whoami).toHaveBeenCalled());
    const experienceReads = vi.mocked(fake.loadExperience).mock.calls.length;
    // b signs in in another tab of this browser: that tab announces it.
    state.caller = "b";
    announceFromAnotherTab("caller-b");
    // At once, not at the next 15 s experience poll.
    await waitFor(() => expect(screen.queryByText("Queued while the agent works, by a")).toBeNull(), { timeout: 1_000 });
    expect(vi.mocked(fake.loadExperience).mock.calls.length).toBeGreaterThan(experienceReads);
    await waitFor(() => expect((screen.getByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement).value).toBe(""), { timeout: 1_000 });
    // a's queue stays in a's own scope, unsent.
    const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey(namespaceOf("a"), identity))!);
    expect(saved.attempts.map((attempt: { text: string; state: string }) => [attempt.text, attempt.state]))
      .toEqual([["Queued while the agent works, by a", "draft"]]);
  });

  it("auto_drain_rereads_the_experience_first", async () => {
    const draft = createConsoleSendAttempt({ id: "persisted-a", scope: namespaceOf("a"), destination: identity, origin: "console:pane", idempotencyKey: "persisted-key", text: "Persisted draft of a", now: 1 });
    saveConsoleSendAttempts(window.localStorage, namespaceOf("a"), identity, [draft]);
    const send = vi.fn(async (input: { identity: string }) => ({ interaction_id: "sent", identity: input.identity }));
    const state: { caller: Caller } = { caller: "a" };
    const { fake } = transport(state, send as never);
    // The console starts as a; by the time the queue would drain, b holds
    // this browser's login.
    fake.loadExperience = vi.fn()
      .mockResolvedValueOnce(experienceFor("a"))
      .mockResolvedValue(experienceFor("b"));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 300)); });
    expect(send).not.toHaveBeenCalled();
    expect(vi.mocked(fake.loadExperience).mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("auto_drain_sends_after_the_experience_confirms_the_caller", async () => {
    const draft = createConsoleSendAttempt({ id: "persisted-a", scope: namespaceOf("a"), destination: identity, origin: "console:pane", idempotencyKey: "persisted-key", text: "Persisted draft of a", now: 1 });
    saveConsoleSendAttempts(window.localStorage, namespaceOf("a"), identity, [draft]);
    const state: { caller: Caller } = { caller: "a" };
    const { fake } = transport(state);
    const readsAtSend: number[] = [];
    fake.send = vi.fn(async (input) => {
      readsAtSend.push(vi.mocked(fake.loadExperience).mock.calls.length);
      return { interaction_id: "sent", identity: input.identity };
    });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await waitFor(() => expect(fake.send).toHaveBeenCalledTimes(1));
    expect(vi.mocked(fake.send).mock.calls[0][0]).toMatchObject({ idempotencyKey: "persisted-key", content: "Persisted draft of a" });
    // The experience was read again for the drain, after the console's own load.
    expect(readsAtSend[0]).toBeGreaterThanOrEqual(2);
  });

  it("caller_changed_refusal_switches_at_once", async () => {
    const state: { caller: Caller } = { caller: "a" };
    const send = vi.fn(async () => {
      state.caller = "b";
      throw Object.assign(new Error("meta/console/send RPC error: this console change was made for another caller"), {
        rpcError: { code: -32056, message: "this console change was made for another caller; read meta/console/whoami again", data: { kind: "caller_changed" } },
      });
    });
    const { fake } = transport(state, send as never);
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await waitFor(() => expect(fake.whoami).toHaveBeenCalled());
    const experienceReads = vi.mocked(fake.loadExperience).mock.calls.length;
    await compose("Made as a");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    // The server refused the send before anything was sent: a's row says so.
    await waitFor(() => {
      const saved = JSON.parse(window.localStorage.getItem(consoleSendStorageKey(namespaceOf("a"), identity)) || "{\"attempts\":[]}");
      expect(saved.attempts.map((attempt: { state: string; failureKind?: string }) => attempt.state)).toEqual(["definitely-rejected"]);
    }, { timeout: 1_000 });
    // And the console reads who it acts as again, at once.
    await waitFor(() => expect(vi.mocked(fake.loadExperience).mock.calls.length).toBeGreaterThan(experienceReads), { timeout: 1_000 });
    await waitFor(() => expect(screen.queryByText("Made as a")).toBeNull(), { timeout: 1_000 });
  });

  it("host_items_cleared_on_principal_change", async () => {
    const state: { caller: Caller } = { caller: "a" };
    const { fake } = transport(state);
    fake.loadExperience = vi.fn(async () => experienceFor(state.caller, {
      console_config: { layout: { initial_agent: identity }, attention: { sources: [{ id: "host", label: "Host", url: "/host/attention" }] } },
    })) as never;
    // The host's source answers for whoever this browser's login is now.
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ items: [
      { id: `item-${state.caller}`, title: `Waiting for ${state.caller}`, href: `/host/items/${state.caller}` },
    ] }), { status: 200, headers: { "content-type": "application/json" } })));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    fireEvent.click(await screen.findByRole("button", { name: /^Needs you/ }));
    expect(await screen.findByTestId("host-attention:host:item-a")).toBeVisible();
    await waitFor(() => expect(fake.whoami).toHaveBeenCalled());
    state.caller = "b";
    announceFromAnotherTab("caller-b");
    // a's host items go at once, not at the next 30 s source read.
    await waitFor(() => expect(screen.queryByTestId("host-attention:host:item-a")).toBeNull(), { timeout: 1_000 });
  });
});

describe("one sign-in state", () => {
  type Trigger = "stream" | "approvals" | "experience";
  it.each<Trigger>(["stream", "approvals", "experience"])("unauthorized_enters_sign_in_state (%s)", async (trigger) => {
    const state: { caller: Caller } = { caller: "a" };
    const handle = transport(state);
    const { fake } = handle;
    let refused = false;
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/gating/pending", "meta/gating/decide"] }) as never;
    fake.executeCommand = async (input) => {
      if (input.command === "listGatingPending") {
        if (trigger === "approvals" && refused) throw Object.assign(new Error("meta/gating/pending request failed 401"), { httpStatus: 401 });
        return { command: input.command, accepted: true, result: { pending: [{ pending_id: "gate-1", action_id: "action-1", action: "Publish the report", origin: { identity }, risk_tier: "r2" }] } } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    };
    const experience = vi.fn(async () => {
      if (trigger === "experience" && refused) throw Object.assign(new Error("Request failed 401 for /console/experience"), { httpStatus: 401 });
      return experienceFor(state.caller);
    });
    fake.loadExperience = experience as never;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`sidebar-agent:${identity}`);
    await waitFor(() => expect(handle.streaming()).toBe(true));
    // The login ends (it expired, or it was signed out in another tab).
    refused = true;
    await act(async () => {
      if (trigger === "stream") handle.streamState({ phase: "authentication-required", stale: true, freshness: "unknown", httpStatus: 401 });
      if (trigger === "approvals") fireEvent(document, new Event("visibilitychange"));
      if (trigger === "experience") handle.frame({ id: "refresh", event: "interaction_started", identity, interactionId: "refresh", timestampMs: 2, data: { content: "refresh" } });
    });
    // One sign-in state replaces the views: no stale data stays on screen.
    expect(await screen.findByTestId("console-sign-in", {}, { timeout: 2_000 })).toBeVisible();
    expect(screen.queryByTestId(`sidebar-agent:${identity}`)).toBeNull();
    expect(screen.queryByText("Approval access denied")).toBeNull();
    // Signing in again, in another tab, brings the console back.
    refused = false;
    announceFromAnotherTab("caller-a");
    expect(await screen.findByTestId(`sidebar-agent:${identity}`, {}, { timeout: 2_000 })).toBeTruthy();
    expect(screen.queryByTestId("console-sign-in")).toBeNull();
  });

  it("a console opened without a login shows the sign-in state", async () => {
    const { fake } = transport({ caller: "a" });
    fake.loadExperience = vi.fn(async () => { throw Object.assign(new Error("Request failed 401 for /console/experience"), { httpStatus: 401 }); }) as never;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    expect(await screen.findByTestId("console-sign-in")).toBeVisible();
    expect(screen.queryByTestId("console-error")).toBeNull();
  });
});

describe("identity chip", () => {
  it("identity_chip_names_the_caller_and_offers_sign_out", async () => {
    const { fake } = transport({ caller: "a" });
    fake.loadExperience = vi.fn(async () => experienceFor("a", {
      console_config: { layout: { initial_agent: identity }, sign_out: { href: "/logout", method: "post" } },
    })) as never;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const chip = await screen.findByTestId("identity-chip");
    await waitFor(() => expect(chip.textContent).toContain("a@example.test"));
    fireEvent.click(within(chip).getByRole("button", { name: /a@example\.test/ }));
    const signOut = await screen.findByRole("button", { name: "Sign out" });
    const form = signOut.closest("form")!;
    expect(form.getAttribute("method")).toBe("post");
    expect(form.getAttribute("action")).toBe("/logout");
  });

  it("names an open console's caller and offers no sign-out without a configured link", async () => {
    const { fake } = transport({ caller: "a" });
    fake.whoami = vi.fn(async () => ({ actorKind: "anonymous", approverIds: [], canAdminister: false, callerScope: "caller-open" }));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const chip = await screen.findByTestId("identity-chip");
    await waitFor(() => expect(chip.textContent).toMatch(/not signed in/i));
    expect(screen.queryByRole("button", { name: "Sign out" })).toBeNull();
  });
});
