import { test } from "node:test";
import assert from "node:assert/strict";
import { migrateConsoleWorkbenchTarget } from "../../../packages/console-core/src/targets";
import { selectedPanelRuntime, scopedPanelParams, framesForPanelRuntime, loadPanelRuntimeSelection, savePanelRuntimeSelection } from "./panel-runtime";

test("reload restores the selected runtime without crossing authenticated host scopes", () => {
  const entries = new Map<string, string>();
  const storage = {
    getItem: (key: string) => entries.get(key) ?? null,
    setItem: (key: string, value: string) => { entries.set(key, value); },
  };
  const runtimes = [
    {key: "alpha", label: "Alpha", memory: true, workgraph: true},
    {key: "beta", label: "Beta", memory: true, workgraph: true},
  ];
  savePanelRuntimeSelection(storage, "host-a:principal-a", "beta");
  const restored = loadPanelRuntimeSelection(storage, "host-a:principal-a");
  assert.equal(selectedPanelRuntime(restored, runtimes), "beta");
  assert.equal(selectedPanelRuntime(restored, runtimes.slice(0, 1)), "beta");
  assert.equal(loadPanelRuntimeSelection(storage, "host-a:principal-b"), "");
  assert.equal(loadPanelRuntimeSelection(storage, "host-b:principal-a"), "");
  savePanelRuntimeSelection(storage, null, "alpha");
  assert.equal(entries.size, 1);
  assert.equal(loadPanelRuntimeSelection(storage, null), "");
});

test("unavailable preference storage does not prevent runtime selection", () => {
  const storage = {
    getItem: () => { throw new Error("Storage disabled"); },
    setItem: () => { throw new Error("Quota exceeded"); },
  };
  assert.equal(loadPanelRuntimeSelection(storage, "host"), "");
  assert.doesNotThrow(() => savePanelRuntimeSelection(storage, "host", "beta"));
});

test("runtime scope never falls back from a removed explicit selection", () => {
  const runtimes = [{key: "repo-b", label: "B", memory: true, workgraph: true}];
  assert.equal(selectedPanelRuntime("repo-a", runtimes), "repo-a");
  assert.equal(selectedPanelRuntime("", runtimes), "repo-b");
  assert.equal(selectedPanelRuntime("", []), "");
});

test("scope is attached only to native store inspection calls", () => {
  for (const kind of ["memory", "workgraph"]) {
    const target = migrateConsoleWorkbenchTarget({ id: kind, kind, title: kind });
    assert.ok(target);
    assert.deepEqual(scopedPanelParams(target.kind, "repo-a", {limit: 10}), {limit: 10, runtime_key: "repo-a"});
    assert.deepEqual(scopedPanelParams(target.kind, "repo-a", {runtime_key: "repo-b"}), {runtime_key: "repo-b"});
  }
  assert.deepEqual(scopedPanelParams("meta/identity-chat", "repo-a", {identity: "worker"}), {identity: "worker"});
  assert.equal(scopedPanelParams("meta/memory", ""), undefined);
});

test("shared memory evidence excludes other runtimes and unknown provenance", () => {
  const a = { runtimeKey: "repo-a", id: "a" };
  const b = { runtimeKey: "repo-b", id: "b" };
  const unknown = { id: "unknown" };
  const frames = [a, b, unknown];
  assert.deepEqual(framesForPanelRuntime(frames, "repo-a"), [a]);
  assert.deepEqual(framesForPanelRuntime(frames, "repo-b"), [b]);
  assert.deepEqual(framesForPanelRuntime(frames, "removed"), []);
  assert.equal(framesForPanelRuntime(frames), frames);
});
