/**
 * Observe a real callback before the public build() returns; not a proof of
 * the window before the native gateway publishes ready. The test gate delays
 * only the SDK's return from the real superclass meta/init sendAsync call.
 * The native reader, settlement notifications and callback replies keep running.
 */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { Meta, PersistentTransport, type CallbackHandler, type MetaRuntime } from "../../src/index.js";

const [mode, directory] = process.argv.slice(2);
assert.ok(mode === "runtime-only" || mode === "builder");
const runnable = "stored.collect";
const reader = fileURLToPath(new URL("./read-schedule-outcome.py", import.meta.url));
const database = join(directory, "state", "schedule.sqlite");
const read = () => JSON.parse(execFileSync("python3", [reader, database, runnable, "1"], { encoding: "utf8" }));
let releaseGate!: () => void;
const gate = new Promise<void>((resolve) => { releaseGate = resolve; });
let buildReturned = false;
let buildError: unknown;
let runtime: MetaRuntime | undefined;
const handled: Record<string, unknown>[] = [];

class GatedInitTransport extends PersistentTransport {
  initResponseReceived = false;
  readonly callbacks: Array<{
    params: Record<string, unknown>;
    beforeBuildReturned: boolean;
    result?: unknown;
    error?: string;
  }> = [];

  override async sendAsync(request: Record<string, unknown>, options: { timeoutMs?: number } = {}): Promise<unknown> {
    const response = await super.sendAsync(request, options);
    if (request.method === "meta/init") {
      this.initResponseReceived = true;
      await gate;
    }
    return response;
  }

  override setCallbackHandler(handler: CallbackHandler): void {
    super.setCallbackHandler(async (method, params, context) => {
      if (method !== "callback/schedule_fire") return handler(method, params, context);
      const observed: (typeof this.callbacks)[number] = { params, beforeBuildReturned: !buildReturned };
      this.callbacks.push(observed);
      try {
        const result = await handler(method, params, context);
        observed.result = result;
        return result;
      } catch (error) {
        observed.error = error instanceof Error ? error.message : String(error);
        throw error;
      }
    });
  }
}

const transport = new GatedInitTransport(process.env.META_RPC_GATEWAY_BIN!);
const configured = Meta.builder()
  .gateway(process.env.META_RPC_GATEWAY_BIN!)
  .gatewayTransport(() => transport)
  .mob(join(directory, "mob.toml"))
  .persistentState(join(directory, "state"))
  .demoLlm()
  .rosterProvider({ async roster() {
    return [{ identity: "reader", profile: "reader", addressability: "addressable",
      displayName: "Reader", labels: {}, context: null, additionalInstructions: [] }];
  } })
  .mobAnnouncements()
  .hostRunnables([runnable]);
const handler = (occurrence: Record<string, unknown>) => { handled.push(occurrence); };
if (mode === "builder") configured.onScheduleFire(runnable, handler);
const build = configured.build().then((value) => {
  buildReturned = true;
  runtime = value;
  return value;
});
// Observe an early failure without leaving an unhandled rejected build promise.
void build.catch((error: unknown) => { buildError = error; });
let whileGated: unknown;
try {
  const deadline = Date.now() + 90_000;
  let stored = read();
  while ((!transport.initResponseReceived || transport.callbacks.length === 0 || !stored ||
    stored.phase === "pending" || stored.action_at_ms !== null) && Date.now() < deadline) {
    if (buildError) throw buildError;
    await delay(100);
    stored = read();
  }
  whileGated = { buildReturned, initResponseReceived: transport.initResponseReceived,
    callbacks: transport.callbacks, stored };
  writeFileSync(join(directory, `${mode}-while-gated.json`), JSON.stringify(whileGated, null, 2) + "\n");
  assert.equal(buildReturned, false);
  assert.equal(transport.initResponseReceived, true, "real superclass meta/init response was received");
  assert.equal(transport.callbacks.length, 1, "the gateway must send its real schedule callback while build is gated");
  assert.equal(transport.callbacks[0].beforeBuildReturned, true);
  assert.equal(stored?.phase, mode === "builder" ? "completed" : "delivery_failed");
  releaseGate();
  const connected = await build;
  if (mode === "runtime-only") connected.onScheduleFire(runnable, handler);
} finally {
  releaseGate();
  try { await build; } finally { await runtime?.shutdown(); }
}
writeFileSync(join(directory, `${mode}.json`), JSON.stringify({
  label: "a real callback before the public build() returns; not a proof of the window before the native gateway publishes ready",
  pid: process.pid, mode, whileGated, buildReturned, callbacks: transport.callbacks,
  handled, afterShutdown: read(), termination: transport.lastTermination(),
}, null, 2) + "\n");
