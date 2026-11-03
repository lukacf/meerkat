import test from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { mountToolkitView } from "@rkat/toolkit/ui";

// Independent public DTO fixture. No private renderer imports or native effects.
const ownerControl = (offer = "offer-1", reference = "ref-1") => ({
  offer, kind: "continue", target: { owner: "owner-a", reference, revision: "owner-r1" },
});
function snapshot(revision = 1) {
  return {
    scope: "viewer-a", revision, observedAt: "observed", announcement: `Announcement ${revision}`,
    tasks: [{ id: "task-a", title: `Task ${revision}`, application: "App", requester: "Person A",
      reason: "Requested setup", phase: { tone: "pending", text: "Host says pending" },
      nextAction: "Choose a host offer", roles: [], userActions: [],
      controls: [ownerControl()], diagnostics: "Owner diagnostic" }],
    connections: [{ id: "connection-a", packageId: "package-a", service: "Service A", account: "Account A",
      credentialScope: "Read only", status: { tone: "ready", text: "Connected" }, grants: [],
      pendingTaskIds: ["task-a"], controls: [ownerControl()] }],
    frontends: [], packages: [{ id: "package-a", label: "Package A", version: "v1",
      status: { tone: "ready", text: "Installed" }, exports: [], components: [] }],
  };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
const buttons = (u, label) => [...u.container.querySelectorAll("button")].filter(n => n.textContent === label);
const description = (u, b) => u.doc.getElementById(b.getAttribute("aria-describedby")).textContent;
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function mount(t, dispatch = async () => {}, first = snapshot()) {
  const dom = new JSDOM("<!doctype html><main></main>", { url: "https://app.example.test/" });
  const doc = dom.window.document, container = doc.querySelector("main");
  const view = mountToolkitView(container, { scope: "viewer-a", dispatch });
  assert.equal(view.update(first), true);
  t.after(() => { view.dispose(); dom.window.close(); });
  return { dom, doc, container, view };
}
function refuseReplacement(u, next) {
  const content = u.container.querySelector(".toolkit-view").lastElementChild;
  const replace = content.replaceChildren;
  let detached;
  content.replaceChildren = function(...nodes) {
    detached = nodes[0];
    throw new Error("synthetic DOM replacement refusal");
  };
  try { assert.throws(() => u.view.update(next), /replacement refusal/); }
  finally { content.replaceChildren = replace; }
  return detached;
}

test("cloned scope and displayed payload are checked from the same snapshot", t => {
  const u = mount(t), next = snapshot(2);
  let reads = 0, selectedScope;
  Object.defineProperty(next, "scope", { enumerable: true, get() {
    selectedScope = ++reads === 1 ? "viewer-a" : "viewer-b";
    return selectedScope;
  } });
  const rows = next.tasks;
  Object.defineProperty(next, "tasks", { enumerable: true, get() {
    return [{ ...rows[0], title: `Selected ${selectedScope}` }];
  } });
  u.view.update(next);
  assert.ok(!u.container.textContent.includes("Selected viewer-b"), "a differently scoped clone must not be displayed");
  assert.equal(reads, 1, "one detached snapshot determines both validation and display");
});

test("a changing revision cannot regress the accepted snapshot or captured dispatch revision", t => {
  const calls = [], u = mount(t, async request => { calls.push(request); });
  const next = snapshot(2);
  let reads = 0;
  Object.defineProperty(next, "revision", { enumerable: true, get() { return ++reads <= 3 ? 2 : 0; } });
  assert.equal(u.view.update(next), true);
  buttons(u, "Continue")[0].click();
  assert.equal(calls[0].viewRevision, 2, "the accepted detached revision must match the one validated");
  assert.equal(u.view.update(snapshot(1)), false, "an older revision stays older");
  assert.equal(reads, 1);
});

test("a button from a refused detached render never becomes active after same-revision recovery", async t => {
  const calls = [], u = mount(t, async request => { calls.push(request); });
  const bad = snapshot(2);
  bad.tasks[0].controls = [ownerControl("refused-offer", "refused-target")];
  bad.connections[0].controls = [];
  const detached = refuseReplacement(u, bad);
  const stale = [...detached.querySelectorAll("button")].find(n => n.textContent === "Continue");
  stale.click(); assert.deepEqual(calls, [], "the refused generation has never been adopted");
  const good = snapshot(2);
  good.tasks[0].controls = [ownerControl("accepted-offer", "accepted-target")];
  good.connections[0].controls = [];
  assert.equal(u.view.update(good), true);
  stale.click(); await tick();
  assert.deepEqual(calls, [], "a later accepted render must not arm rejected controls");
  buttons(u, "Continue")[0].click(); await tick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].control.target.reference, "accepted-target");
});

