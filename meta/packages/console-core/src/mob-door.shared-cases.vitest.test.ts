import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  CONSOLE_MEMBER_ROW_NAME_FIELDS,
  CONSOLE_MOB_ADDRESSING_CAPABILITY,
  CONSOLE_MOB_ID_PARAM,
  CONSOLE_MOBS_SELECTOR_EVERY,
  CONSOLE_MOBS_SELECTOR_PARAM,
} from "./contract";
import * as door from "./mob-door";
import { memberDoor, memberRowKey, mobDoorTable, type MobDoorSource } from "./mob-door";

// The console's door of a member and its row key, over the cases the
// gateway's functions and the contract's own reading are checked against
// too (contract v0.6.0, mob_addressing.row_key).

interface DoorCase {
  case: string;
  mob_id: string;
  name: string;
  row_key: string | null;
  request: { mob_id: string | null; identity: string } | null;
}

interface FrameCase {
  case: string;
  mob_id: string | null;
  identity: string;
  row_key: string | null;
}

const CASES_PATH = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../crates/meerkat-meta/src/console_row_key_cases.json",
);
interface RowCase {
  case: string;
  mob_id: string;
  name: string;
  row: { identity: string; member_id: string; agent_id: string; mob_id: string };
}

const CASES = JSON.parse(readFileSync(CASES_PATH, "utf8")) as { mobs: MobDoorSource[]; door: DoorCase[]; frame: FrameCase[]; row: RowCase[] };
// The list names its launch mob itself (`default`), as a gateway's does.
const TABLE = mobDoorTable(CASES.mobs, null, true);

/** The row key of a member by its own mob and its name, as the module exports it. */
const rowKeyOfMember = (door as unknown as {
  memberRowKeyOf(mobId: string, name: string, table: typeof TABLE): string | null;
}).memberRowKeyOf;

describe("the door of a member, over the shared cases", () => {
  it("gives every case its row key", () => {
    expect(CASES.door.length).toBeGreaterThan(0);
    for (const entry of CASES.door) {
      expect(rowKeyOfMember(entry.mob_id, entry.name, TABLE), entry.case).toBe(entry.row_key);
    }
  });

  it("gives every case the pair its request names", () => {
    for (const entry of CASES.door) {
      if (entry.row_key === null) continue;
      const pair = memberDoor(entry.row_key, TABLE);
      expect({ mob_id: pair.mobId, identity: pair.identity }, entry.case).toEqual(entry.request);
    }
  });
});

describe("the row key of a frame, over the shared cases", () => {
  it("gives every case its row key, and none to a frame that belongs to no row", () => {
    expect(CASES.frame.length).toBeGreaterThan(0);
    for (const entry of CASES.frame) {
      expect(memberRowKey({ mob_id: entry.mob_id, identity: entry.identity }, TABLE), entry.case).toBe(entry.row_key);
    }
  });

  it("is the key of the member the same pair addresses", () => {
    for (const entry of CASES.door) {
      if (entry.row_key === null || entry.request === null) continue;
      expect(memberRowKey(entry.request, TABLE), entry.case).toBe(entry.row_key);
    }
  });
});

describe("what a member row carries, over the shared cases", () => {
  it("names its member by the row key under every name of the row, and carries the member's own mob", () => {
    expect(CASES.row.length).toBeGreaterThan(0);
    for (const entry of CASES.row) {
      const key = rowKeyOfMember(entry.mob_id, entry.name, TABLE);
      for (const field of CONSOLE_MEMBER_ROW_NAME_FIELDS) expect(entry.row[field], `${entry.case}: ${field}`).toBe(key);
      // The row names the mob whose door it was read on, and the member's own mob beside it.
      expect(entry.row.mob_id, entry.case).toBe(TABLE.doors.get(entry.mob_id) ?? TABLE.launchMobId);
      expect((entry.row as Record<string, unknown>).member_mob_id, entry.case).toBe(entry.mob_id);
      expect((entry.row as Record<string, unknown>).member_name, entry.case).toBe(entry.name);
      // Read by any of its names, the row is behind the door its own mob's members are behind.
      for (const field of CONSOLE_MEMBER_ROW_NAME_FIELDS) {
        expect(memberDoor(entry.row[field], TABLE), `${entry.case}: ${field}`).toEqual(memberDoor(key as string, TABLE));
      }
    }
  });
});

describe("the words the console sends", () => {
  it("are the contract's", () => {
    const words = door as unknown as Record<string, unknown>;
    // The module names no word of its own for what the contract names.
    for (const own of ["MOB_ADDRESSING_FEATURE", "MOB_ID_PARAM", "MOBS_PARAM", "EVERY_MOB", "MOB_OPEN_METHOD"]) {
      expect(own in words, own).toBe(false);
    }
    const mobs = door.createConsoleMobAddressing();
    mobs.noteFeatures([CONSOLE_MOB_ADDRESSING_CAPABILITY]);
    expect(mobs.enabled).toBe(true);
    expect(mobs.everyMob()).toEqual({ [CONSOLE_MOBS_SELECTOR_PARAM]: CONSOLE_MOBS_SELECTOR_EVERY });
    expect(CONSOLE_MOB_ID_PARAM).toBe("mob_id");
  });
});
