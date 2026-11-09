export const SWARM_STRESS_MODEL = "gemini-3.1-flash-lite-preview";
export const BASE_PER_MOB = 75;
export const BURST_PARENTS_PER_MOB = 3;
export const SUBAGENTS_PER_PARENT = 20;
export const DYNAMIC_PER_MOB = BURST_PARENTS_PER_MOB * SUBAGENTS_PER_PARENT;
export const BASE_TOTAL = BASE_PER_MOB * 4;
export const BURST_TOTAL = DYNAMIC_PER_MOB * 4;
export const TOTAL_AFTER_BURST = BASE_TOTAL + BURST_TOTAL;
export const BASE_PEERS_PER_AGENT = 150;
export const PEER_FANOUT_PER_PARENT = BASE_PEERS_PER_AGENT;
export const SPAWN_CONCURRENCY = 240;
export const SEND_CONCURRENCY = 32;
export const MAX_SESSIONS = 620;

export const SWARM_MOBS = [
  {
    id: "atlas",
    title: "Atlas Load Mob",
    lane: "control-plane-pressure",
    mission: "Track global fan-out progress and session visibility.",
  },
  {
    id: "borealis",
    title: "Borealis Memory Mob",
    lane: "timeline-replay-pressure",
    mission:
      "Exercise late member discovery, backfill, and transcript projection.",
  },
  {
    id: "cygnus",
    title: "Cygnus Comms Mob",
    lane: "peer-comms-pressure",
    mission: "Create broad peer wiring and parallel operator sends.",
  },
  {
    id: "draco",
    title: "Draco Recovery Mob",
    lane: "restart-and-roster-pressure",
    mission: "Keep enough agents live to expose stale roster and watermarks.",
  },
] as const;

const BURST_ROLE_CYCLE = [
  "swarm_worker",
  "swarm_probe",
  "swarm_auditor",
] as const;

export type SwarmMobId = (typeof SWARM_MOBS)[number]["id"];
export type SwarmProfile =
  | "swarm_coordinator"
  | (typeof BURST_ROLE_CYCLE)[number];
export type SwarmWave = "base" | "burst";

export interface SwarmAgentSpec {
  readonly identity: string;
  readonly profile: SwarmProfile;
  readonly displayName: string;
  readonly swarmMob: SwarmMobId;
  readonly consoleGroup: string;
  readonly lane: string;
  readonly shard: string;
  readonly wave: SwarmWave;
  readonly ordinal: number;
  readonly mission: string;
  readonly parentIdentity?: string;
  readonly parentOrdinal?: number;
}

function pad(value: number): string {
  return String(value).padStart(3, "0");
}

function roleFor(index: number): SwarmProfile {
  return BURST_ROLE_CYCLE[index % BURST_ROLE_CYCLE.length];
}

function labelForRole(profile: SwarmProfile): string {
  return profile.replace(/^swarm_/, "").replace(/_/g, " ");
}

function agentForMob(
  mob: (typeof SWARM_MOBS)[number],
  wave: SwarmWave,
  ordinal: number,
  profile: SwarmProfile,
  parent?: SwarmAgentSpec,
): SwarmAgentSpec {
  const shard = `${mob.id}-${String(((ordinal - 1) % 12) + 1).padStart(2, "0")}`;
  return {
    identity: `${mob.id}-${wave}-${pad(ordinal)}`,
    profile,
    displayName: `${mob.title} ${wave === "base" ? "seat" : "sub-agent"} ${pad(ordinal)}`,
    swarmMob: mob.id,
    consoleGroup: mob.title,
    lane: mob.lane,
    shard,
    wave,
    ordinal,
    mission: mob.mission,
    parentIdentity: parent?.identity,
    parentOrdinal: parent?.ordinal,
  };
}

export function generateBaseAgents(): SwarmAgentSpec[] {
  return SWARM_MOBS.flatMap((mob) =>
    Array.from({ length: BASE_PER_MOB }, (_, index) => {
      const ordinal = index + 1;
      return agentForMob(
        mob,
        "base",
        ordinal,
        ordinal === 1 ? "swarm_coordinator" : roleFor(index),
      );
    }),
  );
}

export function generateBurstAgents(): SwarmAgentSpec[] {
  const parentsByMob = new Map<SwarmMobId, SwarmAgentSpec[]>();
  for (const parent of generateBurstParents()) {
    const current = parentsByMob.get(parent.swarmMob) ?? [];
    current.push(parent);
    parentsByMob.set(parent.swarmMob, current);
  }
  return SWARM_MOBS.flatMap((mob) => {
    const parents = parentsByMob.get(mob.id) ?? [];
    return parents.flatMap((parent, parentIndex) =>
      Array.from({ length: SUBAGENTS_PER_PARENT }, (_, childIndex) => {
        const ordinal = parentIndex * SUBAGENTS_PER_PARENT + childIndex + 1;
        return agentForMob(mob, "burst", ordinal, roleFor(ordinal - 1), parent);
      }),
    );
  });
}

