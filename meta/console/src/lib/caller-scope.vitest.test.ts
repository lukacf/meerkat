import { afterEach, describe, expect, it, vi } from "vitest";
import { createHttpConsoleTransport } from "./headless";
import { createHttpConsoleTransport as createCoreHttpConsoleTransport } from "../../../packages/console-core/src/headless";
import { createNativeMcpAppsHost } from "../mcp-apps/native-host";
import { consoleCallerFailure } from "./console-caller";
import { createVoiceRpc } from "./voice-session";

// The stock console names the caller it acts as on every console request:
// `expected_caller_scope`, the `caller_scope` its `meta/console/whoami`
// read reported. A request the gateway serves for another caller is refused
// (`caller_changed`) instead of being made as that caller.

const json = (value: unknown) => new Response(JSON.stringify(value), { status: 200, headers: { "content-type": "application/json" } });

interface Seen { method: string; params: Record<string, unknown> }

function gateway() {
  const seen: Seen[] = [];
  const fetch = vi.fn(async (_url: string, init: RequestInit = {}) => {
    let request: { id?: unknown; method?: string; params?: Record<string, unknown> };
    if (init.body instanceof FormData) request = JSON.parse(String(init.body.get("payload")));
    else request = JSON.parse(String(init.body));
    seen.push({ method: String(request.method), params: request.params ?? {} });
    const answer = (result: unknown) => json({ jsonrpc: "2.0", id: request.id, result });
    switch (request.method) {
      case "meta/console/whoami":
        return answer({ actor: { kind: "person", principal: "a@example.test", assurance: "person_authenticated" }, posture: "enforced", login_expires_at_ms: null, can_administer: false, approver_ids: [], caller_scope: "caller-a" });
      case "meta/console/send":
        return answer({ interaction_id: "interaction", identity: "member" });
      case "meta/blob/upload":
        return answer({ blob_id: "sha256:blob" });
      default:
        return answer({});
    }
  });
  return { fetch, seen };
}

const transports = [
  ["console", createHttpConsoleTransport],
  ["console-core", createCoreHttpConsoleTransport],
] as const;

afterEach(() => { vi.unstubAllGlobals(); });

