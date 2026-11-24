/**
 * A real callback before the public build() returns; not a proof of the
 * window before the native gateway publishes ready. Opt in with
 * META_RPC_GATEWAY_BIN. Public transport composition gates only the return
 * of the actual meta/init superclass await; it generates no fake callback.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { setTimeout as delay } from "node:timers/promises";

const gateway = process.env.META_RPC_GATEWAY_BIN;
const seedHost = fileURLToPath(new URL("./fixtures/schedule-fire-restart-host.ts", import.meta.url));
const gatedHost = fileURLToPath(new URL("./fixtures/schedule-fire-gated-host.ts", import.meta.url));
const run = promisify(execFile);

describe("real stored schedule callback before the public build returns", { concurrency: false }, () => {
  let previousCaseSucceeded = true;
  for (const mode of ["runtime-only", "builder"]) {
    it(mode === "builder"
      ? "completes with builder registration while the SDK init return is gated"
      : "records delivery_failed before runtime-only registration can occur",
    { skip: !gateway && "META_RPC_GATEWAY_BIN is not set", timeout: 180_000 }, async (t) => {
      if (!previousCaseSucceeded) {
        t.skip("previous native case did not succeed; inspect that result before another gateway invocation");
        return;
      }
      // Fail closed for setup errors and node:test timeout/cancellation too.
      // Only completed assertions and cleanup may permit another invocation.
      previousCaseSucceeded = false;
      const evidenceRoot = process.env.META_SCHEDULE_TEST_EVIDENCE_DIR;
      if (evidenceRoot) mkdirSync(evidenceRoot, { recursive: true });
      const directory = mkdtempSync(join(evidenceRoot ?? tmpdir(), "meta-schedule-gated-"));
      writeFileSync(join(directory, "mob.toml"), `[mob]
id = "schedule-restart"
[profiles.reader]
model = "claude-haiku-5-5"
runtime_mode = "turn_driven"
external_addressable = true
`);
      const runHost = async (host: string, leg: string) => {
        try {
          const result = await run(process.execPath, ["--import", "tsx", host, leg, directory], { timeout: 110_000 });
          writeFileSync(join(directory, `${leg}.log`), result.stdout + result.stderr);
          return JSON.parse(readFileSync(join(directory, `${leg}.json`), "utf8"));
        } catch (error) {
          const failure = error as { stdout?: string; stderr?: string };
          writeFileSync(join(directory, `${leg}.log`), (failure.stdout ?? "") + (failure.stderr ?? ""));
          throw error;
        }
      };
      try {
        const seed = await runHost(seedHost, "seed");
        assert.equal(seed.first.phase, "completed");
        assert.equal(seed.second.phase, "pending");
        const stoppedAt = Date.now();
        const dueAt = Date.parse(seed.second.occurrence.due_at_utc);
        assert.ok(dueAt > stoppedAt && dueAt - stoppedAt <= 60_000);
        while (Date.now() < dueAt + 200) await delay(Math.min(1000, dueAt + 200 - Date.now()));
        const restartedAt = Date.now();
        const observed = await runHost(gatedHost, mode);
        t.diagnostic(JSON.stringify({ stoppedAt, dueAt, restartedAt, seedPid: seed.pid, observed }));
        assert.notEqual(observed.pid, seed.pid);
        assert.equal(observed.whileGated.buildReturned, false);
        assert.equal(observed.whileGated.initResponseReceived, true);
        assert.equal(observed.buildReturned, true);
        assert.equal(observed.callbacks.length, 1);
        const callback = observed.callbacks[0];
        assert.equal(callback.beforeBuildReturned, true);
        assert.equal(callback.params.occurrence.occurrence_id, seed.second.occurrence.occurrence_id);
        const stored = observed.afterShutdown;
        assert.equal(stored.occurrence.occurrence_id, seed.second.occurrence.occurrence_id);
        assert.equal(stored.occurrence.schedule_id, seed.second.occurrence.schedule_id);
        assert.equal(stored.phase, mode === "builder" ? "completed" : "delivery_failed");
        assert.equal(stored.occurrence.attempt_count, 1);
        assert.equal(stored.action_at_ms, null);
        const terminalReceiptId = stored.occurrence.last_receipt?.receipt_id;
        assert.equal(typeof terminalReceiptId, "string", "terminal occurrence records its receipt ID");
        const matches = stored.receipts.filter((item: { receipt_id: string }) => item.receipt_id === terminalReceiptId);
        assert.equal(matches.length, 1, "the occurrence's terminal receipt must exist exactly once");
        const receipt = matches[0];
        assert.equal(receipt.stage, stored.phase);
        assert.equal(stored.schedule.phase, "active");
        assert.ok(stored.future_pending > 0);
        assert.equal(observed.handled.length, mode === "builder" ? 1 : 0);
        if (mode === "builder") {
          assert.equal(callback.result, null);
          assert.equal(callback.error, undefined);
        } else {
          assert.equal(callback.error, "no schedule-fire handler registered for runnable: stored.collect");
          assert.equal(stored.occurrence.failure_class, "runtime_rejected");
          assert.equal(receipt.failure_class, "runtime_rejected");
          assert.match(receipt.detail, /no schedule-fire handler registered for runnable: stored.collect/);
        }
      } finally {
        if (!evidenceRoot) rmSync(directory, { recursive: true, force: true });
      }
      if (!t.signal.aborted) previousCaseSucceeded = true;
    });
  }
});
