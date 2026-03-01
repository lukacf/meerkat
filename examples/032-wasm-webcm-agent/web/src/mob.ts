/**
 * Mob orchestration — planner + coder + reviewer collaborating in the browser VM.
 *
 * Uses meerkat-web-runtime WASM mob APIs with exact MobDefinition format.
 */

import type { WebCMHost } from "./webcm-host";

// ── Types ───────────────────────────────────────────────────────────────────

export interface MobEvent {
  agent: string;
  type: "thinking" | "text" | "tool_call" | "tool_result" | "status";
  content: string;
  toolName?: string;
}

type MobEventHandler = (e: MobEvent) => void;

interface RuntimeModule {
  default(): Promise<void>;
  register_tool_callback(name: string, description: string, schema_json: string, callback: any): void;
  clear_tool_callbacks(): void;
  init_runtime_from_config(config_json: string): any;
  mob_create(definition_json: string): Promise<any>;
  mob_spawn(mob_id: string, specs_json: string): Promise<any>;
  mob_wire(mob_id: string, a: string, b: string): Promise<void>;
  mob_send_message(mob_id: string, meerkat_id: string, message: string): Promise<void>;
  mob_member_subscribe(mob_id: string, meerkat_id: string): Promise<number>;
  mob_run_flow(mob_id: string, flow_id: string, params_json: string): Promise<any>;
  mob_flow_status(mob_id: string, run_id: string): Promise<any>;
  poll_subscription(handle: number): string;
}

// ── Tool schemas ────────────────────────────────────────────────────────────

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

// ── Mob definition (exact MobDefinition format) ─────────────────────────────

function buildMobDefinition(model: string): object {
  const toolConfig = {
    builtins: true,
    shell: false,
    comms: true,
    memory: false,
    mob: false,
    mob_tasks: false,
    mcp: [] as string[],
    rust_bundles: [] as string[],
  };

  return {
    id: "dev-team",
    orchestrator: null,
    profiles: {
      planner: {
        model,
        skills: ["planner-role"],
        tools: toolConfig,
        peer_description: "Senior architect who creates implementation plans",
        external_addressable: false,
        backend: null,
        runtime_mode: "autonomous_host",
        max_inline_peer_notifications: null,
        output_schema: null,
      },
      coder: {
        model,
        skills: ["coder-role"],
        tools: toolConfig,
        peer_description: "Expert programmer who implements code",
        external_addressable: false,
        backend: null,
        runtime_mode: "autonomous_host",
        max_inline_peer_notifications: null,
        output_schema: null,
      },
      reviewer: {
        model,
        skills: ["reviewer-role"],
        tools: toolConfig,
        peer_description: "Code reviewer who verifies quality",
        external_addressable: false,
        backend: null,
        runtime_mode: "autonomous_host",
        max_inline_peer_notifications: null,
        output_schema: null,
      },
    },
    mcp_servers: {},
    wiring: {
      auto_wire_orchestrator: false,
      role_wiring: [
        { a: "planner", b: "coder" },
        { a: "planner", b: "reviewer" },
        { a: "coder", b: "reviewer" },
      ],
    },
    skills: {
      "planner-role": {
        source: "inline",
        content: `You are a senior software architect. When given a task:
1. Analyze requirements carefully
2. Write a clear, actionable plan to /workspace/plan.md using the write_file tool
3. The plan should list files to create, functions to implement, and test strategy
Work in /workspace/. Use shell and write_file tools.`,
      },
      "coder-role": {
        source: "inline",
        content: `You are an expert programmer. When given a task:
1. Read /workspace/plan.md to understand what to build
2. Implement the code in /workspace/src/ using write_file
3. Test it using the shell tool (run the code, check output)
Work in /workspace/. Use shell, write_file, and read_file tools.`,
      },
      "reviewer-role": {
        source: "inline",
        content: `You are a code reviewer. When given code to review:
1. Read the source files in /workspace/src/ using read_file
2. Run the code with shell to verify it works correctly
3. Write review feedback to /workspace/review.md
Work in /workspace/. Use shell, write_file, and read_file tools.`,
      },
    },
    backend: {
      default: "subagent",
      external: null,
    },
    flows: {
      implement: {
        description: "Plan, implement, and review code",
        steps: {
          plan: {
            role: "planner",
            message: "Create an implementation plan for: {{objective}}. Write it to /workspace/plan.md",
            depends_on: [],
            dispatch_mode: "fan_out",
            collection_policy: { type: "all" },
            condition: null,
            timeout_ms: 120000,
            expected_schema_ref: null,
            branch: null,
            depends_on_mode: "all",
            allowed_tools: null,
            blocked_tools: null,
          },
          code: {
            role: "coder",
            message: "Read /workspace/plan.md and implement the code in /workspace/src/. Test it.",
            depends_on: ["plan"],
            dispatch_mode: "fan_out",
            collection_policy: { type: "all" },
            condition: null,
            timeout_ms: 180000,
            expected_schema_ref: null,
            branch: null,
            depends_on_mode: "all",
            allowed_tools: null,
            blocked_tools: null,
          },
          review: {
            role: "reviewer",
            message: "Review the code in /workspace/src/. Write feedback to /workspace/review.md",
            depends_on: ["code"],
            dispatch_mode: "fan_out",
            collection_policy: { type: "all" },
            condition: null,
            timeout_ms: 120000,
            expected_schema_ref: null,
            branch: null,
            depends_on_mode: "all",
            allowed_tools: null,
            blocked_tools: null,
          },
        },
      },
    },
    topology: null,
    supervisor: null,
    limits: null,
    spawn_policy: null,
    event_router: null,
  };
}

