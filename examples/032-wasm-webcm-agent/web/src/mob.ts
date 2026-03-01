/**
 * Mob orchestration — planner + coder + reviewer collaborating in the browser VM.
 *
 * Uses meerkat-web-runtime WASM mob APIs (mob_create, mob_spawn, mob_wire,
 * mob_run_flow) with WebCM tools registered via register_tool_callback.
 */

import type { WebCMHost } from "./webcm-host";

// ── Types ───────────────────────────────────────────────────────────────────

export interface MobEvent {
  agent: string; // "planner" | "coder" | "reviewer"
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
  mob_list_members(mob_id: string): Promise<any>;
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

// ── Mob definition ──────────────────────────────────────────────────────────

function buildMobDefinition(model: string): object {
  return {
    id: "dev-team",
    profiles: {
      planner: {
        model,
        system_prompt: `You are a senior software architect. When given a task:
1. Analyze requirements
2. Write a clear plan to /workspace/plan.md using the write_file tool
3. Notify the coder via the send tool when done

Use shell and write_file tools. Work in /workspace/.`,
        runtime_mode: "autonomous_host",
        tools: { shell: true, file: true, comms: true },
      },
      coder: {
        model,
        system_prompt: `You are an expert programmer. When given a task:
1. Read /workspace/plan.md to understand what to build
2. Implement the code in /workspace/src/ using write_file
3. Test it using the shell tool
4. Notify the reviewer via the send tool when done

Use shell, write_file, and read_file tools. Work in /workspace/.`,
        runtime_mode: "autonomous_host",
        tools: { shell: true, file: true, comms: true },
      },
      reviewer: {
        model,
        system_prompt: `You are a code reviewer. When given code to review:
1. Read the source files in /workspace/src/
2. Run the code with shell to verify it works
3. Write feedback to /workspace/review.md
4. If changes needed, notify the coder. If approved, notify the planner.

Use shell, write_file, and read_file tools. Work in /workspace/.`,
        runtime_mode: "autonomous_host",
        tools: { shell: true, file: true, comms: true },
      },
    },
    flows: {
      implement: {
        steps: [
          { id: "plan", profile: "planner", message: "{{objective}}" },
          { id: "code", profile: "coder", depends_on: ["plan"], message: "Read /workspace/plan.md and implement the code." },
          { id: "review", profile: "reviewer", depends_on: ["code"], message: "Review the code in /workspace/src/ and write feedback to /workspace/review.md" },
          { id: "revise", profile: "coder", depends_on: ["review"], message: "Read /workspace/review.md and address the feedback." },
          { id: "approve", profile: "reviewer", depends_on: ["revise"], message: "Final review. Verify the changes address your feedback." },
        ],
      },
    },
    wiring: {
      auto_wire_all: true,
    },
  };
}

// ── Mob orchestrator ────────────────────────────────────────────────────────

export class MobOrchestrator {
  private vm: WebCMHost;
  private onEvent: MobEventHandler;
  private runtime: RuntimeModule | null = null;
  private mobId = "dev-team";
  private members: Map<string, { meerkatId: string; subHandle: number }> = new Map();
  private pollInterval: number | null = null;

  constructor(vm: WebCMHost, onEvent: MobEventHandler) {
    this.vm = vm;
    this.onEvent = onEvent;
  }

  async init(apiKey: string, model: string): Promise<void> {
    this.onEvent({ agent: "system", type: "status", content: "Loading Meerkat WASM runtime..." });

    // Load the WASM runtime
    const url = new URL("/meerkat-pkg/meerkat_web_runtime.js", window.location.href).toString();
    this.runtime = (await import(/* @vite-ignore */ url)) as RuntimeModule;
    await this.runtime.default();

    // Register WebCM tools
    this.registerTools();

    // Init runtime with API key
    this.runtime.init_runtime_from_config(JSON.stringify({
      api_key: apiKey,
      model,
      max_sessions: 16,
    }));

    // Setup workspace
    await this.vm.exec("mkdir -p /workspace/src");

    this.onEvent({ agent: "system", type: "status", content: "Meerkat runtime initialized" });
  }

