import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import YAML from "yaml";

import {
  Meta,
  type AgentBuildContext,
  type AgentBuildDraft,
  type AgentCustomizer,
  type DurableAgentSpec,
  type ManagedPeerEdge,
  type RosterProvider,
  type SessionAgentBuilder,
  type SessionBuildOptions,
  type TopologyProvider,
} from "../../sdk/typescript/src/index.ts";

type ScenarioAgent = {
  identity: string;
  profile: string;
  display_name: string;
  console_group: string;
  org: string;
  lane: string;
  confidence: string;
  artifact: string;
  addressability: "addressable" | "internal_only";
  watched?: boolean;
  alert_level?: "elevated" | "critical";
  tool_role: string;
  focus_tags?: string[];
  prompts?: Array<{ label: string; value: string }>;
};

type ScenarioSignal = {
  id: string;
  title: string;
  confidence: number;
  tags: string[];
  body: string;
  implication: string;
};

type ScoreDimension = {
  score: number;
  rationale: string;
};

type Scenario = {
  scenario_id: string;
  mission: string;
  default_prompt: string;
  agents: ScenarioAgent[];
  links: Array<[string, string]>;
  signals: ScenarioSignal[];
  scorecard: Record<string, ScoreDimension>;
  red_team: Array<{ risk: string; severity: string; mitigation: string }>;
  experiments: Array<{ name: string; owner: string; window: string; success_metric: string }>;
};

type ToolContext = {
  identity: string;
  lane: string;
  org: string;
  focusTags: string[];
};

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../..");
const configDir = join(here, "config");
const scenarioPath = join(here, "scenario.yaml");
const scenario = YAML.parse(readFileSync(scenarioPath, "utf-8")) as Scenario;

function repoCargoEnv(): Record<string, string> {
  const script = join(repoRoot, "scripts/repo-cargo");
  const result = spawnSync(script, ["--print-env"], {
    cwd: repoRoot,
    encoding: "utf-8",
  });
  if (result.status !== 0) {
    throw new Error(`repo-cargo --print-env failed:\n${result.stderr}`);
  }
  return Object.fromEntries(
    result.stdout
      .trim()
      .split(/\n/)
      .map((line) => line.split("=", 2))
      .filter(([key, value]) => key && value),
  );
}

function ensureGatewayBin(skipBuild: boolean): string {
  if (process.env.META_RPC_GATEWAY_BIN) {
    return process.env.META_RPC_GATEWAY_BIN;
  }
  const env = repoCargoEnv();
  const gateway = join(env.CARGO_TARGET_DIR, "debug", "rpc_gateway");
  if (!skipBuild || !existsSync(gateway)) {
    const result = spawnSync(
      join(repoRoot, "scripts/repo-cargo"),
      ["build", "-p", "meerkat-meta", "--bin", "rpc_gateway"],
      { cwd: repoRoot, stdio: "inherit" },
    );
    if (result.status !== 0) {
      throw new Error("failed to build meerkat-meta rpc_gateway");
    }
  }
  return gateway;
}

function labelsFor(agent: ScenarioAgent): Record<string, string> {
  const labels: Record<string, string> = {
    display_name: agent.display_name,
    role: agent.profile,
    group: agent.console_group,
    console_group: agent.console_group,
    org: agent.org,
    lane: agent.lane,
    confidence: agent.confidence,
    artifact: agent.artifact,
    durable_identity: agent.identity,
    tool_role: agent.tool_role,
  };
  if (agent.addressability === "internal_only") labels.addressable = "false";
  if (agent.watched) labels.console_watched = "true";
  if (agent.alert_level) labels.console_alert_level = agent.alert_level;
  for (const [index, prompt] of (agent.prompts ?? []).entries()) {
    const n = index + 1;
    labels[`console_prompt_${n}_label`] = prompt.label;
    labels[`console_prompt_${n}_value`] = prompt.value;
  }
  return labels;
}

function agentContext(agent: ScenarioAgent, peers: string[]): ToolContext & Record<string, unknown> {
  return {
    identity: agent.identity,
    lane: agent.lane,
    org: agent.org,
    focusTags: agent.focus_tags ?? [],
    mission: scenario.mission,
    peers,
    artifact: agent.artifact,
    toolRole: agent.tool_role,
  };
}

function peerList(identity: string, edges: readonly ManagedPeerEdge[]): string[] {
  const peers = new Set<string>();
  for (const edge of edges) {
    if (edge.a === identity) peers.add(edge.b);
    if (edge.b === identity) peers.add(edge.a);
  }
  return [...peers].sort();
}

