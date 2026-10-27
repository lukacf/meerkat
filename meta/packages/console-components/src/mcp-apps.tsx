import React from "react";
import { AppBridge, getToolUiResourceUri, RESOURCE_MIME_TYPE } from "@modelcontextprotocol/ext-apps/app-bridge";
import { JSONRPCMessageSchema, ReadResourceResultSchema, CallToolResultSchema } from "@modelcontextprotocol/core";
import { type Transport, type JSONRPCMessage, type Tool, type CallToolResult, type ReadResourceResult, type Resource } from "@modelcontextprotocol/client";

/** Host-owned locator. A tool result cannot grant access by supplying this data. */
export interface McpAppLocator {
  identity: string;
  sessionId: string;
  toolCallId: string;
}

/** An invocation-bound adapter over the runtime's existing MCP connection. */
export interface ConsoleMcpAppSession {
  tool: Tool;
  arguments: Record<string, unknown>;
  /** Matching resources/list metadata, when supplied by the MCP server. */
  resource?: Resource;
  result: CallToolResult;
  readResource: (uri: string, signal: AbortSignal) => Promise<ReadResourceResult>;
  /** Omit when the viewer may not act. The server still enforces app visibility and admission. */
  callTool?: (name: string, args: Record<string, unknown>, signal: AbortSignal) => Promise<CallToolResult>;
  /** Dispose this view's subscriptions, never the shared MCP connection. */
  dispose?: () => void;
}

export interface ConsoleMcpAppsHost {
  /** A dedicated, isolated origin serving Console's sandbox proxy. */
  sandboxProxyUrl: string;
  /** Reauthorizes the viewer and original member, returning persisted result context without executing its tool. */
  resolve: (locator: McpAppLocator, signal: AbortSignal) => Promise<ConsoleMcpAppSession | null>;
}

export const ConsoleMcpAppsProviderContext = React.createContext<{
  host: ConsoleMcpAppsHost; authority: string; readOnly: boolean;
} | null>(null);

export const ConsoleMcpAppsProvider = ConsoleMcpAppsProviderContext.Provider;

/** Source and origin are both checked, including after an unexpected navigation. */
export class McpAppTransport implements Transport {
  onmessage?: Transport["onmessage"];
  onerror?: Transport["onerror"];
  onclose?: Transport["onclose"];
  private phase: "open" | "closing" | "closed" = "open";
  private readonly receive = (event: MessageEvent) => {
    if (this.phase === "closed" || event.source !== this.target || event.origin !== this.origin) return;
    if (event.data?.type === "console-mcp-app-retired") {
      this.beginTeardown();
      this.retired?.();
      return;
    }
    const parsed = JSONRPCMessageSchema.safeParse(event.data);
    if (parsed.success && (this.phase === "open" || !("method" in parsed.data))) this.onmessage?.(parsed.data);
  };
  constructor(private readonly target: Window, private readonly origin: string, private readonly retired?: () => void) {}
  async start() { window.addEventListener("message", this.receive); }
  async send(message: JSONRPCMessage) {
    if (this.phase === "closed" || (this.phase === "closing" && !("method" in message && message.method === "ui/resource-teardown"))) return;
    this.target.postMessage(message, this.origin);
  }
  /** Retain only teardown responses. Retired views cannot initiate more host work. */
  beginTeardown() { if (this.phase === "open") this.phase = "closing"; }
  async close() {
    if (this.phase === "closed") return;
    this.phase = "closed";
    window.removeEventListener("message", this.receive);
    this.onclose?.();
  }
}

export function mcpSandboxUrl(url: string, hostOrigin = window.location.origin): URL {
  const parsed = new URL(url);
  if (!["https:", "http:"].includes(parsed.protocol) || parsed.origin === hostOrigin || parsed.username || parsed.password || parsed.hash) {
    throw new Error("MCP Apps require a separate sandbox origin");
  }
  return parsed;
}

async function withAppRequest<T>(lifetime: AbortSignal, request: AbortSignal, run: (signal: AbortSignal) => Promise<T>): Promise<T> {
  const combined = new AbortController();
  const abort = () => combined.abort();
  lifetime.addEventListener("abort", abort, { once: true });
  request.addEventListener("abort", abort, { once: true });
  try {
    if (lifetime.aborted || request.aborted) combined.abort();
    combined.signal.throwIfAborted();
    const result = await run(combined.signal);
    combined.signal.throwIfAborted();
    return result;
  } finally {
    lifetime.removeEventListener("abort", abort);
    request.removeEventListener("abort", abort);
  }
}

