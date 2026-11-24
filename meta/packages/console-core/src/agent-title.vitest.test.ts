import { describe, expect, it } from "vitest";

import { agentDisplayTitle, identityDisplayTitle } from "./agent-title";

// A member of a mob other than the console's own carries its mob in its
// identity. Its name for people is its display name, else its member name:
// never the mob and the name in one string.
const nested = (extra: Record<string, unknown> = {}) => ({
  identity: "research-team/reviewer",
  member_id: "research-team/reviewer",
  agent_id: "research-team/reviewer",
  mob_id: "research-team",
  role: "analyst",
  ...extra,
});

describe("the name of a member of a nested mob", () => {
  it("is its display name when the row carries one", () => {
    expect(agentDisplayTitle(nested({ label: "Senior reviewer" }))).toBe("Senior reviewer");
  });

  it("is its member name in words when the row carries no display name", () => {
    expect(agentDisplayTitle(nested())).toBe("Reviewer");
    // A row whose label is just its identity has no display name.
    expect(agentDisplayTitle(nested({ label: "research-team/reviewer" }))).toBe("Reviewer");
    const lane = nested({ identity: "research-team/audit:lane-2", member_id: "research-team/audit:lane-2", agent_id: "research-team/audit:lane-2" });
    expect(agentDisplayTitle(lane)).toBe("Audit lane 2");
  });

  it("is the member's kind and an id tail for a generated member name, without the mob", () => {
    const generated = nested({
      identity: "research-team/0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70",
      member_id: "research-team/0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70",
      agent_id: "research-team/0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70",
    });
    expect(agentDisplayTitle(generated)).toBe("Analyst 5e6f70");
  });

  it("keeps a primary member's name, and an identity whose first part is not its mob, as before", () => {
    expect(agentDisplayTitle({ identity: "reviewer", member_id: "reviewer", mob_id: "ops-team" })).toBe("Reviewer");
    // A runtime namespace prefixes an identity the same way; it is not a mob.
    expect(agentDisplayTitle({ identity: "east/reviewer", member_id: "east/reviewer", mob_id: "ops-team" })).toBe("East reviewer");
    expect(agentDisplayTitle({ identity: "east/reviewer", member_id: "east/reviewer" })).toBe("East reviewer");
  });
});

describe("the name of an identity no row names", () => {
  const listed = new Set(["research-team", "alpha"]);

  it("drops the mob of a listed nested mob and reads the member name in words", () => {
    expect(identityDisplayTitle("research-team/reviewer", listed)).toBe("Reviewer");
    expect(identityDisplayTitle("identity:alpha/queue-agent", listed)).toBe("Queue agent");
    expect(identityDisplayTitle("alpha/0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70", listed, { role: "helper" })).toBe("Helper 5e6f70");
  });

  it("reads any other identity as before", () => {
    expect(identityDisplayTitle("reviewer", listed)).toBe("Reviewer");
    expect(identityDisplayTitle("east/reviewer", listed)).toBe("East reviewer");
    expect(identityDisplayTitle("research-team/reviewer")).toBe("Research team reviewer");
    expect(identityDisplayTitle("identity:support-triage", null)).toBe("Support triage");
  });
});
