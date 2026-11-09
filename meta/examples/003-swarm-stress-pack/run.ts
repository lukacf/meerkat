import { existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

import {
  Meta,
  type AgentBuildContext,
  type AgentBuildDraft,
  type AgentCustomizer,
  type DurableAgentSpec,
  type ManagedPeerEdge,
  type MemberSnapshot,
  type MobHandle,
  type RosterProvider,
  type SessionAgentBuilder,
  type SessionBuildOptions,
  type TopologyProvider,
} from "../../sdk/typescript/src/index.ts";

import {
  BASE_TOTAL,
  BURST_TOTAL,
  MAX_SESSIONS,
  PEER_FANOUT_PER_PARENT,
  SEND_CONCURRENCY,
  SPAWN_CONCURRENCY,
  SWARM_STRESS_MODEL,
  computeEdges,
  contextFor,
  generateAllAgents,
  generateBaseAgents,
  generateBurstAgents,
  generateBurstParents,
  groupBurstAgentsByParent,
  labelsFor,
  peersFor,
  runBounded,
  type SwarmAgentSpec,
} from "./scenario.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../..");
const configDir = join(here, "config");

function sleep(ms: number): Promise<void> {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
}

function countUndirectedWires(members: readonly MemberSnapshot[]): number {
  const active = new Set(members.map((member) => member.agentIdentity));
  const edges = new Set<string>();
  for (const member of members) {
    for (const peer of member.wiredTo) {
      if (!active.has(peer)) continue;
      const [a, b] =
        member.agentIdentity <= peer
          ? [member.agentIdentity, peer]
          : [peer, member.agentIdentity];
      edges.add(`${a}\u0000${b}`);
    }
  }
  return edges.size;
}

const DENSE_RESTORE_EDGE_FLOOR = Math.floor(
  (BASE_TOTAL * PEER_FANOUT_PER_PARENT) / 2,
);

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

function specFor(
  agent: SwarmAgentSpec,
  edges: readonly ManagedPeerEdge[],
): DurableAgentSpec {
  const peers = peersFor(agent.identity, edges);
  return {
    identity: agent.identity,
    profile: agent.profile,
    addressability: "addressable",
    displayName: agent.displayName,
    labels: labelsFor(agent),
    context: contextFor(agent, peers),
    additionalInstructions: [
      `You are ${agent.displayName}, a ${agent.profile} in ${agent.consoleGroup}.`,
      `Stress lane: ${agent.lane}. Shard: ${agent.shard}. Wave: ${agent.wave}.`,
      `Use model ${SWARM_STRESS_MODEL}. This run must use a real Gemini-backed provider.`,
    ],
  };
}

class SwarmRosterProvider implements RosterProvider {
  constructor(private readonly includeBurst: boolean) {}

  async roster(): Promise<DurableAgentSpec[]> {
    const agents = this.includeBurst
      ? generateAllAgents()
      : generateBaseAgents();
    const edges = computeEdges(agents.map((agent) => agent.identity));
    return agents.map((agent) => specFor(agent, edges));
  }
}

class SwarmTopologyProvider implements TopologyProvider {
  private denseBaseEnabled = false;

  enableDenseBase(): void {
    this.denseBaseEnabled = true;
  }

  async computeEdges(targetIdentities: string[]): Promise<ManagedPeerEdge[]> {
    if (this.denseBaseEnabled) return computeEdges(targetIdentities);
    return [];
  }
}

class SwarmCustomizer implements AgentCustomizer {
  async customizeBuild(
    context: AgentBuildContext,
    spec: DurableAgentSpec,
    draft: AgentBuildDraft,
  ): Promise<void> {
    const all = new Map(
      generateBurstAgents()
        .concat(generateBaseAgents())
        .map((agent) => [agent.identity, agent]),
    );
    const agent = all.get(spec.identity);
    if (!agent) return;
    const edgeTargets =
      agent.wave === "base" ? generateBaseAgents() : generateAllAgents();
    const peers = peersFor(
      spec.identity,
      computeEdges(edgeTargets.map((candidate) => candidate.identity)),
    );
    draft.labels = {
      ...draft.labels,
      ...labelsFor(agent),
      peer_count: String(peers.length),
    };
    draft.appContext = contextFor(agent, peers);
    draft.additionalInstructions = [
      ...draft.additionalInstructions,
      "This is Example 3, a high-cardinality Meta and Meerkat stress run.",
      "Keep outputs compact so the test stresses session/timeline mechanics, not token volume.",
      `Visible peer count for this build: ${peers.length}.`,
    ];
  }

