import { describe, expect, it } from "vitest";

import {
  consoleMobTree,
  listedMobOf,
  mobCreatorNamer,
  mobDisplayName,
  mobFullName,
  mobState,
  mobStateTone,
  mobStateWords,
  topologyInMobs,
  type ConsoleMobEntry,
} from "./mobs";

// A mob has a name for people on every surface: never its id, and never a
// piece of its id.

const GENERATED = "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
const GENERATED_B = "9b8a7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";
const GENERATED_C = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const IMPLICIT = "implicit-0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70";

/** Whether a name shows the id or six characters in a row of it. */
function showsIdPiece(name: string, id: string): boolean {
  const compactName = name.toLowerCase().replace(/[^a-z0-9]+/g, "");
  const compactId = id.toLowerCase().replace(/[^a-z0-9]+/g, "");
  for (let start = 0; start + 6 <= compactId.length; start += 1) {
    if (compactName.includes(compactId.slice(start, start + 6))) return true;
  }
  return false;
}

describe("a mob's name from its id", () => {
  it("reads a readable id in words", () => {
    expect(mobDisplayName("research-team")).toBe("Research team");
    expect(mobDisplayName("research")).toBe("Research team");
    expect(mobDisplayName("release_engineering")).toBe("Release engineering team");
    expect(mobDisplayName("readingRoom")).toBe("Reading room team");
    expect(mobDisplayName("team-1")).toBe("Team 1");
    expect(mobDisplayName("night-team-2")).toBe("Night team 2");
    expect(mobDisplayName("implicit-bias")).toBe("Implicit bias team");
  });

  it("keeps a readable start and drops the generated rest", () => {
    expect(mobDisplayName("research-0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70")).toBe("Research team");
    expect(mobDisplayName("build-20261110")).toBe("Build team");
    expect(mobDisplayName("deploy-20261110")).toBe("Deploy team");
  });

  it("never shows a generated id or a piece of it", () => {
    expect(mobDisplayName(GENERATED)).toBe("Team");
    expect(mobDisplayName("a1b2c3")).toBe("Team");
    expect(mobDisplayName("x9")).toBe("Team");
    expect(mobDisplayName(IMPLICIT)).toBe("Delegated helpers");
    expect(mobDisplayName("council--3f2a9c4e-8d71-4b6a-9e20-5c1d7a4e8b19")).toBe("Council");
    for (const id of [GENERATED, "a1b2c3", IMPLICIT, "research-0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70", "build-20261110"]) {
      expect(showsIdPiece(mobDisplayName(id), id.replace(/^(research|build|implicit)-/, "")), id).toBe(false);
      expect(showsIdPiece(mobDisplayName(id, "Planner"), id.replace(/^(research|build|implicit)-/, "")), id).toBe(false);
    }
  });

  it("names a mob a member created as that member's", () => {
    expect(mobDisplayName("research-team", "Planner")).toBe("Planner's research team");
    expect(mobDisplayName("research", "Planner")).toBe("Planner's research team");
    expect(mobDisplayName(GENERATED, "Planner")).toBe("Planner's team");
    expect(mobDisplayName(IMPLICIT, "Planner")).toBe("Planner's helpers");
    expect(mobDisplayName("night-shift", "Atlas")).toBe("Atlas' night shift team");
  });

  it("reads a state it does not know as unknown, with no word of its own", () => {
    for (const state of ["held", "quarantined", "paused-by-a-later-release", undefined, null, ""]) {
      expect(mobState(state)).toBe("unknown");
      expect(mobStateWords(state)).toBe("Status unknown");
      expect(mobStateTone(state)).toBe("warn");
    }
    expect([mobStateWords("running"), mobStateTone("running")]).toEqual(["Running", "ok"]);
    expect([mobStateWords("stopped"), mobStateTone("stopped")]).toEqual(["Stopped", "crit"]);
  });
});

const AGENTS = [
  { identity: "planner", member_id: "planner", agent_id: "planner", label: "Planner", mob_id: "ops-team" },
  { identity: "research-team/researcher", member_id: "research-team/researcher", agent_id: "research-team/researcher", label: "research-team/researcher", mob_id: "research-team" },
];
const creatorName = mobCreatorNamer(AGENTS, new Set(["research-team"]));

