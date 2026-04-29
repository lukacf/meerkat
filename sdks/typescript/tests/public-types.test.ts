import type { SpawnSpec } from "../src/index.js";

const spawnSpec: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-1",
};

void spawnSpec;

const spawnSpecWithGeneration: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-2",
  // @ts-expect-error generation is runtime-owned and not a public spawn knob.
  generation: 1,
};

void spawnSpecWithGeneration;
