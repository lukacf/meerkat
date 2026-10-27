import test from "node:test";
import assert from "node:assert/strict";
import { JSDOM, ResourceLoader } from "jsdom";
import { mountToolkitView } from "@rkat/toolkit/ui";
import { fixture, control } from "../examples/management-fixture.mjs";

function mount(dispatch = async () => {}, snapshot = fixture()) {
  const requests = [];
  class NoNetwork extends ResourceLoader {
    fetch(url) { requests.push(url); return null; }
  }
  const dom = new JSDOM("<!doctype html><main></main>", {
    url: "https://host.example.test/", resources: new NoNetwork(), runScripts: "dangerously",
  });
  const doc = dom.window.document;
  const host = doc.querySelector("main");
  const view = mountToolkitView(host, { scope: "fixture-viewer", dispatch });
  view.update(snapshot);
  return { dom, doc, host, view, requests, close() { view.dispose(); dom.window.close(); } };
}
function buttons(context, text) {
  return [...context.doc.querySelectorAll("button")].filter(button => button.textContent === text);
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise(resolve => setImmediate(resolve));

test("service grants, connectivity, frontend audience and independent status remain distinct", t => {
  const ui = mount(); t.after(() => ui.close());
  for (const text of ["Service connections", "Agent frontends", "Until this task finishes",
    "Account connected", "Provider credential scope", "Approved Support group", "Work Assistant",
    "Waiting for administrator installation", "Access ready for this task", "One role ready; frontend setup pending"]) {
    assert.ok(ui.host.textContent.includes(text), text);
  }
  assert.equal(ui.doc.querySelectorAll('[role="status"]').length, 1);
  assert.equal(ui.doc.querySelector('[role="status"]').getAttribute("aria-live"), "polite");
});

test("ordinary management keeps explicit empty inventories without inferring task-only presentation", t => {
  const snapshot = { ...fixture(), tasks: [], connections: [], frontends: [], packages: [] };
  const ui = mount(undefined, snapshot); t.after(() => ui.close());
  assert.deepEqual([...ui.host.querySelectorAll("h2")].map(heading => heading.textContent),
    ["Connections and agent frontends", "Setup requests", "Service connections", "Agent frontends", "Packages"]);
  for (const text of ["No setup requests.", "No service connections.", "No agent frontends.", "No packages."]) {
    assert.ok(ui.host.textContent.includes(text), text);
  }
});

test("wrong viewer and invalid revisions refuse; out-of-order/equal snapshots cannot replace current controls", t => {
  const ui = mount(); t.after(() => ui.close());
  const before = ui.host.textContent;
  assert.throws(() => ui.view.update({ ...fixture(2), scope: "other-person" }), /scope/);
  assert.throws(() => ui.view.update(fixture(Number.MAX_SAFE_INTEGER + 1)), /revision/);
  const changed = fixture(1); changed.tasks[0].title = "Stale replacement";
  assert.equal(ui.view.update(changed), false);
  assert.equal(ui.view.update(fixture(0)), false);
  assert.equal(ui.host.textContent, before);
  assert.equal(ui.view.update(fixture(2)), true);
});

test("mutating original projection cannot retarget an already displayed action", async t => {
  const snapshot = fixture(), calls = [];
  const ui = mount(async request => { calls.push(request); }, snapshot); t.after(() => ui.close());
  const action = snapshot.tasks[0].userActions[0];
  action.authorizationOrigin = "https://attacker.example.test/";
  action.controls[0].target.reference = "another-owner-action";
  buttons(ui, "Review sign-in")[0].click(); await tick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].viewRevision, 1);
  assert.equal(calls[0].control.target.reference, "slack-auth");
  assert.deepEqual(Object.keys(calls[0]).sort(), ["control", "scope", "viewRevision"]);
  assert.ok(!JSON.stringify(calls[0]).includes("https://"));
  assert.ok(ui.host.textContent.includes("Destination origin: https://slack.example.test"));
});

