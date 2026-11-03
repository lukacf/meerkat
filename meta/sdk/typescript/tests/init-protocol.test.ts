/**
 * meta/init: accepted, then settled (#550, part 2), against stand-in
 * gateways. Twin of the Python `test_init_protocol.py`:
 * - a legal slow startup (a callback round trip, then a settlement later than
 *   the SDK's request timeout) settles ready;
 * - a failed settlement throws the typed error carrying `durable_effects`;
 * - once the init request is written, a lost acceptance, a gateway exit, an
 *   init deadline or a foreign init id is `InitOutcomeUnknownError`, never a
 *   refusal;
 * - an older gateway's single response still connects.
 */

import { afterEach, describe, it, mock } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  COMPOSITION_PROVENANCE_CODE,
  CompositionProvenanceError,
  GatewayStartupCleanupError,
  INIT_IN_PROGRESS_CODE,
  InitInProgressError,
  InitOutcomeUnknownError,
  Meta,
  MetaRuntime,
  RpcError,
  StorageResolutionError,
  isRpcError,
} from "../dist/index.js";

// Shared prelude: read the init request, record it, expose answer helpers.
// `steps` is an async function body using them.
const PRELUDE = `
const fs = require("node:fs");
const readline = require("node:readline");
const record = process.env.RECORD_FILE;
const log = (entry) => fs.appendFileSync(record, JSON.stringify(entry) + "\\n");
const emit = (obj) => process.stdout.write(JSON.stringify(obj) + "\\n");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const rl = readline.createInterface({ input: process.stdin });
const lines = [];
let wake = null;
rl.on("line", (line) => { lines.push(JSON.parse(line)); if (wake) { const w = wake; wake = null; w(); } });
rl.on("close", () => process.exit(0));
async function next() {
  while (lines.length === 0) await new Promise((resolve) => { wake = resolve; });
  return lines.shift();
}
async function serveShutdown() {
  for (;;) {
    const message = await next();
    log({ after_init: message });
    if (message.method === "meta/shutdown") {
      emit({ jsonrpc: "2.0", id: message.id, result: { shutdown: true, runtime_cleanup_completed: true } });
      return;
    }
  }
}
(async () => {
  const init = await next();
  log({ init });
  const initId = init.params.init_id;
  const accepted = () => emit({ jsonrpc: "2.0", id: init.id, result: {
    init_state: "accepted", init_id: initId, provider_callback_timeout_ms: 130000,
    stdio_shutdown_handshake: true, stdio_shutdown_horizon_ms: 30000 } });
  const progress = (phase, forInit) => emit({ jsonrpc: "2.0", method: "meta/init_progress",
    params: { init_id: forInit ?? initId, phase } });
  const settled = (params, forInit) => emit({ jsonrpc: "2.0", method: "meta/init_settled",
    params: { init_id: forInit ?? initId, ...params } });
  await (async () => { STEPS })();
})();
`;

const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function gateway(steps: string): { bin: string; record: string } {
  const dir = mkdtempSync(join(tmpdir(), "meta-init-gateway-"));
  dirs.push(dir);
  const script = join(dir, "gateway.cjs");
  const record = join(dir, "record.jsonl");
  writeFileSync(script, PRELUDE.replace("STEPS", steps));
  const bin = join(dir, "gateway.sh");
  writeFileSync(
    bin,
    `#!/bin/sh\nRECORD_FILE="${record}" exec "${process.execPath}" "${script}" "$@"\n`,
    { mode: 0o755 },
  );
  return { bin, record };
}

