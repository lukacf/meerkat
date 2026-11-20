/** Opt in with META_RPC_GATEWAY_BIN; uses two fresh SDK host processes. */
import { it } from "node:test";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { setTimeout as delay } from "node:timers/promises";

const gateway = process.env.META_RPC_GATEWAY_BIN;
const host = fileURLToPath(new URL("./fixtures/schedule-fire-restart-host.ts", import.meta.url));
const run = promisify(execFile);

it("handles an existing schedule due while down after a fresh-process build with registered handlers",
{ skip: !gateway && "META_RPC_GATEWAY_BIN is not set", timeout: 180_000 }, async (t) => {
  const evidenceRoot = process.env.META_SCHEDULE_TEST_EVIDENCE_DIR;
  if (evidenceRoot) mkdirSync(evidenceRoot, { recursive: true });
  const directory = mkdtempSync(join(evidenceRoot ?? tmpdir(), "meta-schedule-restart-"));
  writeFileSync(join(directory, "mob.toml"), `[mob]
id = "schedule-restart"
[profiles.reader]
model = "claude-haiku-5-5"
runtime_mode = "turn_driven"
external_addressable = true
`);
  const runHost = async (mode: string) => {
    const result = await run(process.execPath, ["--import", "tsx", host, mode, directory], { timeout: 100_000 });
    writeFileSync(join(directory, `${mode}.log`), result.stdout + result.stderr);
    return JSON.parse(readFileSync(join(directory, `${mode}.json`), "utf8"));
  };
  try {
    const seed = await runHost("seed");
    assert.equal(seed.first.phase, "completed");
    assert.equal(seed.second.phase, "pending");
    const dueAt = Date.parse(seed.second.occurrence.due_at_utc);
    const stoppedAt = Date.now();
    assert.ok(dueAt > stoppedAt, "gateway must stop before the next stored occurrence is due");
    assert.ok(dueAt - stoppedAt <= 60_000);
    while (Date.now() < dueAt + 200) await delay(Math.min(1000, dueAt + 200 - Date.now()));
    const restartedAt = Date.now();
    const resumed = await runHost("resume");
    t.diagnostic(JSON.stringify({ stoppedAt, dueAt, restartedAt, seed, resumed }));
    assert.notEqual(seed.pid, resumed.pid, "restart must use a fresh SDK host process");
    assert.equal(resumed.second.occurrence.occurrence_id, seed.second.occurrence.occurrence_id);
    assert.equal(resumed.second.occurrence.schedule_id, seed.second.occurrence.schedule_id);
    assert.equal(resumed.second.phase, "completed");
    assert.equal(resumed.second.occurrence.attempt_count, 1);
    assert.equal(resumed.second.action_at_ms, null);
    assert.equal(resumed.second.receipts.at(-1).stage, "completed");
    assert.deepEqual(resumed.callbacks.map((item: Record<string, unknown>) => item.occurrence_id),
      [seed.second.occurrence.occurrence_id]);
    assert.equal(resumed.second.schedule.phase, "active");
    assert.ok(resumed.second.future_pending > 0);
  } finally {
    if (!evidenceRoot) rmSync(directory, { recursive: true, force: true });
  }
});