test("safe auth context is inert text with no script, image, iframe, link or metadata request", async t => {
  const snapshot = fixture(); snapshot.tasks[0].title = '<img src="https://attacker.example.test/x">';
  snapshot.tasks[0].userActions[0].requestedAccountLabel = '<script>window.attacked=true</script>';
  const ui = mount(undefined, snapshot); t.after(() => ui.close()); await tick();
  assert.equal(ui.doc.querySelectorAll("a, img, iframe, script, link, input, textarea").length, 0);
  assert.equal(ui.dom.window.attacked, undefined); assert.deepEqual(ui.requests, []);
  assert.ok(ui.host.textContent.includes(snapshot.tasks[0].title));
  assert.ok(ui.host.textContent.includes(snapshot.tasks[0].userActions[0].authorizationOrigin));
  assert.ok(ui.host.textContent.includes(snapshot.tasks[0].userActions[0].requestedAccountLabel));
});

test("non-origin and obscured destinations are rejected atomically without reflection", async t => {
  const calls = [], ui = mount(async request => { calls.push(request); }); t.after(() => ui.close());
  const before = ui.host.innerHTML;
  for (const url of ["javascript:alert(1)", "data:text/html,hello", "https://user:secret@example.test",
    " https://example.test", "https://example.test/\u202eevil", "https://example.test/authorize?state=marker",
    "https://example.test?state=marker", "https://example.test#marker", "https://example.test/"]) {
    const snapshot = fixture(2); snapshot.tasks[0].userActions[0].authorizationOrigin = url;
    assert.throws(() => ui.view.update(snapshot), error => {
      assert.ok(error instanceof TypeError); assert.ok(!error.message.includes(url)); return true;
    });
    assert.equal(ui.host.innerHTML, before);
  }
  await tick(); assert.deepEqual(calls, []);
});

test("open_authorization on a generic card hands off only its owner reference without requiring a URL", async t => {
  const snapshot = fixture(); snapshot.tasks[0].controls = [control("open_authorization", "host-presenter")];
  snapshot.tasks[0].userActions = [];
  const calls = [], ui = mount(async request => { calls.push(request); }, snapshot); t.after(() => ui.close());
  assert.equal(buttons(ui, "Review sign-in")[0].disabled, false);
  buttons(ui, "Review sign-in")[0].click(); await tick();
  assert.deepEqual(calls, [{scope:"fixture-viewer",viewRevision:1,control:snapshot.tasks[0].controls[0]}]);
  assert.equal(ui.doc.querySelectorAll("a, iframe, input").length, 0);
});

test("one control repeated in task and management dispatches once and never marks roles complete", async t => {
  const pending = deferred(), calls = [];
  const ui = mount(request => { calls.push(request); return pending.promise; }); t.after(() => ui.close());
  const copies = buttons(ui, "Revoke this access"); assert.equal(copies.length, 2);
  copies[0].click(); copies[1].click(); copies[0].click();
  assert.equal(calls.length, 1); assert.ok(copies.every(button => button.disabled));
  pending.resolve(); await tick();
  assert.ok(ui.host.textContent.includes("Access ready for this task"));
  assert.ok(ui.host.textContent.includes("Waiting for administrator installation"));
  ui.view.update(fixture(2)); assert.ok(buttons(ui, "Revoke this access").every(button => button.disabled));
});

test("transport failure is redacted and uncertain, without retry or invented ceremony failure", async t => {
  const ui = mount(async () => { throw new Error("secret-token-private-account"); }); t.after(() => ui.close());
  buttons(ui, "Review sign-in")[0].click(); await tick();
  assert.ok(ui.host.textContent.includes("Could not confirm this request"));
  assert.ok(!ui.host.textContent.includes("secret-token-private-account"));
  assert.equal(buttons(ui, "Retry").length, 0);
  assert.ok(ui.host.textContent.includes("Action needed"));
  assert.equal(buttons(ui, "Review sign-in")[0].disabled, true);
});

test("old DOM controls and late callbacks cannot affect a newer view", async t => {
  const pending = deferred(), calls = [];
  const ui = mount(request => { calls.push(request); return pending.promise; }); t.after(() => ui.close());
  const old = buttons(ui, "Review sign-in")[0]; old.click();
  const next = fixture(2); next.tasks[0].userActions[0].state = "no_longer_needed";
  next.tasks[0].userActions[0].controls = []; ui.view.update(next);
  const before = ui.host.textContent; old.click(); pending.reject(new Error("private")); await tick();
  assert.equal(ui.host.textContent, before); assert.equal(calls.length, 1);
});