export function generateBurstParents(): SwarmAgentSpec[] {
  const parentOrdinals = Array.from(
    { length: BURST_PARENTS_PER_MOB },
    (_, index) => index * 25 + 1,
  );
  return generateBaseAgents().filter(
    (agent) => agent.wave === "base" && parentOrdinals.includes(agent.ordinal),
  );
}

export function groupBurstAgentsByParent(): Map<string, SwarmAgentSpec[]> {
  const groups = new Map<string, SwarmAgentSpec[]>();
  for (const agent of generateBurstAgents()) {
    if (!agent.parentIdentity) continue;
    const current = groups.get(agent.parentIdentity) ?? [];
    current.push(agent);
    groups.set(agent.parentIdentity, current);
  }
  return groups;
}

export function generateAllAgents(): SwarmAgentSpec[] {
  return [...generateBaseAgents(), ...generateBurstAgents()];
}

export function labelsFor(agent: SwarmAgentSpec): Record<string, string> {
  return {
    display_name: agent.displayName,
    role: agent.profile,
    console_group: agent.consoleGroup,
    group: agent.consoleGroup,
    swarm_mob: agent.swarmMob,
    lane: agent.lane,
    shard: agent.shard,
    wave: agent.wave,
    ordinal: String(agent.ordinal),
    model: SWARM_STRESS_MODEL,
    burst_cohort:
      agent.wave === "burst" ? `${agent.swarmMob}-fanout` : "bootstrap",
    parent_identity: agent.parentIdentity ?? "",
    parent_ordinal: agent.parentOrdinal ? String(agent.parentOrdinal) : "",
    console_watched:
      agent.profile === "swarm_coordinator" || agent.ordinal % 25 === 0
        ? "true"
        : "false",
    console_alert_level:
      agent.wave === "burst" && agent.ordinal % 40 === 0
        ? "elevated"
        : "normal",
    role_label: labelForRole(agent.profile),
  };
}

export function contextFor(
  agent: SwarmAgentSpec,
  peers: string[],
): Record<string, unknown> {
  return {
    identity: agent.identity,
    swarmMob: agent.swarmMob,
    lane: agent.lane,
    shard: agent.shard,
    wave: agent.wave,
    mission: agent.mission,
    stressScenario: "example-003-swarm-stress",
    parentIdentity: agent.parentIdentity ?? null,
    peers,
  };
}

export interface ManagedPeerEdge {
  readonly a: string;
  readonly b: string;
}

export function computeEdges(
  targetIdentities: readonly string[],
): ManagedPeerEdge[] {
  const targets = new Set(targetIdentities);
  const base = generateBaseAgents()
    .map((agent) => agent.identity)
    .filter((identity) => targets.has(identity));
  const edges = new Map<string, ManagedPeerEdge>();
  const add = (a: string, b: string) => {
    if (!targets.has(a) || !targets.has(b) || a === b) return;
    const [left, right] = [a, b].sort();
    edges.set(`${left}::${right}`, { a: left, b: right });
  };
  const halfDegree = BASE_PEERS_PER_AGENT / 2;
  for (let i = 0; i < base.length; i += 1) {
    for (let offset = 1; offset <= halfDegree; offset += 1) {
      add(base[i], base[(i + offset) % base.length]);
    }
  }
  for (const child of generateBurstAgents()) {
    if (child.parentIdentity) add(child.identity, child.parentIdentity);
  }
  return [...edges.values()].sort((left, right) =>
    `${left.a}:${left.b}`.localeCompare(`${right.a}:${right.b}`),
  );
}

export function peersFor(
  identity: string,
  edges: readonly ManagedPeerEdge[],
): string[] {
  const peers = new Set<string>();
  for (const edge of edges) {
    if (edge.a === identity) peers.add(edge.b);
    if (edge.b === identity) peers.add(edge.a);
  }
  return [...peers].sort();
}

export async function runBounded<T>(
  items: readonly T[],
  concurrency: number,
  fn: (item: T, index: number) => Promise<void>,
): Promise<void> {
  let next = 0;
  const workers = Array.from(
    { length: Math.min(concurrency, items.length) },
    async () => {
      while (next < items.length) {
        const index = next;
        next += 1;
        await fn(items[index], index);
      }
    },
  );
  await Promise.all(workers);
}