function records(record: string): Array<Record<string, any>> {
  if (!existsSync(record)) return [];
  return readFileSync(record, "utf-8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

/**
 * A gateway that negotiated the shutdown handshake and then exited leaves the
 * runtime's stop nobody to ask, so that stop rejects: the startup error
 * arrives exactly, as `startupError` and `cause` of the cleanup error, the
 * stop's own record stays apart, and a retry has nothing left to clean up.
 * Returns the original startup error.
 */
async function startupErrorBehindRejectedStop(rt: MetaRuntime, exitCode: number): Promise<any> {
  let failure: unknown = null;
  await assert.rejects(rt.connect(), (error: unknown) => {
    failure = error;
    return true;
  });
  assert.ok(failure instanceof GatewayStartupCleanupError, String(failure));
  assert.equal(failure.cause, failure.startupError);
  assert.match((failure.cleanupError as Error).message, /gateway shutdown failed after bounded cleanup/);
  const record = {
    state: "exited",
    exitCode,
    signal: null,
    escalation: "none",
    nativeShutdown: "unanswered",
  };
  assert.deepEqual(failure.cleanup.lastTermination(), record);
  await failure.cleanup.shutdown();
  assert.deepEqual(rt.lastGatewayTermination(), record);
  return failure.startupError;
}

function runtime(bin: string, options: { timeoutMs?: number; initDeadlineMs?: number } = {}): MetaRuntime {
  const builder = Meta.builder().gateway(bin);
  if (options.timeoutMs !== undefined) builder.gatewayTimeoutMs(options.timeoutMs);
  if (options.initDeadlineMs !== undefined) builder.initDeadline(options.initDeadlineMs);
  return new MetaRuntime((builder as any)._config);
}

describe("meta/init accepted then settled", () => {
  it("settles ready after a callback round trip, later than the request timeout", async () => {
    const { bin, record } = gateway(`
      accepted();
      progress("storage");
      emit({ jsonrpc: "2.0", id: "cb-1", method: "callback/unknown_startup_probe", params: {} });
      log({ callback_answer: await next() });
      progress("restore");
      await sleep(1500);
      settled({ outcome: "ready", http_base_url: "http://127.0.0.1:9" });
      await serveShutdown();
    `);
    const rt = runtime(bin, { timeoutMs: 500 });
    try {
      await rt.connect();
      assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:9");
    } finally {
      await rt.shutdown();
    }
    const entries = records(record);
    assert.equal(entries[0].init.params.init_protocol, "accepted_then_settled");
    assert.match(entries[0].init.params.init_id, /^init-/);
    assert.equal(entries.find((entry) => entry.callback_answer)?.callback_answer.id, "cb-1");
  });

  for (const [code, durableEffects, errorType] of [
    [-32602, "none", RpcError],
    [-32014, "possible", StorageResolutionError],
  ] as const) {
    it(`throws the typed error with durable_effects=${durableEffects} for a failed settlement`, async () => {
      const { bin } = gateway(`
        accepted();
        progress("storage");
        settled({ outcome: "failed", code: ${code}, message: "refused for the test", durable_effects: "${durableEffects}" });
        // A gateway exits right after a failed settlement.
        await sleep(20);
        process.exit(1);
      `);
      const rt = runtime(bin);
      const error = await startupErrorBehindRejectedStop(rt, 1);
      assert.ok(error instanceof errorType);
      assert.ok(!(error instanceof InitOutcomeUnknownError));
      assert.equal(error.code, code);
      assert.equal((error.data as Record<string, unknown>).durable_effects, durableEffects);
    });
  }

  it("throws CompositionProvenanceError with its kind and fields for a failed settlement", async () => {
    const { bin } = gateway(`
      accepted();
      progress("storage");
      settled({ outcome: "failed", code: -32019,
        message: "Runtime bootstrap failed: the candidate definition differs",
        durable_effects: "none",
        data: { kind: "candidate_divergent", fields: ["profiles.lead.tools.deny"] } });
      await sleep(20);
      process.exit(1);
    `);
    const rt = runtime(bin);
    const error = await startupErrorBehindRejectedStop(rt, 1);
    assert.ok(error instanceof CompositionProvenanceError);
    assert.equal(error.code, COMPOSITION_PROVENANCE_CODE);
    assert.equal(error.kind, "candidate_divergent");
    assert.deepEqual(error.fields, ["profiles.lead.tools.deny"]);
    assert.equal((error.data as Record<string, unknown>).durable_effects, "none");
  });

  it("throws CompositionProvenanceError for a refusal answered as the init's error response", async () => {
    const { bin } = gateway(`
      emit({ jsonrpc: "2.0", id: init.id, error: { code: -32019,
        message: "Runtime bootstrap failed: the composition diverged",
        data: { kind: "divergent", fields: ["profiles.default.model"],
                manifest: "/state/mob.sqlite3.composition.json" } } });
      await sleep(20);
      process.exit(1);
    `);
    const rt = runtime(bin);
    await assert.rejects(rt.connect(), (error: any) => {
      assert.ok(error instanceof CompositionProvenanceError);
      assert.equal(error.kind, "divergent");
      assert.deepEqual(error.fields, ["profiles.default.model"]);
      assert.equal((error.data as Record<string, unknown>).manifest, "/state/mob.sqlite3.composition.json");
      return true;
    });
  });

  it("reports a lost acceptance as outcome unknown", async () => {
    const { bin, record } = gateway(`await new Promise(() => {});`);
    const rt = runtime(bin, { timeoutMs: 500 });
    await assert.rejects(rt.connect(), (error: any) => {
      assert.ok(error instanceof InitOutcomeUnknownError);
      assert.equal(error.initId, records(record)[0].init.params.init_id);
      assert.equal(error.lastPhase, null);
      assert.match(error.reason, /no answer/);
      return true;
    });
  });

  it("reports a gateway exit after acceptance as outcome unknown, not a refusal", async () => {
    const { bin } = gateway(`
      accepted();
      progress("prewarm");
      await sleep(50);
      process.exit(3);
    `);
    const rt = runtime(bin);
    const error = await startupErrorBehindRejectedStop(rt, 3);
    assert.ok(error instanceof InitOutcomeUnknownError);
    assert.ok(!(error instanceof RpcError));
    assert.equal(error.lastPhase, "prewarm");
    assert.match(error.reason, /closed its stdout/);
  });

  it("reports an init deadline as outcome unknown and asks the gateway to shut down", async () => {
    const { bin, record } = gateway(`
      accepted();
      progress("prewarm");
      await serveShutdown();
    `);
    const rt = runtime(bin, { initDeadlineMs: 500 });
    await assert.rejects(rt.connect(), (error: any) => {
      assert.ok(error instanceof InitOutcomeUnknownError);
      assert.equal(error.lastPhase, "prewarm");
      assert.match(error.reason, /deadline/);
      return true;
    });
    const methods = records(record).filter((entry) => entry.after_init).map((entry) => entry.after_init.method);
    assert.deepEqual(methods, ["meta/shutdown"]);
  });

  it("ignores a settlement for another init", async () => {
    const { bin } = gateway(`
      accepted();
      settled({ outcome: "failed", code: -32603, message: "not this init", durable_effects: "possible" }, "init-someone-else");
      progress("restore", "init-someone-else");
      settled({ outcome: "ready", http_base_url: "http://127.0.0.1:7" });
      await serveShutdown();
    `);
    const rt = runtime(bin);
    try {
      await rt.connect();
      assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:7");
    } finally {
      await rt.shutdown();
    }
  });

  it("reports an acceptance for a different init id as outcome unknown", async () => {
    const { bin } = gateway(`
      emit({ jsonrpc: "2.0", id: init.id, result: { init_state: "accepted", init_id: "init-not-yours" } });
      await new Promise(() => {});
    `);
    const rt = runtime(bin);
    await assert.rejects(rt.connect(), (error: any) => {
      assert.ok(error instanceof InitOutcomeUnknownError);
      assert.match(error.reason, /different init_id/);
      return true;
    });
  });

  it("still connects to an older gateway that answers once", async () => {
    const { bin } = gateway(`
      emit({ jsonrpc: "2.0", id: init.id, result: { http_base_url: "http://127.0.0.1:5" } });
      await new Promise(() => {});
    `);
    const rt = runtime(bin);
    try {
      await rt.connect();
      assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:5");
    } finally {
      await rt.shutdown();
    }
  });

  it("settles ready after a prewarm longer than the callback bound: the SDK arms no timer", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-init-release-"));
    dirs.push(dir);
    const release = join(dir, "release");
    const { bin } = gateway(`
      accepted();
      progress("prewarm");
      while (!fs.existsSync(${JSON.stringify(release)})) await sleep(20);
      settled({ outcome: "ready", http_base_url: "http://127.0.0.1:3" });
      await serveShutdown();
    `);
    const rt = runtime(bin);
    mock.timers.enable({ apis: ["setTimeout"] });
    let connected = false;
    const connect = rt.connect().then(() => {
      connected = true;
    });
    try {
      const yieldOnce = () => new Promise<void>((resolve) => setImmediate(resolve));
      while ((rt as any)._transport?._initWatch?.lastPhase !== "prewarm") await yieldOnce();
      // Far past the 130 s callback bound: any SDK timer would fire now.
      mock.timers.tick(600_000);
      for (let i = 0; i < 20; i++) await yieldOnce();
      assert.equal(connected, false, "connect must still be waiting for the settlement");
      mock.timers.reset();
      writeFileSync(release, "go");
      await connect;
      assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:3");
    } finally {
      mock.timers.reset();
      await rt.shutdown();
    }
  });

  it("validates initDeadline", () => {
    const builder = Meta.builder();
    for (const bad of [0, -1, Number.POSITIVE_INFINITY, Number.NaN]) {
      assert.throws(() => builder.initDeadline(bad), TypeError);
    }
    assert.equal((builder.initDeadline(null) as any)._config.initDeadlineMs, null);
    assert.equal((builder.initDeadline(2000) as any)._config.initDeadlineMs, 2000);
  });
});

describe("init in progress refusal", () => {
  it("maps -32018 to the typed InitInProgressError", async () => {
    const { bin } = gateway(`
      emit({ jsonrpc: "2.0", id: init.id, result: { http_base_url: "http://127.0.0.1:5" } });
      const request = await next();
      emit({ jsonrpc: "2.0", id: request.id, error: { code: -32018, message: "meta/status refused: meta/init has not settled yet",
        data: { kind: "init_in_progress", method: "meta/status" } } });
      await new Promise(() => {});
    `);
    const rt = runtime(bin);
    try {
      await rt.connect();
      await assert.rejects((rt as any)._rpcUnchecked("meta/status", {}), (error: any) => {
        assert.ok(error instanceof InitInProgressError);
        assert.equal(error.code, INIT_IN_PROGRESS_CODE);
        assert.equal((error.data as Record<string, unknown>).kind, "init_in_progress");
        return true;
      });
    } finally {
      await rt.shutdown();
    }
  });
});

describe("CompositionProvenanceError", () => {
  it("is a typed RpcError with the pinned code that passes the structural check", () => {
    const err = new CompositionProvenanceError("refused", "init:1", "meta/init", {
      kind: "divergent",
      fields: ["profiles.default.model", 7, "profiles.worker"],
    });
    assert.ok(err instanceof RpcError);
    assert.equal(err.name, "CompositionProvenanceError");
    assert.equal(COMPOSITION_PROVENANCE_CODE, -32019);
    assert.equal(err.code, COMPOSITION_PROVENANCE_CODE);
    assert.deepEqual(err.fields, ["profiles.default.model", "profiles.worker"]);
    assert.equal(isRpcError({ name: "CompositionProvenanceError", code: -32019 }), true);
  });

  it("reifies without refusal data", () => {
    const err = new CompositionProvenanceError("refused");
    assert.equal(err.kind, null);
    assert.deepEqual(err.fields, []);
  });
});
