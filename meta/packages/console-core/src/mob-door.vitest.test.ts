import { describe, expect, it } from "vitest";

import {
  answerUnderRowKeys,
  createConsoleMobAddressing,
  frameUnderRowKey,
  hostListAgentWords,
  hostListNameAmbiguous,
  listedMobsOf,
  memberDoor,
  memberMobId,
  memberMobNotLoaded,
  memberRowKey,
  mobDoorTable,
  NO_MOB_DOORS,
  type MobDoorSource,
} from "./mob-door";

// The launch mob with a mob a member created under it, a mob the host
// created with a mob nested under it, and a hosted mob that is asleep.
const MOBS: MobDoorSource[] = [
  { mob_id: "ops", provenance: "host", default: true, state: "running" },
  { mob_id: "research", parent_mob_id: "ops", provenance: "member", state: "running" },
  { mob_id: "support", provenance: "host", state: "running" },
  { mob_id: "lab", parent_mob_id: "support", provenance: "member", state: "running" },
  { mob_id: "archive", provenance: "host", state: "dormant" },
  { mob_id: "billing", provenance: "host", state: "unavailable" },
];
const TABLE = mobDoorTable(MOBS, "ops", true);

// The door and the row key are pinned case by case in
// mob-door.shared-cases.vitest.test.ts, over the file the gateway reads
// too. The members below serve the rows about a gateway that does not
// judge a request's mob, and a mob that is asleep.
const MEMBERS: Array<{ rowKey: string; mob_id: string | null; identity: string }> = [
  { rowKey: "reviewer", mob_id: null, identity: "reviewer" },
  { rowKey: "research/scout", mob_id: null, identity: "research/scout" },
  { rowKey: "support/reviewer", mob_id: "support", identity: "reviewer" },
  { rowKey: "support/a/b", mob_id: "support", identity: "a/b" },
  { rowKey: "support/lab/tester", mob_id: "support", identity: "lab/tester" },
  { rowKey: "archive/keeper", mob_id: "archive", identity: "keeper" },
  // A launch member whose name starts like a mob the console does not list.
  { rowKey: "east/reviewer", mob_id: null, identity: "east/reviewer" },
];

describe("the door of a listed mob", () => {
  it("is its own for a mob the host created at the top level, and shared by the mobs nested under it", () => {
    expect([...TABLE.doors]).toEqual([
      ["ops", null],
      ["research", null],
      ["support", "support"],
      ["lab", "support"],
      ["archive", "archive"],
      ["billing", "billing"],
    ]);
    expect(TABLE.launchMobId).toBe("ops");
    expect([...TABLE.notLoaded]).toEqual(["archive", "billing"]);
  });

  it("is the launch mob's for every mob when the gateway does not judge a request's mob", () => {
    const table = mobDoorTable(MOBS, "ops", false);
    expect([...table.doors.values()].every((door) => door === null)).toBe(true);
    for (const member of MEMBERS) {
      expect(memberDoor(member.rowKey, table)).toEqual({ mobId: null, identity: member.rowKey });
    }
  });

  it("is the launch mob's for a top-level mob whose provenance is not the host's, and for a mob marked as the launch mob", () => {
    const table = mobDoorTable([
      { mob_id: "ops" },
      { mob_id: "unplaced" },
      { mob_id: "other", provenance: "something-new" },
      { mob_id: "second", provenance: "host", default: true },
    ], "ops", true);
    expect([...table.doors.values()]).toEqual([null, null, null, null]);
    expect(table.launchMobId).toBe("second");
  });

  it("ends at the mob itself when its parents loop back to it", () => {
    const table = mobDoorTable([
      { mob_id: "a", parent_mob_id: "b", provenance: "host" },
      { mob_id: "b", parent_mob_id: "a", provenance: "host" },
    ], "ops", true);
    expect([...table.doors]).toEqual([["a", "a"], ["b", "b"]]);
  });
});

describe("the request pair of a member", () => {
  it("names the door of a mob that is asleep: a person's request wakes it", () => {
    expect(memberDoor("archive/keeper", TABLE)).toEqual({ mobId: "archive", identity: "keeper" });
  });

  it("names no mob without a table", () => {
    expect(memberDoor("support/reviewer", NO_MOB_DOORS)).toEqual({ mobId: null, identity: "support/reviewer" });
  });
});

