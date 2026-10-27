import test from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { mountToolkitView } from "@rkat/toolkit/ui";
import { fixture, control } from "../examples/management-fixture.mjs";

// These are host-projection/control-port tests, not authentication or native
// owner conformance. The fixture, DOM and dispatch acknowledgements are synthetic.
function projection(revision = 1) {
  return { ...fixture(revision), announcement: `Owner projection ${revision}.` };
}

function offered(kind, reference, offer, owner = "fixture-owner") {
  const value = control(kind, reference);
  return { ...value, offer, target: { ...value.target, owner } };
}

function accessControl(snapshot, value) {
  snapshot.tasks[0].roles.find(role => role.kind === "service").controls = [structuredClone(value)];
  snapshot.connections[0].grants[0].controls = [structuredClone(value)];
  return snapshot;
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const settledCallbacks = () => new Promise(resolve => setImmediate(resolve));

function mount(t, snapshot, dispatch = async () => {}) {
  const dom = new JSDOM("<!doctype html><main></main>", { url: "https://host.example.test/" });
  const doc = dom.window.document;
  const container = doc.querySelector("main");
  const view = mountToolkitView(container, { scope: snapshot.scope, dispatch });
  view.update(snapshot);
  t.after(() => { view.dispose(); dom.window.close(); });
  return { dom, doc, container, view };
}

function buttons(ui, label) {
  return [...ui.container.querySelectorAll("button")].filter(button => button.textContent === label);
}

function described(ui, button) {
  const ids = (button.getAttribute("aria-describedby") ?? "").split(/\s+/).filter(Boolean);
  assert.ok(ids.length > 0, "the control has an accessible description");
  return ids.map(id => {
    const node = ui.doc.getElementById(id);
    assert.ok(node, "each accessible description resolves in this document");
    return node.textContent;
  }).join(" ");
}

function expectUniformDescriptions(ui, label, expected) {
  const copies = buttons(ui, label);
  assert.equal(copies.length, 2, "fixture exposes the same control in task and management");
  const texts = copies.map(button => described(ui, button));
  assert.equal(texts[0], texts[1], "both occurrences report the same dispatch observation");
  assert.match(texts[0], expected);
  assert.ok(copies.every(button => button.disabled));
}

function taskOnly(source, id, title) {
  return { ...structuredClone(source), id, title, roles: [], userActions: [], controls: [] };
}

test("opaque colon-bearing task IDs cannot collide with a nested service navigation target", t => {
  const snapshot = projection();
  const source = snapshot.tasks[0];
  const namedTask = taskOnly(source, "x:service:y", "Task with a colon-bearing ID");
  const nestedTask = taskOnly(source, "x", "Task with a service role");
  nestedTask.roles = [{
    ...structuredClone(source.roles.find(role => role.kind === "service")),
    id: "y", label: "Unrelated nested service", controls: [],
  }];
  // This order makes the old concatenated role key overwrite the task key.
  snapshot.tasks = [namedTask, nestedTask];
  snapshot.connections[0].pendingTaskIds = [namedTask.id];
  const ui = mount(t, snapshot);
  buttons(ui, "View setup: Task with a colon-bearing ID")[0].click();
  assert.equal(ui.doc.activeElement.tagName, "H3");
  assert.equal(ui.doc.activeElement.textContent, namedTask.title);
});

test("navigation focus identity separates opaque source and destination ID tuples", t => {
  const snapshot = projection();
  const source = snapshot.tasks[0];
  snapshot.tasks = [taskOnly(source, "z", "Task Z"), taskOnly(source, "y:task:z", "Task Y")];
  const connection = snapshot.connections[0];
  snapshot.connections = [
    { ...structuredClone(connection), id: "x:task:y", grants: [], controls: [], pendingTaskIds: ["z"] },
    { ...structuredClone(connection), id: "x", grants: [], controls: [], pendingTaskIds: ["y:task:z"] },
  ];
  snapshot.packages.find(item => item.id === "docs-package").exports[0].instanceIds = snapshot.connections.map(item => item.id);
  const ui = mount(t, snapshot);
  buttons(ui, "View setup: Task Z")[0].focus();
  const next = structuredClone(snapshot); next.revision = 2;
  ui.view.update(next);
  assert.equal(ui.doc.activeElement, buttons(ui, "View setup: Task Z")[0]);
  assert.notEqual(ui.doc.activeElement, buttons(ui, "View setup: Task Y")[0]);
});

test("the same offer in colliding-looking rows preserves its exact focused occurrence", t => {
  const snapshot = projection();
  const source = snapshot.tasks[0];
  const shared = offered("revoke_grant", "shared-access", "one-offer");
  const direct = taskOnly(source, "x:service:y", "Direct task controls");
  direct.controls = [structuredClone(shared)];
  const nested = taskOnly(source, "x", "Nested task controls");
  nested.roles = [{
    ...structuredClone(source.roles.find(role => role.kind === "service")),
    id: "y", label: "Nested access controls", controls: [structuredClone(shared)],
  }];
  snapshot.tasks = [direct, nested]; snapshot.connections = [];
  snapshot.frontends = []; snapshot.packages = [];
  const ui = mount(t, snapshot);
  const occurrences = buttons(ui, "Revoke this access");
  assert.equal(occurrences.length, 2); occurrences[0].focus();
  const next = structuredClone(snapshot); next.revision = 2;
  ui.view.update(next);
  assert.equal(ui.doc.activeElement, buttons(ui, "Revoke this access")[0]);
  assert.equal(ui.doc.activeElement.closest("article").querySelector("h3").textContent, direct.title);
});

test("all duplicate offer occurrences show pending and later unknown across snapshot replacement", async t => {
  const pending = deferred(), calls = [];
  const selection = offered("revoke_grant", "docs-grant", "shared-grant-offer");
  const snapshot = accessControl(projection(), selection);
  const ui = mount(t, snapshot, request => { calls.push(request); return pending.promise; });
  const copies = buttons(ui, "Revoke this access");
  copies[0].click(); copies[1].click(); copies[0].click();
  assert.equal(calls.length, 1);
  expectUniformDescriptions(ui, "Revoke this access", /Waiting for owner status/);
  ui.view.update(accessControl(projection(2), selection));
  expectUniformDescriptions(ui, "Revoke this access", /Waiting for owner status/);
  pending.reject(new Error("private-transport-rejection")); await settledCallbacks();
  expectUniformDescriptions(ui, "Revoke this access", /Could not confirm/);
  assert.ok(!ui.container.textContent.includes("private-transport-rejection"));
  assert.ok(ui.container.textContent.includes("Access ready for this task"));
  assert.equal(calls.length, 1);
});

test("a fresh host offer can be used on an unchanged underlying owner target", async t => {
  const calls = [];
  const first = offered("revoke_grant", "docs-grant", "offer-one");
  const second = { ...structuredClone(first), offer: "offer-two" };
  assert.deepEqual(first.target, second.target);
  const ui = mount(t, accessControl(projection(), first), async request => { calls.push(request); });
  buttons(ui, "Revoke this access")[0].click(); await settledCallbacks();
  ui.view.update(accessControl(projection(2), second));
  assert.ok(buttons(ui, "Revoke this access").every(button => !button.disabled));
  buttons(ui, "Revoke this access")[1].click(); await settledCallbacks();
  assert.deepEqual(calls.map(call => call.control.offer), ["offer-one", "offer-two"]);
  assert.deepEqual(calls.map(call => call.viewRevision), [1, 2]);
  assert.deepEqual(calls[0].control.target, calls[1].control.target);
});

test("late rejection of an old offer cannot corrupt a new offer on the same target", async t => {
  const older = deferred(), newer = deferred(), calls = [];
  const first = offered("revoke_grant", "docs-grant", "old-offer");
  const second = { ...structuredClone(first), offer: "new-offer" };
  const ui = mount(t, accessControl(projection(), first), request => {
    calls.push(request);
    return request.control.offer === "old-offer" ? older.promise : newer.promise;
  });
  buttons(ui, "Revoke this access")[0].click();
  ui.view.update(accessControl(projection(2), second));
  buttons(ui, "Revoke this access")[1].click();
  const before = ui.container.textContent;
  older.reject(new Error("private-old-result")); await settledCallbacks();
  assert.equal(ui.container.textContent, before);
  expectUniformDescriptions(ui, "Revoke this access", /Waiting for owner status/);
  newer.reject(new Error("private-new-result")); await settledCallbacks();
  expectUniformDescriptions(ui, "Revoke this access", /Could not confirm/);
  assert.equal(calls.length, 2);
  assert.ok(!ui.container.textContent.includes("private-old-result"));
  assert.ok(!ui.container.textContent.includes("private-new-result"));
});

test("an accepted offer stays consumed across revisions, removal and reappearance during one mount", async t => {
  const calls = [];
  const same = offered("revoke_grant", "docs-grant", "consumed-offer");
  const ui = mount(t, accessControl(projection(), same), async request => { calls.push(request); });
  buttons(ui, "Revoke this access")[0].click(); await settledCallbacks();
  ui.view.update(accessControl(projection(2), same));
  assert.ok(buttons(ui, "Revoke this access").every(button => button.disabled));
  const removed = projection(3);
  removed.tasks[0].roles.find(role => role.kind === "service").controls = [];
  removed.connections[0].grants[0].controls = [];
  ui.view.update(removed);
  assert.equal(buttons(ui, "Revoke this access").length, 0);
  ui.view.update(accessControl(projection(4), same));
  for (const button of buttons(ui, "Revoke this access")) {
    assert.equal(button.disabled, true); button.click();
  }
  await settledCallbacks(); assert.equal(calls.length, 1);
});

test("a host announcement for a secondary task is exposed without deriving it from the first task", t => {
  const snapshot = projection();
  const second = taskOnly(snapshot.tasks[0], "second-task", "Secondary setup");
  second.phase = { tone: "pending", text: "Queued" }; snapshot.tasks.push(second);
  snapshot.announcement = "Secondary setup is queued.";
  const ui = mount(t, snapshot);
  assert.equal(ui.doc.querySelector('[role="status"]').textContent, snapshot.announcement);
  const next = structuredClone(snapshot); next.revision = 2;
  next.tasks[1].phase = { tone: "attention", text: "Needs your account selection" };
  next.announcement = "Secondary setup needs your account selection.";
  assert.deepEqual(next.tasks[0], snapshot.tasks[0]);
  ui.view.update(next);
  const live = ui.doc.querySelector('[role="status"]');
  assert.equal(live.textContent, next.announcement);
  assert.equal(live.getAttribute("aria-live"), "polite");
});

test("every generic control location sends only the offered reference to the protected host presenter", async t => {
  const snapshot = projection(), calls = [];
  snapshot.tasks[0].userActions = [];
  const locations = [snapshot.tasks[0], ...snapshot.tasks[0].roles,
    snapshot.connections[0], snapshot.connections[0].grants[0], snapshot.frontends[0]];
  for (const [index, location] of locations.entries()) {
    location.controls = [offered("open_authorization", `missing-url-${index}`, `offer-${index}`)];
  }
  const ui = mount(t, snapshot, async request => { calls.push(request); });
  const candidates = buttons(ui, "Review sign-in");
  assert.equal(candidates.length, 6);
  assert.equal(ui.container.querySelectorAll(".toolkit-destination").length, 0);
  assert.ok(candidates.every(button => !button.disabled));
  for (const button of candidates) button.click();
  await settledCallbacks();
  const offers = new Set(locations.map(location => JSON.stringify([location.controls[0].target.owner, location.controls[0].offer])));
  assert.equal(calls.length, offers.size);
  assert.ok(calls.every(call => Object.keys(call).sort().join() === "control,scope,viewRevision"));
});

test("different owners may reuse local offer and target spellings without sharing suppression", async t => {
  const snapshot = projection(), calls = [];
  snapshot.tasks[0].roles = []; snapshot.tasks[0].userActions = [];
  snapshot.tasks[0].controls = [offered("continue", "same-local-reference", "same-local-offer", "owner-a")];
  const second = taskOnly(snapshot.tasks[0], "other-owner-task", "Other owner task");
  second.controls = [offered("continue", "same-local-reference", "same-local-offer", "owner-b")];
  snapshot.tasks.push(second); snapshot.connections = []; snapshot.frontends = []; snapshot.packages = [];
  const ui = mount(t, snapshot, async request => { calls.push(request); });
  const choices = buttons(ui, "Continue"); assert.equal(choices.length, 2);
  choices[0].click(); assert.equal(choices[1].disabled, false); choices[1].click();
  await settledCallbacks();
  assert.deepEqual(calls.map(call => call.control.target.owner), ["owner-a", "owner-b"]);
  assert.ok(calls.every(call => call.control.offer === "same-local-offer"));
});

test("disposing does not mutate pending controls later or dispatch cancel into a replacement view", async t => {
  const pending = deferred(), calls = [];
  const snapshot = accessControl(projection(), offered("revoke_grant", "docs-grant", "dispose-offer"));
  const ui = mount(t, snapshot, request => { calls.push(request); return pending.promise; });
  const button = buttons(ui, "Revoke this access")[0]; button.click();
  const oldDescription = ui.doc.getElementById(button.getAttribute("aria-describedby"));
  const oldText = oldDescription.textContent;
  ui.view.dispose(); assert.equal(ui.container.childElementCount, 0);
  const replacement = mountToolkitView(ui.container, {
    scope: snapshot.scope, dispatch: async request => { calls.push(request); },
  });
  t.after(() => replacement.dispose());
  replacement.update(projection(2)); const replacementText = ui.container.textContent;
  pending.reject(new Error("private-post-disposal-error")); await settledCallbacks();
  assert.equal(oldDescription.textContent, oldText);
  assert.equal(ui.container.textContent, replacementText);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].control.kind, "revoke_grant");
});

