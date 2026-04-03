import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execSync } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const binaryPath = (() => {
  const debugBinary = fileURLToPath(
    new URL("../../../target/debug/rkat-rpc", import.meta.url),
  );
  if (existsSync(debugBinary)) {
    return debugBinary;
  }
  const workspaceBinary = fileURLToPath(
    new URL("../../../target-codex/debug/rkat-rpc", import.meta.url),
  );
  if (existsSync(workspaceBinary)) {
    return workspaceBinary;
  }
  try {
    return execSync("which rkat-rpc", { encoding: "utf8" }).trim();
  } catch {
    try {
      return execSync("which rkat", { encoding: "utf8" }).trim();
    } catch {
      return "";
    }
  }
})();

const fakeStreamBinary = fileURLToPath(
  new URL("../../test-fixtures/fake_stream_rpc.mjs", import.meta.url),
);

describe("E2E Smoke: TypeScript SDK package", { skip: !binaryPath }, () => {
  let MeerkatClient;
  let client;

  before(async () => {
    ({ MeerkatClient } = await import("../dist/index.js"));
    client = new MeerkatClient(binaryPath);
    await client.connect();
  });

  after(async () => {
    if (client) {
      await client.close();
    }
  });

  it("connects through the packaged SDK and fetches capabilities", async () => {
    const capabilities = client.capabilities;
    assert.ok(Array.isArray(capabilities), "capabilities should be an array");
    assert.ok(capabilities.length > 0, "capabilities should not be empty");
    assert.ok(
      capabilities.some((capability) => capability.id === "sessions"),
      "sessions capability should be present",
    );
  });

  it("surfaces a typed error for reading a missing session", async () => {
    await assert.rejects(
      () => client.readSession("00000000-0000-0000-0000-000000000000"),
      /Session not found|RPC error/i,
    );
  });

  it("opens and explicitly closes a live mob stream through the packaged SDK", async () => {
    const created = await client.callMobTool("mob_create", { definition: { id: "coding_swarm", orchestrator: { profile: "lead" }, profiles: { lead: { model: "claude-opus-4-6" }, worker: { model: "claude-sonnet-4-6" } } } });
    const mobId = String(created.mob_id ?? "");
    assert.ok(mobId, "mob_create should return mob_id");

    const sub = await client.subscribeMobEvents(mobId);
    await sub.close();

    for (let attempt = 0; attempt < 10; attempt += 1) {
      if (sub.terminalOutcome?.outcome === "explicit_close") {
        break;
      }
      await delay(20);
    }

    assert.equal(sub.terminalOutcome?.outcome, "explicit_close");
  });

  it("manages schedules through the packaged SDK", async () => {
    const stateRoot = mkdtempSync(path.join(os.tmpdir(), "meerkat-schedule-sdk-"));
    const scheduleClient = new MeerkatClient(binaryPath);
    await scheduleClient.connect({
      realmId: `ts-schedule-${Date.now()}`,
      realmBackend: "redb",
      stateRoot,
    });

    try {
      const created = await scheduleClient.createSchedule({
        name: "typescript-sdk-schedule",
        description: "typescript sdk schedule smoke",
        trigger: {
          type: "interval",
          start_at_utc: new Date(Date.now() + 5 * 60 * 1000).toISOString(),
          every_seconds: 300,
          end_at_utc: null,
        },
        target: {
          target_kind: "session",
          type: "exact_session",
          session_id: "00000000-0000-0000-0000-000000000041",
          action: {
            type: "prompt",
            prompt: "typescript sdk schedule smoke",
            system_prompt: null,
            render_metadata: null,
            skill_references: [],
            additional_instructions: [],
          },
        },
        misfire_policy: { type: "skip" },
        overlap_policy: "skip_if_running",
        missing_target_policy: "mark_misfired",
        labels: {},
        planning_horizon_days: 1,
        planning_horizon_occurrences: 2,
      });

      assert.ok(created.schedule_id, "createSchedule should return schedule_id");
      assert.equal(created.phase, "active");

      const fetched = await scheduleClient.getSchedule(created.schedule_id);
      assert.equal(fetched.schedule_id, created.schedule_id);

      const listed = await scheduleClient.listSchedules();
      assert.ok(
        listed.some((schedule) => schedule.schedule_id === created.schedule_id),
        "listSchedules should include the created schedule",
      );

      const occurrences = await scheduleClient.listScheduleOccurrences(created.schedule_id);
      assert.ok(occurrences.length > 0, "listScheduleOccurrences should return planned rows");

      const paused = await scheduleClient.pauseSchedule(created.schedule_id);
      assert.equal(paused.phase, "paused");

      const resumed = await scheduleClient.resumeSchedule(created.schedule_id);
      assert.equal(resumed.phase, "active");

      const deleted = await scheduleClient.deleteSchedule(created.schedule_id);
      assert.equal(deleted.phase, "deleted");
    } finally {
      await scheduleClient.close();
    }
  });
});

describe("E2E Smoke: TypeScript SDK package against scripted stream RPC", () => {
  let MeerkatClient;
  let client;

  before(async () => {
    ({ MeerkatClient } = await import("../dist/index.js"));
    client = new MeerkatClient(fakeStreamBinary);
    await client.connect();
  });

  after(async () => {
    if (client) {
      await client.close();
    }
  });

  it("replays buffered standalone stream events in order and exposes remote_end", async () => {
    const sub = await client.subscribeSessionEvents("buffered-session");
    const deltas = [];
    for await (const event of sub) {
      deltas.push(event.payload.delta);
    }

    assert.deepEqual(deltas, ["hi", "there"]);
    assert.equal(sub.terminalOutcome?.outcome, "remote_end");
  });

  it("surfaces terminal_error on the packaged standalone subscription API", async () => {
    const sub = await client.subscribeSessionEvents("terminal-error-session");
    const iterator = sub[Symbol.asyncIterator]();
    const first = await iterator.next();

    assert.equal(first.done, true);
    assert.equal(sub.terminalOutcome?.outcome, "terminal_error");
    assert.equal(sub.terminalOutcome?.error?.code, "stream_queue_overflow");
  });

  it("drains late tail turn events through the packaged streaming API", async () => {
    const stream = client._startTurnStreaming(
      "late-tail-stream-session",
      "trigger late tail",
    );

    let text = "";
    for await (const event of stream) {
      if (event.type === "text_delta") {
        text += event.delta;
      }
    }

    assert.equal(text, "LATE_TAIL_PUBLIC");
    assert.equal(stream.result.text, "late tail final result");
  });
});
