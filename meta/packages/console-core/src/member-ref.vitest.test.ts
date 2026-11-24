import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  memberDisplayName,
  memberNameInMob,
  nestedMemberIdentity,
  nestedMobIds,
  parseNestedMemberIdentity,
} from "./member-ref";

interface ComposeCase {
  mob_id: string;
  name: string;
  identity: string | null;
}

interface ParseCase {
  identity: string;
  mob_id: string | null;
  name: string | null;
}

// The cases the gateway's owner of the form is checked against too.
const CASES_PATH = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../crates/meerkat-meta/src/console_member_ref_cases.json",
);
const CASES = JSON.parse(readFileSync(CASES_PATH, "utf8")) as { compose: ComposeCase[]; parse: ParseCase[] };

describe("a nested member's console identity", () => {
  it("composes every shared case as the gateway does", () => {
    expect(CASES.compose.length).toBeGreaterThan(0);
    for (const entry of CASES.compose) {
      expect(nestedMemberIdentity(entry.mob_id, entry.name), JSON.stringify(entry)).toBe(entry.identity);
    }
  });

  it("parses every shared case as the gateway does", () => {
    expect(CASES.parse.length).toBeGreaterThan(0);
    for (const entry of CASES.parse) {
      const parsed = parseNestedMemberIdentity(entry.identity);
      const expected = entry.mob_id !== null && entry.name !== null ? { mobId: entry.mob_id, name: entry.name } : null;
      expect(parsed, JSON.stringify(entry)).toEqual(expected);
    }
  });

  it("parses back what it composes, a name holding the separator included", () => {
    for (const [mobId, name] of [["team", "reviewer"], ["team", "a/b"], ["t", "rt:x:0"]] as const) {
      const identity = nestedMemberIdentity(mobId, name);
      expect(identity).not.toBeNull();
      expect(parseNestedMemberIdentity(identity!)).toEqual({ mobId, name });
      expect(memberNameInMob(identity!, mobId)).toBe(name);
      expect(memberNameInMob(identity!, "other")).toBe(identity);
      expect(memberNameInMob(identity!, null)).toBe(identity);
    }
    expect(nestedMemberIdentity("a/b", "reviewer")).toBeNull();
    expect(memberNameInMob("reviewer", "team")).toBe("reviewer");
  });
});

describe("the name shown for a console identity", () => {
  const listed = nestedMobIds(
    [{ mob_id: "ops-team" }, { mob_id: "research-team" }, { mob_id: "alpha" }],
    "ops-team",
  );

  it("is the member's name for a member of a listed mob other than the console's own", () => {
    expect(memberDisplayName("research-team/reviewer", listed)).toBe("reviewer");
    expect(memberDisplayName("alpha/audit/lane", listed)).toBe("audit/lane");
  });

  it("is the identity itself for a bare name, a runtime namespace and a mob the console does not list", () => {
    expect(memberDisplayName("reviewer", listed)).toBe("reviewer");
    expect(memberDisplayName("east/reviewer", listed)).toBe("east/reviewer");
    // The console's own mob names its members by their bare names.
    expect(memberDisplayName("ops-team/reviewer", listed)).toBe("ops-team/reviewer");
    expect(memberDisplayName("research-team/reviewer", null)).toBe("research-team/reviewer");
    expect(memberDisplayName("", listed)).toBe("");
  });

  it("lists every mob but the console's own as a nested mob", () => {
    expect([...listed].sort()).toEqual(["alpha", "research-team"]);
    expect([...nestedMobIds(null, "ops-team")]).toEqual([]);
    expect([...nestedMobIds([{ mob_id: " beta " }, { mob_id: "" }], undefined)]).toEqual(["beta"]);
  });
});
