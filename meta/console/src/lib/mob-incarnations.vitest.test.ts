import { describe, expect, it } from "vitest";

import { mobDoorTable, type MobDoorSource } from "../../../packages/console-core/src/mob-door";
import { memberKeyPrefixes, memberOfMob, mobIncarnationsStorageKey, readSeenMobs, updateSeenMobs, writeSeenMobs, type SeenMobs } from "./mob-incarnations";
import { consoleComposerDraftKey, consoleSendStorageKey, predecessorConsoleSendStorageKey, purgeConsoleMobKeys } from "./send-attempt-storage";

/** A Storage stand-in that enumerates its keys like the browser's. */
function memoryStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return {
    data,
    get length() { return data.size; },
    key: (index: number) => [...data.keys()][index] ?? null,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => { data.set(key, value); },
    removeItem: (key: string) => { data.delete(key); },
  };
}

const mob = (id: string, incarnation: string | undefined, extra: MobDoorSource = {}): MobDoorSource =>
  ({ mob_id: id, state: "running", ...(incarnation ? { incarnation } : {}), ...extra });
const LISTED: MobDoorSource[] = [
  mob("ops", "ops-1", { provenance: "host", default: true }),
  mob("research", "research-1", { parent_mob_id: "ops", provenance: "member" }),
  mob("support", "support-1", { provenance: "host" }),
  mob("lab", "lab-1", { parent_mob_id: "support", provenance: "member" }),
];
const table = (mobs: MobDoorSource[]) => mobDoorTable(mobs, "ops", true);
const SEEN: SeenMobs = {
  research: { incarnation: "research-1", door: null },
  support: { incarnation: "support-1", door: "support" },
  lab: { incarnation: "lab-1", door: "support" },
};

describe("the mobs a console has seen", () => {
  it("are recorded at first sight, and nothing is forgotten", () => {
    expect(updateSeenMobs({}, LISTED, table(LISTED))).toEqual({ forget: [], seen: SEEN, changed: true });
    expect(updateSeenMobs(SEEN, LISTED, table(LISTED))).toEqual({ forget: [], seen: SEEN, changed: false });
  });

  it("forget a mob created again under its id, and only that mob", () => {
    const listed = LISTED.map((entry) => (entry.mob_id === "support" ? { ...entry, incarnation: "support-2" } : entry));
    const update = updateSeenMobs(SEEN, listed, table(listed));
    expect(update.forget).toEqual(["support"]);
    expect(update.seen.support).toEqual({ incarnation: "support-2", door: "support" });
    expect(update.changed).toBe(true);
  });

  it("forget a mob that left the list, with the mobs that were behind its door", () => {
    const listed = LISTED.filter((entry) => entry.mob_id !== "support" && entry.mob_id !== "lab");
    const update = updateSeenMobs(SEEN, listed, table(listed));
    expect(update.forget.sort()).toEqual(["lab", "support"]);
    expect(Object.keys(update.seen)).toEqual(["research"]);
  });

  it("keep a mob that is asleep, and the mob behind its door that is not listed meanwhile", () => {
    const listed = [LISTED[0]!, LISTED[1]!, { ...LISTED[2]!, state: "dormant" }];
    const update = updateSeenMobs(SEEN, listed, table(listed));
    expect(update).toEqual({ forget: [], seen: SEEN, changed: false });
    // The mob wakes with the same token: still nothing to forget.
    expect(updateSeenMobs(update.seen, LISTED, table(LISTED)).forget).toEqual([]);
  });

  it("keep a mob behind the launch mob's door that one list does not hold: such a mob is listed only while it is held and a member of it is seen", () => {
    const listed = LISTED.filter((entry) => entry.mob_id !== "research");
    const update = updateSeenMobs(SEEN, listed, table(listed));
    expect(update.forget).toEqual([]);
    expect(update.seen.research).toEqual(SEEN.research);
    // It is forgotten when it comes back as another mob.
    const again = LISTED.map((entry) => (entry.mob_id === "research" ? { ...entry, incarnation: "research-2" } : entry));
    expect(updateSeenMobs(update.seen, again, table(again)).forget).toEqual(["research"]);
  });

  it("keep a mob behind a hosted mob's door that a list does not hold while that hosted mob is the same and loaded", () => {
    const listed = LISTED.filter((entry) => entry.mob_id !== "lab");
    const update = updateSeenMobs(SEEN, listed, table(listed));
    expect(update.forget).toEqual([]);
    expect(update.seen.lab).toEqual(SEEN.lab);
  });

  it("forget what sat behind a hosted mob's door when that hosted mob is created again or gone", () => {
    const recreated = LISTED.filter((entry) => entry.mob_id !== "lab")
      .map((entry) => (entry.mob_id === "support" ? { ...entry, incarnation: "support-2" } : entry));
    expect(updateSeenMobs(SEEN, recreated, table(recreated)).forget.sort()).toEqual(["lab", "support"]);
    const gone = LISTED.filter((entry) => entry.mob_id !== "support" && entry.mob_id !== "lab");
    expect(updateSeenMobs(SEEN, gone, table(gone)).forget.sort()).toEqual(["lab", "support"]);
  });

  it("keep no more than a bounded number of mobs that are not listed", () => {
    const many: Record<string, { incarnation: string; door: string | null }> = {};
    for (let index = 0; index < 400; index += 1) many[`council--${index}`] = { incarnation: `token-${index}`, door: null };
    const update = updateSeenMobs({ ...many, ...SEEN }, LISTED, table(LISTED));
    expect(update.forget).toEqual([]);
    const unlisted = Object.keys(update.seen).filter((id) => id.startsWith("council--"));
    expect(unlisted.length).toBe(200);
    // The most recently recorded stay.
    expect(unlisted).toContain("council--399");
    expect(unlisted).not.toContain("council--0");
    expect(Object.keys(update.seen)).toEqual(expect.arrayContaining(["research", "support", "lab"]));
  });

  it("never forget on a list without tokens, and never the launch mob", () => {
    const untold = LISTED.map(({ incarnation: _token, ...entry }) => entry);
    expect(updateSeenMobs({}, untold, table(untold))).toEqual({ forget: [], seen: {}, changed: false });
    const relaunched = LISTED.map((entry) => (entry.mob_id === "ops" ? { ...entry, incarnation: "ops-2" } : entry));
    expect(updateSeenMobs(SEEN, relaunched, table(relaunched)).forget).toEqual([]);
  });

  it("are kept per user scope, and an unreadable record reads as none", () => {
    const storage = memoryStorage();
    const key = mobIncarnationsStorageKey("scope a");
    expect(key).toBe("meta-console-mob-incarnations:scope%20a");
    writeSeenMobs(storage, key, SEEN);
    expect(readSeenMobs(storage, key)).toEqual(SEEN);
    expect(readSeenMobs(storage, mobIncarnationsStorageKey("scope b"))).toEqual({});
    storage.setItem(key, "not json");
    expect(readSeenMobs(storage, key)).toEqual({});
    expect(readSeenMobs(null, key)).toEqual({});
  });

  it("tell a member of a mob by its key", () => {
    expect(memberOfMob("support/reviewer", "support")).toBe(true);
    expect(memberOfMob("support/a/b", "support")).toBe(true);
    expect(memberOfMob("support-lab/tester", "support")).toBe(false);
    expect(memberOfMob("reviewer", "support")).toBe(false);
    expect(memberOfMob(undefined, "support")).toBe(false);
    // A mob nested under a hosted mob: its members are keyed behind the hosted mob's door,
    // and so are of the hosted mob too; a key an earlier console gave them is still read.
    expect(memberOfMob("support/support-lab/tester", "support-lab", "support")).toBe(true);
    expect(memberOfMob("support-lab/tester", "support-lab", "support")).toBe(true);
    expect(memberOfMob("support/support-lab/tester", "support", "support")).toBe(true);
    expect(memberOfMob("support/reviewer", "support-lab", "support")).toBe(false);
    expect(memberOfMob("other/support-lab/tester", "support-lab", "support")).toBe(false);
    expect(memberKeyPrefixes("support-lab", "support")).toEqual(["support/support-lab/", "support-lab/"]);
    expect(memberKeyPrefixes("support", "support")).toEqual(["support/"]);
    expect(memberKeyPrefixes("research", null)).toEqual(["research/"]);
  });
});

