import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleCommandRequest, ConsoleWhoami, MetaConsoleTransport } from "./headless";
import { useVoiceController } from "./use-voice-controller";
import { createNativeMcpAppsHost } from "../mcp-apps/native-host";
import { createConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, saveConsoleSendAttempts } from "./send-attempt-storage";

// The console's caller belongs to the console instance: one whoami read per
// instance, never rebound by a later answer, named on every change (voice and
// MCP Apps calls included), and no change leaves while the caller is unknown.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const identity = "identity:binding-agent";
type Caller = "a" | "b";
function experienceFor(caller: Caller, extra: Record<string, unknown> = {}) {
  return {
    runtime_id: "binding-test",
    storage_scope: `console-storage:v1:${caller}`,
    console_config: { layout: { initial_agent: identity }, mcp_apps_sandbox_url: "https://sandbox.test/sandbox.html" },
    agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Binding agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
    ...extra,
  };
}
const whoamiFor = (caller: Caller): ConsoleWhoami => ({
  actorKind: "person", principal: `${caller}@example.test`, approverIds: [], canAdminister: false, callerScope: `caller-${caller}`,
});
function transport(whoami: () => Promise<ConsoleWhoami>, send: MetaConsoleTransport["send"] = vi.fn(async (input) => ({ interaction_id: "sent", identity: input.identity }))): MetaConsoleTransport {
  return {
    loadExperience: vi.fn(async () => experienceFor("a")) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline"] }) as never,
    whoami: vi.fn(whoami),
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {},
    send,
    executeCommand: async () => ({ result: {} }) as never,
  };
}
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });
async function compose(text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
}
/** The caller getter and failure report the console hands its voice controller. */
const voiceSeams = () => {
  const call = vi.mocked(useVoiceController).mock.lastCall!;
  return { callerScope: call[1] as unknown as () => unknown, reportFailure: (call as unknown[])[2] as ((error: unknown) => void) | undefined };
};
/** The options the console hands its MCP Apps host. */
const mcpOptions = () => vi.mocked(createNativeMcpAppsHost).mock.lastCall![0] as unknown as {
  callerScope: () => unknown; reportFailure?: (error: unknown) => void;
};
const settled = async (value: unknown) => {
  let state = "pending";
  void Promise.resolve(value).then(() => { state = "resolved"; }, () => { state = "rejected"; });
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  return state;
};

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  vi.mocked(useVoiceController).mockClear();
  vi.mocked(createNativeMcpAppsHost).mockClear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.unstubAllGlobals();
});

