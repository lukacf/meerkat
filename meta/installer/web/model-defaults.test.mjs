import { test } from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_MODELS, newClass } from "./state.mjs";

test("OpenAI provider changes use the new target preference and retain other defaults", () => {
  assert.deepEqual(DEFAULT_MODELS, {
    openai: "gpt-6.1-sol",
    anthropic: "claude-sonnet-4-6",
    gemini: "gemini-3.1-pro-preview",
  });
  for (const provider of ["openai", "anthropic", "gemini"]) {
    const blueprint = { provider, model: DEFAULT_MODELS[provider], agent_classes: [] };
    const added = newClass(blueprint);
    assert.equal(added.provider, provider);
    assert.equal(added.model, DEFAULT_MODELS[provider]);
  }
});

test("new classes inherit explicit saved model choices without rewriting the saved draft", () => {
  for (const [provider, model] of [
    ["openai", "gpt-6-astra"],
    ["anthropic", "claude-opus-4-6"],
    ["gemini", "gemini-custom-user-model"],
  ]) {
    const blueprint = Object.freeze({ provider, model, agent_classes: Object.freeze([]) });
    const before = JSON.stringify(blueprint);
    const added = newClass(blueprint);
    assert.equal(added.provider, provider);
    assert.equal(added.model, model);
    assert.equal(JSON.stringify(blueprint), before);
  }
});
