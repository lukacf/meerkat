import { describe, expect, it } from "vitest";

import type { ConsoleAccessConfig, ConsoleAccessRule } from "../types";
import {
  accessApprovers,
  actionPatternIsKnown,
  actionPatternMatches,
  cellVerdict,
  checkSubject,
  effectiveRules,
  familiesFor,
  matrixRows,
  ruleCoverage,
} from "./access-model";

const CATALOG = [
  "agent.view", "agent.send", "agent.memory.read", "agent.memory.write", "mob.memory.read",
  "agent.spawn", "gating.view", "gating.decide", "workgraph.manage", "runtime.admin",
  "access.admin", "mobpack.author", "mobpack.deploy",
];

/** The seeded preview's configuration: everyone may operate, deploys are
 * denied, leads decide approvals, contractors reach five desks and never
 * read memory. */
const seeded: ConsoleAccessConfig = {
  enabled: true,
  admins: ["ops-lead@example.com"],
  groups: {
    contractors: { members: ["temp1@example.com", "temp2@example.com"] },
    "support-leads": { members: ["ana.lima@example.com"] },
  },
  rules: [
    { id: "console-operators", effect: "allow", actions: ["*"], description: "Anyone may operate the team" },
    { id: "no-host-deploys", effect: "deny", actions: ["mobpack.deploy"], description: "Deploys run only from the pipeline" },
    { id: "leads-decide-approvals", effect: "allow", groups: ["support-leads"], actions: ["gating.view", "gating.decide", "workgraph.manage"] },
    { id: "contractors-domain-desks", effect: "allow", groups: ["contractors"], actions: ["agent.view", "agent.send", "agent.memory.read"], agents: ["billing", "shipping"] },
    { id: "contractors-no-memory", effect: "deny", groups: ["contractors"], actions: ["agent.memory.read", "mob.memory.read"] },
  ],
};

function family(id: string) {
  const found = familiesFor(CATALOG).find((candidate) => candidate.id === id);
  if (!found) throw new Error(`no family ${id}`);
  return found;
}

describe("access patterns", () => {
  it("match as the owner matches them and are known only against its list", () => {
    expect(actionPatternMatches("*", "agent.view")).toBe(true);
    expect(actionPatternMatches("agent.*", "agent.memory.read")).toBe(true);
    expect(actionPatternMatches("agent.*", "agentx.view")).toBe(false);
    expect(actionPatternMatches("agent.view", "agent.view")).toBe(true);
    expect(actionPatternIsKnown("*", CATALOG)).toBe(true);
    expect(actionPatternIsKnown("gating.*", CATALOG)).toBe(true);
    expect(actionPatternIsKnown("billing.*", CATALOG)).toBe(false);
    expect(actionPatternIsKnown("custom.old.action", CATALOG)).toBe(false);
  });

  it("let read ride view in a configuration that names no memory permission", () => {
    const naive: ConsoleAccessRule[] = [{ id: "view", effect: "allow", actions: ["agent.view"] }];
    expect(effectiveRules(naive)[0].actions).toEqual(["agent.view", "agent.memory.read"]);
    expect(effectiveRules(seeded.rules ?? [])[3].actions).toEqual(["agent.view", "agent.send", "agent.memory.read"]);
  });
});