class StudioRosterProvider implements RosterProvider {
  async roster(): Promise<DurableAgentSpec[]> {
    const edges = computeEdges(scenario.agents.map((agent) => agent.identity));
    return scenario.agents.map((agent) => ({
      identity: agent.identity,
      profile: agent.profile,
      addressability: agent.addressability,
      displayName: agent.display_name,
      labels: labelsFor(agent),
      context: agentContext(agent, peerList(agent.identity, edges)),
      additionalInstructions: [
        `You are ${agent.display_name} in the ${agent.org} org.`,
        `Your primary lane is ${agent.lane}; expected artifact: ${agent.artifact}.`,
      ],
    }));
  }
}

function computeEdges(targetIdentities: string[]): ManagedPeerEdge[] {
  const targets = new Set(targetIdentities);
  const edges = new Map<string, ManagedPeerEdge>();
  const add = (a: string, b: string) => {
    if (!targets.has(a) || !targets.has(b) || a === b) return;
    const key = [a, b].sort().join("::");
    edges.set(key, { a, b });
  };
  for (const [a, b] of scenario.links) add(a, b);
  for (const agent of scenario.agents) {
    if (agent.identity !== "studio-director") add("studio-director", agent.identity);
    if (agent.identity !== "board-scribe") add("board-scribe", agent.identity);
  }
  return [...edges.values()].sort((left, right) => `${left.a}:${left.b}`.localeCompare(`${right.a}:${right.b}`));
}

class StudioTopologyProvider implements TopologyProvider {
  async computeEdges(targetIdentities: string[]): Promise<ManagedPeerEdge[]> {
    return computeEdges(targetIdentities);
  }
}

class StudioCustomizer implements AgentCustomizer {
  async customizeBuild(
    context: AgentBuildContext,
    spec: DurableAgentSpec,
    draft: AgentBuildDraft,
  ): Promise<void> {
    const agent = scenario.agents.find((candidate) => candidate.identity === spec.identity);
    if (!agent) return;
    const peers = peerList(spec.identity, context.managedEdges);
    draft.labels = {
      ...draft.labels,
      ...labelsFor(agent),
      peer_count: String(peers.length),
    };
    draft.appContext = agentContext(agent, peers);
    draft.additionalInstructions = [
      ...draft.additionalInstructions,
      `Mission: ${scenario.mission}`,
      `Active peer graph for this run: ${peers.join(", ") || "no peers"}.`,
      "Before making a board-facing claim, cite at least one signal ID or name the gap.",
      "If your answer changes launch confidence, send a short note to board-scribe.",
    ];
  }

  async afterCreate(identity: string, sessionId: string): Promise<void> {
    console.log(`[foresight] seated ${identity} as session ${sessionId}`);
  }
}

function normalizeArgs(args: Record<string, unknown>): string {
  return Object.values(args)
    .flatMap((value) => Array.isArray(value) ? value : [value])
    .map((value) => String(value ?? ""))
    .join(" ")
    .toLowerCase();
}

function matchingSignals(args: Record<string, unknown>, context: ToolContext): ScenarioSignal[] {
  const haystack = normalizeArgs(args);
  const requestedTags = new Set([
    ...context.focusTags,
    ...String(args.tags ?? "")
      .split(/[,\s]+/)
      .map((tag) => tag.trim())
      .filter(Boolean),
  ]);
  const scored = scenario.signals.map((signal) => {
    const text = `${signal.title} ${signal.body} ${signal.implication} ${signal.tags.join(" ")}`.toLowerCase();
    let score = signal.confidence;
    for (const tag of signal.tags) if (requestedTags.has(tag)) score += 0.4;
    for (const token of haystack.split(/\W+/).filter((token) => token.length > 3)) {
      if (text.includes(token)) score += 0.08;
    }
    return { signal, score };
  });
  return scored
    .sort((left, right) => right.score - left.score)
    .slice(0, Number(args.limit ?? 3) || 3)
    .map(({ signal }) => signal);
}

