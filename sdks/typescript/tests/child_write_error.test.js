/**
 * A child whose input is gone: a request or a callback reply that can no
 * longer be written is settled to its caller, never raised outside the call.
 *
 * - A child that died while the host program was busy: the call that
 *   follows in the same turn fails with `PROCESS_EXITED`, as the call the
 *   child died on does, and the host program runs on.
 * - A child that asks the host something and is gone at once: unchanged.
 * - A child that closed its input and then exits: the next request fails with
 *   `PROCESS_EXITED` and the child's stderr.
 * - A child that closed its input and keeps running, and a failed read of
 *   the child's output: the request fails with `PROTOCOL_ERROR` about a
 *   second later, the client stops the child, and later calls fail the same.
 * - A failed read of the child's stderr, and a healthy child: unchanged.
 */

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, it } from "node:test";

import { MeerkatClient } from "../dist/client.js";
import { CONTRACT_VERSION } from "../dist/generated/types.js";

delete process.env.MEERKAT_BIN_PATH;

const CLIENT_ENTRY = new URL("../dist/client.js", import.meta.url).href;

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/**
 * A fake rkat-rpc that answers the handshake, then runs `onRequest` (Node
 * source, with `msg` the request) on every later line.
 */
function writeFixture(onRequest) {
  const root = mkdtempSync(path.join(os.tmpdir(), "meerkat-ts-input-gone-"));
  roots.push(root);
  const bin = path.join(root, "rkat-rpc.cjs");
  writeFileSync(bin, `#!/usr/bin/env node
const fs = require("node:fs");
const rl = require("node:readline").createInterface({ input: process.stdin });
const emit = (obj) => fs.writeSync(1, JSON.stringify(obj) + "\\n");
const reply = (msg, result) => emit({ jsonrpc: "2.0", id: msg.id, result });
// Destroying the stream leaves descriptor 0 open: close it too, so that the
// host's next write finds no reader.
let inputClosed = false;
const closeInput = () => { inputClosed = true; rl.close(); process.stdin.destroy(); fs.closeSync(0); };
// Gone at once, as a native process is: no exit handlers, no teardown.
const exitNow = () => process.kill(process.pid, "SIGKILL");
let asked = null;
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.method === "initialize") {
    reply(msg, { contract_version: ${JSON.stringify(CONTRACT_VERSION)}, methods: [] });
  } else if (msg.method === "capabilities/get") {
    reply(msg, { capabilities: [] });
  } else {
    ${onRequest}
  }
});
rl.on("close", () => { if (!inputClosed) process.exit(0); });
`);
  chmodSync(bin, 0o755);
  return bin;
}

// Gone at once on a request, without an answer.
const IS_GONE = `
    exitNow();`;

// On a request, asks the host something and is gone before the answer.
const ASKS_AND_IS_GONE = `
    emit({ jsonrpc: "2.0", id: "ask-1", method: "fixture/unknown", params: {} });
    exitNow();`;

// Closes its input, answers the request, says why on stderr and exits.
const CLOSES_INPUT_THEN_EXITS = `
    closeInput();
    reply(msg, { closed: true });
    setTimeout(() => {
      fs.writeSync(2, "fixture: going away\\n");
      process.exit(7);
    }, 300);`;

// Closes its input, answers the request and keeps running.
const CLOSES_INPUT_AND_STAYS = `
    closeInput();
    reply(msg, { closed: true });
    setTimeout(() => process.exit(0), 30000);`;

// Never answers a request.
const SILENT = "";

// Answers every request; `fixture/ask` first asks the host and passes the
// host's answer on.
const HEALTHY = `
    if (asked !== null && msg.id === "ask-1") {
      reply(asked, { host_answered: msg.error.code });
      asked = null;
    } else if (msg.method === "config/get") {
      reply(msg, { answered: true });
    } else {
      asked = msg;
      emit({ jsonrpc: "2.0", id: "ask-1", method: "fixture/unknown", params: {} });
    }`;

// A host program: every call settled by its own handler, as many starts as
// asked, with no handler for uncaught exceptions. It reaches its last line
// only when every call settled to its caller. With "busy" it works for a
// while after its first call, without yielding, and then calls again: the
// child dies meanwhile, and the host has not yet been told.
const HOST = `
const { MeerkatClient } = await import(process.argv[2]);
const code = (call) => call.then(() => "resolved", (error) => String(error?.code));
const outcomes = [];
for (let round = 0; round < Number(process.argv[4]); round += 1) {
  const client = new MeerkatClient(process.argv[3]);
  try {
    await client.connect();
    const calls = [code(client.getConfig())];
    if (process.argv[5] === "busy") {
      const until = Date.now() + 400;
      while (Date.now() < until) { /* synchronous work */ }
      calls.push(code(client.getConfig()));
    }
    outcomes.push(await Promise.all(calls));
  } finally {
    await client.close();
  }
}
process.stdout.write(JSON.stringify(outcomes) + "\\n");
`;

/**
 * Run the host program for `rounds` starts of `bin`. The program must end by
 * itself with exit code 0 and one outcome per start: an exception raised
 * outside a call would have ended it early.
 */
