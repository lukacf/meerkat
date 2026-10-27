import assert from "node:assert/strict";

import {
  BASE_PER_MOB,
  BASE_PEERS_PER_AGENT,
  BASE_TOTAL,
  BURST_PARENTS_PER_MOB,
  BURST_TOTAL,
  DYNAMIC_PER_MOB,
  MAX_SESSIONS,
  PEER_FANOUT_PER_PARENT,
  SEND_CONCURRENCY,
  SPAWN_CONCURRENCY,
  SWARM_MOBS,
  SWARM_STRESS_MODEL,
  SUBAGENTS_PER_PARENT,
  TOTAL_AFTER_BURST,
  computeEdges,
  generateAllAgents,
  generateBaseAgents,
  generateBurstAgents,
  generateBurstParents,
  groupBurstAgentsByParent,
  labelsFor,
  peersFor,
} from "./scenario.ts";

const base = generateBaseAgents();
const burst = generateBurstAgents();
const all = generateAllAgents();
const parents = generateBurstParents();
const groups = groupBurstAgentsByParent();

assert.equal(SWARM_MOBS.length, 4);
assert.equal(base.length, SWARM_MOBS.length * BASE_PER_MOB);
assert.equal(burst.length, SWARM_MOBS.length * DYNAMIC_PER_MOB);
assert.equal(BASE_TOTAL, 300);
assert.equal(BURST_TOTAL, 240);
assert.equal(parents.length, SWARM_MOBS.length * BURST_PARENTS_PER_MOB);
assert.equal(DYNAMIC_PER_MOB, BURST_PARENTS_PER_MOB * SUBAGENTS_PER_PARENT);
assert.equal(all.length, TOTAL_AFTER_BURST);
assert.equal(all.length, 540);
assert.equal(MAX_SESSIONS >= all.length, true);
assert.equal(SPAWN_CONCURRENCY >= 80, true);
assert.equal(SEND_CONCURRENCY >= 24, true);
assert.equal(SWARM_STRESS_MODEL, "gemini-3.1-flash-lite-preview");

const identities = new Set(all.map((agent) => agent.identity));
assert.equal(identities.size, all.length);
assert.equal(
  base.filter((agent) => agent.profile === "swarm_coordinator").length,
  SWARM_MOBS.length,
);
assert.equal(
  burst.every((agent) => agent.wave === "burst"),
  true,
);
assert.equal(
  burst.every((agent) => Boolean(agent.parentIdentity)),
  true,
);
assert.equal(
  burst.filter((agent) => agent.profile === "swarm_worker").length,
  80,
);
assert.equal(
  burst.filter((agent) => agent.profile === "swarm_probe").length,
  80,
);
assert.equal(
  burst.filter((agent) => agent.profile === "swarm_auditor").length,
  80,
);

for (const mob of SWARM_MOBS) {
  assert.equal(
    base.filter((agent) => agent.swarmMob === mob.id).length,
    BASE_PER_MOB,
  );
  assert.equal(
    burst.filter((agent) => agent.swarmMob === mob.id).length,
    DYNAMIC_PER_MOB,
  );
  assert.ok(identities.has(`${mob.id}-base-001`));
  assert.ok(identities.has(`${mob.id}-burst-060`));
}

for (const parent of parents) {
  assert.equal(groups.get(parent.identity)?.length, SUBAGENTS_PER_PARENT);
}

const labels = labelsFor(burst[0]);
assert.equal(labels.wave, "burst");
assert.equal(labels.model, SWARM_STRESS_MODEL);
assert.equal(labels.console_group, SWARM_MOBS[0].title);
assert.equal(labels.parent_identity, burst[0].parentIdentity);

const edges = computeEdges(all.map((agent) => agent.identity));
assert.equal(
  edges.length,
  (BASE_TOTAL * BASE_PEERS_PER_AGENT) / 2 + BURST_TOTAL,
);
for (const agent of base) {
  assert.equal(
    peersFor(agent.identity, edges).filter((peer) => peer.includes("-base-"))
      .length,
    BASE_PEERS_PER_AGENT,
  );
}
assert.equal(
  peersFor(parents[0].identity, edges).slice(0, PEER_FANOUT_PER_PARENT).length,
  150,
);
assert.ok(
  edges.some(
    (edge) => edge.a === "atlas-base-001" && edge.b === "borealis-base-001",
  ),
);
assert.ok(
  edges.some(
    (edge) => edge.a === "atlas-base-001" && edge.b === "atlas-burst-001",
  ),
);

console.log("ts-smoke:ok");
