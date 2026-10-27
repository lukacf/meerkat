import { CallToolResultSchema, ReadResourceResultSchema, ToolSchema } from "@modelcontextprotocol/core";
import { getToolUiResourceUri } from "@modelcontextprotocol/ext-apps/app-bridge";
import { mcpSandboxUrl, type ConsoleMcpAppsHost, type McpAppLocator } from "../../../packages/console-components/src/mcp-apps";
import { CONSOLE_REST_PATHS } from "../lib/contract";

export interface NativeMcpAppsHostOptions {
  baseUrl: string;
  sandboxProxyUrl: string;
  authority: string;
  runtimeId?: string;
  readOnly?: boolean;
  requestTimeoutMs?: number;
}

/** The stock gateway remains the only owner of viewer, member and MCP admission. */
export function createNativeMcpAppsHost(options: NativeMcpAppsHostOptions): ConsoleMcpAppsHost {
  const base = new URL(options.baseUrl || "/", window.location.href);
  if (base.origin !== window.location.origin || base.username || base.password || base.search || base.hash) {
    throw new Error("The stock MCP Apps adapter requires the Console's authenticated origin");
  }
  const sandboxProxyUrl = mcpSandboxUrl(options.sandboxProxyUrl).href;
  const request = async (path: string, locator: McpAppLocator, params: Record<string, unknown>, signal: AbortSignal): Promise<unknown> => {
    signal.throwIfAborted();
    const controller = new AbortController();
    const abort = () => controller.abort(signal.reason);
    signal.addEventListener("abort", abort, { once: true });
    const timeout = setTimeout(() => controller.abort(new Error("MCP App request timed out")), options.requestTimeoutMs ?? 60_000);
    try {
      const url = new URL(`${base.pathname.replace(/\/$/, "")}${path}`, base.origin);
      const response = await fetch(url.href, {
        method: "POST", credentials: "same-origin", redirect: "error", cache: "no-store", signal: controller.signal,
        headers: {
          "Content-Type": "application/json",
          // Routing context only. These headers cannot establish viewer authority.
          "X-Console-Authority": encodeURIComponent(JSON.stringify({ key: options.authority, runtimeId: options.runtimeId })),
          "X-Console-Conversation": encodeURIComponent(JSON.stringify({ scopeKey: options.authority, identity: locator.identity })),
        },
        body: JSON.stringify({ ...locator, ...params }),
      });
      controller.signal.throwIfAborted();
      if (!response.ok) throw new Error(`MCP App request failed (HTTP ${response.status})`);
      const result: unknown = await response.json();
      controller.signal.throwIfAborted();
      return result;
    } finally {
      clearTimeout(timeout);
      signal.removeEventListener("abort", abort);
    }
  };
  return {
    sandboxProxyUrl,
    async resolve(locator, signal) {
      // Capture a locator, never a caller-owned object that can change between requests.
      const original = { identity: locator.identity, sessionId: locator.sessionId, toolCallId: locator.toolCallId };
      if (Object.values(original).some(value => typeof value !== "string" || !value.trim())) throw new Error("Invalid MCP App invocation");
      const response = await request(CONSOLE_REST_PATHS.mcpAppsResolve, original, {}, signal);
      if (response === null) return null;
      if (!response || typeof response !== "object" || Array.isArray(response)) throw new Error("Invalid MCP App response");
      const resolved = response as Record<string, unknown>;
      const tool = ToolSchema.parse(resolved.tool);
      const args = resolved.arguments;
      if (!args || typeof args !== "object" || Array.isArray(args)) throw new Error("Invalid MCP App arguments");
      const result = CallToolResultSchema.parse(resolved.result);
      let cachedResource = resolved.resource == null ? undefined : ReadResourceResultSchema.parse(resolved.resource);
      let disposed = false;
      const resourceUri = getToolUiResourceUri(tool);
      const current = (requestSignal: AbortSignal) => {
        signal.throwIfAborted();
        requestSignal.throwIfAborted();
        if (disposed) throw new DOMException("MCP App view is closed", "AbortError");
      };
      return {
        tool, arguments: args as Record<string, unknown>, result,
        dispose() { disposed = true; cachedResource = undefined; },
        async readResource(uri, requestSignal) {
          current(requestSignal);
          // Retained HTML makes historical invocations readable without reconnecting.
          // Later reads return through native member and resource custody checks.
          if (cachedResource && uri === resourceUri && cachedResource.contents.some(content => content.uri === uri)) {
            const cached = cachedResource;
            cachedResource = undefined;
            return cached;
          }
          return ReadResourceResultSchema.parse(await request(CONSOLE_REST_PATHS.mcpAppsReadResource, original, { uri }, requestSignal));
        },
        ...(resolved.canCallTools === true && !options.readOnly ? {
          async callTool(name: string, toolArgs: Record<string, unknown>, requestSignal: AbortSignal) {
            current(requestSignal);
            return CallToolResultSchema.parse(await request(CONSOLE_REST_PATHS.mcpAppsCallTool, original, { name, arguments: toolArgs }, requestSignal));
          },
        } : {}),
      };
    },
  };
}