// ── Mob orchestrator ────────────────────────────────────────────────────────

export class MobOrchestrator {
  private vm: WebCMHost;
  private onEvent: MobEventHandler;
  runtime: RuntimeModule | null = null;
  private mobId = "dev-team";
  members: Map<string, { meerkatId: string; subHandle: number }> = new Map();
  private pollInterval: number | null = null;

  constructor(vm: WebCMHost, onEvent: MobEventHandler) {
    this.vm = vm;
    this.onEvent = onEvent;
  }

  async init(apiKey: string, model: string): Promise<void> {
    this.onEvent({ agent: "system", type: "status", content: "Loading Meerkat WASM runtime..." });

    const url = new URL("/meerkat-pkg/meerkat_web_runtime.js", window.location.href).toString();
    this.runtime = (await import(/* @vite-ignore */ url)) as RuntimeModule;
    await this.runtime.default();

    this.registerTools();

    this.runtime.init_runtime_from_config(JSON.stringify({
      api_key: apiKey,
      model,
      max_sessions: 16,
    }));

    await this.vm.exec("mkdir -p /workspace/src");

    this.onEvent({ agent: "system", type: "status", content: "Meerkat runtime initialized" });
  }

  async createMob(model: string): Promise<void> {
    if (!this.runtime) throw new Error("Runtime not initialized");

    this.onEvent({ agent: "system", type: "status", content: "Creating mob: planner + coder + reviewer..." });

    const def = buildMobDefinition(model);
    await this.runtime.mob_create(JSON.stringify(def));

    // Spawn members (array of specs)
    for (const profile of ["planner", "coder", "reviewer"]) {
      const specs = [{ profile, meerkat_id: profile }];
      const resultRaw = await this.runtime.mob_spawn(this.mobId, JSON.stringify(specs));
      const results = typeof resultRaw === "string" ? JSON.parse(resultRaw) : resultRaw;

      if (Array.isArray(results) && results[0]?.status === "error") {
        throw new Error(`Failed to spawn ${profile}: ${results[0].error}`);
      }

      const meerkatId = profile;
      const subHandle = await this.runtime.mob_member_subscribe(this.mobId, meerkatId);
      this.members.set(profile, { meerkatId, subHandle });
      this.onEvent({ agent: profile, type: "status", content: `Spawned` });
    }

    // Wire all pairs (done via definition's role_wiring, but explicit wire is also fine)
    const profiles = Array.from(this.members.keys());
    for (let i = 0; i < profiles.length; i++) {
      for (let j = i + 1; j < profiles.length; j++) {
        try {
          await this.runtime.mob_wire(this.mobId, profiles[i], profiles[j]);
        } catch {
          // May already be wired from role_wiring
        }
      }
    }

    this.onEvent({ agent: "system", type: "status", content: "Mob created and wired" });

    // Start polling events immediately — autonomous agents may already be active
    this.startPolling();
  }

  async runFlow(objective: string): Promise<void> {
    if (!this.runtime) throw new Error("Runtime not initialized");

    this.onEvent({ agent: "system", type: "status", content: `Task: "${objective}"` });

    // Send the objective directly to the planner via comms message.
    // Flow step messages don't interpolate {{variables}}, so we
    // deliver the user's request as a direct message instead.
    const planner = this.members.get("planner");
    if (planner) {
      await this.runtime.mob_send_message(
        this.mobId,
        planner.meerkatId,
        `New task from user: ${objective}\n\nPlease create an implementation plan in /workspace/plan.md, then notify the coder to start implementing.`,
      );
      this.onEvent({ agent: "system", type: "status", content: "Sent task to planner" });
    }

    // Wait for the agents to work autonomously
    // (they coordinate via comms — planner writes plan, tells coder, coder implements, tells reviewer)
    this.onEvent({ agent: "system", type: "status", content: "Agents working autonomously..." });
  }