describe.each(transports)("every_console_request_names_its_caller (%s transport)", (_name, create) => {
  it("names the caller on commands, sends, multipart sends and uploads after whoami", async () => {
    const { fetch, seen } = gateway();
    vi.stubGlobal("fetch", fetch);
    const transport = create({ baseUrl: "" });
    expect((await transport.whoami!()).callerScope).toBe("caller-a");
    await transport.executeCommand!({ command: "retireIdentity", target: { kind: "meta/chat", identity: "member" } as never, params: { identity: "member" } });
    await transport.send({ identity: "member", content: "Hello", origin: "console:pane", idempotencyKey: "key-1" });
    const file = new File(["png"], "picture.png", { type: "image/png" });
    await transport.send({ identity: "member", content: "With a picture", origin: "console:pane", idempotencyKey: "key-2", attachments: [file] });
    await transport.upload!({ file, mediaType: "image/png" });
    const named = seen.filter((request) => request.method !== "meta/console/whoami");
    expect(named.map((request) => request.method)).toEqual(["meta/retire", "meta/console/send", "meta/console/send", "meta/blob/upload"]);
    for (const request of named) expect(request.params.expected_caller_scope, request.method).toBe("caller-a");
  });

  it("reads whoami before the first change it makes", async () => {
    const { fetch, seen } = gateway();
    vi.stubGlobal("fetch", fetch);
    const transport = create({ baseUrl: "" });
    await transport.send({ identity: "member", content: "First change", origin: "console:pane", idempotencyKey: "key-1" });
    expect(seen.map((request) => request.method)).toEqual(["meta/console/whoami", "meta/console/send"]);
    expect(seen[1].params.expected_caller_scope).toBe("caller-a");
  });

  it("names the caller of its first whoami answer: a later answer for another caller does not rebind it", async () => {
    const { fetch, seen } = gateway();
    vi.stubGlobal("fetch", fetch);
    const transport = create({ baseUrl: "" });
    const retire = () => transport.executeCommand!({ command: "retireIdentity", target: { kind: "meta/chat", identity: "member" } as never, params: { identity: "member" } });
    await retire();
    // Someone else signs in to this browser. The transport keeps acting for
    // a: a change it makes now is refused by the gateway, never made as b.
    fetch.mockImplementationOnce(async (_url: string, init: RequestInit = {}) => {
      const request = JSON.parse(String(init.body));
      seen.push({ method: request.method, params: request.params ?? {} });
      return json({ jsonrpc: "2.0", id: request.id, result: { actor: { kind: "person", principal: "b@example.test", assurance: "person_authenticated" }, caller_scope: "caller-b" } });
    });
    expect((await transport.whoami!()).callerScope).toBe("caller-b");
    await retire();
    const retires = seen.filter((request) => request.method === "meta/retire");
    expect(retires.map((request) => request.params.expected_caller_scope)).toEqual(["caller-a", "caller-a"]);
  });

  it("refuses a change made while no caller is known, and sends nothing", async () => {
    const seen: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit = {}) => {
      const request = JSON.parse(String(init.body));
      seen.push(request.method);
      if (request.method === "meta/console/whoami") return new Response("unavailable", { status: 503 });
      return json({ jsonrpc: "2.0", id: request.id, result: { interaction_id: "interaction", identity: "member" } });
    }));
    const transport = create({ baseUrl: "" });
    await expect(transport.send({ identity: "member", content: "Sent without a known caller", origin: "console:pane", idempotencyKey: "key-1" }))
      .rejects.toMatchObject({ transportFailure: "caller_unknown" });
    expect(seen.filter((method) => method !== "meta/console/whoami")).toEqual([]);
  });

  it("names no caller on a gateway without whoami, and does not ask it again", async () => {
    const seen: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit = {}) => {
      const request = JSON.parse(String(init.body));
      seen.push(request.method);
      if (request.method === "meta/console/whoami") return json({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "method not found" } });
      expect(request.params?.expected_caller_scope).toBeUndefined();
      return json({ jsonrpc: "2.0", id: request.id, result: { interaction_id: "interaction", identity: "member" } });
    }));
    const transport = create({ baseUrl: "" });
    await transport.send({ identity: "member", content: "One", origin: "console:pane", idempotencyKey: "key-1" });
    await transport.send({ identity: "member", content: "Two", origin: "console:pane", idempotencyKey: "key-2" });
    expect(seen).toEqual(["meta/console/whoami", "meta/console/send", "meta/console/send"]);
  });
});

describe("MCP Apps tool calls name their caller", () => {
  it("sends expectedCallerScope on call-tool", async () => {
    const original = {
      tool: { name: "find", inputSchema: { type: "object" }, _meta: { ui: { resourceUri: "ui://example/result" } } },
      arguments: { query: "records" },
      result: { content: [{ type: "text", text: "Seven records" }] },
      resource: null,
      canCallTools: true,
    };
    const fetch = vi.fn()
      .mockResolvedValueOnce(json(original))
      .mockResolvedValueOnce(json({ content: [{ type: "text", text: "Eight records" }] }));
    vi.stubGlobal("fetch", fetch);
    const host = createNativeMcpAppsHost({
      baseUrl: "", sandboxProxyUrl: "https://sandbox.test/sandbox.html", authority: "viewer:one",
      callerScope: () => "caller-a",
    } as Parameters<typeof createNativeMcpAppsHost>[0]);
    const signal = new AbortController().signal;
    const view = (await host.resolve({ identity: "member:one", sessionId: "session:one", toolCallId: "call:one" }, signal))!;
    await view.callTool!("refresh", { page: 2 }, signal);
    const callTool = JSON.parse(fetch.mock.calls[1][1].body);
    expect(new URL(fetch.mock.calls[1][0]).pathname).toBe("/console/mcp-apps/call-tool");
    expect(callTool.expectedCallerScope).toBe("caller-a");
  });

  const appView = async (options: Record<string, unknown>, callToolResponse: () => Response) => {
    const original = {
      tool: { name: "find", inputSchema: { type: "object" }, _meta: { ui: { resourceUri: "ui://example/result" } } },
      arguments: { query: "records" }, result: { content: [{ type: "text", text: "Seven records" }] },
      resource: null, canCallTools: true,
    };
    const fetch = vi.fn(async (url: string) => (String(url).endsWith("/call-tool") ? callToolResponse() : json(original)));
    vi.stubGlobal("fetch", fetch);
    const host = createNativeMcpAppsHost({
      baseUrl: "", sandboxProxyUrl: "https://sandbox.test/sandbox.html", authority: "viewer:one", ...options,
    } as Parameters<typeof createNativeMcpAppsHost>[0]);
    const signal = new AbortController().signal;
    const view = (await host.resolve({ identity: "member:one", sessionId: "session:one", toolCallId: "call:one" }, signal))!;
    const callTools = () => fetch.mock.calls.filter(([url]) => String(url).endsWith("/call-tool"));
    return { call: () => view.callTool!("refresh", { page: 2 }, signal), callTools };
  };

  it("waits for the console's caller, and is refused while the caller is unknown", async () => {
    let answer!: (scope: string) => void;
    const waiting = await appView({ callerScope: () => new Promise<string>((resolve) => { answer = resolve; }) },
      () => json({ content: [{ type: "text", text: "Eight records" }] }));
    const call = waiting.call();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(waiting.callTools()).toHaveLength(0);
    answer("caller-a");
    await call;
    expect(JSON.parse(String(waiting.callTools()[0][1]!.body)).expectedCallerScope).toBe("caller-a");

    const unknown = await appView({ callerScope: async () => { throw new Error("caller unknown"); } },
      () => json({ content: [] }));
    await expect(unknown.call()).rejects.toBeTruthy();
    expect(unknown.callTools()).toHaveLength(0);
  });

  it.each([
    [409, "caller_changed", "caller_changed"],
    [401, "unauthorized", "sign_in"],
  ] as const)("reports a call-tool refused with HTTP %s as the console's %s", async (status, kind, failure) => {
    const reported: unknown[] = [];
    const view = await appView({ callerScope: async () => "caller-a", reportFailure: (error: unknown) => reported.push(error) },
      () => new Response(JSON.stringify({ error: kind, message: "refused" }), { status, headers: { "content-type": "application/json" } }));
    await expect(view.call()).rejects.toBeTruthy();
    expect(reported.map(consoleCallerFailure)).toEqual([failure]);
  });
});

