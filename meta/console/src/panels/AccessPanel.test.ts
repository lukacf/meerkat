import assert from "node:assert/strict";
import test from "node:test";

import { __accessTest } from "./AccessPanel";
import { accessActionLabel, isKnownAccessAction } from "../lib/access-actions";

const {
  parseListInput,
  formatListInput,
  parseLabelSelectorInput,
  formatLabelSelectorInput,
  summarizeRuleSubjects,
  summarizeRuleResources,
  ruleFromDraft,
  draftFromRule,
  emptyRuleDraft,
  ruleTitle,
  ruleDeleteConfirmText,
} = __accessTest;

test("list inputs round-trip through comma separated text", () => {
  assert.deepEqual(parseListInput(" alice@example.com,, bob@example.com \n carol@x "), [
    "alice@example.com",
    "bob@example.com",
    "carol@x",
  ]);
  assert.equal(formatListInput(["a", "b"]), "a, b");
  assert.equal(formatListInput(undefined), "");
});

test("label selectors parse key=value pairs and ignore malformed tokens", () => {
  assert.deepEqual(parseLabelSelectorInput("org=payments, tier=1, =bad, plain"), {
    org: "payments",
    tier: "1",
  });
  assert.equal(
    formatLabelSelectorInput({ org: "payments", tier: "1" }),
    "org=payments, tier=1",
  );
});

test("rule summaries describe unconstrained dimensions", () => {
  assert.equal(
    summarizeRuleSubjects({ id: "r", actions: ["agent.view"] }),
    "everyone",
  );
  assert.equal(
    summarizeRuleSubjects({ id: "r", actions: [], groups: ["ops"], subjects: ["a@x"] }),
    "groups: ops · a@x",
  );
  assert.equal(
    summarizeRuleResources({ id: "r", actions: [] }),
    "all agents",
  );
  assert.equal(
    summarizeRuleResources({
      id: "r",
      actions: [],
      agents: ["identity:lead"],
      match_labels: { org: "payments" },
    }),
    "agents: identity:lead · labels: org=payments",
  );
});

test("rule drafts only serialize the dimensions the user constrained", () => {
  const draft = emptyRuleDraft();
  draft.id = " ops-view-all ";
  draft.groups = "ops";
  draft.actions = ["agent.view"];
  const rule = ruleFromDraft(draft);
  assert.deepEqual(rule, {
    id: "ops-view-all",
    effect: "allow",
    actions: ["agent.view"],
    groups: ["ops"],
  });

  draft.effect = "deny";
  draft.agents = "identity:secret";
  draft.matchLabels = "org=payments";
  const deny = ruleFromDraft(draft);
  assert.equal(deny.effect, "deny");
  assert.deepEqual(deny.agents, ["identity:secret"]);
  assert.deepEqual(deny.match_labels, { org: "payments" });
});

test("a rule without a description is an unnamed rule; its ID is never its name", () => {
  assert.equal(ruleTitle({ description: "Ops can see every agent" }), "Ops can see every agent");
  assert.equal(ruleTitle({}), "Unnamed rule");
  assert.equal(ruleTitle({ description: "   " }), "Unnamed rule");
  // The delete confirmation names the rule as people know it; the ID is
  // only the parenthetical that tells two rules apart.
  assert.equal(
    ruleDeleteConfirmText({ id: "ops-view-all", description: "Ops can see every agent" }),
    'Delete the rule "Ops can see every agent" (ID ops-view-all)? Access it grants (or denies) stops immediately.',
  );
  assert.equal(
    ruleDeleteConfirmText({ id: "deny-lifecycle-outside-ops" }),
    "Delete this unnamed rule (ID deny-lifecycle-outside-ops)? Access it grants (or denies) stops immediately.",
  );
  assert.doesNotMatch(ruleDeleteConfirmText({ id: "deny-lifecycle-outside-ops" }), /^Delete rule "/);
});

test("permissions over team packages read as team packages, not mobpacks", () => {
  assert.equal(accessActionLabel("mobpack.author"), "Author team packages");
  assert.equal(accessActionLabel("mobpack.deploy"), "Deploy team packages");
  for (const action of ["mobpack.author", "mobpack.deploy"]) {
    assert.doesNotMatch(accessActionLabel(action), /mobpack/i, action);
  }
});

test("the tool grant permission reads in words and joins the agent family", () => {
  assert.equal(accessActionLabel("agent.tool_grants.manage"), "Grant tools to agents");
  assert.equal(isKnownAccessAction("agent.tool_grants.manage"), true);
  assert.equal(accessActionLabel("agent.tool_grants.*"), "All agent tool grant actions");
});

test("member_keyed: a rule that lists mobs keeps them through the editor's draft", () => {
  const stored = {
    id: "harbor-desks",
    effect: "allow" as const,
    groups: ["ops"],
    actions: ["agent.view"],
    agents: ["billing"],
    mobs: ["harbor", "quay"],
  };
  const draft = draftFromRule(stored);
  draft.description = "Ops see the harbor desks";
  draft.actions = ["agent.view", "agent.send"];
  assert.deepEqual(ruleFromDraft(draft), {
    ...stored,
    description: "Ops see the harbor desks",
    actions: ["agent.view", "agent.send"],
  });
  // Every mob stays every mob.
  assert.deepEqual(ruleFromDraft(draftFromRule({ id: "all", actions: ["agent.view"], mobs: ["*"] })).mobs, ["*"]);
  // The saved rule holds its own list, not the stored rule's array.
  assert.notEqual(ruleFromDraft(draftFromRule(stored)).mobs, stored.mobs);
});

test("member_keyed: a rule of the default mob is saved without mobs", () => {
  assert.equal("mobs" in ruleFromDraft(draftFromRule({ id: "desks", actions: ["agent.view"] })), false);
  assert.equal("mobs" in ruleFromDraft(draftFromRule({ id: "desks", actions: ["agent.view"], mobs: [] })), false);
  const fresh = emptyRuleDraft();
  fresh.id = "new";
  fresh.actions = ["agent.view"];
  assert.equal("mobs" in ruleFromDraft(fresh), false);
});

test("member_keyed: a rule summary names the teams the rule lists", () => {
  assert.equal(
    summarizeRuleResources({ id: "r", actions: ["agent.view"], agents: ["billing"], mobs: ["harbor", "quay"] }),
    "agents: billing · teams: harbor, quay",
  );
  assert.equal(summarizeRuleResources({ id: "r", actions: ["agent.view"], mobs: ["*"] }), "every team");
});