describe("the access matrix", () => {
  const rules = effectiveRules(seeded.rules ?? []);
  const rows = matrixRows(seeded);
  const row = (id: string) => rows.find((candidate) => candidate.id === id)!;

  it("has admins, each group, each named person and everyone", () => {
    expect(rows.map((candidate) => candidate.id)).toEqual(["admins", "group:contractors", "group:support-leads", "everyone"]);
    expect(familiesFor(CATALOG).map((candidate) => candidate.id)).toEqual(["view", "message", "lifecycle", "approvals", "memory", "workgraph", "routing", "admin"]);
    expect(familiesFor([...CATALOG, "custom.new.action"]).at(-1)?.id).toBe("other");
  });

  it("marks what the rules decide: deny beats allow, selectors make some agents", () => {
    expect(cellVerdict(rules, row("admins"), family("admin")).state).toBe("all");
    // Everyone may do everything but deploy.
    expect(cellVerdict(rules, row("everyone"), family("view")).state).toBe("all");
    const admin = cellVerdict(rules, row("everyone"), family("admin"));
    expect(admin.state).toBe("some");
    expect(admin.actions.find((action) => action.action === "mobpack.deploy")?.state).toBe("denied");
    expect(admin.actions.find((action) => action.action === "mobpack.deploy")?.rule?.id).toBe("no-host-deploys");
    // Contractors read no memory: the deny wins over both allows.
    const memory = cellVerdict(rules, row("group:contractors"), family("memory"));
    expect(memory.actions.find((action) => action.action === "agent.memory.read")?.state).toBe("denied");
    expect(memory.actions.find((action) => action.action === "mob.memory.read")?.state).toBe("denied");
    expect(memory.state).toBe("some");
  });

  it("reads a configuration without the catch-all rule as the owner would", () => {
    const strict = { ...seeded, rules: (seeded.rules ?? []).filter((rule) => rule.id !== "console-operators") };
    const strictRules = effectiveRules(strict.rules ?? []);
    expect(cellVerdict(strictRules, row("group:contractors"), family("message")).state).toBe("some");
    expect(cellVerdict(strictRules, row("group:contractors"), family("memory")).state).toBe("denied");
    expect(cellVerdict(strictRules, row("group:contractors"), family("lifecycle")).state).toBe("none");
    expect(cellVerdict(strictRules, row("group:support-leads"), family("approvals")).state).toBe("all");
    expect(cellVerdict(strictRules, row("everyone"), family("approvals")).state).toBe("none");
  });

  it("checks a row as someone it stands for", () => {
    expect(checkSubject(row("admins"), seeded)).toEqual({ subject: "ops-lead@example.com", note: "Checked as ops-lead@example.com, an admin." });
    expect(checkSubject(row("group:contractors"), seeded)?.subject).toBe("temp1@example.com");
    expect(checkSubject(row("everyone"), seeded)?.subject).toBe(null);
    const empty = { ...seeded, groups: { empty: { members: [] } } };
    expect(checkSubject(matrixRows(empty).find((candidate) => candidate.id === "group:empty")!, empty)).toBe(null);
  });

  it("lists the people a rule names by name, in their groups", () => {
    const named = { ...seeded, rules: [...(seeded.rules ?? []), { id: "dana", effect: "deny" as const, subjects: ["temp2@example.com", "*"], actions: ["agent.spawn"] }] };
    const person = matrixRows(named).find((candidate) => candidate.id === "person:temp2@example.com");
    expect(person?.groups).toEqual(["contractors"]);
    expect(matrixRows(named).some((candidate) => candidate.id === "person:*")).toBe(false);
  });
});

describe("rule coverage", () => {
  it("names the deny rules that take permissions from an allow rule, and what a deny overrides", () => {
    const rules = seeded.rules ?? [];
    const operators = ruleCoverage(rules[0], rules, CATALOG);
    expect(operators.shadowed).toBe(false);
    expect(operators.deniedBy.map((overlap) => [overlap.rule.id, overlap.actions])).toEqual([["no-host-deploys", ["mobpack.deploy"]]]);
    const desks = ruleCoverage(rules[3], rules, CATALOG);
    expect(desks.deniedBy.map((overlap) => [overlap.rule.id, overlap.actions])).toEqual([["contractors-no-memory", ["agent.memory.read"]]]);
    const deny = ruleCoverage(rules[1], rules, CATALOG);
    expect(deny.overrides.map((overlap) => overlap.rule.id)).toEqual(["console-operators"]);
    const shadowed = ruleCoverage({ id: "deploy", effect: "allow", actions: ["mobpack.deploy"] }, [...rules, { id: "deploy", effect: "allow", actions: ["mobpack.deploy"] }], CATALOG);
    expect(shadowed.shadowed).toBe(true);
    // A deny for one group does not shadow a rule for everyone.
    expect(ruleCoverage(rules[0], [rules[0], rules[4]], CATALOG).deniedBy).toEqual([]);
  });
});

describe("approvers", () => {
  it("are read from the configuration, read only", () => {
    expect(accessApprovers({ ...seeded, approvers: { "role:release": { subjects: ["carol@example.com"], groups: ["support-leads"] } } } as ConsoleAccessConfig))
      .toEqual([{ id: "role:release", subjects: ["carol@example.com"], groups: ["support-leads"] }]);
    expect(accessApprovers(seeded)).toEqual([]);
  });
});
