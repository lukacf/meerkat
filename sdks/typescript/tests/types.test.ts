import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  CONTRACT_VERSION,
  MeerkatError,
  CapabilityUnavailableError,
  SessionNotFoundError,
  SkillNotFoundError,
} from "../src/generated/index";

describe("Contract Version", () => {
  it("should be 0.1.0", () => {
    assert.equal(CONTRACT_VERSION, "0.1.0");
  });

  it("should be semver format", () => {
    const parts = CONTRACT_VERSION.split(".");
    assert.equal(parts.length, 3);
    for (const part of parts) {
      assert.ok(!isNaN(Number(part)));
    }
  });
});

describe("Error Types", () => {
  it("MeerkatError should capture code and message", () => {
    const err = new MeerkatError("TEST_CODE", "test message");
    assert.equal(err.code, "TEST_CODE");
    assert.equal(err.message, "test message");
    assert.ok(err instanceof Error);
  });

  it("CapabilityUnavailableError should extend MeerkatError", () => {
    const err = new CapabilityUnavailableError("CAP_UNAVAIL", "not available");
    assert.ok(err instanceof MeerkatError);
    assert.equal(err.name, "CapabilityUnavailableError");
  });

  it("SessionNotFoundError should extend MeerkatError", () => {
    const err = new SessionNotFoundError("NOT_FOUND", "session gone");
    assert.ok(err instanceof MeerkatError);
    assert.equal(err.name, "SessionNotFoundError");
  });

  it("SkillNotFoundError should extend MeerkatError", () => {
    const err = new SkillNotFoundError("SKILL_NOT_FOUND", "no such skill");
    assert.ok(err instanceof MeerkatError);
    assert.equal(err.name, "SkillNotFoundError");
  });

  it("MeerkatError should support capability hints", () => {
    const err = new MeerkatError("CAP", "msg", undefined, {
      capability_id: "comms",
      message: "build with --features comms",
    });
    assert.ok(err.capabilityHint);
    assert.equal(err.capabilityHint!.capability_id, "comms");
  });
});