test("a consumed owner offer cannot be rearmed by changing the target revision", async t => {
  const calls = [];
  const selection = offered("revoke_grant", "docs-grant", "same-owner-offer");
  const ui = mount(t, accessControl(projection(), selection), async request => { calls.push(request); });
  buttons(ui, "Revoke this access")[0].click(); await settledCallbacks();
  const changed = {
    ...structuredClone(selection),
    target: { ...selection.target, revision: "later-owner-revision" },
  };
  assert.notEqual(changed.target.revision, selection.target.revision);
  assert.equal(changed.target.owner, selection.target.owner);
  assert.equal(changed.offer, selection.offer);
  ui.view.update(accessControl(projection(2), changed));
  expectUniformDescriptions(ui, "Revoke this access", /Waiting for owner status/);
  for (const button of buttons(ui, "Revoke this access")) button.click();
  await settledCallbacks();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].control.target.revision, selection.target.revision);
});

test("Arabic and directional marks in destination origins reject without rendering or dispatch", async t => {
  const calls = [], ui = mount(t, projection(), async request => { calls.push(request); });
  const before = ui.container.innerHTML;
  for (const codePoint of [0x061c, 0x200e, 0x200f]) {
    const snapshot = projection(); snapshot.revision = 2;
    snapshot.tasks[0].userActions[0].authorizationOrigin = `https://auth.example.test/${String.fromCodePoint(codePoint)}`;
    assert.throws(() => ui.view.update(snapshot), TypeError);
    assert.equal(ui.container.innerHTML, before);
  }
  await settledCallbacks(); assert.deepEqual(calls, []);
});
