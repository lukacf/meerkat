import assert from "node:assert/strict";
import test from "node:test";

import { createSingleFlight } from "./single-flight";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

test("requests during a run coalesce into one trailing run", async () => {
  const flight = createSingleFlight();
  const runs: ReturnType<typeof deferred>[] = [];
  const task = () => { const d = deferred(); runs.push(d); return d.promise; };
  const first = flight("topology", task);
  const waiters = Array.from({ length: 25 }, () => flight("topology", task));
  assert.equal(runs.length, 1, "nothing starts while a run is outstanding");
  runs[0].resolve();
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(runs.length, 2, "exactly one trailing run");
  let settled = false;
  void Promise.all(waiters).then(() => { settled = true; });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(settled, false, "later callers wait for the trailing run");
  runs[1].resolve();
  await first;
  await Promise.all(waiters);
  assert.equal(runs.length, 2);
});

test("keys are independent and a failure reaches the callers of that run", async () => {
  const flight = createSingleFlight();
  const a = deferred();
  const b = deferred();
  const pa = flight("a", () => a.promise);
  const pb = flight("b", () => b.promise);
  b.resolve();
  await pb;
  a.reject(new Error("boom"));
  await assert.rejects(pa, /boom/);
  // The key is free again afterwards.
  await flight("a", async () => {});
});