async function hostOutcomes(bin, rounds, busy = "") {
  const host = path.join(path.dirname(bin), "host.mjs");
  writeFileSync(host, HOST);
  const result = await new Promise((resolve) => {
    execFile(
      process.execPath,
      [host, CLIENT_ENTRY, bin, String(rounds), busy],
      { cwd: path.dirname(bin), timeout: 120_000 },
      (error, stdout, stderr) =>
        resolve({ code: error === null ? 0 : (error.code ?? error.signal ?? null), stdout, stderr }),
    );
  });
  assert.equal(
    result.code,
    0,
    `the host program was ended before its last line: ${result.stderr.trim().split("\n").slice(0, 6).join(" | ")}`,
  );
  const outcomes = JSON.parse(result.stdout);
  assert.equal(outcomes.length, rounds);
  return outcomes;
}

/** Collect exceptions raised outside any call while `body` runs. */
async function withUncaughtProbe(body) {
  const uncaught = [];
  const probe = (error) => {
    uncaught.push(error);
  };
  process.on("uncaughtException", probe);
  try {
    await body();
    // A stream error is raised on a later turn of the event loop.
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.deepEqual(uncaught.map(String), [], "no exception may be raised outside the call");
  } finally {
    process.removeListener("uncaughtException", probe);
  }
}

/** A call's error, or a failure when it has not settled within `ms`. */
async function failureWithin(call, ms) {
  let handle;
  const late = new Promise((resolve) => {
    handle = setTimeout(() => resolve("unsettled"), ms);
  });
  const outcome = await Promise.race([
    call.then(
      () => "resolved",
      (error) => error,
    ),
    late,
  ]).finally(() => clearTimeout(handle));
  assert.notEqual(outcome, "unsettled", `the call did not settle within ${ms} ms`);
  assert.notEqual(outcome, "resolved", "the call must fail");
  return outcome;
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

describe("A child whose input is gone", () => {
  it("a child that died while the host was busy: the next call in that turn fails with its exit", { timeout: 150_000 }, async () => {
    const outcomes = await hostOutcomes(writeFixture(IS_GONE), 5, "busy");
    for (const calls of outcomes) assert.deepEqual(calls, ["PROCESS_EXITED", "PROCESS_EXITED"]);
  });

  it("a child that asks the host and is gone at once: the request fails every time", { timeout: 150_000 }, async () => {
    const outcomes = await hostOutcomes(writeFixture(ASKS_AND_IS_GONE), 5);
    for (const calls of outcomes) assert.deepEqual(calls, ["PROCESS_EXITED"]);
  });

  it("a child that closed its input, then exits: the next request fails with its exit", { timeout: 20_000 }, async () => {
    const client = new MeerkatClient(writeFixture(CLOSES_INPUT_THEN_EXITS));
    try {
      await withUncaughtProbe(async () => {
        await client.connect();
        assert.deepEqual(await client.getConfig(), { closed: true });
        const failure = await failureWithin(client.getConfig(), 5_000);
        assert.equal(failure.code, "PROCESS_EXITED", String(failure));
        assert.ok(failure.message.endsWith(": fixture: going away"), failure.message);
      });
    } finally {
      await client.close();
    }
  });

  it("a running child that closed its input: the next request fails, and the child is stopped", { timeout: 20_000 }, async () => {
    const client = new MeerkatClient(writeFixture(CLOSES_INPUT_AND_STAYS));
    try {
      await withUncaughtProbe(async () => {
        await client.connect();
        const pid = client.process.pid;
        assert.deepEqual(await client.getConfig(), { closed: true });
        const failure = await failureWithin(client.getConfig(), 5_000);
        assert.equal(failure.code, "PROTOCOL_ERROR", String(failure));
        assert.match(failure.message, /write/);
        // Nothing can be asked of this child any more: a later call fails
        // the same way, and the client lets the child go as close() does.
        const later = await failureWithin(client.getConfig(), 1_000);
        assert.equal(later, failure);
        await client.close();
        assert.equal(alive(pid), false, "the child must have been stopped");
      });
    } finally {
      await client.close();
    }
  });

  it("a failed read of the child's output: the waiting request fails, and the child is stopped", { timeout: 20_000 }, async () => {
    const client = new MeerkatClient(writeFixture(SILENT));
    try {
      await withUncaughtProbe(async () => {
        await client.connect();
        const pid = client.process.pid;
        const waiting = client.getConfig();
        // The host's end of the child's output fails, as a read error does.
        client.process.stdout.destroy(new Error("read failed"));
        const failure = await failureWithin(waiting, 5_000);
        assert.equal(failure.code, "PROTOCOL_ERROR", String(failure));
        assert.match(failure.message, /read failed/);
        await client.close();
        assert.equal(alive(pid), false, "the child must have been stopped");
      });
    } finally {
      await client.close();
    }
  });

  it("a failed read of the child's stderr leaves the client working", { timeout: 20_000 }, async () => {
    const client = new MeerkatClient(writeFixture(HEALTHY));
    try {
      await withUncaughtProbe(async () => {
        await client.connect();
        client.process.stderr.destroy(new Error("read failed"));
        await new Promise((resolve) => setTimeout(resolve, 50));
        assert.deepEqual(await client.getConfig(), { answered: true });
      });
    } finally {
      await client.close();
    }
  });

  it("a healthy child: requests, a callback reply and close() are unchanged", { timeout: 20_000 }, async () => {
    const client = new MeerkatClient(writeFixture(HEALTHY));
    await withUncaughtProbe(async () => {
      await client.connect();
      const pid = client.process.pid;
      for (let call = 0; call < 3; call += 1) {
        assert.deepEqual(await client.getConfig(), { answered: true });
      }
      // The child asks the host; the host's reply reaches it.
      assert.deepEqual(await client.request("fixture/ask", {}), { host_answered: -32601 });
      await client.close();
      assert.equal(alive(pid), false);
    });
  });
});
