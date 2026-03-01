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

// ── Tool definitions ────────────────────────────────────────────────────────

const SYSTEM_PROMPT = `You are a skilled software engineer with access to an Alpine Linux virtual machine running in the user's browser. You can execute shell commands, read and write files.

The VM has: ash (busybox shell), micropython (use "micropython" not "python3"), lua5.4, quickjs (qjs), tcc (C compiler), git, curl, jq, sqlite3, vim, neovim.

Guidelines:
- Use the shell tool to run commands. Always check exit codes.
- Use write_file to create files (avoids shell escaping issues).
- Use read_file to inspect file contents.
- Install additional packages with: apk add <package>
- Work in /root/ or /tmp/.
- Be concise in your responses. Show your work.`;

const SHELL_SCHEMA = JSON.stringify({
  type: "object",
  properties: {
    command: { type: "string", description: "Shell command to execute in the Alpine Linux VM" },
  },
  required: ["command"],
});

const WRITE_FILE_SCHEMA = JSON.stringify({
  type: "object",
  properties: {
    path: { type: "string", description: "Absolute path to write to" },
    content: { type: "string", description: "File content" },
  },
  required: ["path", "content"],
});

const READ_FILE_SCHEMA = JSON.stringify({
  type: "object",
  properties: {
    path: { type: "string", description: "Absolute path to read" },
  },
  required: ["path"],
});

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
    this.runtime = (await import(/* @vite-ignore */ url)) as RuntimeModule;
    await this.runtime.default();

    // Register WebCM tools
    this.runtime.clear_tool_callbacks();

    this.runtime.register_tool_callback(
      "shell",
      "Execute a shell command in the Alpine Linux VM. Returns stdout+stderr and exit code.",
      SHELL_SCHEMA,
      async (argsJson: string) => {
        const args = JSON.parse(argsJson);
        const { output, exitCode } = await this.vm.exec(args.command);
        return JSON.stringify({
          content: exitCode === 0 ? (output || "(no output)") : `Exit code ${exitCode}\n${output}`,
          is_error: exitCode !== 0,
        });
      },
    );

    this.runtime.register_tool_callback(
      "write_file",
      "Write content to a file in the VM filesystem. Creates parent directories if needed.",
      WRITE_FILE_SCHEMA,
      async (argsJson: string) => {
        const args = JSON.parse(argsJson);
        await this.vm.exec(`mkdir -p $(dirname ${args.path})`);
        await this.vm.writeFile(args.path, args.content);
        return JSON.stringify({ content: `Wrote ${args.path}`, is_error: false });
      },
    );

    this.runtime.register_tool_callback(
      "read_file",
      "Read the contents of a file from the VM filesystem.",
      READ_FILE_SCHEMA,
      async (argsJson: string) => {
        const args = JSON.parse(argsJson);
        try {
          const content = await this.vm.readFile(args.path);
          return JSON.stringify({ content, is_error: false });
        } catch (e: any) {
          return JSON.stringify({ content: e.message, is_error: true });
        }
      },
    );

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