  async createMob(model: string): Promise<void> {
    if (!this.runtime) throw new Error("Runtime not initialized");

    this.onEvent({ agent: "system", type: "status", content: "Creating mob: planner + coder + reviewer..." });

    const def = buildMobDefinition(model);
    await this.runtime.mob_create(JSON.stringify(def));

    // Spawn members
    for (const profile of ["planner", "coder", "reviewer"]) {
      const result = await this.runtime.mob_spawn(this.mobId, JSON.stringify({
        profile,
        meerkat_id: profile,
      }));
      const parsed = typeof result === "string" ? JSON.parse(result) : result;
      const meerkatId = parsed.meerkat_id || profile;

      // Subscribe to events
      const subHandle = await this.runtime.mob_member_subscribe(this.mobId, meerkatId);
      this.members.set(profile, { meerkatId, subHandle });

      this.onEvent({ agent: profile, type: "status", content: `Spawned ${profile}` });
    }

    // Wire all pairs
    const profiles = Array.from(this.members.keys());
    for (let i = 0; i < profiles.length; i++) {
      for (let j = i + 1; j < profiles.length; j++) {
        const a = this.members.get(profiles[i])!.meerkatId;
        const b = this.members.get(profiles[j])!.meerkatId;
        await this.runtime.mob_wire(this.mobId, a, b);
      }
    }

    this.onEvent({ agent: "system", type: "status", content: "Mob created and wired" });
  }

  async runFlow(objective: string): Promise<void> {
    if (!this.runtime) throw new Error("Runtime not initialized");

    this.onEvent({ agent: "system", type: "status", content: `Starting flow: "${objective}"` });

    // Start polling events
    this.startPolling();

    // Run the implement flow
    const runId = await this.runtime.mob_run_flow(
      this.mobId,
      "implement",
      JSON.stringify({ objective }),
    );

    // Poll flow status until complete
    const runIdStr = typeof runId === "string" ? JSON.parse(runId) : runId;
    await this.pollFlowStatus(typeof runIdStr === "string" ? runIdStr : runIdStr.run_id || "unknown");
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

    // Shell tool
    this.runtime.register_tool_callback(
      "shell",
      "Execute a shell command in the Alpine Linux VM",
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

    // Write file tool
    this.runtime.register_tool_callback(
      "write_file",
      "Write content to a file in the VM",
      WRITE_FILE_SCHEMA,
      async (argsJson: string) => {
        const args = JSON.parse(argsJson);
        await this.vm.exec(`mkdir -p $(dirname ${args.path})`);
        await this.vm.writeFile(args.path, args.content);
        return JSON.stringify({ content: `Wrote ${args.path}`, is_error: false });
      },
    );

    // Read file tool
    this.runtime.register_tool_callback(
      "read_file",
      "Read a file from the VM",
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
  }

  private startPolling(): void {
    if (this.pollInterval) return;
    this.pollInterval = window.setInterval(() => this.pollEvents(), 500);
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

  private routeEvent(profile: string, event: any): void {
    const type = event.type || event.event_type;
    switch (type) {
      case "text_delta":
        this.onEvent({ agent: profile, type: "text", content: event.text || event.delta || "" });
        break;
      case "tool_call_requested":
        this.onEvent({
          agent: profile,
          type: "tool_call",
          content: event.args || JSON.stringify(event.input || {}),
          toolName: event.name || event.tool_name,
        });
        break;
      case "tool_result_received":
        this.onEvent({
          agent: profile,
          type: "tool_result",
          content: event.content || event.result || "",
          toolName: event.name || event.tool_name,
        });
        break;
      case "run_completed":
        this.onEvent({ agent: profile, type: "status", content: "Turn completed" });
        break;
      case "run_failed":
        this.onEvent({ agent: profile, type: "status", content: `Error: ${event.error || "unknown"}` });
        break;
    }
  }

  private async pollFlowStatus(runId: string): Promise<void> {
    if (!this.runtime) return;

    for (let i = 0; i < 300; i++) { // Max 5 minutes
      await new Promise((r) => setTimeout(r, 2000));
      try {
        const statusRaw = await this.runtime.mob_flow_status(this.mobId, runId);
        const status = typeof statusRaw === "string" ? JSON.parse(statusRaw) : statusRaw;

        if (status.state === "completed") {
          this.onEvent({ agent: "system", type: "status", content: "Flow completed successfully!" });
          this.stop();
          return;
        } else if (status.state === "failed") {
          this.onEvent({ agent: "system", type: "status", content: `Flow failed: ${status.error || "unknown"}` });
          this.stop();
          return;
        }

        // Update current step
        if (status.current_step) {
          this.onEvent({ agent: "system", type: "status", content: `Flow step: ${status.current_step}` });
        }
      } catch {
        // Status endpoint may not be ready
      }
    }

    this.onEvent({ agent: "system", type: "status", content: "Flow timed out" });
    this.stop();
  }
}
