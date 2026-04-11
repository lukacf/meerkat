import { describe, it } from "node:test";
import assert from "node:assert/strict";
import packageJson from "../package.json" with { type: "json" };
import { MeerkatClient, Session, DeferredSession, Mob } from "../dist/index.js";

describe("Phase 1 release parity targets", () => {
  it("keeps the packaged TypeScript entrypoints declared", () => {
    assert.equal(packageJson.name, "@rkat/sdk");
    assert.ok(packageJson.version);
    assert.equal(packageJson.main, "./dist/index.js");
    assert.equal(packageJson.types, "./dist/index.d.ts");
    assert.ok(packageJson.exports["."], "root export should exist");
  });

  it("exposes canonical app-facing wrapper methods", () => {
    const clientMethods = Object.getOwnPropertyNames(MeerkatClient.prototype);
    const expectedClientMethods = [
      "createSession",
      "createSessionStreaming",
      "createDeferredSession",
      "listSessions",
      "readSession",
      "readSessionHistory",
      "sendExternalEvent",
      "injectContext",
      "getModelsCatalog",
      "createSchedule",
      "getSchedule",
      "listSchedules",
      "updateSchedule",
      "pauseSchedule",
      "resumeSchedule",
      "deleteSchedule",
      "listScheduleOccurrences",
      "listScheduleTools",
      "callScheduleTool",
      "readMobEvents",
      "spawnMobMembers",
      "createMobProfile",
      "getMobProfile",
      "listMobProfiles",
      "updateMobProfile",
      "deleteMobProfile",
      "waitMobKickoff",
    ];
    for (const method of expectedClientMethods) {
      assert.ok(
        clientMethods.includes(method),
        `missing MeerkatClient.${method}`,
      );
    }
  });

  it("exposes convenience wrappers on Session/DeferredSession/Mob", () => {
    const sessionMethods = Object.getOwnPropertyNames(Session.prototype);
    const deferredMethods = Object.getOwnPropertyNames(DeferredSession.prototype);
    const mobMethods = Object.getOwnPropertyNames(Mob.prototype);

    for (const method of ["injectContext"]) {
      assert.ok(sessionMethods.includes(method), `missing Session.${method}`);
      assert.ok(deferredMethods.includes(method), `missing DeferredSession.${method}`);
    }
    for (const method of ["spawnMany", "readEvents"]) {
      assert.ok(mobMethods.includes(method), `missing Mob.${method}`);
    }
  });
});
