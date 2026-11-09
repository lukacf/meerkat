import test from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { mountToolkitView } from "@rkat/toolkit/ui";
import { fixture } from "../examples/management-fixture.mjs";

const buttons = (ui, label) => [...ui.container.querySelectorAll("button")].filter(button => button.textContent === label);
function mount(t, dispatch = async () => {}) {
  const dom = new JSDOM("<!doctype html><main></main>");
  const doc = dom.window.document, container = doc.querySelector("main");
  const view = mountToolkitView(container, { scope: "fixture-viewer", dispatch });
  view.update(fixture(1));
  t.after(() => { view.dispose(); dom.window.close(); });
  return { doc, container, view };
}

// Complements the unchanged independent counterexamples with related public
// recovery controls. This is presentation evidence, not host authorization.
test("retained navigation from a rejected render stays inert after correction", t => {
  const ui = mount(t), content = ui.container.querySelector(".toolkit-view").lastElementChild;
  const replace = content.replaceChildren;
  let rejected;
  content.replaceChildren = function(node) { rejected = node; throw new Error("replacement refused"); };
  try { assert.throws(() => ui.view.update(fixture(2)), /replacement refused/); }
  finally { content.replaceChildren = replace; }
  assert.equal(ui.view.update(fixture(2)), true);
  const accepted = buttons(ui, "Cancel whole task")[0]; accepted.focus();
  const stale = [...rejected.querySelectorAll("button")].find(button => button.textContent === "View package");
  assert.ok(stale);
  // Even making the retained node reachable does not make its callbacks current.
  ui.container.append(rejected);
  stale.click();
  assert.equal(ui.doc.activeElement, accepted);
});

test("focus-triggered dispatch retains its current pending announcement", t => {
  const calls = [], ui = mount(t, request => { calls.push(request); return new Promise(() => {}); });
  buttons(ui, "Cancel whole task")[0].focus();
  let once = true;
  ui.container.addEventListener("focusin", () => {
    if (once) { once = false; buttons(ui, "Cancel whole task")[0].click(); }
  });
  assert.equal(ui.view.update(fixture(2)), true);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].viewRevision, 2);
  assert.match(ui.container.querySelector('[role="status"]').textContent, /Waiting for owner status/);
});

test("a newer snapshot accepted during input cloning cannot be overwritten by the outer update", t => {
  const ui = mount(t), next = fixture(2), newest = fixture(3);
  newest.announcement = "Newest observed owner status";
  const tasks = next.tasks;
  Object.defineProperty(next, "tasks", { enumerable: true, get() {
    assert.equal(ui.view.update(newest), true);
    return tasks;
  } });
  assert.equal(ui.view.update(next), false);
  assert.equal(ui.view.update(fixture(2)), false);
  assert.equal(ui.container.querySelector('[role="status"]').textContent, newest.announcement);
});

test("focus-triggered disposal remains disposed without dispatch or a replacement view", t => {
  const calls = [], ui = mount(t, async request => { calls.push(request); });
  buttons(ui, "Cancel whole task")[0].focus();
  ui.container.addEventListener("focusin", () => ui.view.dispose(), { once: true });
  assert.equal(ui.view.update(fixture(2)), true);
  assert.equal(ui.container.childElementCount, 0);
  assert.throws(() => ui.view.update(fixture(3)), /disposed/);
  assert.deepEqual(calls, []);
});

test("disposal during cloning retains teardown when a prior dispatch later rejects", async t => {
  let rejectDispatch;
  const pending = new Promise((resolve, reject) => { rejectDispatch = reject; });
  const calls = [], ui = mount(t, request => { calls.push(request); return pending; });
  const button = buttons(ui, "Cancel whole task")[0]; button.click();
  const description = ui.doc.getElementById(button.getAttribute("aria-describedby"));
  const previous = description.textContent;
  const next = fixture(2), tasks = next.tasks;
  Object.defineProperty(next, "tasks", { enumerable: true, get() { ui.view.dispose(); return tasks; } });
  assert.throws(() => ui.view.update(next), { name: "Error", message: "Toolkit view has been disposed" });
  assert.equal(ui.container.childElementCount, 0);
  rejectDispatch(new Error("private receipt failure"));
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(description.textContent, previous);
  assert.equal(ui.container.childElementCount, 0);
  button.click();
  assert.equal(calls.length, 1);
  assert.throws(() => ui.view.update(fixture(3)), /disposed/);
});
