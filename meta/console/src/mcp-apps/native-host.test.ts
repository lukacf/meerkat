import { afterEach, describe, expect, it, vi } from "vitest";
import { createNativeMcpAppsHost } from "./native-host";

const locator = { identity: "member:one", sessionId: "session:one", toolCallId: "call:one" };
const options = { baseUrl: "", sandboxProxyUrl: "https://sandbox.test/sandbox.html", authority: "viewer:one", runtimeId: "runtime:one" };
const original = () => ({
  tool: { name: "find", inputSchema: { type: "object" }, _meta: { ui: { resourceUri: "ui://example/result" } } },
  arguments: { query: "records" },
  result: { content: [{ type: "text", text: "Seven records" }], structuredContent: { count: 7 }, _meta: { privateDetail: "Do not persist in browser storage" } },
  resource: { contents: [{ uri: "ui://example/result", mimeType: "text/html;profile=mcp-app", text: "<html>Original renderer</html>", _meta: { ui: { csp: { connectDomains: [] } } } }] },
  canCallTools: true,
});
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { "Content-Type": "application/json" } });
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

describe("stock native MCP Apps adapter", () => {
  it("uses authenticated fixed endpoints and replays full retained data without repeating the original tool", async () => {
    const resolveResult = original();
    const refreshResult = { content: [{ type: "text", text: "Eight records" }], _meta: { refreshed: true } };
    const fetch = vi.fn().mockResolvedValueOnce(json(resolveResult)).mockResolvedValueOnce(json(resolveResult.resource)).mockResolvedValueOnce(json(refreshResult));
    vi.stubGlobal("fetch", fetch);
    const host = createNativeMcpAppsHost(options);
    const controller = new AbortController();
    const mutableLocator = { ...locator };
    const view = (await host.resolve(mutableLocator, controller.signal))!;
    mutableLocator.identity = "other-member";
    expect(view.result).toEqual(resolveResult.result);
    expect(await view.readResource("ui://example/result", controller.signal)).toEqual(resolveResult.resource);
    expect(fetch).toHaveBeenCalledTimes(1);
    await view.readResource("ui://example/result", controller.signal);
    expect(await view.callTool!("refresh", { page: 2 }, controller.signal)).toEqual(refreshResult);
    expect(fetch.mock.calls.map(([url]) => new URL(url).pathname)).toEqual([
      "/console/mcp-apps/resolve", "/console/mcp-apps/read-resource", "/console/mcp-apps/call-tool",
    ]);
    expect(fetch.mock.calls.map(([, init]) => JSON.parse(init.body))).toEqual([
      locator, { ...locator, uri: "ui://example/result" }, { ...locator, name: "refresh", arguments: { page: 2 } },
    ]);
    for (const [, init] of fetch.mock.calls) {
      expect(init).toMatchObject({ method: "POST", credentials: "same-origin", redirect: "error", cache: "no-store" });
      expect(JSON.parse(decodeURIComponent(init.headers["X-Console-Authority"]))).toEqual({ key: "viewer:one", runtimeId: "runtime:one" });
      expect(JSON.parse(decodeURIComponent(init.headers["X-Console-Conversation"]))).toEqual({ scopeKey: "viewer:one", identity: locator.identity });
    }
    expect(localStorage.length).toBe(0);
  });

  it("rejects cross-origin gateways, same-origin sandboxes and empty locators", async () => {
    const fetch = vi.fn(); vi.stubGlobal("fetch", fetch);
    expect(() => createNativeMcpAppsHost({ ...options, baseUrl: "https://other.test" })).toThrow(/authenticated origin/);
    expect(() => createNativeMcpAppsHost({ ...options, sandboxProxyUrl: `${window.location.origin}/sandbox.html` })).toThrow(/separate/);
    const host = createNativeMcpAppsHost(options);
    await expect(host.resolve({ ...locator, toolCallId: "" }, new AbortController().signal)).rejects.toThrow(/invocation/);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("keeps authority cancellation and rejects a late resolve response", async () => {
    let finish: (value: Response) => void;
    const fetch = vi.fn(() => new Promise<Response>(done => { finish = done; })); vi.stubGlobal("fetch", fetch);
    const host = createNativeMcpAppsHost(options);
    const controller = new AbortController();
    const pending = host.resolve(locator, controller.signal);
    controller.abort(); finish!(json(original()));
    await expect(pending).rejects.toThrow();
    expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
    await expect(host.resolve(locator, controller.signal)).rejects.toThrow();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("never retries failed actions", async () => {
    const fetch = vi.fn().mockResolvedValueOnce(json(original())).mockResolvedValueOnce(json({ error: "denied" }, 403));
    vi.stubGlobal("fetch", fetch);
    const signal = new AbortController().signal;
    const view = (await createNativeMcpAppsHost(options).resolve(locator, signal))!;
    await expect(view.callTool!("refresh", {}, signal)).rejects.toThrow(/403/);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("reads the declared resource live when resolve has no retained HTML", async () => {
    const response = original();
    const fetch = vi.fn().mockResolvedValueOnce(json({ ...response, resource: null })).mockResolvedValueOnce(json(response.resource));
    vi.stubGlobal("fetch", fetch);
    const signal = new AbortController().signal;
    const view = (await createNativeMcpAppsHost(options).resolve(locator, signal))!;
    expect(await view.readResource("ui://example/result", signal)).toEqual(response.resource);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it.each([{ canCallTools: false, readOnly: false }, { canCallTools: true, readOnly: true }])("omits actions when the view cannot act: %j", async ({ canCallTools, readOnly }) => {
    vi.stubGlobal("fetch", vi.fn(async () => json({ ...original(), canCallTools })));
    const view = (await createNativeMcpAppsHost({ ...options, readOnly }).resolve(locator, new AbortController().signal))!;
    expect(view.callTool).toBeUndefined();
  });

  it("invalidates retained HTML and live operations when the view is disposed", async () => {
    const fetch = vi.fn(async () => json(original())); vi.stubGlobal("fetch", fetch);
    const signal = new AbortController().signal;
    const view = (await createNativeMcpAppsHost(options).resolve(locator, signal))!;
    view.dispose!();
    await expect(view.readResource("ui://example/result", signal)).rejects.toThrow(/closed/);
    await expect(view.callTool!("refresh", {}, signal)).rejects.toThrow(/closed/);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("bounds setup requests and discards bodies delivered after timeout", async () => {
    vi.useFakeTimers();
    let finish: (value: Response) => void;
    const fetch = vi.fn(() => new Promise<Response>(done => { finish = done; })); vi.stubGlobal("fetch", fetch);
    const pending = createNativeMcpAppsHost({ ...options, requestTimeoutMs: 10 }).resolve(locator, new AbortController().signal);
    await vi.advanceTimersByTimeAsync(10);
    expect(fetch.mock.calls[0][1].signal.aborted).toBe(true);
    finish!(json(original()));
    await expect(pending).rejects.toThrow(/timed out/);
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