  async afterCreate(identity: string, sessionId: string): Promise<void> {
    console.log(`[swarm-stress] seated ${identity} as session ${sessionId}`);
  }
}

function stressTools(
  context: Record<string, unknown>,
): Record<string, (args: Record<string, unknown>) => unknown> {
  return {
    stress_ping(args) {
      return {
        tool: "stress_ping",
        identity: context.identity,
        swarmMob: context.swarmMob,
        shard: context.shard,
        wave: context.wave,
        nonce: args.nonce ?? "none",
        ok: true,
      };
    },
    shard_digest(args) {
      return {
        tool: "shard_digest",
        identity: context.identity,
        shard: context.shard,
        requestedBy: args.requested_by ?? "operator",
        summary: `${context.swarmMob}/${context.shard}/${context.wave} ready`,
      };
    },
    fanout_ack(args) {
      return {
        tool: "fanout_ack",
        identity: context.identity,
        burst: args.burst ?? "manual",
        cohort: args.cohort ?? context.swarmMob,
        received: true,
      };
    },
  };
}

class SwarmSessionBuilder implements SessionAgentBuilder {
  async buildAgent(options: SessionBuildOptions): Promise<void> {
    const context = (options.appContext ?? {}) as Record<string, unknown>;
    options.additionalInstructions.push(
      "The stress tools are synthetic and safe to call repeatedly.",
      "Prefer one-sentence replies after tool use.",
    );
    options.labels = {
      ...options.labels,
      sdk_toolbelt: "swarm-stress",
    };
    for (const [name, handler] of Object.entries(stressTools(context))) {
      options.registerTool(name, handler);
    }
  }
}

export async function spawnBurst(handle: MobHandle): Promise<MemberSnapshot[]> {
  const burst = generateBurstAgents();
  const all = generateBaseAgents().concat(burst);
  const edges = computeEdges(all.map((agent) => agent.identity));
  const created: MemberSnapshot[] = new Array(burst.length);
  await runBounded(burst, SPAWN_CONCURRENCY, async (agent, index) => {
    created[index] = await handle.ensureMember(agent.identity, agent.profile, {
      labels: labelsFor(agent),
      context: contextFor(agent, peersFor(agent.identity, edges)),
      additionalInstructions: [
        `You were dynamically spawned as a real Meerkat sub-agent in the ${agent.consoleGroup} burst wave.`,
        "Your creation should become visible in roster and aggregate console history without app refresh calls.",
      ],
    });
    if ((index + 1) % 40 === 0 || index + 1 === burst.length) {
      console.log(`[swarm-stress] burst seated ${index + 1}/${burst.length}`);
    }
  });
  return created;
}

async function runParentBurstAction(
  handle: MobHandle,
  parent: SwarmAgentSpec,
  children: readonly SwarmAgentSpec[],
  parentIndex: number,
): Promise<MemberSnapshot[]> {
  const all = generateBaseAgents().concat(generateBurstAgents());
  const edges = computeEdges(all.map((agent) => agent.identity));
  const baseEdges = computeEdges(
    generateBaseAgents().map((agent) => agent.identity),
  );
  const parentPeers = peersFor(parent.identity, baseEdges).slice(
    0,
    PEER_FANOUT_PER_PARENT,
  );
  console.log(
    `[swarm-stress] parent ${parent.identity} launching ${children.length} sub-agents after stagger ${parentIndex}`,
  );
  await handle.send(
    parent.identity,
    `stress action ${parentIndex + 1}: spawn ${children.length} sub-agents over a burst window; after their returns, fan out a compact status packet to ${parentPeers.length} wired peers.`,
    { handlingMode: "queue" },
  );
  const created: MemberSnapshot[] = new Array(children.length);
  await runBounded(children, children.length, async (agent, index) => {
    created[index] = await handle.ensureMember(agent.identity, agent.profile, {
      labels: {
        ...labelsFor(agent),
        parent_identity: parent.identity,
        parent_display_name: parent.displayName,
      },
      context: contextFor(agent, peersFor(agent.identity, edges)),
      additionalInstructions: [
        `You were dynamically spawned by parent ${parent.identity} (${parent.displayName}).`,
        "Return a compact status message to your parent when prompted, including your identity, shard, and readiness.",
        "Your creation and reply should become visible in roster and aggregate console history without app refresh calls.",
      ],
    });
  });
  await runBounded(children, SEND_CONCURRENCY, async (agent, index) => {
    await handle.send(
      agent.identity,
      `sub-agent return ${index + 1}/${children.length}: send a compact return for parent=${parent.identity}; include identity=${agent.identity}, shard=${agent.shard}, and wave=${agent.wave}.`,
      { handlingMode: "queue" },
    );
  });
  await handle.send(
    parent.identity,
    `sub-agent returns received for stress action ${parentIndex + 1}: ${children
      .map((child) => `${child.identity}/${child.shard}`)
      .join(", ")}. Now publish a compact fanout packet to your wired peers.`,
    { handlingMode: "queue" },
  );
  await runBounded(
    parentPeers,
    SEND_CONCURRENCY,
    async (peerIdentity, peerIndex) => {
      await handle.send(
        peerIdentity,
        `peer fanout from ${parent.identity} action ${parentIndex + 1}: acknowledge parent=${parent.identity}, peer_index=${peerIndex + 1}/${parentPeers.length}, burst_children=${children.length}.`,
        { handlingMode: "queue" },
      );
    },
  );
  console.log(
    `[swarm-stress] parent ${parent.identity} completed ${children.length} sub-agents and ${parentPeers.length} peer fanouts`,
  );
  return created;
}

