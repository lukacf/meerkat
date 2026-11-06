import { afterEach, describe, expect, it, vi } from "vitest";
import { createHttpConsoleTransport } from "./headless";
import { createHttpConsoleTransport as createCoreHttpConsoleTransport } from "../../../packages/console-core/src/headless";
import { createNativeMcpAppsHost } from "../mcp-apps/native-host";

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

  it("names the caller of its latest whoami read, and keeps it when a read fails", async () => {
    const { fetch, seen } = gateway();
    vi.stubGlobal("fetch", fetch);
    const transport = create({ baseUrl: "" });
    const retire = () => transport.executeCommand!({ command: "retireIdentity", target: { kind: "meta/chat", identity: "member" } as never, params: { identity: "member" } });
    await retire();
    // Someone else signs in to this browser; a console that loads again
    // reads who it acts as now, and its changes name that caller.
    fetch.mockImplementationOnce(async (_url: string, init: RequestInit = {}) => {
      const request = JSON.parse(String(init.body));
      seen.push({ method: request.method, params: request.params ?? {} });
      return json({ jsonrpc: "2.0", id: request.id, result: { actor: { kind: "person", principal: "b@example.test", assurance: "person_authenticated" }, caller_scope: "caller-b" } });
    });
    expect((await transport.whoami!()).callerScope).toBe("caller-b");
    await retire();
    fetch.mockImplementationOnce(async () => new Response("unavailable", { status: 503 }));
    await expect(transport.whoami!()).rejects.toBeTruthy();
    await retire();
    const retires = seen.filter((request) => request.method === "meta/retire");
    expect(retires.map((request) => request.params.expected_caller_scope)).toEqual(["caller-a", "caller-b", "caller-b"]);
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
});
