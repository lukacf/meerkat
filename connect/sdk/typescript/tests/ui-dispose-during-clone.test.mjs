import test from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { mountToolkitView } from "@rkat/toolkit/ui";

// Public DTO getters can synchronously call the host. Teardown before adoption
// must stay final; this is not an auth boundary or a DOM monkeypatch guarantee.
test("disposal during snapshot cloning prevents the in-progress update from adopting it", t => {
  const dom = new JSDOM("<!doctype html><main></main>");
  const container = dom.window.document.querySelector("main"), calls = [];
  const view = mountToolkitView(container, { scope: "viewer", dispatch: async request => { calls.push(request); } });
  t.after(() => { view.dispose(); dom.window.close(); });
  const first = { scope: "viewer", revision: 1, observedAt: "now", announcement: "First",
    tasks: [], connections: [], frontends: [], packages: [] };
  assert.equal(view.update(first), true);
  let reads = 0;
  const next = { ...first, revision: 2, announcement: "Must not be adopted" };
  Object.defineProperty(next, "tasks", { enumerable: true, get() {
    reads++;
    view.dispose();
    return [];
  } });
  let failure;
  try { view.update(next); } catch (error) { failure = error; }
  assert.equal(reads, 1);
  assert.equal(container.childElementCount, 0);
  assert.deepEqual(calls, []);
  assert.ok(failure instanceof Error, "an update disposed before adoption must report disposal, not success");
  assert.match(failure.message, /disposed/);
  assert.throws(() => view.update({ ...first, revision: 3 }), /disposed/);
});
