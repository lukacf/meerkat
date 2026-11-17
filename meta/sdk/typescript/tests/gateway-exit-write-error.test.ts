/**
 * A gateway whose input is gone: a request that can no longer be written is
 * rejected to its caller, never raised outside the call.
 *
 * - A gateway that refuses a start and exits, or exits after accepting the
 *   init: `build()` rejects, every time, with an error carrying the startup
 *   failure, and the host program runs on.
 * - A gateway that closed its input while it still runs: the next request
 *   and every later one reject with `TransportReaderFailedError`.
 * - A read of the gateway's output that fails: the waiting request and
 *   every later one reject the same way.
 * - A healthy gateway: requests, callback replies and the stop are unchanged.
 */

import { afterEach, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFile, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { PersistentTransport, TransportReaderFailedError } from "../dist/index.js";

const SDK_ENTRY = new URL("../dist/index.js", import.meta.url).href;

const REFUSAL_CODE = -32000;
const REFUSAL_MESSAGE = "Runtime bootstrap failed: the start was refused";

// A stand-in gateway; its first argument picks what it does.
const GATEWAY = `
const fs = require("node:fs");
const readline = require("node:readline");
const mode = process.argv[2];
const emit = (obj) => fs.writeSync(1, JSON.stringify(obj) + "\\n");
const rl = readline.createInterface({ input: process.stdin });
// Destroying the stream leaves descriptor 0 open: close it too, so that the
// host's next write finds no reader.
const closeInput = () => { rl.close(); process.stdin.destroy(); fs.closeSync(0); };
// Gone at once, as a native process is: no exit handlers, no teardown.
const exitNow = () => process.kill(process.pid, "SIGKILL");
const accepted = (init) => ({ jsonrpc: "2.0", id: init.id, result: {
  init_state: "accepted", init_id: init.params.init_id,
  stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 30000 } });
const refused = (init) => ({ jsonrpc: "2.0", method: "meta/init_settled", params: {
  init_id: init.params.init_id, outcome: "failed", code: ${REFUSAL_CODE},
  message: ${JSON.stringify(REFUSAL_MESSAGE)}, durable_effects: "none" } });
let answered = 0;
let callbackFor = null;
rl.on("line", (line) => {
  const message = JSON.parse(line);
  if (mode === "refuses-and-exits") {
    // The refusal is written, then the process is gone at once.
    emit(accepted(message));
    emit(refused(message));
    exitNow();
  } else if (mode === "refuses-with-input-closed") {
    // The input is gone before the refusal can be read; the exit follows.
    closeInput();
    emit(accepted(message));
    emit(refused(message));
    setTimeout(() => process.exit(1), 300);
  } else if (mode === "accepts-and-exits") {
    emit(accepted(message));
    exitNow();
  } else if (mode === "closes-input-after-first-answer") {
    emit({ jsonrpc: "2.0", id: message.id, result: { answered: ++answered } });
    closeInput();
    emit({ jsonrpc: "2.0", method: "fixture/input_closed", params: {} });
    setTimeout(() => process.exit(0), 30000);
  } else if (mode === "silent") {
    // Reads its input and never answers.
  } else if (callbackFor !== null && message.id === "callback-1") {
    // The host's reply to the callback completes the request that asked.
    emit({ jsonrpc: "2.0", id: callbackFor, result: { host_replied: message.result } });
    callbackFor = null;
  } else if (message.method === "fixture/call_host") {
    callbackFor = message.id;
    emit({ jsonrpc: "2.0", id: "callback-1", method: "fixture/host_value", params: {} });
  } else {
    emit({ jsonrpc: "2.0", id: message.id, result: { answered: ++answered } });
  }
});
rl.on("close", () => { if (mode === "healthy" || mode === "silent") process.exit(0); });
`;

// A host program: `build()` inside a try/catch, as many times as asked, with
// no handler for uncaught exceptions. It reaches its last line only when
// every `build()` settled to its caller.
const HOST = `
const sdk = await import(process.argv[2]);
const outcomes = [];
for (let round = 0; round < Number(process.argv[4]); round += 1) {
  try {
    const runtime = await sdk.Meta.builder().gateway(process.argv[3]).build();
    await runtime.shutdown();
    outcomes.push({ rejected: false });
  } catch (error) {
    const wrapped = error instanceof sdk.GatewayStartupCleanupError;
    const startup = wrapped ? error.startupError : error;
    outcomes.push({
      rejected: true,
      wrapped,
      outcomeUnknown: startup instanceof sdk.InitOutcomeUnknownError,
      rpcError: sdk.isRpcError(startup),
      code: startup?.code ?? null,
      message: String(startup?.message),
    });
  }
}
process.stdout.write(JSON.stringify(outcomes) + "\\n");
`;

interface Outcome {
  rejected: boolean;
  wrapped?: boolean;
  outcomeUnknown?: boolean;
  rpcError?: boolean;
  code?: number | null;
  message?: string;
}

const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function gateway(mode: string): string {
  const dir = mkdtempSync(join(tmpdir(), "meta-ts-input-gone-"));
  dirs.push(dir);
  const script = join(dir, "gateway.cjs");
  writeFileSync(script, GATEWAY);
  const bin = join(dir, "gateway.sh");
  writeFileSync(bin, `#!/bin/sh\nexec "${process.execPath}" "${script}" ${mode} "$@"\n`, {
    mode: 0o755,
  });
  return bin;
}

/**
 * Run the host program for `rounds` starts of `bin`. The program must end by
 * itself with exit code 0 and one outcome per start: an exception raised
 * outside `build()` would have ended it early.
 */
async function hostOutcomes(bin: string, rounds: number): Promise<Outcome[]> {
  const dir = mkdtempSync(join(tmpdir(), "meta-ts-input-gone-host-"));
  dirs.push(dir);
  const host = join(dir, "host.mjs");
  writeFileSync(host, HOST);
  const result = await new Promise<{ code: number | string | null; stdout: string; stderr: string }>(
    (resolve) => {
      execFile(
        process.execPath,
        [host, SDK_ENTRY, bin, String(rounds)],
        { cwd: dir, timeout: 120_000 },
        (error, stdout, stderr) =>
          resolve({ code: error === null ? 0 : (error.code ?? error.signal ?? null), stdout, stderr }),
      );
    },
  );
  assert.equal(
    result.code,
    0,
    `the host program was ended before its last line: ${result.stderr.trim().split("\n").slice(0, 6).join(" | ")}`,
  );
  const outcomes = JSON.parse(result.stdout) as Outcome[];
  assert.equal(outcomes.length, rounds);
  return outcomes;
}

/**
 * Every start rejected to its caller with the gateway's refusal: either the
 * refusal itself, or the startup cleanup error carrying it as `startupError`
 * when the stop of the gateway was rejected too.
 */
function assertRefusedInBand(outcomes: Outcome[]): void {
  for (const [round, outcome] of outcomes.entries()) {
    assert.equal(outcome.rejected, true, `round ${round}: build() must reject`);
    assert.equal(outcome.rpcError, true, `round ${round}: ${JSON.stringify(outcome)}`);
    assert.equal(outcome.outcomeUnknown, false, `round ${round}: ${JSON.stringify(outcome)}`);
    assert.equal(outcome.code, REFUSAL_CODE, `round ${round}: ${JSON.stringify(outcome)}`);
    assert.match(outcome.message ?? "", /the start was refused/, `round ${round}`);
  }
}

/** A short bounded wait for a child's exit, scaling the stop's waits. */
async function quickWait(child: ChildProcess): Promise<boolean> {
  if (child.exitCode !== null || child.signalCode !== null) return true;
  let handle: NodeJS.Timeout | undefined;
  const timer = new Promise<boolean>((resolve) => {
    handle = setTimeout(() => resolve(child.exitCode !== null || child.signalCode !== null), 100);
  });
  return Promise.race([once(child, "exit").then(() => true), timer]).finally(() =>
    clearTimeout(handle),
  );
}

/** Collect exceptions raised outside any call while `body` runs. */
async function withUncaughtProbe(body: (uncaught: unknown[]) => Promise<void>): Promise<void> {
  const uncaught: unknown[] = [];
  const probe = (error: unknown): void => {
    uncaught.push(error);
  };
  process.on("uncaughtException", probe);
  try {
    await body(uncaught);
    // A stream error is raised on a later turn of the event loop.
    await new Promise((resolve) => setTimeout(resolve, 50));
    assert.deepEqual(uncaught.map(String), [], "no exception may be raised outside the call");
  } finally {
    process.removeListener("uncaughtException", probe);
  }
}

function request(id: string, method = "fixture/echo"): Record<string, unknown> {
  return { jsonrpc: "2.0", id, method, params: {} };
}

describe("a gateway whose input is gone", () => {
  it("a gateway that refuses the start and exits at once: build() rejects every time", async () => {
    assertRefusedInBand(await hostOutcomes(gateway("refuses-and-exits"), 50));
  });

  it("a refusing gateway whose input closed before its exit: build() rejects every time", async () => {
    assertRefusedInBand(await hostOutcomes(gateway("refuses-with-input-closed"), 5));
  });

  it("a gateway that exits after accepting the init: build() rejects every time", async () => {
    const outcomes = await hostOutcomes(gateway("accepts-and-exits"), 5);
    for (const [round, outcome] of outcomes.entries()) {
      assert.equal(outcome.rejected, true, `round ${round}: build() must reject`);
      assert.equal(outcome.outcomeUnknown, true, `round ${round}: ${JSON.stringify(outcome)}`);
    }
  });

  it("a running gateway that closed its input: the next request rejects to its caller", async () => {
    const transport = new PersistentTransport(gateway("closes-input-after-first-answer"), {
      timeout: 20_000,
    });
    (transport as unknown as { _waitForExit: typeof quickWait })._waitForExit = quickWait;
    let inputClosed: () => void = () => undefined;
    const closed = new Promise<void>((resolve) => {
      inputClosed = resolve;
    });
    transport.setCallbackHandler(async (method) => {
      if (method === "fixture/input_closed") inputClosed();
      return null;
    });
    try {
      await withUncaughtProbe(async () => {
        transport.start();
        const first = (await transport.sendAsync(request("1"))) as { result: unknown };
        assert.deepEqual(first.result, { answered: 1 });
        await closed;

        const failure = await transport.sendAsync(request("2")).then(
          () => null,
          (error: unknown) => error,
        );
        assert.ok(failure instanceof TransportReaderFailedError, String(failure));
        assert.equal(
          transport.isRunning(),
          true,
          "the failed write rejected the request, not a later exit of the gateway",
        );
        // Nothing can be asked of this gateway any more: a later request
        // rejects the same way instead of waiting out its timeout.
        await assert.rejects(transport.sendAsync(request("3")), TransportReaderFailedError);
        assert.equal(transport.isRunning(), true);
      });
    } finally {
      await transport.stop().catch(() => undefined);
    }
    assert.equal(transport.isRunning(), false);
  });

  it("a failed read of the gateway's output: the waiting request rejects to its caller", async () => {
    const transport = new PersistentTransport(gateway("silent"), { timeout: 20_000 });
    try {
      await withUncaughtProbe(async () => {
        transport.start();
        const waiting = transport.sendAsync(request("1")).then(
          () => null,
          (error: unknown) => error,
        );
        // The host's end of the gateway's output fails, as a read error does.
        const child = (transport as unknown as { _process: ChildProcess })._process;
        child.stdout?.destroy(new Error("read failed"));

        const failure = await waiting;
        assert.ok(failure instanceof TransportReaderFailedError, String(failure));
        assert.match(failure.reason, /read failed/);
        assert.equal(transport.isRunning(), true);
        await assert.rejects(transport.sendAsync(request("2")), TransportReaderFailedError);
      });
    } finally {
      await transport.stop().catch(() => undefined);
    }
    assert.equal(transport.isRunning(), false);
  });

  it("a healthy gateway: requests, a callback reply and the stop are unchanged", async () => {
    const transport = new PersistentTransport(gateway("healthy"), { timeout: 20_000 });
    transport.setCallbackHandler(async (method) =>
      method === "fixture/host_value" ? { value: 7 } : null,
    );
    await withUncaughtProbe(async () => {
      transport.start();
      for (const id of ["1", "2", "3"]) {
        const answer = (await transport.sendAsync(request(id))) as { result: unknown };
        assert.deepEqual(answer.result, { answered: Number(id) });
      }
      const viaHost = (await transport.sendAsync(request("4", "fixture/call_host"))) as {
        result: unknown;
      };
      assert.deepEqual(viaHost.result, { host_replied: { value: 7 } });
      await transport.stop();
      assert.deepEqual(transport.lastTermination(), {
        state: "exited",
        exitCode: 0,
        signal: null,
        escalation: "none",
        nativeShutdown: "not_negotiated",
      });
    });
  });
});
