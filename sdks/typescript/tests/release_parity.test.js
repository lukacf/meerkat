import { describe, it } from "node:test";
import assert from "node:assert/strict";
import packageJson from "../package.json" with { type: "json" };

describe("Phase 1 release parity targets", () => {
  it("keeps the packaged TypeScript entrypoints declared", () => {
    assert.equal(packageJson.name, "@rkat/sdk");
    assert.ok(packageJson.version);
    assert.equal(packageJson.main, "./dist/index.js");
    assert.equal(packageJson.types, "./dist/index.d.ts");
    assert.ok(packageJson.exports["."], "root export should exist");
  });
});
