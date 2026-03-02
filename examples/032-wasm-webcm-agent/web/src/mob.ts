/**
 * Mob orchestrator — multi-provider sub-agent management.
 *
 * Creates a 3-agent mob (planner, coder, reviewer) with different LLM
 * providers per role. Manages subscriptions, background polling, and
 * per-panel StreamRenderer routing.
 */

import { StreamRenderer } from "./stream";

// ── Types ───────────────────────────────────────────────────────────────────

export interface ApiKeys {
  anthropic?: string;
  openai?: string;
  gemini?: string;
}

export interface ModelAssignments {
  main: string;
  mainKey: string;
  planner: string;
  coder: string;
  reviewer: string;
}

interface PanelState {
  stream: StreamRenderer;
  statusEl: HTMLElement;
  subHandle: number | null;
  currentCard: HTMLElement | null;
}

export interface MobRuntime {
  init_runtime_from_config(config_json: string): void;
  mob_create(definition_json: string): Promise<string>;
  mob_spawn(mob_id: string, specs_json: string): Promise<string>;
  mob_wire(mob_id: string, a: string, b: string): Promise<void>;
  mob_member_subscribe(mob_id: string, meerkat_id: string): Promise<number>;
  mob_send_message(mob_id: string, meerkat_id: string, message: string): Promise<void>;
  poll_subscription(handle: number): string;
}

// ── Model resolution ────────────────────────────────────────────────────────

/**
 * Resolve model assignments based on available API keys.
 * Preferred: main=opus, planner=gpt-5.2, coder=gpt-5.3-codex, reviewer=gemini-3.1-pro-preview.
 * Falls back to available providers when keys are missing.
 */
export function resolveModels(keys: ApiKeys): ModelAssignments {
  const hasAnthropic = !!keys.anthropic;
  const hasOpenAI = !!keys.openai;
  const hasGemini = !!keys.gemini;

  // Main agent: prefer Anthropic Opus
  let main: string;
  let mainKey: string;
  if (hasAnthropic) { main = "claude-opus-4-6"; mainKey = keys.anthropic!; }
  else if (hasOpenAI) { main = "gpt-5.2"; mainKey = keys.openai!; }
  else { main = "gemini-3.1-pro-preview"; mainKey = keys.gemini!; }

  // Planner: prefer OpenAI gpt-5.2
  const planner = hasOpenAI ? "gpt-5.2"
    : hasAnthropic ? "claude-sonnet-4-5"
    : "gemini-3.1-pro-preview";

  // Coder: prefer OpenAI gpt-5.3-codex
  const coder = hasOpenAI ? "gpt-5.3-codex"
    : hasAnthropic ? "claude-sonnet-4-5"
    : "gemini-3.1-pro-preview";

  // Reviewer: prefer Gemini (schema issues fixed in PR #93)
  const reviewer = hasGemini ? "gemini-3-flash-preview"
    : hasAnthropic ? "claude-sonnet-4-5"
    : "gpt-5.2";

  return { main, mainKey, planner, coder, reviewer };
}

// ── Mob definition builder ──────────────────────────────────────────────────

const VM_ENV = `
ENVIRONMENT: Alpine Linux VM (RISC-V, WebAssembly/Cartesi Machine)
Shell: ash (BusyBox)
Languages: micropython (NOT python3), lua5.4, quickjs (qjs), tcc (C compiler), mruby
Tools: git, curl, jq, sqlite3, vim, neovim, grep, sed, awk, bc
Package manager: apk add <package>
IMPORTANT: There is NO python3/pip. Use "micropython" for Python. There is NO node/npm. Use "qjs" for JavaScript.
Working directory: /workspace/ (use /workspace/src/ for source code)
`.trim();

const MOB_ID = "dev-team";