describe("forgetting a mob", () => {
  it("removes its members' queued messages and drafts in one scope, and keeps everything else", async () => {
    const scope = "scope-a";
    const kept = [
      consoleSendStorageKey(scope, "reviewer"),
      consoleSendStorageKey(scope, "support-lab/tester"),
      consoleSendStorageKey("scope-b", "support/reviewer"),
      "meta-console-sidebar-view:principal-a",
    ];
    const local = memoryStorage(Object.fromEntries([
      consoleSendStorageKey(scope, "support/reviewer"),
      consoleSendStorageKey(scope, "support/a/b"),
      predecessorConsoleSendStorageKey(scope, "support/triage"),
      ...kept,
    ].map((key) => [key, "{}"])));
    const keptDrafts = [consoleComposerDraftKey(scope, "reviewer", "composer-1"), consoleComposerDraftKey(scope, "support-lab/tester", "composer-1")];
    const session = memoryStorage(Object.fromEntries([
      consoleComposerDraftKey(scope, "support/reviewer", "composer-1"),
      consoleComposerDraftKey(scope, "support/reviewer"),
      ...keptDrafts,
    ].map((key) => [key, "{}"])));
    await purgeConsoleMobKeys(local, session, scope, "support");
    expect([...local.data.keys()].sort()).toEqual([...kept].sort());
    expect([...session.data.keys()].sort()).toEqual([...keptDrafts].sort());
  });

  it("clears one storage alone when the other is not given", async () => {
    const scope = "scope-a";
    const local = memoryStorage({ [consoleSendStorageKey(scope, "support/reviewer")]: "{}" });
    const session = memoryStorage({ [consoleComposerDraftKey(scope, "support/reviewer", "composer-1")]: "{}" });
    await purgeConsoleMobKeys(null, session, scope, "support");
    expect(local.data.size).toBe(1);
    expect(session.data.size).toBe(0);
  });

  it("removes a mob nested under a hosted mob by the keys behind that door, and keeps the hosted mob's own", async () => {
    const scope = "scope-a";
    const kept = [consoleSendStorageKey(scope, "support/reviewer"), consoleSendStorageKey(scope, "support/support-labs/tester")];
    const local = memoryStorage(Object.fromEntries([
      consoleSendStorageKey(scope, "support/support-lab/tester"),
      consoleSendStorageKey(scope, "support-lab/tester"),
      ...kept,
    ].map((key) => [key, "{}"])));
    await purgeConsoleMobKeys(local, null, scope, "support-lab", "support");
    expect([...local.data.keys()].sort()).toEqual([...kept].sort());
  });
});