describe("voice requests name the console's caller", () => {
  it("wait for the caller, are refused while it is unknown, and report a refused request", async () => {
    const seen: Array<Record<string, unknown>> = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit = {}) => {
      const request = JSON.parse(String(init.body));
      seen.push(request.params ?? {});
      if (request.method === "meta/console/voice/close") return new Response(JSON.stringify({ error: { code: -32600, message: "unauthorized", data: { kind: "unauthenticated" } } }), { status: 401 });
      return json({ jsonrpc: "2.0", id: request.id, result: {} });
    }));
    let answer!: (scope: string) => void;
    const reported: unknown[] = [];
    const rpc = createVoiceRpc("", () => new Promise<string>((resolve) => { answer = resolve; }), (error) => reported.push(error));
    const open = rpc("meta/console/voice/open", { identity: "member" }, 1_000);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(seen).toHaveLength(0);
    answer("caller-a");
    await open;
    expect(seen[0].expected_caller_scope).toBe("caller-a");
    await expect(rpc("meta/console/voice/close", { channel_id: "c" }, 1_000)).rejects.toBeTruthy();
    expect(reported.map(consoleCallerFailure)).toEqual(["sign_in"]);

    const refused = createVoiceRpc("", async () => { throw new Error("caller unknown"); }, () => {});
    await expect(refused("meta/console/voice/open", { identity: "member" }, 1_000)).rejects.toBeTruthy();
    expect(seen).toHaveLength(2);
  });
});

describe.each(transports)("blob uploads refused for the caller are typed (%s transport)", (_name, create) => {
  it.each([
    ["caller_changed", () => json({ jsonrpc: "2.0", id: "x", error: { code: -32056, message: "caller changed", data: { kind: "caller_changed" } } })],
    ["sign_in", () => new Response(JSON.stringify({ jsonrpc: "2.0", id: "x", error: { code: -32600, message: "unauthorized", data: { kind: "unauthenticated" } } }), { status: 401 })],
  ] as const)("an upload refused reads as the console's %s", async (failure, refusal) => {
    const { fetch } = gateway();
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit = {}) => (init.body instanceof FormData ? refusal() : fetch(url, init))));
    const transport = create({ baseUrl: "" });
    const file = new File(["png"], "picture.png", { type: "image/png" });
    const error = await transport.upload!({ file, mediaType: "image/png" }).then(() => null, (reason: unknown) => reason);
    expect(consoleCallerFailure(error)).toBe(failure);
  });
});
