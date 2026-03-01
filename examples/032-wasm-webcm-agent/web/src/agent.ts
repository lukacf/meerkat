/**
 * Agent — drives the Meerkat WASM runtime for a single-agent session.
 *
 * Uses meerkat-web-runtime exports:
 * - register_tool_callback: register WebCM tools before session creation
 * - create_session_simple: create a session without mobpack
 * - start_turn: run a full agent turn (LLM + tool dispatch loop)
 * - poll_events: drain AgentEvents from the completed turn
 * - destroy_session: cleanup
 *
 * The agent loop runs in Rust (meerkat-core). Tool calls are dispatched
 * to JS callbacks (WebCM) via the JsToolDispatcher bridge.
 */

import type { WebCMHost } from "./webcm-host";
import { registerWebCMTools } from "./tools";

// ── Types ───────────────────────────────────────────────────────────────────

export interface AgentEvent {
  type: "thinking" | "text" | "tool_call" | "tool_result" | "error" | "done";
  content: string;
  toolName?: string;
}

interface RuntimeModule {
  default(): Promise<void>;
  register_tool_callback(name: string, description: string, schema_json: string, callback: any): void;
  clear_tool_callbacks(): void;
  create_session_simple(config_json: string): number;
  start_turn(handle: number, prompt: string, options_json: string): Promise<any>;
  poll_events(handle: number): string;
  destroy_session(handle: number): void;
}

// ── System prompt ──────────────────────────────────────────────────────────

const SYSTEM_PROMPT = `You are a skilled software engineer with access to an Alpine Linux virtual machine running in the user's browser. You can execute shell commands, read and write files.

The VM has: ash (busybox shell), micropython (use "micropython" not "python3"), lua5.4, quickjs (qjs), tcc (C compiler), git, curl, jq, sqlite3, vim, neovim.

Guidelines:
- Use the shell tool to run commands. Always check exit codes.
- Use write_file to create files (avoids shell escaping issues).
- Use read_file to inspect file contents.
- Install additional packages with: apk add <package>
- Work in /root/ or /tmp/.
- Be concise in your responses. Show your work.`;

// ── Agent ───────────────────────────────────────────────────────────────────

export class Agent {
  private vm: WebCMHost;
  private onEvent: (e: AgentEvent) => void;
  private runtime: RuntimeModule | null = null;
  private sessionHandle: number | null = null;
  private apiKey: string;
  private model: string;

  constructor(apiKey: string, vm: WebCMHost, onEvent: (e: AgentEvent) => void, model = "claude-opus-4-6") {
    this.apiKey = apiKey;
    this.vm = vm;
    this.onEvent = onEvent;
    this.model = model;
  }

  /** Load the WASM runtime, register tools, create session. */
  async init(): Promise<void> {
    // Load meerkat-web-runtime WASM
    const url = new URL("/meerkat-pkg/meerkat_web_runtime.js", window.location.href).toString();
    let mod: RuntimeModule;
    try {
      mod = (await import(/* @vite-ignore */ url)) as RuntimeModule;
    } catch (err: any) {
      throw new Error(
        `Failed to load Meerkat WASM runtime from ${url}. ` +
        `Make sure you built the WASM bundle (see examples.sh) and the dev server is serving /meerkat-pkg/. ` +
        `Original error: ${err.message}`,
      );
    }
    this.runtime = mod;
    await this.runtime.default();

    // Register WebCM tools (shared with mob.ts)
    registerWebCMTools(this.runtime, this.vm);

    // Create session
    this.sessionHandle = this.runtime.create_session_simple(JSON.stringify({
      model: this.model,
      api_key: this.apiKey,
      system_prompt: SYSTEM_PROMPT,
      max_tokens: 4096,
    }));
  }

  /** Run a full agent turn. Events are emitted via onEvent callback. */
  async run(userMessage: string): Promise<void> {
    if (!this.runtime || this.sessionHandle === null) {
      throw new Error("Agent not initialized — call init() first");
    }

    // start_turn runs the full agent loop (LLM + tool calls) and returns
    // when done. Events are buffered and retrieved via poll_events.
    const resultStr = await this.runtime.start_turn(this.sessionHandle, userMessage, "{}");
    const result = typeof resultStr === "string" ? JSON.parse(resultStr) : resultStr;

    // Drain events
    const eventsJson = this.runtime.poll_events(this.sessionHandle);
    const events: any[] = JSON.parse(eventsJson);

    // Route events to the UI
    for (const event of events) {
      this.routeEvent(event);
    }

    // Final status
    if (result.status === "failed") {
      this.onEvent({ type: "error", content: result.error || "Agent run failed" });
    }
    this.onEvent({ type: "done", content: "" });
  }

  private routeEvent(event: any): void {
    switch (event.type) {
      case "text_delta":
        // Accumulate deltas — we'll emit the full text from text_complete
        break;
      case "text_complete":
        this.onEvent({ type: "text", content: event.content || "" });
        break;
      case "tool_call_requested":
        this.onEvent({
          type: "tool_call",
          content: event.name === "shell"
            ? (typeof event.args === "string" ? JSON.parse(event.args).command : event.args?.command || JSON.stringify(event.args))
            : JSON.stringify(event.args),
          toolName: event.name,
        });
        break;
      case "tool_execution_completed":
        this.onEvent({
          type: "tool_result",
          content: event.result || "(no output)",
          toolName: event.name,
        });
        break;
      case "run_failed":
        this.onEvent({ type: "error", content: event.error || "Unknown error" });
        break;
    }
  }
}
