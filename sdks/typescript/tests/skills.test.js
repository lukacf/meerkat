import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { SkillHelper } from "../dist/skills.js";

describe("SkillHelper legacy ref warnings", () => {
  it("emits deprecation warning for legacy string ref", async () => {
    const calls = [];
    const mockClient = {
      hasCapability(name) {
        return name === "skills";
      },
      async startTurn(sessionId, prompt, opts) {
        calls.push({ sessionId, prompt, opts });
        return { text: "ok" };
      },
    };

    const helper = new SkillHelper(mockClient);
    const originalWarn = console.warn;
    const warnings = [];
    console.warn = (msg) => {
      warnings.push(String(msg ?? ""));
    };
    try {
      await helper.invoke(
        "s-1",
        "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor",
        "run",
      );
    } finally {
      console.warn = originalWarn;
    }

    assert.equal(calls.length, 1);
    assert.equal(warnings.length, 1);
    assert.match(warnings[0], /legacy skill reference/i);
  });
});