function studioTools(context: ToolContext): Record<string, (args: Record<string, unknown>) => unknown> {
  return {
    scan_signal_lake(args) {
      const matches = matchingSignals(args, context);
      return {
        tool: "scan_signal_lake",
        identity: context.identity,
        query: args.query ?? args.angle ?? context.lane,
        matches,
      };
    },
    score_launch_thesis(args) {
      const scores = Object.entries(scenario.scorecard).map(([dimension, entry]) => ({
        dimension,
        score: entry.score,
        rationale: entry.rationale,
      }));
      const weightedAverage = scores.reduce((sum, row) => sum + row.score, 0) / scores.length;
      return {
        tool: "score_launch_thesis",
        identity: context.identity,
        thesis: args.thesis ?? scenario.mission,
        weightedAverage: Number(weightedAverage.toFixed(1)),
        recommendation: weightedAverage >= 6.5
          ? "Launch as a constrained lighthouse program after trust validation."
          : "Delay broad launch until validation improves.",
        scores,
      };
    },
    draft_evidence_card(args) {
      const matches = matchingSignals(args, context);
      return {
        tool: "draft_evidence_card",
        identity: context.identity,
        claim: args.claim ?? `Borealis has a credible ${context.lane} case.`,
        support: matches.map((signal) => ({
          signal_id: signal.id,
          title: signal.title,
          confidence: signal.confidence,
          implication: signal.implication,
        })),
        caveat: "Synthetic evidence pack; use as demo material, not a real market claim.",
      };
    },
    challenge_launch_plan(args) {
      return {
        tool: "challenge_launch_plan",
        identity: context.identity,
        plan: args.plan ?? "Q3 lighthouse launch",
        objections: scenario.red_team,
        mustResolveBeforeBroadLaunch: scenario.red_team
          .filter((risk) => risk.severity === "high")
          .map((risk) => risk.mitigation),
      };
    },
    compose_board_memo(args) {
      const topSignals = matchingSignals(args, context);
      return {
        tool: "compose_board_memo",
        identity: context.identity,
        decision: "Proceed with Q3 lighthouse launch, not broad self-serve release.",
        strongestEvidence: topSignals.map((signal) => `${signal.id}: ${signal.title}`),
        largestRisk: scenario.red_team[0],
        validationSprints: scenario.experiments,
        askOfBoard: "Approve a capped lighthouse motion contingent on citation UX and implementation capacity gates.",
      };
    },
  };
}

class StudioSessionBuilder implements SessionAgentBuilder {
  async buildAgent(options: SessionBuildOptions): Promise<void> {
    const context = (options.appContext ?? {}) as ToolContext;
    const tools = studioTools(context);
    options.additionalInstructions.push(
      "The scenario tools are synthetic but authoritative for this example.",
      "Use tool outputs verbatim for IDs, scores, risks, and experiment names.",
    );
    options.labels = {
      ...options.labels,
      sdk_toolbelt: "foresight-studio",
    };
    for (const [name, handler] of Object.entries(tools)) {
      options.registerTool(name, handler);
    }
  }
}

async function main() {
  const args = new Set(process.argv.slice(2));
  const once = args.has("--once") || args.has("--smoke");
  const kickoff = args.has("--kickoff");
  const skipBuild = args.has("--skip-build");
  const forceDemoLlm = args.has("--demo-llm");
  const forceRealLlm = args.has("--real-llm");
  if (forceDemoLlm && forceRealLlm) {
    throw new Error("--demo-llm and --real-llm are mutually exclusive");
  }
  const useDemoLlm = forceDemoLlm || (!forceRealLlm && !process.env.OPENAI_API_KEY);
  if (!useDemoLlm && !process.env.OPENAI_API_KEY) {
    throw new Error("OPENAI_API_KEY is required for --real-llm");
  }

  const stateDir = join(here, ".state");
  mkdirSync(stateDir, { recursive: true });

  if (useDemoLlm) {
    console.log("[foresight] using deterministic demo LLM");
  }

  let builder = Meta.builder()
    .mob(join(configDir, "mob.toml"))
    .gateway(ensureGatewayBin(skipBuild))
    .consoleConfig(join(configDir, "console.toml"))
    .consoleAuthRequired(false)
    .persistentState(stateDir)
    .sessionService(new StudioSessionBuilder())
    .rosterProvider(new StudioRosterProvider())
    .topologyProvider(new StudioTopologyProvider())
    .agentCustomizer(new StudioCustomizer());
  if (useDemoLlm) {
    builder = builder.demoLlm();
  }
  const runtime = await builder.build();

  try {
    const handle = runtime.mobHandle();
    await handle.setMobLabels({
      example_pack: "002-foresight-studio",
      scenario: scenario.scenario_id,
      mission: "board-readiness",
    });

    const members = await handle.listMembers();
    const baseUrl = runtime.rustHttpBaseUrl;
    console.log(`[foresight] seated ${members.length} studio agents`);
    console.log(`[foresight] console: ${baseUrl}/console`);

    if (baseUrl) {
      const response = await fetch(`${baseUrl}/console/experience`);
      if (!response.ok) {
        throw new Error(`console experience returned ${response.status}: ${await response.text()}`);
      }
      const experience = await response.json() as Record<string, unknown>;
      const config = experience.console_config as Record<string, unknown> | undefined;
      console.log(`[foresight] console title: ${config?.title ?? "stock"}`);
    }

    if (kickoff) {
      console.log("[foresight] sending board-readiness kickoff to studio-director");
      await runtime.send("studio-director", scenario.default_prompt);
    }

    if (!once) {
      console.log("[foresight] running until Ctrl-C");
      await new Promise<void>((resolveWait) => {
        process.once("SIGINT", resolveWait);
        process.once("SIGTERM", resolveWait);
      });
    }
  } finally {
    await runtime.shutdown();
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