export async function runBurstyParentActions(
  handle: MobHandle,
): Promise<MemberSnapshot[]> {
  const parents = generateBurstParents();
  const groups = groupBurstAgentsByParent();
  const created = await Promise.all(
    parents.map(async (parent, index) => {
      await sleep(index * 300);
      return runParentBurstAction(
        handle,
        parent,
        groups.get(parent.identity) ?? [],
        index,
      );
    }),
  );
  return created.flat();
}

export async function sendBurstProbe(handle: MobHandle): Promise<void> {
  const sample = generateBurstAgents()
    .filter((_, index) => index % 12 === 0)
    .slice(0, 24);
  await runBounded(sample, SEND_CONCURRENCY, async (agent, index) => {
    await handle.send(
      agent.identity,
      `fanout probe ${index + 1}: report mob=${agent.swarmMob} shard=${agent.shard} wave=${agent.wave}`,
      { handlingMode: "queue" },
    );
  });
}

async function main() {
  const args = new Set(process.argv.slice(2));
  const once = args.has("--once") || args.has("--smoke");
  const kickoff = args.has("--kickoff");
  const autoBurst = args.has("--autoburst");
  const skipBuild = args.has("--skip-build");
  const forceDemoLlm = args.has("--demo-llm");
  const forceRealLlm = args.has("--real-llm");
  if (forceDemoLlm && forceRealLlm) {
    throw new Error("--demo-llm and --real-llm are mutually exclusive");
  }
  // Env opt-out of real provider calls (same effect as --demo-llm) so keyed
  // shells can run shape-only without editing the command line. An explicit
  // --real-llm flag overrides it.
  const envDemoLlm = process.env.META_EXAMPLE_DEMO_LLM === "1";
  const useDemoLlm = forceDemoLlm || (!forceRealLlm && envDemoLlm);
  if (
    !useDemoLlm &&
    !process.env.GEMINI_API_KEY &&
    !process.env.GOOGLE_API_KEY
  ) {
    throw new Error(
      "Example 003 requires a real Gemini provider for gemini-3.1-flash-lite-preview. Set GEMINI_API_KEY or GOOGLE_API_KEY, or pass --demo-llm only for an explicitly shape-only smoke.",
    );
  }
  const stateDir = join(here, ".state");
  const keepState = Boolean(process.env.META_KEEP_EXAMPLE_STATE);
  const hadPersistentState =
    existsSync(join(stateDir, "runtime.sqlite")) ||
    existsSync(join(stateDir, "meta_console.sqlite"));
  if (!process.env.META_KEEP_EXAMPLE_STATE) {
    rmSync(stateDir, { recursive: true, force: true });
  }
  mkdirSync(stateDir, { recursive: true });
  const restoreBurst =
    process.env.META_SWARM_RESTORE_BURST === "1" ||
    (process.env.META_SWARM_RESTORE_BURST !== "0" &&
      keepState &&
      hadPersistentState);

  if (useDemoLlm) {
    console.log(
      "[swarm-stress] using deterministic demo LLM (shape-only, not the real stress scenario)",
    );
  } else {
    const keySource = process.env.GEMINI_API_KEY
      ? "GEMINI_API_KEY"
      : "GOOGLE_API_KEY";
    console.warn(
      `[swarm-stress] WARNING: real ${SWARM_STRESS_MODEL} provider calls will be made via ${keySource}` +
        ` (hundreds of agents; this burns real tokens${forceRealLlm ? "" : "; implicit because the key is set"}).` +
        " Pass --demo-llm or set META_EXAMPLE_DEMO_LLM=1 for a deterministic shape-only run.",
    );
  }

  const topologyProvider = new SwarmTopologyProvider();
  const skipDense = process.env.META_SWARM_SKIP_DENSE === "1";
  let builder = Meta.builder()
    .mob(join(configDir, "mob.toml"))
    .gateway(ensureGatewayBin(skipBuild))
    .consoleConfig(join(configDir, "console.toml"))
    .consoleAuthRequired(false)
    .maxSessions(MAX_SESSIONS)
    .gatewayTimeoutMs(1_200_000)
    .persistentState(stateDir)
    .sessionService(new SwarmSessionBuilder())
    .rosterProvider(new SwarmRosterProvider(restoreBurst))
    .agentCustomizer(new SwarmCustomizer());
  if (!skipDense) {
    builder = builder.topologyProvider(topologyProvider);
  }
  if (useDemoLlm) {
    builder = builder.demoLlm();
  }
  const runtime = await builder.build();

  try {
    const handle = runtime.mobHandle();
    let members = await handle.listMembers();
    const restoredEdgeCount = countUndirectedWires(members);
    const shouldApplyDense =
      !skipDense &&
      !(
        keepState &&
        hadPersistentState &&
        restoredEdgeCount >= DENSE_RESTORE_EDGE_FLOOR
      );
    let deferredDenseApply: (() => Promise<unknown>) | null = null;
    if (shouldApplyDense) {
      topologyProvider.enableDenseBase();
      const applyDense = async () => {
        console.log(
          `[swarm-stress] applying dense baseline topology (${BASE_TOTAL} agents x ${PEER_FANOUT_PER_PARENT} peers)`,
        );
        const identityReport = (await runtime.reconcileIdentity()) as Record<
          string,
          unknown
        >;
        console.log(
          `[swarm-stress] dense topology managed_edges=${identityReport.managed_edges ?? "unknown"}`,
        );
      };
      if (keepState && hadPersistentState) {
        console.log(
          `[swarm-stress] will apply dense baseline topology in background; restored ${restoredEdgeCount} edges before reapply`,
        );
        deferredDenseApply = applyDense;
      } else {
        await applyDense();
      }
    } else {
      const reason = skipDense
        ? "requested by META_SWARM_SKIP_DENSE"
        : `restored ${restoredEdgeCount} existing topology edges`;
      console.log(`[swarm-stress] skipping dense topology reapply (${reason})`);
    }
    await handle.setMobLabels({
      example_pack: "003-swarm-stress",
      base_agents: String(BASE_TOTAL),
      dynamic_agents: String(BURST_TOTAL),
      base_peer_degree: String(PEER_FANOUT_PER_PARENT),
      max_sessions: String(MAX_SESSIONS),
    });

    members = await handle.listMembers();
    const baseUrl = runtime.rustHttpBaseUrl;
    console.log(`[swarm-stress] bootstrap seated ${members.length} agents`);
    if (restoreBurst) {
      console.log(
        "[swarm-stress] restored dynamic burst roster from persistent state",
      );
    }
    console.log(
      `[swarm-stress] configured burst ${BURST_TOTAL} sub-agents at concurrency ${SPAWN_CONCURRENCY}`,
    );
    console.log(`[swarm-stress] console: ${baseUrl}/console`);

    if (baseUrl) {
      const response = await fetch(`${baseUrl}/console/experience`);
      if (!response.ok) {
        throw new Error(
          `console experience returned ${response.status}: ${await response.text()}`,
        );
      }
      const experience = (await response.json()) as Record<string, unknown>;
      const config = experience.console_config as
        | Record<string, unknown>
        | undefined;
      console.log(`[swarm-stress] console title: ${config?.title ?? "stock"}`);
    }

    const denseApply = deferredDenseApply?.().catch((error: unknown) => {
      console.error("[swarm-stress] dense topology background apply failed", error);
      process.exitCode = 1;
    }) ?? null;

    if (denseApply && (autoBurst || kickoff)) {
      await denseApply;
    }

    if (autoBurst) {
      await runBurstyParentActions(handle);
      console.log("[swarm-stress] autoburst completed");
    }

    if (kickoff) {
      console.log("[swarm-stress] sending kickoff to atlas-base-001");
      await handle.send(
        "atlas-base-001",
        "coordinate a four-mob stress audit; keep your reply compact and name any missing burst cohorts",
        { handlingMode: "queue" },
      );
    }

    if (!once) {
      console.log("[swarm-stress] running until Ctrl-C");
      await new Promise<void>((resolveWait) => {
        process.once("SIGINT", resolveWait);
        process.once("SIGTERM", resolveWait);
      });
    } else if (denseApply) {
      await denseApply;
    }
  } finally {
    await runtime.shutdown();
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
