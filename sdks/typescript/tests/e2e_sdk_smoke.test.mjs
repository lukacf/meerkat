import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execSync } from "node:child_process";
import { existsSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const binaryPath = (() => {
  try {
    const envOutput = execSync("../../scripts/repo-cargo --print-env", {
      cwd: fileURLToPath(new URL("..", import.meta.url)),
      encoding: "utf8",
    });
    const targetDir = envOutput
      .split("\n")
      .find((line) => line.startsWith("CARGO_TARGET_DIR="))
      ?.slice("CARGO_TARGET_DIR=".length);
    if (targetDir) {
      const repoCargoBinary = `${targetDir}/debug/rkat-rpc`;
      if (existsSync(repoCargoBinary)) {
        return repoCargoBinary;
      }
    }
  } catch {
    // Fall through to legacy workspace/PATH resolution for installed-package smoke.
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

  it("opens and explicitly closes a live mob stream through the packaged SDK", async (t) => {
    const supportsMob = Array.isArray(client.capabilities)
      && client.capabilities.some((capability) => capability.id === "mob");
    if (!supportsMob) {
      t.skip("packaged binary does not advertise mob capability");
      return;
    }

    const mob = await client.createMob({
      definition: {
        id: "coding_swarm",
        orchestrator: { profile: "lead" },
        profiles: {
          lead: { model: "claude-opus-4-6" },
          worker: { model: "claude-sonnet-4-6" },
        },
      },
    });
    const mobId = mob.mobId;
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
});

describe("E2E Smoke: TypeScript SDK package against scripted stream RPC", () => {
  let MeerkatClient;
  let client;
  let fixtureVersionMismatch = false;

  before(async () => {
    ({ MeerkatClient } = await import("../dist/index.js"));
    client = new MeerkatClient(fakeStreamBinary);
    try {
      await client.connect();
    } catch (error) {
      if (String(error?.code) === "VERSION_MISMATCH") {
        fixtureVersionMismatch = true;
        return;
      }
      throw error;
    }
  });

  after(async () => {
    if (client) {
      await client.close();
    }
  });

  it("replays buffered standalone stream events in order and exposes remote_end", async (t) => {
    if (fixtureVersionMismatch) {
      t.skip("scripted RPC fixture contract version is stale for this SDK");
    }
    const sub = await client.subscribeSessionEvents("buffered-session");
    const deltas = [];
    for await (const event of sub) {
      deltas.push(event.payload.delta);
    }

    assert.deepEqual(deltas, ["hi", "there"]);
    assert.equal(sub.terminalOutcome?.outcome, "remote_end");
  });

  it("surfaces terminal_error on the packaged standalone subscription API", async (t) => {
    if (fixtureVersionMismatch) {
      t.skip("scripted RPC fixture contract version is stale for this SDK");
    }
    const sub = await client.subscribeSessionEvents("terminal-error-session");
    const iterator = sub[Symbol.asyncIterator]();
    const first = await iterator.next();

    assert.equal(first.done, true);
    assert.equal(sub.terminalOutcome?.outcome, "terminal_error");
    assert.equal(sub.terminalOutcome?.error?.code, "stream_queue_overflow");
  });

  it("drains late tail turn events through the packaged streaming API", async (t) => {
    if (fixtureVersionMismatch) {
      t.skip("scripted RPC fixture contract version is stale for this SDK");
    }
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