function buildMobDefinition(models: ModelAssignments): object {
  const tools = { builtins: false, comms: true, shell: false };

  // Shared profile defaults — only model, skills, tools, and peer_description vary per role.
  const base = {
    tools,
    external_addressable: true,
    backend: null,
    runtime_mode: "autonomous_host" as const,
    max_inline_peer_notifications: null,
    output_schema: null,
    provider_params: { reasoning_effort: "low" },
  };

  return {
    id: MOB_ID,
    profiles: {
      orchestrator: { ...base, model: models.main, skills: ["orchestrator-role"],
        peer_description: "Alpha Meerkat — lead orchestrator who coordinates the team" },
      planner: { ...base, model: models.planner, skills: ["planner-role"],
        peer_description: "Senior architect who creates implementation plans" },
      coder: { ...base, model: models.coder, skills: ["coder-role"],
        peer_description: "Expert programmer who implements and tests code" },
      reviewer: { ...base, model: models.reviewer, skills: ["reviewer-role"],
        peer_description: "Code reviewer who verifies quality and correctness" },
    },
    mcp_servers: {},
    wiring: {
      role_wiring: [
        { a: "orchestrator", b: "planner" },
        { a: "orchestrator", b: "coder" },
        { a: "orchestrator", b: "reviewer" },
        { a: "planner", b: "coder" },
        { a: "planner", b: "reviewer" },
        { a: "coder", b: "reviewer" },
      ],
    },
    skills: {
      "orchestrator-role": {
        source: "inline",
        content: `You are the Alpha Meerkat — lead agent of a coding team. You coordinate three specialists via messaging.

${VM_ENV}

Your peers:
- dev-team/planner/planner — creates implementation plans
- dev-team/coder/coder — writes and tests code
- dev-team/reviewer/reviewer — reviews code quality

ON STARTUP (no messages): Say "Alpha Meerkat ready." — no tool calls, end your turn.

WHEN YOU RECEIVE A USER TASK (external message):
- Send the task to the planner via send(to: "dev-team/planner/planner", kind: "peer_message", body: "<detailed task description including language/runtime constraints from the environment spec>")
- Then END YOUR TURN. Do NOT wait or poll. The system wakes you when a reply arrives.

WHEN YOU RECEIVE A REPLY FROM THE PLANNER:
- Read the plan if needed (read_file /workspace/plan.md)
- Send instructions to the coder via send(to: "dev-team/coder/coder", kind: "peer_message", body: "<implementation instructions referencing the plan>")
- Then END YOUR TURN.

WHEN YOU RECEIVE A REPLY FROM THE CODER:
- Send a review request to the reviewer via send(to: "dev-team/reviewer/reviewer", kind: "peer_message", body: "<review request>")
- Then END YOUR TURN.

WHEN YOU RECEIVE A REPLY FROM THE REVIEWER:
- Read /workspace/review.md if needed
- If the reviewer found issues to fix: send the feedback to the coder via send(to: "dev-team/coder/coder", kind: "peer_message", body: "<fix instructions based on review>"). Then END YOUR TURN. When the coder replies, send it back to the reviewer for re-review.
- If the review is clean (no issues): summarize the final results for the user and END YOUR TURN.

CRITICAL RULES:
- NEVER call the wait tool. The system handles message delivery automatically between turns.
- Each turn: process one message, send one reply, end turn. Do not try to do the whole pipeline in one turn.
- You also have shell, write_file, read_file tools for quick checks.`,
      },
      "planner-role": {
        source: "inline",
        content: `You are the PLANNER in a dev team.

${VM_ENV}

ON STARTUP (no messages): Say "Planner standing by." — no tool calls, end your turn.

WHEN YOU RECEIVE A TASK via message:
1. Analyze requirements — consider the VM environment constraints above
2. Break into ordered steps with file paths, function signatures, and test commands
3. Write the plan to /workspace/plan.md using write_file
4. Send the plan summary back to the Alpha Meerkat: send(to: "dev-team/orchestrator/orchestrator", kind: "peer_message", body: "Plan written to /workspace/plan.md. <brief summary>")

Tools: shell, write_file, read_file, send.`,
      },
      "coder-role": {
        source: "inline",
        content: `You are the CODER in a dev team.

${VM_ENV}

ON STARTUP (no messages): Say "Coder standing by." — no tool calls, end your turn.

WHEN YOU RECEIVE INSTRUCTIONS via message:
1. Read /workspace/plan.md if referenced
2. Implement the code in /workspace/src/ using write_file
3. Test with shell — run the code, fix errors until it works
4. Send completion notice to the Alpha Meerkat: send(to: "dev-team/orchestrator/orchestrator", kind: "peer_message", body: "Implementation complete. <summary of what was built and test results>")

Tools: shell, write_file, read_file, send.`,
      },
      "reviewer-role": {
        source: "inline",
        content: `You are the REVIEWER in a dev team.

${VM_ENV}

ON STARTUP (no messages): Say "Reviewer standing by." — no tool calls, end your turn.

WHEN YOU RECEIVE A REVIEW REQUEST via message:
1. Read source files in /workspace/src/
2. Run the code with shell to verify correctness
3. Write review to /workspace/review.md using write_file
4. Send review results to the Alpha Meerkat: send(to: "dev-team/orchestrator/orchestrator", kind: "peer_message", body: "Review complete. <summary of findings>")

Tools: shell, write_file, read_file, send.`,
      },
    },
    backend: { default: "subagent" },
  };
}

