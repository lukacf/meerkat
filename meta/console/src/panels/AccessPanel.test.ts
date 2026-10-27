import assert from "node:assert/strict";
import test from "node:test";

import { __accessTest } from "./AccessPanel";

const {
  parseListInput,
  formatListInput,
  parseLabelSelectorInput,
  formatLabelSelectorInput,
  summarizeRuleSubjects,
  summarizeRuleResources,
  ruleFromDraft,
  emptyRuleDraft,
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