describe("the row key of what a frame names", () => {
  it("is the identity for a frame that names no mob or the launch mob", () => {
    expect(memberRowKey({ identity: "reviewer" }, TABLE)).toBe("reviewer");
    expect(memberRowKey({ mob_id: "ops", identity: "reviewer" }, TABLE)).toBe("reviewer");
    expect(memberRowKey({ mob_id: null, identity: "research/scout" }, TABLE)).toBe("research/scout");
  });

  it("never is the key of a launch member of the same name", () => {
    expect(memberRowKey({ mob_id: "support", identity: "reviewer" }, TABLE)).toBe("support/reviewer");
    // A mob the console does not list yet: no row, and its frame is not
    // handed on: neither under the launch member's key nor under one made
    // of the mob's id.
    expect(memberRowKey({ mob_id: "newcomer", identity: "reviewer" }, TABLE)).toBeNull();
    expect(frameUnderRowKey({ event: "text_delta", identity: "reviewer", mobId: "newcomer" }, TABLE)).toBeNull();
    expect(frameUnderRowKey({ event: "frame_updated", identity: "reviewer", mobId: "newcomer", data: { frame: { event: "text_delta", identity: "reviewer" } } }, TABLE)).toBeNull();
    // A hosted member whose name starts like a mob nested under the launch mob.
    expect(memberRowKey({ mob_id: "support", identity: "research/scout" }, TABLE)).toBe("support/research/scout");
  });

  it("is the same whether a frame names the member's own nested mob or the mob of its door", () => {
    // A member of a mob nested under a hosted mob is keyed by the hosted mob's door (contract v0.6.0, row_key).
    expect(memberRowKey({ mob_id: "lab", identity: "lab/tester" }, TABLE)).toBe("support/lab/tester");
    expect(memberRowKey({ mob_id: "lab", identity: "tester" }, TABLE)).toBe("support/lab/tester");
    expect(memberRowKey({ mob_id: "support", identity: "lab/tester" }, TABLE)).toBe("support/lab/tester");
    expect(memberDoor("support/lab/tester", TABLE)).toEqual({ mobId: "support", identity: "lab/tester" });
    expect(memberMobId("support/lab/tester", TABLE)).toBe("lab");
    expect(memberMobId("support/reviewer", TABLE)).toBe("support");
    // A hosted member whose name starts like a mob nested under the launch mob is the hosted mob's.
    expect(memberMobId("support/research/scout", TABLE)).toBe("support");
    expect(memberRowKey({ mob_id: "research", identity: "research/scout" }, TABLE)).toBe("research/scout");
    expect(memberRowKey({ mob_id: "research", identity: "scout" }, TABLE)).toBe("research/scout");
  });

  it("is the identity when the gateway does not judge a request's mob", () => {
    const table = mobDoorTable(MOBS, "ops", false);
    expect(memberRowKey({ mob_id: "support", identity: "reviewer" }, table)).toBe("reviewer");
    expect(memberRowKey({ mob_id: "support", identity: "reviewer" }, NO_MOB_DOORS)).toBe("reviewer");
  });
});

describe("a frame handed on under its member's key", () => {
  it("loses the mob it named, so keying it again changes nothing", () => {
    const frame = { id: "f1", event: "text_delta", identity: "reviewer", mobId: "support", data: { delta: "x" } };
    const keyed = frameUnderRowKey(frame, TABLE);
    expect(keyed).toEqual({ id: "f1", event: "text_delta", identity: "support/reviewer", data: { delta: "x" } });
    expect(frameUnderRowKey(keyed, TABLE)).toBe(keyed);
  });

  it("takes the mob of the door it was read through when it names none", () => {
    expect(frameUnderRowKey({ event: "text_delta", identity: "reviewer" }, TABLE, "support").identity).toBe("support/reviewer");
    expect(frameUnderRowKey({ event: "text_delta", identity: "reviewer" }, TABLE).identity).toBe("reviewer");
  });

  it("keys the frame an update carries the same way", () => {
    const update = {
      event: "frame_updated",
      identity: "reviewer",
      mobId: "support",
      data: { frame: { id: "f1", event: "text_complete", identity: "reviewer", mobId: "support", data: {} } },
    };
    expect(frameUnderRowKey(update, TABLE)).toEqual({
      event: "frame_updated",
      identity: "support/reviewer",
      data: { frame: { id: "f1", event: "text_complete", identity: "support/reviewer", data: {} } },
    });
  });

  it("is the frame itself when the gateway does not judge a request's mob", () => {
    const frame = { event: "text_delta", identity: "reviewer", mobId: "support" };
    expect(frameUnderRowKey(frame, NO_MOB_DOORS)).toBe(frame);
  });
});