test("remount restores owner snapshot without repeating work; disposing never cancels dispatched work", async t => {
  const pending = deferred(), calls = [];
  const ui = mount(request => { calls.push(request); return pending.promise; }); t.after(() => ui.close());
  buttons(ui, "Review sign-in")[0].click(); ui.view.dispose();
  assert.equal(calls.length, 1); assert.equal(ui.host.childElementCount, 0);
  const next = mountToolkitView(ui.host, { scope: "fixture-viewer", dispatch: async request => { calls.push(request); } });
  t.after(() => next.dispose()); next.update(fixture(2));
  const before = ui.host.textContent; pending.reject(new Error("private")); await tick();
  assert.equal(ui.host.textContent, before); assert.equal(calls.length, 1);
  assert.throws(() => ui.view.update(fixture(3)), /disposed/);
});

test("focus stays on the same repeated control occurrence; removal falls back to its own heading", t => {
  const ui = mount(); t.after(() => ui.close());
  buttons(ui, "Revoke this access")[1].focus();
  ui.view.update(fixture(2)); assert.equal(ui.doc.activeElement, buttons(ui, "Revoke this access")[1]);
  const next = fixture(3); next.connections[0].grants[0].controls = [];
  ui.view.update(next);
  assert.equal(ui.doc.activeElement.tagName, "H3");
  assert.equal(ui.doc.activeElement.textContent, "Read support documentation");
});

test("distinct destructive controls retain exact owner references and scopes", async t => {
  const calls = [], ui = mount(async request => { calls.push(request); }); t.after(() => ui.close());
  for (const label of ["Stop connecting for this task", "Cancel whole task", "Change account", "Remove frontend"]) {
    buttons(ui, label)[0].click();
  }
  await tick();
  assert.deepEqual(calls.map(call => call.control.kind), ["stop_setup", "cancel_task", "change_account", "remove_frontend"]);
  assert.deepEqual(calls.map(call => call.control.target.reference), ["support-setup", "support-task", "docs-account", "slack-frontend"]);
  assert.ok(calls.every(call => call.scope === "fixture-viewer" && call.viewRevision === 1));
});

test("owner action states are literal projections and never manufacture retry/secret fields", t => {
  const ui = mount(); t.after(() => ui.close());
  for (const [index, state] of ["queued", "verifying", "complete", "expired", "no_longer_needed"].entries()) {
    const next = fixture(index + 2); next.tasks[0].userActions[0].state = state;
    next.tasks[0].userActions[0].kind = "secret_entry"; next.tasks[0].userActions[0].controls = [];
    delete next.tasks[0].userActions[0].authorizationOrigin;
    ui.view.update(next); assert.equal(buttons(ui, "Retry").length, 0);
    assert.equal(ui.doc.querySelectorAll("input, textarea, iframe").length, 0);
  }
});

test("pending task and package export navigation works without URLs or owner dispatch", t => {
  const calls = [], ui = mount(async request => { calls.push(request); }); t.after(() => ui.close());
  buttons(ui, "View setup: Answer support questions in Slack")[0].click();
  assert.equal(ui.doc.activeElement.textContent, "Answer support questions in Slack");
  buttons(ui, "View frontend")[0].click(); assert.equal(ui.doc.activeElement.textContent, "Work Assistant in Slack");
  buttons(ui, "View connection")[0].click(); assert.equal(ui.doc.activeElement.textContent, "Support documentation");
  assert.deepEqual(calls, []);
});

test("messages describe each button and status is available without color or animation", t => {
  const ui = mount(); t.after(() => ui.close());
  for (const button of ui.doc.querySelectorAll(".toolkit-controls button")) {
    assert.equal(button.type, "button");
    assert.ok(ui.doc.getElementById(button.getAttribute("aria-describedby")));
    assert.ok(button.textContent.length);
  }
  for (const status of ui.doc.querySelectorAll(".toolkit-status")) assert.ok(status.textContent.length);
});
