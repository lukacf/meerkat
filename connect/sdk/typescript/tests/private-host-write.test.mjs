// Actual inherited inert child pipes, plus one held-callback Writable control.
// These tests establish local transport lifecycle only, not native settlement.
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { Writable } from "node:stream";
import { setTimeout as delay } from "node:timers/promises";
import { PrivateHostClient, HostTransportError } from "../dist/private-host.js";
const artifact = { path: "/private/selected.json", sha256: "1".repeat(64) };
const selection = { deployment_root: "/private/realm", activation: artifact, policy_manifest: artifact, local_policy: artifact };
const original = { route: "telegram/main", original: "request", event_sha256: "5".repeat(64) };
const authentication = { resource_id: "auth-1", revision: "7", sha256: "2".repeat(64) };
const login = { scope_id: "paired-login", expires_at_ms: 1900000000000 };
async function until(predicate, ms = 1000) {
  const deadline = performance.now() + ms;
  while (!predicate()) { assert.ok(performance.now() < deadline, "inert pipe did not reach the required state"); await delay(5); }
}
async function bounded(promise, ms = 1000) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("bounded local join expired")), ms); })]); }
  finally { clearTimeout(timer); }
}
async function paused(t, timeoutMs, heldCallback = false) {
  const child = spawn(process.execPath, [new URL("./fixtures/private-host-paused-child.mjs", import.meta.url).pathname], {
    stdio: ["pipe", "pipe", "pipe", "pipe", "pipe"], env: { PATH: "/usr/bin:/bin" },
  });
  let stdout = "", stderr = "", held = false, writes = 0;
  child.stdout.setEncoding("utf8"); child.stderr.setEncoding("utf8");
  child.stdout.on("data", value => stdout += value); child.stderr.on("data", value => stderr += value);
  const exited = new Promise((resolve, reject) => { child.once("close", (code, signal) => resolve({ code, signal })); child.once("error", reject); });
  // After bootstrap this owner keeps its _write callback held even on destroy.
  // The SDK must release its own wait; a Node callback is not a join guarantee.
  const commands = heldCallback ? new Writable({
    write(chunk, _encoding, callback) { writes++; if (writes === 1) child.stdio[3].write(chunk, callback); else held = true; },
    destroy(_error, callback) { child.stdio[3].destroy(); callback(null); },
  }) : child.stdio[3];
  const client = new PrivateHostClient(commands, child.stdio[4], { timeoutMs });
  t.after(async () => {
    let closeError;
    try { await bounded(client.close()); } catch (error) { closeError = error; }
    child.stdin.end("drain\n");
    let forced = false;
    const timer = setTimeout(() => { forced = true; child.kill("SIGKILL"); }, 2000);
    let exit;
    try { exit = await exited; } finally { clearTimeout(timer); }
    assert.equal(closeError, undefined, "SDK close must join without the test releasing a held callback");
    assert.equal(forced, false, "forced child cleanup is not successful lifecycle qualification");
    assert.deepEqual(exit, { code: 0, signal: null }); assert.equal(stderr, "");
    assert.equal(commands.destroyed, true); assert.equal(child.stdio[3].destroyed, true); assert.equal(child.stdio[4].destroyed, true);
    const rows = stdout.trim().split("\n").slice(1).map(value => JSON.parse(value));
    assert.deepEqual(Object.keys(rows.at(-1)), ["trailingBytes"]);
    const complete = rows.slice(0, -1);
    assert.equal(new Set(complete.map(v => v.id)).size, complete.length);
    assert.equal(new Set(complete.map(v => v.original)).size, complete.length);
    assert.ok(complete.every(v => v.kind === "review_remote_controller_parent"));
    assert.ok(heldCallback ? complete.length === 0 : complete.length >= 1 && complete.length <= 16);
    t.diagnostic(JSON.stringify({ childPid: child.pid, exit, forcedCleanup: forced, completeCommands: complete.length, trailingBytes: rows.at(-1).trailingBytes }));
  });
  await client.bootstrap(selection);
  await until(() => stdout.startsWith("paused\n"));
  return { client, commands, held: () => held };
}
function begin(client, index, size = 60_000) {
  return client.beginRemoteControllerReview({ ...original, original: `request-${index}` }, "x".repeat(size), authentication, login, { timeoutMs: 3_600_000 });
}
async function rejectedReviews(reviews) {
  for (const review of reviews) {
    const first = await review.outcome.catch(error => error);
    assert.ok(first instanceof HostTransportError); assert.match(first.message, /settlement is unknown/);
    assert.equal(await review.outcome.catch(error => error), first);
  }
}

test("a long human review deadline does not extend actual pipe backpressure", async t => {
  const { client, commands } = await paused(t, 300);
  const reviews = Array.from({ length: 16 }, (_, index) => begin(client, index));
  await until(() => commands.writableLength > 0);
  await bounded(rejectedReviews(reviews));
  assert.throws(() => client.retireControllerReview(reviews[0].reviewId), HostTransportError);
  await bounded(client.close());
});

test("concurrent retire and logout stop the queued writer before the child resumes", async t => {
  const { client, commands } = await paused(t, 2000);
  const reviews = Array.from({ length: 16 }, (_, index) => begin(client, index));
  await until(() => commands.writableLength > 0);
  const retiring = client.retireControllerReview(reviews[0].reviewId);
  const retired = assert.rejects(retiring, HostTransportError);
  await bounded(client.close());
  await bounded(retired); await bounded(rejectedReviews(reviews));
});

for (const mode of ["timeout", "logout"]) {
  test(`${mode} releases the SDK write tail even when destruction never invokes the write callback`, async t => {
    const { client, held } = await paused(t, mode === "timeout" ? 300 : 2000, true);
    const review = begin(client, 0, 20);
    await until(held);
    if (mode === "logout") {
      const retiring = assert.rejects(client.retireControllerReview(review.reviewId), HostTransportError);
      await bounded(client.close()); await bounded(retiring);
    }
    await bounded(rejectedReviews([review]));
    await bounded(client.close());
  });
}