describe("the names of the listed mobs", () => {
  it("names a mob by the row of the member that created it", () => {
    expect(creatorName("planner")).toBe("Planner");
    // A creator's row without a display name reads by its member name.
    expect(creatorName("research-team/researcher")).toBe("Researcher");
    // A creator no row names still reads by its member name, never its identity.
    expect(creatorName("research-team/archivist")).toBe("Archivist");
    const tree = consoleMobTree([
      { mob_id: "ops-team", member_count: 1 },
      { mob_id: "research-team", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1 },
      { mob_id: GENERATED, parent_mob_id: "ops-team", host_identity: "planner", member_count: 1 },
      { mob_id: IMPLICIT, parent_mob_id: "research-team", host_identity: "research-team/researcher", member_count: 1 },
    ], "ops-team", { creatorName });
    expect(tree.map((mob) => [mob.mobId, mob.name, mob.hostName])).toEqual([
      ["ops-team", "Ops team", null],
      ["research-team", "Planner's research team", "Planner"],
      [IMPLICIT, "Researcher's helpers", "Researcher"],
      [GENERATED, "Planner's team", "Planner"],
    ]);
  });

  it("gives mobs with a generated id and no creator named an ordinal, never an id piece", () => {
    const tree = consoleMobTree([
      { mob_id: "ops-team", member_count: 1 },
      { mob_id: GENERATED, member_count: 1 },
      { mob_id: GENERATED_B, member_count: 1 },
    ], "ops-team", { creatorName });
    expect(tree.map((mob) => [mob.mobId, mob.name, mob.detail])).toEqual([
      ["ops-team", "Ops team", null],
      [GENERATED, "Team 1", null],
      [GENERATED_B, "Team 2", null],
    ]);
    // One such mob needs no ordinal.
    const single = consoleMobTree([{ mob_id: "ops-team" }, { mob_id: GENERATED }], "ops-team");
    expect(single.map((mob) => mob.name)).toEqual(["Ops team", "Team"]);
  });

  it("keeps a mob's ordinal while other mobs come and go", () => {
    const ordinals = new Map<string, number>();
    const names = (mobs: ConsoleMobEntry[]) =>
      Object.fromEntries(consoleMobTree(mobs, "ops-team", { ordinals }).map((mob) => [mob.mobId, mob.name]));
    const base = [{ mob_id: "ops-team" }, { mob_id: GENERATED }, { mob_id: GENERATED_B }];
    expect(names(base)).toMatchObject({ [GENERATED]: "Team 1", [GENERATED_B]: "Team 2" });
    // A mob whose id sorts first appears: the others keep their ordinals.
    expect(names([...base, { mob_id: GENERATED_C }])).toMatchObject({
      [GENERATED]: "Team 1",
      [GENERATED_B]: "Team 2",
      [GENERATED_C]: "Team 3",
    });
    // The first mob goes away: the one left keeps its name.
    expect(names([{ mob_id: "ops-team" }, { mob_id: GENERATED_B }])).toMatchObject({ [GENERATED_B]: "Team 2" });
  });

  it("tells mobs of one name apart by where they are nested, else by an ordinal", () => {
    const tree = consoleMobTree([
      { mob_id: "ops-team", member_count: 1 },
      { mob_id: "research", member_count: 1 },
      { mob_id: "research_team", member_count: 1 },
      { mob_id: "library", parent_mob_id: "ops-team", member_count: 1 },
      { mob_id: "notes", parent_mob_id: "ops-team", member_count: 1 },
      { mob_id: "notes-team", parent_mob_id: "library", member_count: 1 },
      { mob_id: IMPLICIT, parent_mob_id: "ops-team", host_identity: "planner", member_count: 1 },
      { mob_id: "implicit-0193b2a5-11aa-7bbb-8ccc-0d1e2f3a4b5c", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1 },
    ], "ops-team", { creatorName });
    const byId = Object.fromEntries(tree.map((mob) => [mob.mobId, mob]));
    expect([byId.research.name, byId.research.detail]).toEqual(["Research team", "1 of 2"]);
    expect([byId.research_team.name, byId.research_team.detail]).toEqual(["Research team", "2 of 2"]);
    expect([byId.notes.name, byId.notes.detail]).toEqual(["Notes team", "in Ops team"]);
    expect([byId["notes-team"].name, byId["notes-team"].detail]).toEqual(["Notes team", "in Library team"]);
    expect([byId[IMPLICIT].name, byId[IMPLICIT].detail]).toEqual(["Planner's helpers", "1 of 2"]);
    expect(mobFullName(byId[IMPLICIT])).toBe("Planner's helpers (1 of 2)");
    expect([byId.library.name, byId.library.detail, mobFullName(byId.library)]).toEqual(["Library team", null, "Library team"]);
    for (const mob of tree) {
      expect(showsIdPiece(mobFullName(mob), mob.mobId.replace(/^implicit-/, "")) && /\d/.test(mob.mobId), mob.mobId).toBe(false);
    }
  });
});

describe("a row whose mob the console does not list", () => {
  const listed = new Set(["ops-team", "research-team"]);

  it("is shown in the console's own mob under every view", () => {
    expect(listedMobOf({ mob_id: "research-team" }, listed, "ops-team")).toBe("research-team");
    expect(listedMobOf({ mob_id: "gone" }, listed, "ops-team")).toBe("ops-team");
    expect(listedMobOf({}, listed, "ops-team")).toBe("ops-team");
    // Without the console's own mob in the list: the first mob listed.
    expect(listedMobOf({ mob_id: "gone" }, new Set(["research-team"]), "ops-team")).toBe("research-team");
  });

  it("stays in the topology of the console's own mob, and of no other", () => {
    const agents = [
      { identity: "planner", member_id: "planner", agent_id: "planner", mob_id: "ops-team" },
      { identity: "stray", member_id: "stray", agent_id: "stray", mob_id: "gone" },
      { identity: "research-team/reviewer", member_id: "research-team/reviewer", agent_id: "research-team/reviewer", mob_id: "research-team" },
    ];
    const nodes = agents.map((agent) => ({ identity: agent.identity, mob_id: agent.mob_id }));
    const own = topologyInMobs({ nodes, agents, mobIds: new Set(["ops-team"]), primaryMobId: "ops-team", listedMobIds: listed });
    expect(own.agents.map((agent) => agent.identity)).toEqual(["planner", "stray"]);
    expect(own.nodes.map((node) => node.identity)).toEqual(["planner", "stray"]);
    const nested = topologyInMobs({ nodes, agents, mobIds: new Set(["research-team"]), primaryMobId: "ops-team", listedMobIds: listed });
    expect(nested.agents.map((agent) => agent.identity)).toEqual(["research-team/reviewer"]);
  });
});