describe("an answer of a door other than the launch mob's", () => {
  it("is handed on with the members it names at its top level under the console's keys", () => {
    expect(answerUnderRowKeys({ agent_identity: "helper", side_chat_of: "reviewer", display_name: "Helper" }, "support", TABLE))
      .toEqual({ agent_identity: "support/helper", side_chat_of: "support/reviewer", display_name: "Helper" });
    // A door's schedules name their agent as the door knows it: each is kept
    // under the console's key for that agent; who it sends as is left as it came.
    expect(answerUnderRowKeys({ identity: "reviewer", schedules: [{ identity: "reviewer", sender: "triage" }, { identity: "lab/tester" }, "no schedule"] }, "support", TABLE))
      .toEqual({ identity: "support/reviewer", schedules: [{ identity: "support/reviewer", sender: "triage" }, { identity: "support/lab/tester" }, "no schedule"] });
    expect(answerUnderRowKeys({ schedule: { identity: "reviewer", name: "Morning sort" } }, "support", TABLE))
      .toEqual({ schedule: { identity: "support/reviewer", name: "Morning sort" } });
  });

  it("keys the member an inspect answer names one level down", () => {
    expect(answerUnderRowKeys({ identity: { identity: "reviewer", state: "active" }, peers: [] }, "support", TABLE))
      .toEqual({ identity: { identity: "support/reviewer", state: "active" }, peers: [] });
    // The word a request named its member by names it in the answer too.
    expect(answerUnderRowKeys({ name: "reviewer", profile: "analyst" }, "support", TABLE, "name"))
      .toEqual({ name: "support/reviewer", profile: "analyst" });
    expect(answerUnderRowKeys({ name: "Weekly report" }, "support", TABLE)).toEqual({ name: "Weekly report" });
  });

  it("is left as it came when it names no member in words, or is no object", () => {
    const inspect = { identity: { name: "reviewer" }, state: "active" };
    expect(answerUnderRowKeys(inspect, "support", TABLE)).toBe(inspect);
    const list = ["reviewer"];
    expect(answerUnderRowKeys(list, "support", TABLE)).toBe(list);
    expect(answerUnderRowKeys(null, "support", TABLE)).toBeNull();
    expect(answerUnderRowKeys("reviewer", "support", TABLE)).toBe("reviewer");
  });
});

describe("a member of a mob that is not loaded", () => {
  it("is told apart from a member of a loaded mob", () => {
    expect(memberMobNotLoaded("archive/keeper", TABLE)).toBe(true);
    expect(memberMobNotLoaded("billing/clerk", TABLE)).toBe(true);
    expect(memberMobNotLoaded("support/reviewer", TABLE)).toBe(false);
    expect(memberMobNotLoaded("reviewer", TABLE)).toBe(false);
    expect(memberMobNotLoaded("unlisted/reviewer", TABLE)).toBe(false);
  });

  it("includes a member of a mob that was seen behind a hosted mob's door, while that mob sleeps and does not list it", () => {
    // A gateway lists a mob nested under a hosted mob only while the hosted mob is loaded.
    const asleep: MobDoorSource[] = [
      { mob_id: "ops", provenance: "host", default: true, state: "running" },
      { mob_id: "support", provenance: "host", state: "dormant" },
    ];
    const forgetful = mobDoorTable(asleep, "ops", true);
    expect(memberMobNotLoaded("lab/tester", forgetful)).toBe(false);
    expect(memberDoor("lab/tester", forgetful)).toEqual({ mobId: null, identity: "lab/tester" });

    const table = mobDoorTable(asleep, "ops", true, new Map([["lab", "support"], ["stray", "gone"]]));
    expect(memberMobNotLoaded("lab/tester", table)).toBe(true);
    expect(memberDoor("lab/tester", table)).toEqual({ mobId: "support", identity: "lab/tester" });
    expect(memberRowKey({ mob_id: "support", identity: "lab/tester" }, table)).toBe("support/lab/tester");
    expect(memberMobNotLoaded("support/lab/tester", table)).toBe(true);
    expect(memberDoor("support/lab/tester", table)).toEqual({ mobId: "support", identity: "lab/tester" });
    // A mob remembered behind a door the list does not hold has no door.
    expect(table.doors.has("stray")).toBe(false);
    // Without mob addressing nothing is remembered into the table.
    expect(mobDoorTable(asleep, "ops", false, new Map([["lab", "support"]])).doors.has("lab")).toBe(false);
  });

  it("is remembered by a connection from its own lists, and from what the browser kept, until the hosted mob is loaded again", () => {
    const loaded = MOBS;
    const asleep = MOBS.filter((mob) => mob.mob_id !== "lab").map((mob) => (mob.mob_id === "support" ? { ...mob, state: "dormant" } : mob));
    const mobs = createConsoleMobAddressing();
    mobs.noteFeatures(["mob_addressing"]);
    mobs.noteMobs(loaded, "ops");
    expect(mobs.heldBack("lab/tester")).toBe(false);
    mobs.noteMobs(asleep, "ops");
    expect(mobs.heldBack("lab/tester")).toBe(true);
    expect(mobs.door("lab/tester")).toEqual({ mobId: "support", identity: "lab/tester" });
    // Loaded again without the nested mob: the list says all there is.
    mobs.noteMobs(loaded.filter((mob) => mob.mob_id !== "lab"), "ops");
    expect(mobs.heldBack("lab/tester")).toBe(false);
    expect(mobs.door("lab/tester")).toEqual({ mobId: null, identity: "lab/tester" });

    // After a page load on a sleeping root only the browser's record knows.
    const fresh = createConsoleMobAddressing();
    fresh.noteFeatures(["mob_addressing"]);
    fresh.rememberBehind([["lab", "support"], ["support", "support"], ["research", null]]);
    fresh.noteMobs(asleep, "ops");
    expect(fresh.heldBack("lab/tester")).toBe(true);
    expect(fresh.door("lab/tester")).toEqual({ mobId: "support", identity: "lab/tester" });
    expect(fresh.heldBack("research/scout")).toBe(false);
  });
});