// ── MobOrchestrator ─────────────────────────────────────────────────────────

const AGENTS = ["orchestrator", "planner", "coder", "reviewer"] as const;

export class MobOrchestrator {
  private runtime: MobRuntime;
  private panels: Map<string, PanelState>;
  private mobId = "";
  private pollInterval: ReturnType<typeof setInterval> | null = null;

  constructor(runtime: MobRuntime, panels: Map<string, PanelState>) {
    this.runtime = runtime;
    this.panels = panels;
  }

  /**
   * Initialize the mob runtime and create + spawn the dev team.
   * Must be called after WebCM tools are registered.
   */
  async init(keys: ApiKeys, models: ModelAssignments): Promise<void> {
    // Build config with all available API keys
    const config: Record<string, any> = { model: models.main, max_sessions: 32 };
    if (keys.anthropic) config.anthropic_api_key = keys.anthropic;
    if (keys.openai) config.openai_api_key = keys.openai;
    if (keys.gemini) config.gemini_api_key = keys.gemini;

    this.runtime.init_runtime_from_config(JSON.stringify(config));

    // Create mob
    const definition = buildMobDefinition(models);
    const createResult = await this.runtime.mob_create(JSON.stringify(definition));
    const resultStr = typeof createResult === "string" ? createResult : String(createResult);
    try {
      const parsed = JSON.parse(resultStr);
      this.mobId = parsed.mob_id || parsed.id || MOB_ID;
    } catch {
      this.mobId = resultStr || MOB_ID;
    }

    // Spawn all agents
    const specs = AGENTS.map((profile) => ({
      profile,
      meerkat_id: profile,
      runtime_mode: "autonomous_host" as const,
    }));
    await this.runtime.mob_spawn(this.mobId, JSON.stringify(specs));

    // Wire all pairs (definition already declares wiring, but explicit wire
    // ensures the comms trust is established even if definition wiring is lazy)
    const wirePairs = AGENTS.flatMap((a, i) => AGENTS.slice(i + 1).map(b => [a, b] as const));
    await Promise.all(wirePairs.map(([a, b]) =>
      this.runtime.mob_wire(this.mobId, a, b).catch(() => {/* already wired */}),
    ));

    // Subscribe to each agent's events (parallel — independent operations)
    await Promise.all(AGENTS.map(async (agent) => {
      const handle = await this.runtime.mob_member_subscribe(this.mobId, agent);
      const panel = this.panels.get(agent);
      if (panel) {
        panel.subHandle = handle;
        this.updateStatus(agent, "spawned", "thinking");
        setTimeout(() => this.updateStatus(agent, "idle"), 2000);
      }
    }));
  }