test("DOM replacement refusal preserves pending copies, focus and navigation, without consuming a revision", async t => {
  const pending = deferred(), calls = [];
  const u = mount(t, request => { calls.push(request); return pending.promise; });
  const nav = buttons(u, "View package")[0]; nav.focus();
  buttons(u, "Continue")[0].click();
  const html = u.container.innerHTML;
  refuseReplacement(u, snapshot(2));
  assert.equal(u.container.innerHTML, html);
  assert.equal(u.doc.activeElement, nav);
  pending.reject(new Error("sensitive receipt failure")); await tick();
  assert.ok(buttons(u, "Continue").every(b => b.disabled && /Could not confirm/.test(description(u, b))));
  assert.ok(!u.container.textContent.includes("sensitive receipt failure"));
  nav.click(); assert.equal(u.doc.activeElement.textContent, "Package A v1");
  assert.equal(u.view.update(snapshot(2)), true);
  for (const b of buttons(u, "Continue")) b.click();
  await tick(); assert.equal(calls.length, 1);
  assert.ok(u.container.textContent.includes("Host says pending"), "receipt rejection does not compute business state");
});

test("focus-driven nested projection does not leave an older live announcement over the newest view", t => {
  const u = mount(t);
  buttons(u, "Continue")[0].focus();
  let once = true;
  u.container.addEventListener("focusin", () => {
    if (once) { once = false; assert.equal(u.view.update(snapshot(3)), true); }
  });
  assert.equal(u.view.update(snapshot(2)), true);
  assert.equal(u.container.querySelector("article h3").textContent, "Task 3");
  assert.equal(u.container.querySelector('[role="status"]').textContent, "Announcement 3");
});

test("clone failure preserves the accepted view and allows same-revision recovery", t => {
  const u = mount(t), html = u.container.innerHTML, next = snapshot(2);
  next.extraUncloneable = () => {};
  assert.throws(() => u.view.update(next), { name: "DataCloneError" });
  assert.equal(u.container.innerHTML, html);
  assert.equal(u.view.update(snapshot(2)), true);
});

test("duplicate offer payloads may cross cards while dispatch captures exact cloned values", async t => {
  const calls = [], first = snapshot();
  const exact = ownerControl('offer:["a",":b"]', 'resource:["x"]');
  first.tasks[0].controls = [structuredClone(exact)];
  first.connections[0].controls = [structuredClone(exact)];
  const u = mount(t, async request => { calls.push(request); }, first);
  first.tasks[0].controls[0].target.reference = "mutated-input";
  buttons(u, "Continue")[1].click(); buttons(u, "Continue")[0].click(); await tick();
  assert.equal(calls.length, 1); assert.deepEqual(calls[0].control, exact);
  assert.equal(calls[0].scope, "viewer-a"); assert.equal(calls[0].viewRevision, 1);
  assert.ok(buttons(u, "Continue").every(b => b.disabled));
});

for (const [name, change] of [
  ["unknown user action state", next => { next.tasks[0].userActions = [{ id: "a", integration: "I", kind: "secret_entry", state: "maybe", instructions: "inert", controls: [] }]; }],
  ["unknown user action kind", next => { next.tasks[0].userActions = [{ id: "a", integration: "I", kind: "magic", state: "queued", instructions: "inert", controls: [] }]; }],
  ["unknown export kind", next => { next.packages[0].exports = [{ id: "x", kind: "magic", label: "X", status: { tone: "ready", text: "Ready" } }]; }],
  ["duplicate connection identity", next => { next.connections.push(structuredClone(next.connections[0])); }],
  ["duplicate package identity", next => { next.packages.push(structuredClone(next.packages[0])); }],
  ["duplicate navigation target in one source row", next => { next.connections[0].pendingTaskIds.push("task-a"); }],
  ["duplicate control in one card", next => { next.tasks[0].controls.push(structuredClone(next.tasks[0].controls[0])); }],
  ["conflicting same-offer kind", next => { next.connections[0].controls[0].kind = "disconnect"; }],
]) {
  test(`${name} refuses the whole projection and does not dispatch`, t => {
    const calls = [], u = mount(t, async request => { calls.push(request); }), html = u.container.innerHTML;
    const next = snapshot(2); change(next);
    assert.throws(() => u.view.update(next), TypeError);
    assert.equal(u.container.innerHTML, html); assert.deepEqual(calls, []);
    assert.equal(u.view.update(snapshot(2)), true);
  });
}

test("dispatch acknowledgement and a failed later projection cannot erase an absent offer's custody", async t => {
  const pending = deferred(), calls = [];
  const u = mount(t, request => { calls.push(request); return pending.promise; });
  buttons(u, "Continue")[0].click();
  const absent = snapshot(2); absent.tasks[0].controls = []; absent.connections[0].controls = [];
  assert.equal(u.view.update(absent), true);
  const bad = snapshot(3); bad.packages[0].components = null;
  assert.throws(() => u.view.update(bad), TypeError);
  pending.resolve(); await tick();
  assert.equal(u.view.update(snapshot(3)), true);
  assert.ok(buttons(u, "Continue").every(b => b.disabled && /Waiting for owner status/.test(description(u, b))));
  for (const b of buttons(u, "Continue")) b.click();
  await tick(); assert.equal(calls.length, 1);
});
