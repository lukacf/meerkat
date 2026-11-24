import { describe, expect, it } from "vitest";

import {
  MOB_VIEW_STORAGE_PREFIX,
  agentMobId,
  consoleMobTree,
  mobAndNestedIds,
  mobDisplayName,
  mobStateWords,
  readConsoleMobView,
  topologyInMobs,
  writeConsoleMobView,
  type ConsoleMobEntry,
} from "./mobs";

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() {
      return values.size;
    },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => Array.from(values.keys())[index] ?? null,
    removeItem: (key) => {
      values.delete(key);
    },
    setItem: (key, value) => {
      values.set(key, String(value));
    },
  };
}

describe("mob names", () => {
  it("reads a mob id as words, as Memory and the topology name a team", () => {
    expect(mobDisplayName("ops-team")).toBe("Ops team");
    expect(mobDisplayName("research")).toBe("Research team");
    expect(mobDisplayName("release_engineering")).toBe("Release engineering team");
    expect(mobDisplayName("readingRoom")).toBe("Reading room team");
    expect(mobDisplayName("")).toBe("Team");
    expect(mobDisplayName(undefined)).toBe("Team");
  });

  it("names a council, the delegate tool's helpers and a generated id without showing the id", () => {
    expect(mobDisplayName("council--3f2a9c4e-8d71-4b6a-9e20-5c1d7a4e8b19")).toBe("Council");
    expect(mobDisplayName("implicit-0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70")).toBe("Delegated helpers");
    expect(mobDisplayName("6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f")).toBe("Team");
    for (const id of [
      "council--3f2a9c4e-8d71-4b6a-9e20-5c1d7a4e8b19",
      "implicit-0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70",
      "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f",
    ]) {
      expect(mobDisplayName(id)).not.toContain(id);
    }
  });

  it("says a mob's state in words, an unknown one as unknown", () => {
    expect(mobStateWords("running")).toBe("Running");
    expect(mobStateWords("stopped")).toBe("Stopped");
    expect(mobStateWords("paused-by-a-later-release")).toBe("Status unknown");
    expect(mobStateWords(undefined)).toBe("Status unknown");
  });
});

const MOBS: ConsoleMobEntry[] = [
  { mob_id: "library", member_count: 1, state: "stopped" },
  { mob_id: "reading-room", parent_mob_id: "research", host_identity: "researcher", member_count: 1, state: "running" },
  { mob_id: "ops-team", member_count: 2, state: "running" },
  { mob_id: "research", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1, state: "running" },
  { mob_id: "archive", parent_mob_id: "ops-team", member_count: 3, state: "running" },
];

describe("the mob tree", () => {
  it("lists mobs in tree order at any depth: the primary mob first, then by name", () => {
    const tree = consoleMobTree(MOBS, "ops-team");
    expect(tree.map((mob) => [mob.mobId, mob.depth, mob.parentMobId])).toEqual([
      ["ops-team", 0, null],
      ["archive", 1, "ops-team"],
      ["research", 1, "ops-team"],
      ["reading-room", 2, "research"],
      ["library", 0, null],
    ]);
    // No creator is named without the roster: the name is the id's words.
    expect(tree.find((mob) => mob.mobId === "research")).toMatchObject({
      name: "Research team",
      detail: null,
      hostIdentity: "planner",
      hostName: null,
      memberCount: 1,
      state: "running",
    });
  });

  it("places a mob whose parent is not listed, or whose parents loop, at the top level", () => {
    const tree = consoleMobTree([
      { mob_id: "orphan", parent_mob_id: "gone", member_count: 1 },
      { mob_id: "loop-a", parent_mob_id: "loop-b", member_count: 1 },
      { mob_id: "loop-b", parent_mob_id: "loop-a", member_count: 1 },
    ]);
    expect(tree.map((mob) => mob.mobId).sort()).toEqual(["loop-a", "loop-b", "orphan"]);
    expect(tree.find((mob) => mob.mobId === "orphan")).toMatchObject({ depth: 0, parentMobId: null });
    expect(tree.filter((mob) => mob.depth === 0).length).toBeGreaterThanOrEqual(2);
    expect(new Set(tree.map((mob) => mob.mobId)).size).toBe(tree.length);
  });

  it("names a mob and the mobs nested under it", () => {
    const tree = consoleMobTree(MOBS, "ops-team");
    expect([...mobAndNestedIds(tree, "research")].sort()).toEqual(["reading-room", "research"]);
    expect([...mobAndNestedIds(tree, "ops-team")].sort()).toEqual(["archive", "ops-team", "reading-room", "research"]);
    expect([...mobAndNestedIds(tree, "library")]).toEqual(["library"]);
    expect([...mobAndNestedIds(tree, "not-listed")]).toEqual([]);
  });

  it("reads an agent's mob, the primary mob for a row that names none", () => {
    expect(agentMobId({ mob_id: "research" }, "ops-team")).toBe("research");
    // The row's own field names its mob: no label does.
    expect(agentMobId({ labels: { source_mob_id: "archive" } } as { mob_id?: string }, "ops-team")).toBe("ops-team");
    expect(agentMobId({}, "ops-team")).toBe("ops-team");
    expect(agentMobId({}, undefined)).toBeUndefined();
  });
});

describe("the mob choice", () => {
  it("narrows the topology to the chosen mobs' agents and nodes", () => {
    const agents = [
      { identity: "planner", member_id: "planner", agent_id: "planner", mob_id: "ops-team" },
      { identity: "researcher", member_id: "researcher", agent_id: "researcher", mob_id: "research" },
      { identity: "archivist", member_id: "archivist", agent_id: "archivist", mob_id: "library" },
    ];
    const nodes = [
      // A topology query node names its member by endpoint key and no mob.
      { identity: "mk1|ops-team|planner", ref: { identity: "planner" } },
      { identity: "researcher", mob_id: "research" },
      { identity: "archivist" },
    ];
    const research = topologyInMobs({ nodes, agents, mobIds: new Set(["research"]), primaryMobId: "ops-team" });
    expect(research.agents.map((agent) => agent.identity)).toEqual(["researcher"]);
    expect(research.nodes.map((node) => node.identity)).toEqual(["researcher"]);
    const primary = topologyInMobs({ nodes, agents, mobIds: new Set(["ops-team", "research"]), primaryMobId: "ops-team" });
    expect(primary.agents.map((agent) => agent.identity)).toEqual(["planner", "researcher"]);
    expect(primary.nodes.map((node) => node.identity)).toEqual(["mk1|ops-team|planner", "researcher"]);
    const all = topologyInMobs({ nodes, agents, mobIds: null, primaryMobId: "ops-team" });
    expect(all.agents).toBe(agents);
    expect(all.nodes).toBe(nodes);
  });

  it("persists one mob or all mobs per user scope", () => {
    const storage = memoryStorage();
    const key = `${MOB_VIEW_STORAGE_PREFIX}:principal-a`;
    expect(readConsoleMobView(storage, key)).toBeNull();
    writeConsoleMobView(storage, key, "research");
    expect(readConsoleMobView(storage, key)).toBe("research");
    expect(JSON.parse(storage.getItem(key) || "{}")).toEqual({ v: 1, mob_id: "research" });
    writeConsoleMobView(storage, key, null);
    expect(readConsoleMobView(storage, key)).toBeNull();
    storage.setItem(key, "not json");
    expect(readConsoleMobView(storage, key)).toBeNull();
  });
});