describe("what a connection knows of the gateway's mobs", () => {
  it("names no mob until an answer lists the capability, and says when that changes", () => {
    const mobs = createConsoleMobAddressing();
    let changes = 0;
    const stop = mobs.onChange(() => { changes += 1; });
    mobs.noteMobs(MOBS, "ops");
    expect(mobs.enabled).toBe(false);
    expect(mobs.everyMob()).toEqual({});
    expect(mobs.door("support/reviewer")).toEqual({ mobId: null, identity: "support/reviewer" });

    expect(mobs.listedMobs).toBe(MOBS);
    expect(mobs.noteFeatures(["schedules"])).toBe(false);
    expect(mobs.noteFeatures(["schedules", "mob_addressing"])).toBe(true);
    expect(mobs.noteFeatures(["mob_addressing"])).toBe(false);
    expect(changes).toBe(1);
    expect(mobs.enabled).toBe(true);
    expect(mobs.everyMob()).toEqual({ mobs: "*" });
    // The list read before the capability was known is not kept: it was
    // not a list of every mob the caller may address.
    expect(mobs.listedMobs).toBeNull();
    expect(mobs.door("support/reviewer")).toEqual({ mobId: null, identity: "support/reviewer" });
    mobs.noteMobs(MOBS, "ops");
    expect(mobs.door("support/reviewer")).toEqual({ mobId: "support", identity: "reviewer" });
    expect(mobs.rowKey({ mob_id: "support", identity: "reviewer" })).toBe("support/reviewer");
    expect(mobs.heldBack("archive/keeper")).toBe(true);

    // A gateway that stops listing it is sent no mob again.
    expect(mobs.noteFeatures(undefined)).toBe(true);
    expect(changes).toBe(2);
    expect(mobs.door("support/reviewer")).toEqual({ mobId: null, identity: "support/reviewer" });
    stop();
    mobs.noteFeatures(["mob_addressing"]);
    expect(changes).toBe(2);
  });

  it("tells a console that went on without an answer from one a capability read answered", () => {
    const mobs = createConsoleMobAddressing();
    let told = 0;
    mobs.onKnown(() => { told += 1; });
    expect([mobs.known, mobs.answered]).toEqual([false, false]);
    // The console stops waiting: it goes on as with a gateway that names no
    // such capability, and nothing has said what the gateway hosts.
    mobs.noteUnanswered();
    expect([mobs.known, mobs.answered, mobs.enabled, told]).toEqual([true, false, false, 1]);
    mobs.noteUnanswered();
    expect(told).toBe(1);
    // A read answers after all, with the same word on the capability: the
    // listeners hear it, so the console can read what it held back.
    expect(mobs.noteFeatures([])).toBe(false);
    expect([mobs.known, mobs.answered, mobs.enabled, told]).toEqual([true, true, false, 2]);
    mobs.noteFeatures([]);
    expect(told).toBe(2);

    // An answer that comes first is both at once, and is told once.
    const answered = createConsoleMobAddressing();
    let heard = 0;
    answered.onKnown(() => { heard += 1; });
    answered.noteFeatures(null);
    expect([answered.known, answered.answered, heard]).toEqual([true, true, 1]);
    // Going on without an answer takes nothing back from one.
    answered.noteUnanswered();
    expect([answered.known, answered.answered, heard]).toEqual([true, true, 1]);
  });

  it("knows what the gateway hosts only once a capability read was answered, and reads no list of mobs as an empty one", () => {
    const mobs = createConsoleMobAddressing();
    let known = 0;
    mobs.onKnown(() => { known += 1; });
    expect(mobs.known).toBe(false);
    // An answer that lists no such capability is an answer.
    expect(mobs.noteFeatures([])).toBe(false);
    expect([mobs.known, mobs.enabled, known]).toEqual([true, false, 1]);
    mobs.noteFeatures(["mob_addressing"]);
    expect(known).toBe(1);
    expect(mobs.listedSource).toBeNull();

    // A gateway leaves an empty list of mobs out of its answer.
    const document = { topology: { live_snapshot: { mob_id: "ops" } } };
    const listed = listedMobsOf(document);
    mobs.noteMobs(listed.mobs, listed.primaryMobId, document);
    expect(mobs.listedMobs).toEqual([]);
    expect(mobs.listedSource).toBe(document);
    // The capability going away drops the list: it was read the other way.
    mobs.noteFeatures([]);
    expect([mobs.listedMobs, mobs.listedSource]).toEqual([null, null]);
    // Without the capability no list stays no list.
    mobs.noteMobs(null, "ops", document);
    expect(mobs.listedMobs).toBeNull();
  });

  it("reads the listed mobs and the launch mob from an experience", () => {
    expect(listedMobsOf({ topology: { live_snapshot: { mob_id: " ops ", mobs: MOBS } } })).toEqual({ mobs: MOBS, primaryMobId: "ops" });
    expect(listedMobsOf({ topology: {} })).toEqual({ mobs: null, primaryMobId: null });
    expect(listedMobsOf(null)).toEqual({ mobs: null, primaryMobId: null });
  });
});