describe("the caller is bound to the console instance", () => {
  it("reads whoami once per console instance", async () => {
    const fake = transport(async () => whoamiFor("a"));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle(500);
    expect(vi.mocked(fake.whoami!)).toHaveBeenCalledTimes(1);
  });

  it("a later whoami answer for another caller never rebinds the instance", async () => {
    let reads = 0;
    const fake = transport(async () => whoamiFor(++reads === 1 ? "a" : "b"));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle(500);
    expect(screen.getByTestId("identity-chip")).toHaveTextContent("a@example.test");
    expect(screen.getByTestId("identity-chip")).not.toHaveTextContent("b@example.test");
  });

  it("a send typed in a's console never goes out as b (the stock transport)", async () => {
    let whoamiReads = 0;
    const sends: Array<Record<string, unknown>> = [];
    const json = (value: unknown) => new Response(JSON.stringify(value), { status: 200, headers: { "content-type": "application/json" } });
    vi.stubGlobal("fetch", vi.fn(async (input: string, init: RequestInit = {}) => {
      const url = String(input);
      if (url.startsWith("/console/experience")) return json(experienceFor("a", { console_config: { layout: { initial_agent: identity } } }));
      if (url.startsWith("/console/modules")) return json({ modules: [] });
      if (url.startsWith("/console/timeline/stream")) {
        return new Response(new ReadableStream({ start() {} }), { status: 200, headers: { "content-type": "text/event-stream" } });
      }
      if (url.startsWith("/console/rpc")) {
        const request = JSON.parse(String(init.body));
        const answer = (result: unknown) => json({ jsonrpc: "2.0", id: request.id, result });
        switch (request.method) {
          case "meta/console/whoami": {
            const who = ++whoamiReads === 1 ? "a" : "b";
            return answer({ actor: { kind: "person", principal: `${who}@example.test`, assurance: "person_authenticated" }, posture: "enforced", can_administer: false, approver_ids: [], caller_scope: `caller-${who}` });
          }
          case "meta/capabilities":
            return answer({ version: "test", methods: ["meta/console/send", "meta/console/query_timeline", "meta/console/whoami"] });
          case "meta/console/query_timeline":
            return answer({ frames: [], available: true });
          case "meta/console/send":
            sends.push(request.params);
            return answer({ interaction_id: "sent", identity });
          default:
            return answer({});
        }
      }
      return new Response("not found", { status: 404 });
    }));
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle(500);
    await compose("Typed in a's console");
    await waitFor(() => expect(sends).toHaveLength(1), { timeout: 3_000 });
    expect(sends[0].expected_caller_scope).toBe("caller-a");
  });

  it("names the instance's caller on its commands", async () => {
    const fake = transport(async () => whoamiFor("a"));
    fake.capabilities = async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/gating/pending", "meta/gating/decide"] }) as never;
    const executeCommand = vi.fn(async (input: ConsoleCommandRequest) => ({ command: input.command, accepted: true, result: { pending: [] } }) as never);
    fake.executeCommand = executeCommand;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await waitFor(() => expect(executeCommand).toHaveBeenCalled());
    expect(executeCommand.mock.calls.map(([input]) => input.expectedCallerScope)).toEqual(executeCommand.mock.calls.map(() => "caller-a"));
  });

  it("sends nothing while every whoami read fails: the message reads Not sent", async () => {
    const send = vi.fn(async (input: Parameters<MetaConsoleTransport["send"]>[0]) => ({ interaction_id: "sent", identity: input.identity }));
    const fake = transport(async () => { throw Object.assign(new Error("whoami unavailable"), { httpStatus: 503 }); }, send);
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle();
    await compose("Nobody knows who sends this");
    await settle(500);
    expect(send).not.toHaveBeenCalled();
    expect(await screen.findByText("Not sent")).toBeVisible();
  });

  it("reads a failed whoami again with its next experience load, and then names that caller", async () => {
    let reads = 0;
    const send = vi.fn(async (input: Parameters<MetaConsoleTransport["send"]>[0]) => ({ interaction_id: "sent", identity: input.identity }));
    const fake = transport(async () => {
      if (++reads === 1) throw Object.assign(new Error("whoami unavailable"), { httpStatus: 503 });
      return whoamiFor("a");
    }, send);
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next as never; return () => {}; };
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await settle();
    expect(reads).toBe(1);
    // A topology change loads the experience again: the caller is read with it.
    await act(async () => { receive?.({ id: "refresh", event: "topology_updated", timestampMs: 2, data: {} } as never); });
    await waitFor(() => expect(screen.getByTestId("identity-chip")).toHaveTextContent("a@example.test"));
    expect(reads).toBe(2);
    await compose("Sent once the caller is known");
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    expect(send.mock.calls[0][0].expectedCallerScope).toBe("caller-a");
  });
});