  /** Send a user message to the orchestrator mob member. */
  async sendToOrchestrator(message: string): Promise<void> {
    await this.runtime.mob_send_message(this.mobId, "orchestrator", message);
  }

  /** Start background polling for sub-agent events. */
  startPolling(): void {
    if (this.pollInterval) return;
    this.pollInterval = setInterval(() => this.pollAll(), 200);
  }

  /** Stop background polling. */
  stopPolling(): void {
    if (this.pollInterval) {
      clearInterval(this.pollInterval);
      this.pollInterval = null;
    }
  }

  /** Clear any pending tool call spinner on a panel. */
  private clearCard(panel: PanelState, output = "", isError = false): void {
    if (panel.currentCard) {
      panel.stream.resolveToolCall(panel.currentCard, output, isError);
      panel.currentCard = null;
    }
  }

  private pollAll(): void {
    for (const agent of AGENTS) {
      const panel = this.panels.get(agent);
      if (!panel || panel.subHandle == null) continue;

      try {
        const raw = this.runtime.poll_subscription(panel.subHandle) as string;
        if (!raw || raw === "[]") continue; // fast path: no events
        const envelopes: any[] = JSON.parse(raw);
        for (const envelope of envelopes) {
          this.routeEnvelope(agent, envelope, panel);
        }
      } catch (e) {
        // Expected: subscription not ready or lagged. Log unexpected errors.
        if (e instanceof SyntaxError) console.warn(`[${agent}] poll parse error:`, e);
      }
    }
  }

  private routeEnvelope(agent: string, envelope: any, panel: PanelState): void {
    // EventEnvelope<AgentEvent> — payload is the AgentEvent
    const ev = envelope.payload || envelope;

    switch (ev.type) {
      case "reasoning_delta":
        panel.stream.appendTextDelta(ev.delta);
        break;

      case "reasoning_complete":
        panel.stream.finalizeText(ev.content);
        break;

      case "text_delta":
        panel.stream.appendTextDelta(ev.delta);
        break;

      case "text_complete":
        panel.stream.finalizeText(ev.content);
        this.updateStatus(agent, "idle");
        break;

      case "tool_call_requested":
        panel.currentCard = panel.stream.beginToolCall(ev.name, ev.args);
        this.updateStatus(agent, `${ev.name}`, "tool-use");
        break;

      case "tool_execution_completed":
        this.clearCard(panel, ev.result || "", ev.is_error || false);
        break;

      case "tool_result_received":
        this.clearCard(panel); // fallback if tool_execution_completed was missed
        break;

      case "tool_execution_started":
        this.updateStatus(agent, ev.name, "tool-use");
        break;

      case "run_started": {
        // Show the incoming prompt as a collapsed card.
        // Comms messages contain peer addresses (e.g. "dev-team/planner/planner").
        // User messages (via mob_send_message) are raw text — already shown as ❯ line.
        const isComms = ev.prompt && ev.prompt.includes(`${this.mobId}/`);
        if (ev.prompt && (agent !== "orchestrator" || isComms)) {
          const card = panel.stream.beginToolCall("message received", { text: ev.prompt });
          panel.stream.resolveToolCall(card, ev.prompt, false);
        }
        this.updateStatus(agent, "thinking", "thinking");
        break;
      }

      case "turn_started":
        this.updateStatus(agent, "thinking", "thinking");
        break;

      case "turn_completed":
      case "run_completed":
        this.clearCard(panel); // clear any lingering tool spinners
        this.updateStatus(agent, "idle");
        break;

      case "run_failed":
        panel.stream.appendError(ev.error || "Run failed");
        this.updateStatus(agent, "error", "error");
        break;
    }
  }

  private updateStatus(agent: string, text: string, cls?: string): void {
    const panel = this.panels.get(agent);
    if (!panel) return;
    panel.statusEl.textContent = `\u25CF ${text}`;
    panel.statusEl.className = "agent-status";
    if (cls) panel.statusEl.classList.add(cls);
  }
}