describe("a bare name in one of the host's lists, which carry no mob", () => {
  // Every hosted mob loaded: the launch mob and `support`, each with a `reviewer`.
  const LOADED = mobDoorTable(MOBS.filter((mob) => mob.state === "running"), "ops", true);
  const ROWS = ["reviewer", "planner", "research/scout", "support/reviewer", "support/triage", "lab/tester"];

  it("is ambiguous when a listed member behind a door of its own is known to its door by that name", () => {
    expect(hostListNameAmbiguous("reviewer", LOADED, ROWS)).toBe(true);
    expect(hostListNameAmbiguous("triage", LOADED, ROWS)).toBe(true);
    // A member of a mob nested under a hosted mob, as that door knows it.
    expect(hostListNameAmbiguous("lab/tester", LOADED, ROWS)).toBe(true);
  });

  it("can only be the launch door's agent when no other door knows the name and every hosted mob is loaded", () => {
    expect(hostListNameAmbiguous("planner", LOADED, ROWS)).toBe(false);
    expect(hostListNameAmbiguous("research/scout", LOADED, ROWS)).toBe(false);
    expect(hostListNameAmbiguous("nobody", LOADED, ROWS)).toBe(false);
  });

  it("is ambiguous, whatever it is, while a mob with a door of its own is not loaded: its members are not known", () => {
    expect(hostListNameAmbiguous("planner", TABLE, ROWS)).toBe(true);
    expect(hostListNameAmbiguous("nobody", TABLE, ROWS)).toBe(true);
  });

  it("is never ambiguous on a gateway that does not judge a request's mob", () => {
    expect(hostListNameAmbiguous("reviewer", mobDoorTable(MOBS, "ops", false), ROWS)).toBe(false);
    expect(hostListNameAmbiguous("reviewer", NO_MOB_DOORS, ROWS)).toBe(false);
  });

  it("is handed on in words that say what the entry does not, and that are no member's key", () => {
    expect(hostListAgentWords("reviewer")).toBe("reviewer (team not named)");
    expect(memberDoor(hostListAgentWords("reviewer"), LOADED)).toEqual({ mobId: null, identity: "reviewer (team not named)" });
  });
});
