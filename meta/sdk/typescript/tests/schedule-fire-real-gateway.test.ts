/**
 * Native schedule failure outcomes. Opt in with META_RPC_GATEWAY_BIN, as
 * in identities-real-gateway.test.ts; Python 3 supplies a read-only SQLite
 * observer. No provider key or model turn is used. These rows create real
 * source-runner schedules and inspect their stored state after shutdown.
 * META_SCHEDULE_TEST_EVIDENCE_DIR optionally retains each test's state.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

import { Meta, PersistentTransport } from "../src/index.js";
import type { CallbackHandler } from "../src/transport.js";

const gateway = process.env.META_RPC_GATEWAY_BIN;
const skip = !gateway && "META_RPC_GATEWAY_BIN is not set";
const readOutcome = fileURLToPath(new URL("./fixtures/read-schedule-outcome.py", import.meta.url));

interface StoredOutcome {
  schedule: { phase: string; target: { target_kind: string; runnable: string } };
  phase: string;
  action_at_ms: number | null;
  occurrence: {
    occurrence_id: string;
    schedule_id: string;
    phase: string;
    failure_class: string;
    failure_detail: string;
    attempt_count: number;
    missing_target_policy: string;
  };
  receipts: Array<{ stage: string; failure_class?: string; detail?: string }>;
  future_pending: number;
}

class RecordingTransport extends PersistentTransport {
  readonly fires: Array<{ params: Record<string, unknown>; error?: string; result?: unknown }> = [];

  override setCallbackHandler(handler: CallbackHandler): void {
    super.setCallbackHandler(async (method, params, context) => {
      if (method !== "callback/schedule_fire") return handler(method, params, context);
      const fire: (typeof this.fires)[number] = { params };
      this.fires.push(fire);
      try {
        const result = await handler(method, params, context);
        fire.result = result;
        return result;
      } catch (error) {
        fire.error = error instanceof Error ? error.message : String(error);
        throw error;
      }
    });
  }
}

function snapshot(database: string, runnable: string): StoredOutcome | null {
  return JSON.parse(execFileSync("python3", [readOutcome, database, runnable], { encoding: "utf8" }));
}

describe("native host runnable missing handlers and targets", () => {
  for (const declared of [true, false]) {
    it(declared
      ? "records a declared runnable with no handler as delivery_failed/runtime_rejected"
      : "records an undeclared runnable as misfired/target_missing without an SDK callback",
    { skip, timeout: 120_000 }, async (t) => {
      const evidenceRoot = process.env.META_SCHEDULE_TEST_EVIDENCE_DIR;
      if (evidenceRoot) mkdirSync(evidenceRoot, { recursive: true });
      const directory = mkdtempSync(join(evidenceRoot ?? tmpdir(), "meta-schedule-outcome-"));
      const mob = join(directory, "mob.toml");
      writeFileSync(mob, `[mob]
id = "schedule-outcomes"
[profiles.reader]
model = "claude-haiku-5-5"
runtime_mode = "turn_driven"
external_addressable = true
`);
      const runnable = declared ? "declared.no-handler" : "undeclared.local-handler";
      const transport = new RecordingTransport(gateway as string);
      const runtime = await Meta.builder()
        .gateway(gateway as string)
        .gatewayTransport(() => transport)
        .mob(mob)
        .persistentState(join(directory, "state"))
        .demoLlm()
        .rosterProvider({
          async roster() {
            return [{ identity: "reader", profile: "reader", addressability: "addressable",
              displayName: "Reader", labels: {}, context: null, additionalInstructions: [] }];
          },
        })
        .mobAnnouncements()
        .hostRunnables(declared ? [runnable] : [])
        .build();
      let handlerCalls = 0;
      // A local handler without a declaration must not make a gateway target.
      if (!declared) runtime.onScheduleFire(runnable, () => { handlerCalls += 1; });
      const database = join(directory, "state", "schedule.sqlite");
      try {
        const handle = runtime.mobHandle();
        await handle.announcementSourcePut("declare-source", "local-feed", {
          provider: "local.example",
          principal: { kind: "host_attested", attester: "example", account: "local-feed" },
          scope: { kind: "feed", id: "local-feed" },
        }, { baseline: "delta", deletions: "explicit" }, ["local.updates"],
        [{ kind: "role", mob_id: "schedule-outcomes", role: "reader" }], {
          connection: { kind: "host", id: "local-feed" },
          runner: { host_runnable: runnable, every_seconds: 60 },
        });
        const source = await handle.announcementSourceGet("local-feed");
        assert.equal(source.source?.health.runnerScheduledHere, true);

        const terminal = new Set(["completed", "skipped", "misfired", "superseded", "delivery_failed"]);
        const deadline = Date.now() + 90_000;
        let observed = snapshot(database, runnable);
        while ((!observed || !terminal.has(observed.phase)) && Date.now() < deadline) {
          await delay(100);
          observed = snapshot(database, runnable);
        }
        assert.ok(observed && terminal.has(observed.phase), "first occurrence must reach a terminal state");
      } finally {
        await runtime.shutdown();
      }

      try {
        const stored = snapshot(database, runnable);
        assert.ok(stored, "the native schedule and occurrence must survive shutdown");
        const evidence = { declared, handlerCalls, callbacks: transport.fires, stored };
        writeFileSync(join(directory, "outcome.json"), JSON.stringify(evidence, null, 2) + "\n");
        t.diagnostic(JSON.stringify(evidence));
        assert.equal(stored.schedule.phase, "active", "one failed fire does not pause the schedule");
        assert.deepEqual(stored.schedule.target.target_kind, "host_runnable");
        assert.equal(stored.schedule.target.runnable, runnable);
        assert.equal(stored.phase, declared ? "delivery_failed" : "misfired");
        assert.equal(stored.occurrence.phase, stored.phase);
        assert.equal(stored.occurrence.failure_class, declared ? "runtime_rejected" : "target_missing");
        assert.equal(stored.occurrence.attempt_count, 1);
        assert.equal(stored.action_at_ms, null, "the terminal occurrence has no next retry action");
        assert.ok(stored.future_pending > 0, "later interval occurrences remain pending");
        assert.equal(stored.occurrence.missing_target_policy, "mark_misfired");
        const last = stored.receipts.at(-1);
        assert.ok(last, "the native store must contain a terminal receipt");
        assert.equal(last.stage, stored.phase);
        assert.equal(last.failure_class, stored.occurrence.failure_class);
        assert.equal(handlerCalls, 0);
        if (declared) {
          assert.equal(transport.fires.length, 1);
          assert.equal(transport.fires[0].error, `no schedule-fire handler registered for runnable: ${runnable}`);
          assert.match(stored.occurrence.failure_detail, /no schedule-fire handler registered/);
          assert.match(last.detail ?? "", /no schedule-fire handler registered/);
        } else {
          assert.equal(transport.fires.length, 0);
          assert.match(stored.occurrence.failure_detail, /no runnable registry|not registered/);
        }
      } finally {
        if (!evidenceRoot) rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