describe("while the console knows no caller", () => {
  it("keeps a queued message queued, and drains it naming the caller once a read binds", async () => {
    const namespace = JSON.stringify(["", "console-storage:v1:a"]);
    saveConsoleSendAttempts(window.localStorage, namespace, identity, [createConsoleSendAttempt({
      id: "queued-a", scope: namespace, destination: identity, origin: "console:pane", idempotencyKey: "key-a", text: "Queued by a", now: 1,
    })]);
    const savedStates = () => (JSON.parse(window.localStorage.getItem(consoleSendStorageKey(namespace, identity)) ?? "{\"attempts\":[]}").attempts as Array<{ state: string; failureKind?: string }>)
      .map((attempt) => `${attempt.state}${attempt.failureKind ? `/${attempt.failureKind}` : ""}`);
    let failing = true;
    const send = vi.fn(async (input: Parameters<MetaConsoleTransport["send"]>[0]) => ({ interaction_id: "sent", identity: input.identity }));
    const fake = transport(async () => {
      if (failing) throw Object.assign(new Error("whoami unavailable"), { httpStatus: 503 });
      return whoamiFor("a");
    }, send);
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next as never; return () => {}; };
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await settle(1_000);
    // The agent is idle, but no caller is known: nothing is sent, and the
    // message stays queued instead of turning into Not sent.
    expect(send).not.toHaveBeenCalled();
    expect(savedStates()).toEqual(["draft"]);
    // A read answers with the next experience load: the message drains,
    // naming the caller.
    failing = false;
    await act(async () => { receive?.({ id: "refresh", event: "topology_updated", timestampMs: 2, data: {} } as never); });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1), { timeout: 3_000 });
    expect(send.mock.calls[0][0].expectedCallerScope).toBe("caller-a");
  });

  it("loads again at once when another tab signs in", async () => {
    const state: { caller: Caller; failing: boolean } = { caller: "a", failing: true };
    const fake = transport(async () => {
      if (state.failing) throw Object.assign(new Error("whoami unavailable"), { httpStatus: 503 });
      return whoamiFor(state.caller);
    });
    fake.loadExperience = vi.fn(async () => experienceFor(state.caller, {
      agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: `Agent seen by ${state.caller}`, kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
    })) as never;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await waitFor(() => expect(screen.queryAllByText("Agent seen by a").length).toBeGreaterThan(0));
    await settle();
    // b signs in in another tab of this browser, which announces b.
    state.caller = "b"; state.failing = false;
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: "meta-console-caller:v1:", newValue: JSON.stringify({ caller: "caller-b", at: Date.now() }) }));
    });
    await settle(1_000);
    // a's views are gone without waiting for the next experience read.
    expect(screen.queryAllByText("Agent seen by a")).toHaveLength(0);
    await waitFor(() => expect(screen.getByTestId("identity-chip")).toHaveTextContent("b@example.test"));
  });
});

describe("voice and MCP Apps calls name the instance's caller", () => {
  it("wait for the console's caller before they are made", async () => {
    let answer!: (value: ConsoleWhoami) => void;
    const fake = transport(() => new Promise((resolve) => { answer = resolve; }));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(createNativeMcpAppsHost).toHaveBeenCalled());
    const voice = voiceSeams().callerScope();
    const mcp = mcpOptions().callerScope();
    expect(voice).toBeInstanceOf(Promise);
    expect(mcp).toBeInstanceOf(Promise);
    expect(await settled(voice)).toBe("pending");
    expect(await settled(mcp)).toBe("pending");
    await act(async () => { answer(whoamiFor("a")); });
    await expect(voice).resolves.toBe("caller-a");
    await expect(mcp).resolves.toBe("caller-a");
  });

  it("are refused while the caller is unknown", async () => {
    const fake = transport(async () => { throw Object.assign(new Error("whoami unavailable"), { httpStatus: 503 }); });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(createNativeMcpAppsHost).toHaveBeenCalled());
    await settle();
    expect(await settled(voiceSeams().callerScope())).toBe("rejected");
    expect(await settled(mcpOptions().callerScope())).toBe("rejected");
  });

  it("a voice request refused with an ended login enters the sign-in state", async () => {
    render(<ConsoleApp baseUrl="" transport={transport(async () => whoamiFor("a"))} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    const { reportFailure } = voiceSeams();
    expect(typeof reportFailure).toBe("function");
    act(() => { reportFailure!(Object.assign(new Error("meta/console/voice/open request failed 401"), { httpStatus: 401 })); });
    expect(await screen.findByTestId("console-sign-in")).toBeVisible();
  });

  it("an MCP Apps tool call refused for another caller loads the console again at once", async () => {
    const fake = transport(async () => whoamiFor("a"));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(createNativeMcpAppsHost).toHaveBeenCalled());
    await settle();
    const { reportFailure } = mcpOptions();
    expect(typeof reportFailure).toBe("function");
    const reads = vi.mocked(fake.loadExperience).mock.calls.length;
    act(() => { reportFailure!(Object.assign(new Error("MCP App request failed (HTTP 409)"), { httpStatus: 409, kind: "caller_changed" })); });
    await waitFor(() => expect(vi.mocked(fake.loadExperience).mock.calls.length).toBeGreaterThan(reads));
  });
});
