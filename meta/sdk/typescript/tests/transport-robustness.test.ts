/**
 * Transport robustness (#550, part 1), one test per defect:
 * - a stdout line that is not a JSON object is skipped, not thrown out of the
 *   readline listener;
 * - a reader exit fails every waiter with the typed TransportReaderFailedError
 *   and fails later requests at once;
 * - a callback result with a non-finite number answers a typed error instead
 *   of silently becoming `null`;
 * - a callback request with no registered handler is answered at once.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { once } from "node:events";
import os from "node:os";
import path from "node:path";

import { PersistentTransport } from "../dist/transport.js";
import {
  GatewayChildEndedError,
  TransportError,
  TransportReaderFailedError,
} from "../dist/index.js";

function withWrites(transport: PersistentTransport): Array<Record<string, unknown>> {
  const writes: Array<Record<string, unknown>> = [];
  (transport as any)._writeLine = (value: Record<string, unknown>) => {
    writes.push(value);
  };
  return writes;
}

describe("persistent transport robustness", () => {
  it("skips stdout lines that are not JSON objects and keeps delivering responses", () => {
    const transport = new PersistentTransport("unused-test-gateway");
    let delivered: unknown = null;
    (transport as any)._pending.set("r1", {
      resolve: (value: unknown) => {
        delivered = value;
      },
      reject: () => assert.fail("must not reject"),
    });
    for (const line of ["not json", "5", "[1, 2]", "null", "\"text\""]) {
      assert.doesNotThrow(() => (transport as any)._handleLine(line));
    }
    (transport as any)._handleLine('{"jsonrpc":"2.0","id":"r1","result":{"ok":true}}');
    assert.deepEqual(delivered, { jsonrpc: "2.0", id: "r1", result: { ok: true } });
  });

  it("fails every waiter with a typed error when the reader stops, and later requests at once", async () => {
    const transport = new PersistentTransport("unused-test-gateway");
    withWrites(transport);
    (transport as any)._ensureRunning = () => {};
    const waiting = (transport as any)._sendAsyncWithTimeout(
      { jsonrpc: "2.0", id: "w1", method: "meta/status" },
      30_000,
    );

    (transport as any)._onReaderClosed("the gateway closed its stdout");

    await assert.rejects(waiting, (error: unknown) => {
      assert.ok(error instanceof TransportReaderFailedError);
      assert.ok(error instanceof TransportError);
      assert.match((error as TransportReaderFailedError).reason, /closed its stdout/);
      return true;
    });
    await assert.rejects(
      (transport as any)._sendAsyncWithTimeout(
        { jsonrpc: "2.0", id: "w2", method: "meta/status" },
        30_000,
      ),
      TransportReaderFailedError,
    );
  });

  for (const value of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    it(`answers a typed error for a callback result containing ${value}`, async () => {
      const transport = new PersistentTransport("unused-test-gateway");
      const writes = withWrites(transport);
      transport.setCallbackHandler(async () => ({ roster: [{ weight: value }] }));

      (transport as any)._handleCallback({
        jsonrpc: "2.0",
        id: "cb-7",
        method: "callback/roster_provider/roster",
        params: {},
      });
      await new Promise<void>((resolve) => setImmediate(resolve));

      assert.equal(writes.length, 1);
      assert.equal(writes[0].id, "cb-7");
      assert.equal("result" in writes[0], false);
      const error = writes[0].error as Record<string, unknown>;
      assert.deepEqual(error.data, { kind: "non_finite_result" });
      assert.match(String(error.message), /NaN or Infinity/);
    });
  }

  it("answers a callback request with no registered handler at once", () => {
    const transport = new PersistentTransport("unused-test-gateway");
    const writes = withWrites(transport);

    (transport as any)._handleCallback({
      jsonrpc: "2.0",
      id: "cb-3",
      method: "callback/build_agent",
      params: {},
    });

    assert.equal(writes.length, 1);
    assert.equal(writes[0].id, "cb-3");
    assert.deepEqual((writes[0].error as Record<string, unknown>).data, {
      kind: "callback_handler_unavailable",
      method: "callback/build_agent",
    });
  });

  it("writes nothing for a notification with no registered handler", () => {
    const transport = new PersistentTransport("unused-test-gateway");
    const writes = withWrites(transport);

    (transport as any)._handleCallback({ jsonrpc: "2.0", method: "notification/x", params: {} });

    assert.equal(writes.length, 0);
  });
});

describe("persistent callback connection lifetime", () => {
  it("retires exited-child waiters before restart while its stdout is still open", { timeout: 10_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meta-ts-exited-reader-"));
    const bin = path.join(root, "gateway.cjs");
    const holderPid = path.join(root, "holder.pid");
    writeFileSync(bin, `#!/usr/bin/env node
if (process.argv.includes("--hold-stdout")) {
  setTimeout(() => {}, 5000);
} else {
  const rl = require("node:readline").createInterface({ input: process.stdin });
  const send = (value) => process.stdout.write(JSON.stringify(value) + "\\n");
  let held;
  rl.on("line", (line) => {
    const msg = JSON.parse(line);
    if (msg.method === "test/exit") {
      const holder = require("node:child_process").spawn(process.execPath,
        [__filename, "--hold-stdout"], { stdio: ["ignore", 1, "ignore"] });
      require("node:fs").writeFileSync(${JSON.stringify(holderPid)}, String(holder.pid));
      holder.unref();
      process.exit(0);
    } else if (msg.method === "test/hold") {
      held = msg.id;
    } else if (msg.method === "test/release") {
      send({ jsonrpc: "2.0", id: held, result: "replacement response" });
      send({ jsonrpc: "2.0", method: "meta/init_settled", params: { init_id: "new-init", status: "ready" } });
      send({ jsonrpc: "2.0", id: msg.id, result: true });
    }
  });
}
`);
    chmodSync(bin, 0o755);
    const transport = new PersistentTransport(bin);
    transport.start();
    const original = (transport as any)._process;
    const exited = once(original, "exit");
    const closed = once(original, "close");
    let oldError: unknown;
    let oldInitError: unknown;
    const oldWatch = transport.openInitWatch("old-init");
    const oldInit = oldWatch.settlement(null).catch((error: unknown) => { oldInitError = error; });
    const oldWork = transport.sendAsync({ jsonrpc: "2.0", id: "reused-request", method: "test/exit" },
      { timeoutMs: 500 }).catch((error: unknown) => { oldError = error; });
    try {
      await exited;
      assert.equal(original.stdout.readableEnded, false, "the descendant must keep old stdout open");
      const currentWork = transport.sendAsync({ jsonrpc: "2.0", id: "reused-request", method: "test/hold" },
        { timeoutMs: 2000 });
      currentWork.catch(() => {});
      const currentWatch = transport.openInitWatch("new-init");
      await new Promise<void>((resolve) => setImmediate(resolve));
      assert.ok(oldError instanceof TransportReaderFailedError, "restart must reject old waiters before old EOF or timeout");
      assert.ok(oldInitError instanceof TransportReaderFailedError, "restart must fail the old init watch");
      await Promise.all([oldWork, oldInit]);
      // Let the retired request's original deadline pass before replying to
      // the reused id. Its timer must not delete the replacement's waiter.
      await new Promise<void>((resolve) => setTimeout(resolve, 550));
      process.kill(Number(readFileSync(holderPid, "utf8")), "SIGTERM");
      await closed;
      assert.equal((transport as any)._readerFailure, null);
      await transport.sendAsync({ jsonrpc: "2.0", id: "release", method: "test/release" });
      assert.deepEqual(await currentWork, { jsonrpc: "2.0", id: "reused-request", result: "replacement response" });
      assert.deepEqual(await currentWatch.settlement(1000), { init_id: "new-init", status: "ready" });
    } finally {
      try { process.kill(Number(readFileSync(holderPid, "utf8")), "SIGTERM"); } catch {}
      await transport.stop();
      await closed;
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("fails a waiting request with the exit when the gateway exits while a process it started holds its stdout", { timeout: 10_000 }, async () => {
    const root = mkdtempSync(path.join(os.tmpdir(), "meta-ts-exit-held-stdout-"));
    const bin = path.join(root, "gateway.cjs");
    const holderPid = path.join(root, "holder.pid");
    writeFileSync(bin, `#!/usr/bin/env node
if (process.argv.includes("--hold-stdout")) {
  setTimeout(() => {}, 30000);
} else {
  const rl = require("node:readline").createInterface({ input: process.stdin });
  rl.on("line", (line) => {
    if (JSON.parse(line).method !== "test/exit") return;
    const holder = require("node:child_process").spawn(process.execPath,
      [__filename, "--hold-stdout"], { stdio: ["ignore", 1, "ignore"] });
    require("node:fs").writeFileSync(${JSON.stringify(holderPid)}, String(holder.pid));
    holder.unref();
    process.exit(7);
  });
}
`);
    chmodSync(bin, 0o755);
    const transport = new PersistentTransport(bin);
    transport.start();
    const child = (transport as any)._process;
    const closed = once(child, "close");
    try {
      const started = performance.now();
      const request = { jsonrpc: "2.0", id: "exit-request", method: "test/exit" };
      const error = await transport.sendAsync(request, { timeoutMs: 8000 }).then(
        () => null,
        (failure: unknown) => failure,
      );
      const settledMs = performance.now() - started;
      // Settled by the exit after its grace, not by the 8 s request timeout.
      assert.ok(
        settledMs < 4000,
        `settled ${Math.round(settledMs)} ms after the request: ${String(error)}`,
      );
      assert.ok(error instanceof GatewayChildEndedError, String(error));
      assert.ok(error instanceof TransportReaderFailedError);
      assert.equal(error.exitCode, 7);
      assert.equal(error.signal, null);
      // The transport let go of the stdout the holder keeps, so the child's
      // streams close while the holder still runs.
      await closed;
      // Letting go of stdout does not relabel the child's failure: an init
      // watch opened now fails at once with the same exit.
      const watch = transport.openInitWatch("after-exit");
      await assert.rejects(
        watch.settlement(1000),
        (failure: unknown) => failure instanceof GatewayChildEndedError && failure.exitCode === 7,
      );
      transport.closeInitWatch(watch);
    } finally {
      try { process.kill(Number(readFileSync(holderPid, "utf8")), "SIGTERM"); } catch {}
      await transport.stop();
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("does not start a queued callback handler after its child was replaced", async () => {
    const transport = new PersistentTransport("unused-test-gateway");
    const original = { stdin: { writable: true, write: () => assert.fail("retired write") } };
    const replacement = { stdin: { writable: true, write: () => assert.fail("foreign write") } };
    let calls = 0;
    transport.setCallbackHandler(async () => { calls += 1; return "old"; });
    (transport as any)._process = original;
    (transport as any)._handleLine(JSON.stringify({
      jsonrpc: "2.0", id: "cb-queued", method: "callback/build_agent", params: {},
    }));
    (transport as any)._process = replacement;
    await new Promise<void>((resolve) => setImmediate(resolve));
    assert.equal(calls, 0, "a retired invocation must not start its host handler");
  });

  it("ignores a retired reader's response and EOF while the replacement is waiting", () => {
    const transport = new PersistentTransport("unused-test-gateway");
    const original = {};
    const replacement = {};
    let response: unknown;
    (transport as any)._process = replacement;
    (transport as any)._pending.set("same-request", {
      resolve: (value: unknown) => { response = value; },
      reject: () => assert.fail("retired EOF rejected replacement request"),
    });
    (transport as any)._handleLine('{"id":"same-request","result":"old"}', original);
    assert.equal(response, undefined);
    (transport as any)._onReaderClosed("old stdout closed", original);
    assert.equal((transport as any)._readerFailure, null);
    assert.equal((transport as any)._pending.size, 1);
    (transport as any)._handleLine('{"id":"same-request","result":"current"}', replacement);
    assert.deepEqual(response, { id: "same-request", result: "current" });
    (transport as any)._onReaderClosed("current stdout closed", replacement);
    assert.ok((transport as any)._readerFailure instanceof TransportReaderFailedError);
  });

  for (const oldOutcome of ["success", "error", "deadline"]) {
    it(`drops an old callback ${oldOutcome} after restart with a reused callback id`, { timeout: 10_000 }, async () => {
      const root = mkdtempSync(path.join(os.tmpdir(), "meta-ts-callback-"));
      const bin = path.join(root, "gateway.cjs");
      writeFileSync(bin, `#!/usr/bin/env node
const rl = require("node:readline").createInterface({ input: process.stdin });
const received = [];
let callbackRequest;
const send = (value) => process.stdout.write(JSON.stringify(value) + "\\n");
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.method === "test/callback") {
    callbackRequest = msg.id;
    send({ jsonrpc: "2.0", id: "cb-reused", method: "callback/build_agent", params: msg.params });
  } else if (msg.method) {
    send({ jsonrpc: "2.0", id: msg.id, result: received });
  } else {
    received.push(msg);
    if (callbackRequest !== undefined) {
      send({ jsonrpc: "2.0", id: callbackRequest, result: msg });
      callbackRequest = undefined;
    }
  }
});
`);
      chmodSync(bin, 0o755);
      const transport = new PersistentTransport(bin);
      let releaseOld!: () => void;
      let releaseCurrent!: () => void;
      let enteredOld!: () => void;
      let enteredCurrent!: () => void;
      let oldAborted!: () => void;
      const oldGate = new Promise<void>((resolve) => { releaseOld = resolve; });
      const currentGate = new Promise<void>((resolve) => { releaseCurrent = resolve; });
      const oldEntered = new Promise<void>((resolve) => { enteredOld = resolve; });
      const currentEntered = new Promise<void>((resolve) => { enteredCurrent = resolve; });
      const oldAbort = new Promise<void>((resolve) => { oldAborted = resolve; });
      const currentResult = { content: [{ type: "text", text: "current result" }] };
      transport.setCallbackHandler(async (_method, { origin }, context) => {
        if (origin === "old") {
          context.signal.addEventListener("abort", oldAborted, { once: true });
          enteredOld();
          await oldGate;
          if (oldOutcome === "error") throw new Error("old failure");
          return { invocation: "old" };
        }
        if (origin === "current") {
          enteredCurrent();
          await currentGate;
          return currentResult;
        }
        if (origin === "control-error") throw new Error("original error control");
        return { invocation: "original control" };
      });
      const request = (id: string, method: string, params = {}) => transport.sendAsync({
        jsonrpc: "2.0", id, method, params,
      }) as Promise<any>;
      try {
        const original = await request("control", "test/callback", { origin: "control" });
        assert.deepEqual(original.result.result, { invocation: "original control" });
        const originalError = await request("control-error", "test/callback", { origin: "control-error" });
        assert.deepEqual(originalError.result.error, { code: -32000, message: "original error control" });
        if (oldOutcome === "deadline") (transport as any)._providerCallbackCompletionMs = 500;
        const oldWork = request("old", "test/callback", { origin: "old" });
        const oldClosed = assert.rejects(oldWork, TransportReaderFailedError);
        await oldEntered;
        await transport.stop();
        await oldClosed;
        (transport as any)._providerCallbackCompletionMs = 125_000;
        const currentWork = request("current", "test/callback", { origin: "current" });
        currentWork.catch(() => {});
        await currentEntered;
        if (oldOutcome === "deadline") await oldAbort;
        else releaseOld();
        await new Promise<void>((resolve) => setImmediate(resolve));
        assert.deepEqual((await request("barrier", "test/received")).result, [],
          "a retired callback must not answer the replacement process's reused id");
        releaseCurrent();
        const current = await currentWork;
        assert.deepEqual(current.result.result, currentResult);
        assert.equal((await request("received", "test/received")).result.length, 1);
      } finally {
        releaseOld();
        releaseCurrent();
        await transport.stop();
        rmSync(root, { recursive: true, force: true });
      }
    });
  }
});
