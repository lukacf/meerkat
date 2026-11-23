/** A fresh SDK host process for each leg of the native stored-schedule test. */
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { Meta } from "../../src/index.js";

const [mode, directory] = process.argv.slice(2);
assert.ok(mode === "seed" || mode === "resume");
const runnable = "stored.collect";
const reader = fileURLToPath(new URL("./read-schedule-outcome.py", import.meta.url));
const database = join(directory, "state", "schedule.sqlite");
const read = (ordinal: number) => JSON.parse(execFileSync("python3",
  [reader, database, runnable, String(ordinal)], { encoding: "utf8" }));
const callbacks: Record<string, unknown>[] = [];
const runtime = await Meta.builder()
  .gateway(process.env.META_RPC_GATEWAY_BIN!)
  .mob(join(directory, "mob.toml"))
  .persistentState(join(directory, "state"))
  .demoLlm()
  .rosterProvider({ async roster() {
    return [{ identity: "reader", profile: "reader", addressability: "addressable",
      displayName: "Reader", labels: {}, context: null, additionalInstructions: [] }];
  } })
  .mobAnnouncements()
  .hostRunnables([runnable])
  .onScheduleFire(runnable, (occurrence) => { callbacks.push(occurrence); })
  .build();
try {
  if (mode === "seed") {
    await runtime.mobHandle().announcementSourcePut("declare-source", "local-feed", {
      provider: "local.example",
      principal: { kind: "host_attested", attester: "example", account: "local-feed" },
      scope: { kind: "feed", id: "local-feed" },
    }, { baseline: "delta", deletions: "explicit" }, ["local.updates"],
    [{ kind: "role", mob_id: "schedule-restart", role: "reader" }], {
      connection: { kind: "host", id: "local-feed" },
      runner: { host_runnable: runnable, every_seconds: 60 },
    });
  }
  const ordinal = mode === "seed" ? 0 : 1;
  const deadline = Date.now() + 90_000;
  let stored = read(ordinal);
  while ((!stored || stored.phase === "pending" || stored.action_at_ms !== null) && Date.now() < deadline) {
    await delay(100);
    stored = read(ordinal);
  }
  assert.equal(stored?.phase, "completed", "the intended stored occurrence must complete");
} finally {
  await runtime.shutdown();
}
// Both snapshots are read only after the gateway has fully shut down.
writeFileSync(join(directory, `${mode}.json`), JSON.stringify({
  pid: process.pid, callbacks, first: read(0), second: read(1),
}, null, 2) + "\n");