  stop(): void {
    if (this.pollInterval) {
      clearInterval(this.pollInterval);
      this.pollInterval = null;
    }
  }

  private registerTools(): void {
    if (!this.runtime) return;
    this.runtime.clear_tool_callbacks();

    this.runtime.register_tool_callback(
      "shell", "Execute a shell command in the Alpine Linux VM", SHELL_SCHEMA,
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
      "write_file", "Write content to a file in the VM", WRITE_FILE_SCHEMA,
      async (argsJson: string) => {
        const args = JSON.parse(argsJson);
        await this.vm.exec(`mkdir -p $(dirname ${args.path})`);
        await this.vm.writeFile(args.path, args.content);
        return JSON.stringify({ content: `Wrote ${args.path}`, is_error: false });
      },
    );

    this.runtime.register_tool_callback(
      "read_file", "Read a file from the VM", READ_FILE_SCHEMA,
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
  }

  private startPolling(): void {
    if (this.pollInterval) return;
    this.pollInterval = window.setInterval(() => this.pollEvents(), 1000);
  }

  private pollEvents(): void {
    if (!this.runtime) return;

    for (const [profile, { subHandle }] of this.members) {
      try {
        const raw = this.runtime.poll_subscription(subHandle);
        const events: any[] = JSON.parse(raw);
        for (const event of events) {
          this.routeEvent(profile, event);
        }
      } catch {
        // Subscription may not be ready yet
      }
    }
  }

  private routeEvent(profile: string, envelope: any): void {
    // EventEnvelope format: { event_id, source_id, seq, timestamp_ms, payload }
    // payload: { type: "text_delta", delta: "..." } or { type: "tool_call_requested", ... }
    const payload = envelope.payload || envelope;
    const type = payload.type;

    switch (type) {
      case "text_delta":
        // Skip deltas, wait for text_complete
        break;
      case "text_complete":
        this.onEvent({ agent: profile, type: "text", content: payload.content || "" });
        break;
      case "tool_call_requested":
        this.onEvent({
          agent: profile,
          type: "tool_call",
          content: payload.name === "shell"
            ? (typeof payload.args === "object" ? payload.args.command || JSON.stringify(payload.args) : String(payload.args))
            : JSON.stringify(payload.args || {}),
          toolName: payload.name,
        });
        break;
      case "tool_execution_completed":
        this.onEvent({
          agent: profile,
          type: "tool_result",
          content: (payload.result || "(no output)").slice(0, 500),
          toolName: payload.name,
        });
        break;
      case "tool_result_received":
        this.onEvent({
          agent: profile,
          type: "tool_result",
          content: payload.name || "tool",
          toolName: payload.name,
        });
        break;
      case "turn_started":
        this.onEvent({ agent: profile, type: "status", content: `Turn ${payload.turn_number || ""}` });
        break;
      case "turn_completed":
        this.onEvent({ agent: profile, type: "status", content: "Turn done" });
        break;
      case "run_completed":
        this.onEvent({ agent: profile, type: "status", content: "Completed" });
        break;
      case "run_failed":
        this.onEvent({ agent: profile, type: "status", content: `Error: ${payload.error || "unknown"}` });
        break;
    }
  }

  private async pollFlowStatus(runId: string): Promise<void> {
    if (!this.runtime) return;

    for (let i = 0; i < 180; i++) { // Max 6 minutes
      await new Promise((r) => setTimeout(r, 2000));
      try {
        const statusRaw = await this.runtime.mob_flow_status(this.mobId, runId);
        const status = typeof statusRaw === "string" ? JSON.parse(statusRaw) : statusRaw;

        if (status.state === "completed" || status.state === "Completed") {
          this.onEvent({ agent: "system", type: "status", content: "Flow completed!" });
          return;
        } else if (status.state === "failed" || status.state === "Failed") {
          this.onEvent({ agent: "system", type: "status", content: `Flow failed: ${status.error || status.reason || "unknown"}` });
          return;
        }
      } catch {
        // Status endpoint may not be ready yet
      }
    }

    this.onEvent({ agent: "system", type: "status", content: "Flow timed out" });
  }
}
