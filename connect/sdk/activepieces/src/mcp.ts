import { Server, type CallToolResult, type ServerContext, type Tool, type Transport } from "@modelcontextprotocol/server";
import { serveStdio, StdioServerTransport } from "@modelcontextprotocol/server/stdio";
import { createReader, createWriter, ActivepiecesError, type Reader, type ReaderConfiguration, type WriterConfiguration } from "./index.js";
import { parseConfiguration, operationConfiguration, selectedEnvironment, describeConfiguration, type McpConfiguration } from "./mcp-config.js";
import { isWriteProfile } from "./effect-profiles.js";
import { mcpToolName } from "./config-fields.js";
import { createOwnedHttpTransport, type OwnedHttpTransport } from "./mcp-http.js";
export { loadConfiguration, parseConfiguration, describeConfiguration, McpConfigurationError } from "./mcp-config.js";
export type { McpConfiguration } from "./mcp-config.js";
export { createOwnedHttpTransport, type OwnedHttpTransport } from "./mcp-http.js";

export interface ServerDependencies {
  readonly environment?: Readonly<Record<string, string | undefined>>;
  /** Each factory call transfers a fresh HTTP owner to its server. */
  readonly httpFactory?: () => OwnedHttpTransport;
}
function failure(kind: string, error?: ActivepiecesError, effect: "read" | "write" = error?.effect ?? "read"): CallToolResult {
  return { isError: true, content: [{ type: "text", text: `Activepieces ${effect === "write" ? "writer" : "reader"} ${kind}` }], structuredContent: {
    error: { kind, entered: error?.entered ?? false, dispatched: error?.dispatched ?? false,
      ...(error?.status === undefined ? {} : { status: error.status }), effect, writeOutcome: error?.writeOutcome ?? (effect === "write" ? "not_attempted" : "not_applicable") },
  } };
}
function tool(config: McpConfiguration): Tool {
  const description = describeConfiguration(config);
  return { name: mcpToolName(config.profile),
    description: `${description.effect === "write" ? "Write" : "Read"} with ${description.profile} using the host-selected connection. Supply manual IDs where required; no connection setup or OAuth refresh.`,
    inputSchema: description.inputSchema as Tool["inputSchema"],
    annotations: { readOnlyHint: description.effect === "read", destructiveHint: description.effect === "write", idempotentHint: description.effect === "read", openWorldHint: true } };
}
export function describeMcpConfiguration(value: unknown): { profile: string; tools: Tool[] } {
  const config = parseConfiguration(value);
  return { profile: config.profile, tools: [tool(config)] };
}
/** Owns the reader's active calls and HTTP dispatcher until close has joined both. */
class ActivepiecesMcpServer extends Server {
  private readonly active = new Set<Promise<CallToolResult>>();
  private readonly shutdown = new AbortController();
  private closing: Promise<void> | undefined;
  constructor(private readonly reader: Reader, private readonly http: OwnedHttpTransport, config: McpConfiguration) {
    super({ name: "meerkat-toolkit-activepieces", version: "0.1.0" }, { capabilities: { tools: {} } });
    const descriptor = tool(config);
    const refuse = (kind: string) => failure(kind, undefined, this.reader.describe().effect);
    this.setRequestHandler("tools/list", () => ({ tools: [structuredClone(descriptor)] }));
    this.setRequestHandler("tools/call", (request, context) => {
      if (this.shutdown.signal.aborted) return refuse("server_closing");
      if (request.params.name !== descriptor.name) return refuse("unknown_tool");
      if (this.active.size >= 4) return refuse("server_busy");
      if (request.params.task !== undefined || context.mcpReq.requestState() !== undefined || context.mcpReq.inputResponses !== undefined) {
        return refuse("invalid_arguments");
      }
      const operation = this.invoke(request.params.arguments ?? {}, context);
      this.active.add(operation);
      void operation.finally(() => this.active.delete(operation));
      return operation;
    });
  }
  private async invoke(arguments_: Record<string, unknown>, context: ServerContext): Promise<CallToolResult> {
    try {
      const value = await this.reader.invoke(arguments_, { signal: AbortSignal.any([context.mcpReq.signal, this.shutdown.signal]) });
      return { content: [{ type: "text", text: JSON.stringify(value) }], structuredContent: { value } };
    } catch (error) {
      return error instanceof ActivepiecesError ? failure(error.kind, error) : failure("operation_failed", undefined, this.reader.describe().effect);
    }
  }
  override close(): Promise<void> {
    if (!this.closing) {
      this.shutdown.abort();
      // SDK close aborts handlers but does not join their promises. Keep this
      // separate owner alive until borrowed response cleanup has completed.
      this.closing = Promise.resolve().then(async () => {
        try {
          try { await super.close(); }
          finally {
            await Promise.allSettled([...this.active]);
            await this.http.close();
          }
        } catch { throw new Error("Activepieces MCP cleanup failed"); }
      });
    }
    return this.closing;
  }
  protected override _onclose(): void {
    try { super._onclose(); }
    finally { void this.close().catch(() => {}); }
  }
}
export async function createMcpServer(value: unknown, dependencies: ServerDependencies = {}): Promise<ActivepiecesMcpServer> {
  const config = parseConfiguration(value);
  const environment=selectedEnvironment(config,dependencies.environment??process.env);
  const token=environment[config.token_env]!;
  let http: OwnedHttpTransport;
  try { http = (dependencies.httpFactory ?? createOwnedHttpTransport)(); }
  catch { throw new Error("Activepieces MCP initialization failed"); }
  try {
    const selected = operationConfiguration(config, token, http.send, environment);
    const operation = isWriteProfile(config.profile) ? createWriter(selected as WriterConfiguration) : createReader(selected as ReaderConfiguration);
    return new ActivepiecesMcpServer(operation, http, config);
  }
  catch { await http.close().catch(() => {}); throw new Error("Activepieces MCP initialization failed"); }
}
export interface StdioOptions extends ServerDependencies {
  /** Ownership of the official SDK transport transfers to this serving handle. */
  readonly transport?: Transport;
  readonly onerror?: () => void;
}
export interface ServingHandle { readonly closed: Promise<void>; close(): Promise<void> }
/** Official SDK framing and era negotiation; this wrapper only joins owned lifetimes. */
export function serveMcpStdio(value: unknown, options: StdioOptions = {}): ServingHandle {
  const config = parseConfiguration(value);
  // Validate the explicit key before opening a protocol connection; never print it.
  const environment=selectedEnvironment(config,options.environment??process.env);
  const wire = options.transport ?? new StdioServerTransport(undefined, undefined, { maxBufferSize: 65_536 });
  const instances = new Set<ActivepiecesMcpServer>();
  const creating = new Set<Promise<ActivepiecesMcpServer>>();
  let resolveClosed!: () => void;
  let rejectClosed!: (error: Error) => void;
  const closed = new Promise<void>((resolve, reject) => { resolveClosed = resolve; rejectClosed = reject; });
  void closed.catch(() => {});
  let closing: Promise<void> | undefined;
  let failed = false;
  const reportError = () => { try { options.onerror?.(); } catch {} };
  const bridge: Transport = {
    async start() {
      try { await wire.start(); }
      catch { failed = true; void bridge.close().catch(() => {}); throw new Error("Activepieces MCP startup failed"); }
    },
    send: (message, settings) => wire.send(message, settings),
    close() {
      return closing ??= Promise.resolve().then(async () => {
        try { await wire.close(); } catch { failed = true; }
        try { bridge.onclose?.(); } catch { failed = true; }
        await Promise.allSettled([...creating]);
        const outcomes = await Promise.allSettled([...instances].map(server => server.close()));
        if (failed || outcomes.some(result => result.status === "rejected")) {
          const error = new Error("Activepieces MCP cleanup failed"); rejectClosed(error); throw error;
        }
        resolveClosed();
      });
    },
  };
  wire.onmessage = (message, extra) => bridge.onmessage?.(message, extra);
  wire.onerror = () => { failed = true; reportError(); void bridge.close().catch(() => {}); };
  wire.onclose = () => { void bridge.close().catch(() => {}); };
  const handle = serveStdio(() => {
    if (closing) throw new Error("Activepieces MCP is closing");
    const creation = createMcpServer(config, { environment, httpFactory: options.httpFactory }).then(
      server => { instances.add(server); return server; },
      () => { failed = true; void bridge.close().catch(() => {}); throw new Error("Activepieces MCP initialization failed"); },
    );
    creating.add(creation);
    void creation.then(() => creating.delete(creation), () => creating.delete(creation));
    return creation;
  }, { transport: bridge, legacy: "serve", onerror: reportError });
  return { closed, async close() { try { await handle.close(); } finally { await bridge.close(); } await closed; } };
}