export function ConsoleMcpAppView({ locator, fallback }: { locator: McpAppLocator; fallback: string }) {
  const state = React.useContext(ConsoleMcpAppsProviderContext);
  const container = React.useRef<HTMLDivElement>(null);
  const [status, setStatus] = React.useState<"loading" | "ready" | "unavailable">("loading");
  const key = JSON.stringify([state?.authority, locator.identity, locator.sessionId, locator.toolCallId, state?.readOnly]);
  React.useLayoutEffect(() => {
    if (!state || !container.current) { setStatus("unavailable"); return; }
    // Every binding owns a separate WindowProxy, including during bounded teardown.
    // Reusing an iframe lets a retired bridge receive a new binding's messages.
    const frame = document.createElement("iframe");
    frame.title = "Interactive tool result";
    frame.setAttribute("sandbox", "allow-scripts allow-same-origin");
    frame.referrerPolicy = "no-referrer";
    Object.assign(frame.style, { width: "100%", height: "240px", border: "0", display: "block" });
    frame.style.visibility = "hidden";
    container.current.append(frame);
    const controller = new AbortController();
    let session: ConsoleMcpAppSession | null = null;
    let bridge: AppBridge | null = null;
    let transport: McpAppTransport | null = null;
    let observer: ResizeObserver | null = null;
    let initialized = false;
    let timer: ReturnType<typeof setTimeout>;
    let stopped = false;
    const stop = () => {
      if (stopped) return;
      stopped = true;
      controller.abort();
      clearTimeout(timer);
      observer?.disconnect();
      // Drop private pixels and revoke requests immediately. Keep this generation's
      // isolated frame alive briefly so the app can acknowledge teardown.
      frame.style.visibility = "hidden";
      frame.style.display = "none";
      frame.title = "Closing interactive tool result";
      transport?.beginTeardown();
      const finish = () => {
        void bridge?.close();
        frame.removeAttribute("src");
        frame.remove();
      };
      if (bridge && initialized) void bridge.teardownResource({}, { timeout: 500 }).catch(() => {}).finally(finish);
      else finish();
      try { session?.dispose?.(); } catch { /* Isolate view cleanup. */ }
    };
    const fail = () => { if (!controller.signal.aborted) { setStatus("unavailable"); stop(); } };
    setStatus("loading");
    timer = setTimeout(fail, 15_000);
    void (async () => {
      const sandbox = mcpSandboxUrl(state.host.sandboxProxyUrl);
      const resolved = await state.host.resolve(locator, controller.signal);
      if (controller.signal.aborted) { resolved?.dispose?.(); return; }
      session = resolved;
      if (!session) throw new Error("No authorized MCP App for this invocation");
      const resourceUri = getToolUiResourceUri(session.tool);
      if (!resourceUri?.startsWith("ui://")) throw new Error("Tool has no MCP App resource");
      const response = ReadResourceResultSchema.parse(await session.readResource(resourceUri, controller.signal));
      controller.signal.throwIfAborted();
      const content = response.contents.find(item => item.uri === resourceUri && item.mimeType === RESOURCE_MIME_TYPE);
      if (!content) throw new Error("MCP App resource is missing or has an unsupported MIME type");
      if ("blob" in content && content.blob.length > 7_000_000) throw new Error("MCP App resource exceeds the host limit");
      const html = "text" in content ? content.text : new TextDecoder().decode(Uint8Array.from(atob(content.blob), ch => ch.charCodeAt(0)));
      if (html.length > 5_000_000) throw new Error("MCP App resource exceeds the host limit");
      const ui = (content._meta?.ui ?? (session.resource?.uri === resourceUri ? session.resource._meta?.ui : undefined)) as { csp?: Record<string, unknown> } | undefined;
      sandbox.searchParams.set("csp", JSON.stringify(ui?.csp ?? {}));
      sandbox.searchParams.set("hostOrigin", window.location.origin);
      bridge = new AppBridge(null, { name: "Meta Console", version: "1.0.0" }, {
        serverResources: {}, ...(session.callTool && !state.readOnly ? { serverTools: {} } : {}),
      }, { hostContext: { displayMode: "inline", availableDisplayModes: ["inline"], platform: "web",
        theme: document.querySelector('[data-cc-theme="dark"]') ? "dark" : "light",
        locale: navigator.language, timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
        containerDimensions: { width: frame.clientWidth, maxHeight: 1200 },
      } });
      bridge.onreadresource = async (params, extra) => {
        controller.signal.throwIfAborted();
        const result = await withAppRequest(controller.signal, extra.mcpReq.signal, signal => session!.readResource(params.uri, signal));
        controller.signal.throwIfAborted();
        return ReadResourceResultSchema.parse(result);
      };
      bridge.oncalltool = async (params, extra) => {
        controller.signal.throwIfAborted();
        if (state.readOnly || !session!.callTool) throw new Error("This view cannot call tools");
        const result = await withAppRequest(controller.signal, extra.mcpReq.signal, signal => session!.callTool!(params.name, params.arguments ?? {}, signal));
        controller.signal.throwIfAborted();
        return CallToolResultSchema.parse(result);
      };
      bridge.onrequestdisplaymode = async () => { controller.signal.throwIfAborted(); return { mode: "inline" }; };
      bridge.onsizechange = ({ height }) => {
        if (controller.signal.aborted) return;
        if (typeof height === "number" && Number.isFinite(height)) frame.style.height = `${Math.min(1200, Math.max(80, height))}px`;
      };
      bridge.oninitialized = () => {
        if (initialized || controller.signal.aborted) return;
        initialized = true;
        clearTimeout(timer);
        void (async () => {
          await bridge!.sendToolInput({ arguments: session!.arguments });
          controller.signal.throwIfAborted();
          await bridge!.sendToolResult(CallToolResultSchema.parse(session!.result));
          if (!controller.signal.aborted) { frame.style.visibility = "visible"; setStatus("ready"); }
        })().catch(fail);
      };
      bridge.onsandboxready = () => {
        if (controller.signal.aborted) return;
        void bridge!.sendSandboxResourceReady({ html, csp: ui?.csp }).catch(fail);
      };
      frame.src = sandbox.href;
      // Listener registration runs synchronously before the sandbox can load.
      transport = new McpAppTransport(frame.contentWindow!, sandbox.origin, fail);
      await bridge.connect(transport);
      controller.signal.throwIfAborted();
      observer = new ResizeObserver(([entry]) => {
        if (initialized && !controller.signal.aborted) void Promise.resolve(bridge!.sendHostContextChange({ containerDimensions: { width: entry.contentRect.width, maxHeight: 1200 } })).catch(fail);
      });
      observer.observe(frame);
    })().catch(fail);
    return stop;
  }, [state?.host, key]);
  return <div className="console-mcp-app" data-mcp-app-call={locator.toolCallId}>
    <div ref={container} />
    {status !== "ready" ? <div role="status">{fallback}{status === "loading" ? " (loading view)" : ""}</div> : null}
  </div>;
}
